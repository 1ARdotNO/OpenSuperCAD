//! The start-up prompt when no OpenSCAD is installed.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, WindowExt, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::Workspace;

/// Offer to download OpenSCAD, or to locate an installed one.
pub fn offer(workspace: WeakEntity<Workspace>, window: &mut Window, cx: &mut App) {
    let download =
        osc_update::openscad::build_for_this_platform().map(|b| crate::openscad::describe(&b));
    window.open_dialog(cx, move |dialog, _, cx| {
        let explain = if download.is_some() {
            "OpenSuperCAD can download the official build from openscad.org for you. \
             It is checked against a pinned SHA-256 checksum and installed just for \
             you, without admin rights."
        } else {
            "Install it from openscad.org, then point OpenSuperCAD to it with Locate…."
        };
        let (ws_locate, ws_download) = (workspace.clone(), workspace.clone());
        dialog
            .title("OpenSCAD is needed")
            .w(px(540.))
            .child(
                v_flex()
                    .gap_3()
                    .child(
                        "OpenSuperCAD uses OpenSCAD to render, preview and export your \
                         designs, and it wasn't found on this computer.",
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(explain),
                    ),
            )
            .footer(
                h_flex()
                    .w_full()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("openscad-later")
                            .label("Not now")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(Button::new("openscad-locate").label("Locate…").on_click(
                        move |_, window, cx| {
                            window.close_dialog(cx);
                            ws_locate
                                .update(cx, |ws, cx| ws.locate_openscad(window, cx))
                                .ok();
                        },
                    ))
                    .when_some(download.clone(), |el, label| {
                        el.child(
                            Button::new("openscad-download")
                                .label(format!("Download {label}"))
                                .primary()
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    ws_download
                                        .update(cx, |ws, cx| ws.download_openscad(window, cx))
                                        .ok();
                                }),
                        )
                    }),
            )
    });
}
