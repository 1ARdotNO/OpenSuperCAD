//! Update notifications: "a new version is available" → Update → Restart.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui_kit::component::WindowExt;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::notification::Notification;
use gpui_kit::*;
use osc_project::Store;
use osc_update::Release;

use crate::update::{self, Outcome};

/// Automatic checks run at most this often.
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

pub fn register(cx: &mut App) {
    cx.on_action(|_: &super::CheckForUpdates, cx| {
        // Deferred: menu actions run while the window is borrowed. With no
        // window manager there may be no active window, so fall back to the
        // first one.
        cx.defer(|cx| {
            let window = cx
                .active_window()
                .or_else(|| cx.windows().into_iter().next());
            if let Some(window) = window {
                window
                    .update(cx, |_, window, cx| check(true, window, cx))
                    .ok();
            }
        });
    });
}

/// The start-up check, if enabled and not done in the last day.
pub fn startup_check(window: &mut Window, cx: &mut App) {
    let store = Store::default_location();
    let mut settings = store.app_settings();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    if !settings.check_for_updates
        || now.saturating_sub(settings.last_update_check) < CHECK_INTERVAL.as_secs()
        || std::env::var_os("OPENSUPERCAD_NO_UPDATE_CHECK").is_some()
    {
        return;
    }
    settings.last_update_check = now;
    let _ = store.save_app_settings(&settings);
    check(false, window, cx);
}

/// Look for a newer release. `manual` checks also report "up to date" and
/// errors; automatic ones stay quiet unless there is an update.
pub fn check(manual: bool, window: &mut Window, cx: &mut App) {
    if manual {
        window.push_notification(Notification::info("Checking for updates…"), cx);
    }
    let job = cx.background_spawn(async move { update::check() });
    window
        .spawn(cx, async move |cx| {
            let result = job.await;
            cx.update(|window, cx| match result {
                Ok(Some(release)) => offer(release, window, cx),
                Ok(None) if manual => window.push_notification(
                    Notification::success(format!(
                        "OpenSuperCAD {} is up to date",
                        osc_update::Version::current()
                    )),
                    cx,
                ),
                Err(e) if manual => window.push_notification(
                    Notification::error(format!("Update check failed: {e}")),
                    cx,
                ),
                _ => {}
            })
            .ok();
        })
        .detach();
}

fn offer(release: Release, window: &mut Window, cx: &mut App) {
    let message = format!(
        "OpenSuperCAD {} is available (you have {}).",
        release.version,
        osc_update::Version::current()
    );
    window.push_notification(
        Notification::info(message)
            .title("Update available")
            .action(move |_, _, _| {
                let release = release.clone();
                Button::new("update-now")
                    .label("Update")
                    .primary()
                    .on_click(move |_, window, cx| install(release.clone(), window, cx))
            }),
        cx,
    );
}

fn install(release: Release, window: &mut Window, cx: &mut App) {
    window.push_notification(
        Notification::info(format!(
            "Downloading and verifying OpenSuperCAD {}…",
            release.version
        )),
        cx,
    );
    let job = cx.background_spawn(async move { update::apply(&release) });
    window
        .spawn(cx, async move |cx| {
            let result = job.await;
            cx.update(|window, cx| match result {
                Ok(Outcome::Installed { version, binary }) => window.push_notification(
                    Notification::success(format!(
                        "Updated to {version}. The download's SHA-256 checksum was verified."
                    ))
                    .title("Restart to finish")
                    .action(move |_, _, _| {
                        let binary = binary.clone();
                        Button::new("restart").label("Restart").primary().on_click(
                            move |_, _, cx| {
                                // Otherwise GPUI restarts the running (now
                                // renamed) executable.
                                cx.set_restart_path(binary.clone());
                                cx.restart();
                            },
                        )
                    }),
                    cx,
                ),
                Ok(Outcome::OpenedDmg { path }) => window.push_notification(
                    Notification::success(format!(
                        "Verified {}. Drag OpenSuperCAD to Applications, then reopen it.",
                        path.file_name().unwrap_or_default().to_string_lossy()
                    ))
                    .autohide(false),
                    cx,
                ),
                Ok(Outcome::InstallerStarted { version }) => {
                    window.push_notification(
                        Notification::success(format!(
                            "Installing {version} (SHA-256 verified). OpenSuperCAD closes now \
                             and restarts when the installer is done."
                        )),
                        cx,
                    );
                    // Let the installer replace the files; it relaunches us.
                    cx.spawn(async move |cx| {
                        cx.background_executor()
                            .timer(std::time::Duration::from_secs(2))
                            .await;
                        cx.update(|cx| cx.quit());
                    })
                    .detach();
                }
                Ok(Outcome::Manual { how }) => {
                    window.push_notification(Notification::info(how).autohide(false), cx)
                }
                Err(e) => window.push_notification(
                    Notification::error(format!("Update failed, nothing was changed: {e}"))
                        .autohide(false),
                    cx,
                ),
            })
            .ok();
        })
        .detach();
}
