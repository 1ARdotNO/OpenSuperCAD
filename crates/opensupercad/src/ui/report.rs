//! "Report a bug" actions and the after-a-crash prompt.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, WindowExt, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::crash;

/// App-level handlers for the Help menu actions.
pub fn register(cx: &mut App) {
    cx.on_action(|_: &super::ReportBug, cx| cx.open_url(&crash::bug_report_url()));
    cx.on_action(|_: &super::RequestFeature, cx| cx.open_url(&crash::feature_request_url()));
    cx.on_action(|_: &super::OpenIssues, cx| cx.open_url(&crash::issues_url()));
    cx.on_action(|_: &super::ShowCrashReports, cx| {
        let dir = crash::crash_dir();
        let _ = std::fs::create_dir_all(&dir);
        cx.open_with_system(&dir);
    });
}

/// If the last run crashed, offer to report it. The user sees a prefilled
/// GitHub issue in the browser and decides whether to submit it.
pub fn offer_crash_report(window: &mut Window, cx: &mut App) {
    let dir = crash::crash_dir();
    let Some(path) = crash::pending(&dir) else {
        return;
    };
    let report = std::fs::read_to_string(&path).unwrap_or_default();
    // Ask once per crash, whatever the answer.
    crash::mark_seen(&dir);
    let url = crash::crash_issue_url(&report);
    let shown = path.with_extension("log");
    window.open_dialog(cx, move |dialog, _, cx| {
        let url = url.clone();
        let shown = shown.clone();
        dialog
            .title("OpenSuperCAD crashed last time")
            .w(px(560.))
            .child(
                v_flex()
                    .gap_3()
                    .child(
                        "A crash report was saved. Reporting it helps fix the problem: \
                         GitHub opens with the report filled in, and you can review and \
                         edit it before submitting. Nothing is sent automatically.",
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(shown.display().to_string()),
                    )
                    .child(
                        Button::new("show-crash-file")
                            .label("Show report file")
                            .ghost()
                            .on_click({
                                let shown = shown.clone();
                                move |_, _, cx| cx.reveal_path(&shown)
                            }),
                    ),
            )
            .footer(
                h_flex()
                    .w_full()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("dismiss-crash")
                            .label("Dismiss")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("report-crash")
                            .label("Report on GitHub")
                            .primary()
                            .on_click(move |_, window, cx| {
                                cx.open_url(&url);
                                window.close_dialog(cx);
                            }),
                    ),
            )
    });
}
