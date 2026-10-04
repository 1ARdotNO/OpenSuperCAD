//! Zed-style pickers: command palette, file finder and recent projects. All
//! three are a fuzzy-searchable list in a dialog.

use std::rc::Rc;

use gpui_kit::component::command::{Command, CommandItem, CommandState};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::{ActiveTheme, WindowExt, h_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::Workspace;

#[derive(Clone)]
pub struct PickerItem {
    pub label: SharedString,
    pub detail: Option<SharedString>,
    /// Shown as a keybinding hint (looked up in the "Workspace" context).
    pub action: Option<Rc<dyn Action>>,
}

impl PickerItem {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            detail: None,
            action: None,
        }
    }

    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

type OnPick = dyn Fn(usize, &mut Workspace, &mut Window, &mut Context<Workspace>);

/// Open a picker over `items`; `on_pick` runs with the chosen index after the
/// dialog closes and focus is back on the workspace.
pub fn open(
    placeholder: &'static str,
    items: Vec<PickerItem>,
    on_pick: impl Fn(usize, &mut Workspace, &mut Window, &mut Context<Workspace>) + 'static,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let state = cx.new(|cx| CommandState::new(window, cx));
    let workspace = cx.weak_entity();
    let on_pick: Rc<OnPick> = Rc::new(on_pick);
    let items = Rc::new(items);
    let command_state = state.clone();
    window.open_dialog(cx, move |dialog, _window, _cx| {
        let workspace = workspace.clone();
        let on_pick = on_pick.clone();
        let rows = items.iter().map(|item| {
            let item = item.clone();
            // Let "view top" match "View: Top", like Zed's palette.
            let plain: String = item
                .label
                .chars()
                .filter(|c| c.is_alphanumeric() || c.is_whitespace())
                .collect();
            let mut row = CommandItem::new()
                .label(item.label.clone())
                .keywords([SharedString::from(plain)]);
            if let Some(detail) = &item.detail {
                row = row.keywords([detail.clone()]);
            }
            row.child(move |window, cx| {
                let muted = cx.theme().muted_foreground;
                h_flex()
                    .w_full()
                    .gap_2()
                    .child(div().flex_shrink_0().child(item.label.clone()))
                    .when_some(item.detail.clone(), |el, d| {
                        el.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(muted)
                                .child(d),
                        )
                    })
                    .child(div().flex_1())
                    .when_some(
                        item.action.as_ref().and_then(|a| {
                            Kbd::binding_for_action(a.as_ref(), Some("Workspace"), window)
                        }),
                        |el, kbd| el.child(kbd),
                    )
            })
        });
        dialog
            .close_button(false)
            .w(px(600.))
            .margin_top(px(80.))
            .p_0()
            .child(
                Command::new(&command_state)
                    .placeholder(placeholder)
                    .bordered(false)
                    .items(rows)
                    .on_confirm(move |ix, window, cx| {
                        window.close_dialog(cx);
                        let on_pick = on_pick.clone();
                        if let Some(ws) = workspace.upgrade() {
                            ws.update(cx, |ws, cx| {
                                window.focus(&ws.focus_handle(cx), cx);
                                on_pick(ix.row, ws, window, cx);
                            });
                        }
                    })
                    .on_cancel(|window, cx| window.close_dialog(cx)),
            )
    });
    state.update(cx, |s, cx| s.focus(window, cx));
}

/// The command palette's entries: every user-facing action.
pub fn commands() -> Vec<(&'static str, Rc<dyn Action>)> {
    use super::*;
    fn a(action: impl Action) -> Rc<dyn Action> {
        Rc::new(action)
    }
    vec![
        ("File: Open Folder…", a(OpenFolder)),
        ("File: Find File…", a(FindFile)),
        ("File: Recent Projects…", a(RecentProjects)),
        ("File: New File…", a(NewFile)),
        ("File: Save", a(Save)),
        ("File: Close Tab", a(CloseTab)),
        ("File: Next Tab", a(NextTab)),
        ("File: Previous Tab", a(PrevTab)),
        ("File: Reload from Disk", a(Reload)),
        ("File: Export as STL", a(ExportStl)),
        ("File: Export…", a(ExportAs)),
        ("Design: Preview (OpenSCAD)", a(PreviewDesign)),
        ("Design: Render", a(RenderDesign)),
        ("Design: Take Checkpoint", a(Checkpoint)),
        ("View: Top", a(ViewTop)),
        ("View: Bottom", a(ViewBottom)),
        ("View: Left", a(ViewLeft)),
        ("View: Right", a(ViewRight)),
        ("View: Front", a(ViewFront)),
        ("View: Back", a(ViewBack)),
        ("View: Diagonal", a(ViewDiagonal)),
        ("View: Reset", a(ResetView)),
        ("View: Toggle Project Panel", a(ToggleProjectPanel)),
        ("View: Toggle Customizer/Console", a(ToggleConsole)),
        ("View: Toggle Agent Panel", a(ToggleAgentPanel)),
        ("View: Toggle Light/Dark Theme", a(ToggleTheme)),
        ("Git: Show Git Panel", a(FocusGit)),
        ("Agent: New Thread", a(NewThread)),
        ("Agent: Stop", a(StopAgent)),
        ("Help: Report a Bug", a(ReportBug)),
        ("Help: Request a Feature", a(RequestFeature)),
        ("Help: Open Issues Page", a(OpenIssues)),
        ("Help: Show Crash Reports", a(ShowCrashReports)),
        ("Help: Check for Updates", a(CheckForUpdates)),
        ("OpenSCAD: Download (SHA-256 verified)", a(DownloadOpenScad)),
        ("OpenSCAD: Locate…", a(LocateOpenScad)),
        ("OpenSCAD: Find Automatically", a(AutoOpenScad)),
        ("App: Settings…", a(OpenSettings)),
        ("App: Quit", a(Quit)),
    ]
}
