//! The GPUI user interface.

mod agent_panel;
mod customizer;
mod git_panel;
mod preview;
mod workspace;

use std::path::PathBuf;

use gpui_kit::component::highlighter::{LanguageConfig, LanguageRegistry};
use gpui_kit::component::menu::AppMenuBar;
use gpui_kit::component::{GlobalState, Theme, ThemeMode, TitleBar};
use gpui_kit::*;

pub use workspace::Workspace;

actions!(
    osc,
    [
        Quit,
        OpenFolder,
        NewFile,
        Save,
        Reload,
        PreviewDesign,
        RenderDesign,
        ExportStl,
        ExportAs,
        Checkpoint,
        NewThread,
        StopAgent,
        ToggleProjectPanel,
        ToggleConsole,
        ToggleAgentPanel,
        FocusGit,
        ToggleTheme,
        ViewTop,
        ViewBottom,
        ViewLeft,
        ViewRight,
        ViewFront,
        ViewBack,
        ViewDiagonal,
        ResetView,
    ]
);

/// Open one of the recent projects (index into the recent list).
#[derive(Clone, PartialEq, Action)]
#[action(namespace = osc, no_json)]
pub struct OpenRecent {
    pub ix: usize,
}

pub fn run(path: Option<PathBuf>) -> anyhow::Result<()> {
    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .with_quit_mode(QuitMode::LastWindowClosed)
        .run(move |cx: &mut App| {
            gpui_kit::init(cx);
            Theme::change(ThemeMode::Dark, None, cx);
            register_openscad_language();
            bind_keys(cx);
            cx.on_action(|_: &Quit, cx| cx.quit());
            cx.on_action(|_: &ToggleTheme, cx| {
                let mode = if cx.theme_is_dark() {
                    ThemeMode::Light
                } else {
                    ThemeMode::Dark
                };
                Theme::change(mode, None, cx);
            });
            cx.set_menus(menus());
            GlobalState::global_mut(cx)
                .set_app_menus(menus().into_iter().map(|m| m.owned()).collect());

            let bounds = Bounds::centered(None, size(px(1480.), px(920.)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(900.), px(560.))),
                window_decorations: Some(WindowDecorations::Client),
                app_id: Some("opensupercad".into()),
                ..TitleBar::window_options()
            };
            let path = path.clone();
            gpui_kit::open_window(options, cx, move |window, cx| {
                let menu_bar = AppMenuBar::new(cx);
                cx.new(|cx| Workspace::new(path, menu_bar, window, cx))
            })
            .expect("failed to open the main window");
            cx.activate(true);
        });
    Ok(())
}

trait ThemeIsDark {
    fn theme_is_dark(&self) -> bool;
}

impl ThemeIsDark for App {
    fn theme_is_dark(&self) -> bool {
        use gpui_kit::component::ActiveTheme;
        self.theme().is_dark()
    }
}

/// Register the OpenSCAD grammar (maintained by the OpenSCAD project) with
/// the editor's highlighter.
fn register_openscad_language() {
    let language: tree_sitter::Language = tree_sitter_openscad::LANGUAGE.into();
    LanguageRegistry::singleton().register(
        "openscad",
        &LanguageConfig::new(
            "openscad",
            language,
            vec![],
            tree_sitter_openscad::HIGHLIGHTS_QUERY,
            "",
            "",
        ),
    );
}

fn bind_keys(cx: &mut App) {
    let ws = Some("Workspace");
    cx.bind_keys([
        KeyBinding::new("secondary-q", Quit, None),
        KeyBinding::new("secondary-o", OpenFolder, ws),
        KeyBinding::new("secondary-n", NewFile, ws),
        KeyBinding::new("secondary-s", Save, ws),
        KeyBinding::new("secondary-r", Reload, ws),
        KeyBinding::new("f5", PreviewDesign, ws),
        KeyBinding::new("f6", RenderDesign, ws),
        KeyBinding::new("f7", ExportStl, ws),
        KeyBinding::new("secondary-shift-e", ExportAs, ws),
        KeyBinding::new("secondary-alt-s", Checkpoint, ws),
        KeyBinding::new("secondary-shift-n", NewThread, ws),
        KeyBinding::new("escape", StopAgent, Some("AgentPanel")),
        KeyBinding::new("secondary-b", ToggleProjectPanel, ws),
        KeyBinding::new("secondary-j", ToggleConsole, ws),
        KeyBinding::new("secondary-?", ToggleAgentPanel, ws),
        KeyBinding::new("secondary-shift-/", ToggleAgentPanel, ws),
        KeyBinding::new("secondary-shift-g", FocusGit, ws),
        KeyBinding::new("secondary-4", ViewTop, ws),
        KeyBinding::new("secondary-5", ViewBottom, ws),
        KeyBinding::new("secondary-6", ViewLeft, ws),
        KeyBinding::new("secondary-7", ViewRight, ws),
        KeyBinding::new("secondary-8", ViewFront, ws),
        KeyBinding::new("secondary-9", ViewBack, ws),
        KeyBinding::new("secondary-0", ViewDiagonal, ws),
        KeyBinding::new("secondary-shift-0", ResetView, ws),
    ]);
}

fn menus() -> Vec<Menu> {
    vec![
        Menu::new("OpenSuperCAD").items([
            MenuItem::action("Toggle Light/Dark Theme", ToggleTheme),
            MenuItem::separator(),
            MenuItem::action("Quit", Quit),
        ]),
        Menu::new("File").items([
            MenuItem::action("Open Folder…", OpenFolder),
            MenuItem::action("New File…", NewFile),
            MenuItem::separator(),
            MenuItem::action("Save", Save),
            MenuItem::action("Reload from Disk", Reload),
            MenuItem::separator(),
            MenuItem::action("Export as STL", ExportStl),
            MenuItem::action("Export…", ExportAs),
        ]),
        Menu::new("Design").items([
            MenuItem::action("Preview (OpenSCAD)", PreviewDesign),
            MenuItem::action("Render", RenderDesign),
            MenuItem::separator(),
            MenuItem::action("Take Checkpoint", Checkpoint),
        ]),
        Menu::new("View").items([
            MenuItem::action("Top", ViewTop),
            MenuItem::action("Bottom", ViewBottom),
            MenuItem::action("Left", ViewLeft),
            MenuItem::action("Right", ViewRight),
            MenuItem::action("Front", ViewFront),
            MenuItem::action("Back", ViewBack),
            MenuItem::action("Diagonal", ViewDiagonal),
            MenuItem::action("Reset View", ResetView),
            MenuItem::separator(),
            MenuItem::action("Toggle Project Panel", ToggleProjectPanel),
            MenuItem::action("Toggle Console", ToggleConsole),
            MenuItem::action("Toggle Agent Panel", ToggleAgentPanel),
        ]),
        Menu::new("Agent").items([
            MenuItem::action("New Thread", NewThread),
            MenuItem::action("Stop", StopAgent),
        ]),
    ]
}
