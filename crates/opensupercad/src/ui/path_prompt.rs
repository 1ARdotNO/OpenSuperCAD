//! File and folder prompts that still work without a native dialog.
//!
//! On Linux the native dialogs go through xdg-desktop-portal. When no portal
//! is running (minimal window managers, remote sessions, CI) the prompt
//! fails, so we fall back to an in-app dialog with a path field.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme, WindowExt, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

type OnPath = Rc<dyn Fn(PathBuf, &mut Window, &mut App)>;
type Accept = Rc<dyn Fn(&mut Window, &mut App)>;

/// Ask for a path to save to, starting in `dir` with `name` suggested.
pub fn new_path(
    title: &'static str,
    hint: &'static str,
    dir: &Path,
    name: &str,
    window: &mut Window,
    cx: &mut App,
    on_path: impl Fn(PathBuf, &mut Window, &mut App) + 'static,
) {
    let rx = cx.prompt_for_new_path(dir, Some(name));
    let suggested = dir.join(name);
    let on_path: OnPath = Rc::new(on_path);
    window
        .spawn(cx, async move |cx| match rx.await {
            Ok(Ok(Some(path))) => {
                cx.update(|window, cx| on_path(path, window, cx)).ok();
            }
            // Cancelled by the user.
            Ok(Ok(None)) => {}
            // No native dialog available.
            _ => {
                cx.update(|window, cx| fallback(title, hint, suggested, on_path, window, cx))
                    .ok();
            }
        })
        .detach();
}

/// Ask for an existing folder, starting at `dir`.
pub fn folder(
    title: &'static str,
    dir: &Path,
    window: &mut Window,
    cx: &mut App,
    on_path: impl Fn(PathBuf, &mut Window, &mut App) + 'static,
) {
    let rx = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: None,
    });
    let on_path: OnPath = Rc::new(on_path);
    let suggested = dir.to_path_buf();
    window
        .spawn(cx, async move |cx| match rx.await {
            Ok(Ok(Some(mut paths))) if !paths.is_empty() => {
                cx.update(|window, cx| on_path(paths.remove(0), window, cx))
                    .ok();
            }
            Ok(Ok(_)) => {}
            _ => {
                cx.update(|window, cx| {
                    fallback(title, "Folder to open", suggested, on_path, window, cx)
                })
                .ok();
            }
        })
        .detach();
}

fn fallback(
    title: &'static str,
    hint: &'static str,
    suggested: PathBuf,
    on_path: OnPath,
    window: &mut Window,
    cx: &mut App,
) {
    let input =
        cx.new(|cx| InputState::new(window, cx).default_value(suggested.display().to_string()));
    // Close the dialog, then hand over the path on the next frame, so
    // notifications the callback shows aren't dropped along with the dialog.
    let accept: Accept = {
        let input = input.clone();
        Rc::new(move |window, cx| {
            let text = input.read(cx).value().trim().to_owned();
            if text.is_empty() {
                return;
            }
            window.close_dialog(cx);
            let on_path = on_path.clone();
            window.defer(cx, move |window, cx| {
                on_path(expand_home(&text), window, cx)
            });
        })
    };
    // Enter while the field has focus is consumed by the field itself.
    let on_enter = accept.clone();
    let sub = window.subscribe(&input, cx, move |_, ev: &InputEvent, window, cx| {
        if matches!(ev, InputEvent::PressEnter { .. }) {
            on_enter(window, cx);
        }
    });
    let field = input.clone();
    window.open_dialog(cx, move |dialog, _, cx| {
        // The subscription lives as long as the dialog's builder.
        let _ = &sub;
        let ok = accept.clone();
        let enter = accept.clone();
        dialog
            .title(title)
            .w(px(560.))
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(hint),
                    )
                    .child(Input::new(&field)),
            )
            .footer(
                h_flex()
                    .w_full()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("path-cancel")
                            .label("Cancel")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("path-ok")
                            .label("OK")
                            .primary()
                            .on_click(move |_, window, cx| ok(window, cx)),
                    ),
            )
            // Enter elsewhere in the dialog confirms it too.
            .on_ok(move |_, window, cx| {
                enter(window, cx);
                // `enter` already closed the dialog when it accepted.
                false
            })
    });
    input.update(cx, |i, cx| i.focus(window, cx));
}

fn expand_home(text: &str) -> PathBuf {
    match (text.strip_prefix("~/"), dirs::home_dir()) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(text),
    }
}
