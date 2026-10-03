//! The main window: Zed-style docks around the editor and the preview.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Editor, EditorState, InputEvent, Position};
use gpui_kit::component::list::ListItem;
use gpui_kit::component::menu::{AppMenuBar, DropdownMenu};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::tree::{Tree, TreeEntry, TreeItem, TreeState};
use gpui_kit::component::{
    ActiveTheme, Icon, IconName, Selectable, Sizable, TitleBar, WindowExt, h_flex, h_resizable,
    resizable_panel, v_flex, v_resizable,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use osc_engine::{Diagnostic, Engine, Severity, View};
use osc_mcp::control::{ControlReply, ControlRequest};
use osc_project::{Project, RecentProject, Store};

use super::agent_panel::{AgentPanel, AgentPanelEvent};
use super::customizer::{Customizer, CustomizerEvent};
use super::git_panel::{GitEvent, GitPanel};
use super::preview::{Preview, PreviewEvent};
use super::settings_panel::{SettingsEvent, SettingsPanel};
use super::*;

#[derive(Clone, Copy, PartialEq)]
enum LeftTab {
    Files,
    Outline,
    Git,
    Settings,
}

#[derive(Clone, Copy, PartialEq)]
enum RightTab {
    Customizer,
    Console,
}

pub struct Workspace {
    focus: FocusHandle,
    store: Store,
    engine: Option<Arc<Engine>>,
    /// The discovered OpenSCAD before per-project settings are applied.
    base_engine: Option<Engine>,
    registry: osc_agent::Registry,
    settings: Entity<SettingsPanel>,
    openscad_version: Option<String>,
    project: Option<Project>,
    recent: Vec<RecentProject>,
    files: Vec<String>,
    file_tree: Entity<TreeState>,
    open_file: Option<PathBuf>,
    editor: Entity<EditorState>,
    dirty: bool,
    disk_mtime: Option<SystemTime>,
    diagnostics: Vec<Diagnostic>,
    preview: Entity<Preview>,
    customizer: Entity<Customizer>,
    agent: Entity<AgentPanel>,
    git: Entity<GitPanel>,
    menu_bar: Entity<AppMenuBar>,
    left_tab: LeftTab,
    right_tab: RightTab,
    show_left: bool,
    show_console: bool,
    show_agent: bool,
    param_gen: u64,
    _poll: Task<()>,
    _subs: Vec<Subscription>,
}

impl Workspace {
    pub fn new(
        path: Option<PathBuf>,
        menu_bar: Entity<AppMenuBar>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let store = Store::default_location();
        let engine = Engine::discover().ok().map(Arc::new);
        let openscad_version = engine.as_ref().and_then(|e| e.version().ok());
        let registry = crate::agents::registry();

        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("openscad")
                .line_number(true)
                .soft_wrap(false)
                .default_value("")
        });
        let file_tree = cx.new(|cx| TreeState::new(cx));
        let preview = cx.new(|_| Preview::new(engine.clone()));
        let customizer = cx.new(|_| Customizer::default());
        let agent = cx.new(|cx| AgentPanel::new(store.clone(), registry.clone(), window, cx));
        let settings = cx.new(|_| SettingsPanel::new());
        let git = cx.new(|cx| GitPanel::new(window, cx));

        let subs = vec![
            cx.subscribe_in(&editor, window, Self::on_editor_event),
            cx.subscribe_in(&preview, window, |this, _, ev: &PreviewEvent, _, cx| {
                let PreviewEvent::Finished(result) = ev;
                this.diagnostics = result.diagnostics.clone();
                this.apply_editor_diagnostics(cx);
                if !result.success {
                    this.right_tab = RightTab::Console;
                }
                cx.notify();
            }),
            cx.subscribe_in(
                &customizer,
                window,
                |this, _, ev: &CustomizerEvent, window, cx| {
                    let CustomizerEvent::Changed { name, value } = ev;
                    this.set_parameter(name, value, window, cx);
                },
            ),
            cx.subscribe_in(
                &agent,
                window,
                |this, _, ev: &AgentPanelEvent, window, cx| match ev {
                    AgentPanelEvent::FilesChanged => this.check_disk(window, cx),
                    AgentPanelEvent::TurnEnded => {
                        this.check_disk(window, cx);
                        this.refresh_files(cx);
                        this.git.update(cx, |g, cx| g.refresh(cx));
                    }
                    AgentPanelEvent::RestoreCheckpoint(id) => {
                        let id = id.clone();
                        this.git.update(cx, |g, cx| g.restore(&id, cx));
                    }
                },
            ),
            cx.subscribe_in(
                &settings,
                window,
                |this, _, ev: &SettingsEvent, window, cx| {
                    let SettingsEvent::Changed(settings) = ev;
                    if let Some(project) = &this.project
                        && let Err(e) = project.save_settings(settings)
                    {
                        window.push_notification(
                            Notification::error(format!("Saving settings failed: {e}")),
                            cx,
                        );
                    }
                    let backend_changed = this.engine.as_ref().map(|e| e.backend.clone())
                        != Some(settings.openscad_backend.clone());
                    this.apply_backend(settings.openscad_backend.clone(), cx);
                    if backend_changed {
                        this.render(cx);
                    }
                },
            ),
            cx.subscribe_in(
                &git,
                window,
                |this, _, ev: &GitEvent, window, cx| match ev {
                    GitEvent::Restored(id) => {
                        this.dirty = false;
                        this.disk_mtime = None;
                        this.check_disk(window, cx);
                        this.refresh_files(cx);
                        window.push_notification(
                            Notification::success(format!(
                                "Restored checkpoint {}",
                                &id[..id.len().min(8)]
                            )),
                            cx,
                        );
                    }
                    GitEvent::Message(msg) => {
                        window.push_notification(Notification::info(msg.clone()), cx)
                    }
                },
            ),
        ];

        // Control channel for the agents' MCP servers (snapshots, camera, saves).
        let control = start_control(window, cx);
        agent.update(cx, |a, _| a.set_control(control));

        // Watch the disk for changes made by agents or other editors.
        let poll = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(1000))
                    .await;
                let alive = this.update_in(cx, |this, window, cx| {
                    this.check_disk(window, cx);
                    this.refresh_files(cx);
                });
                if alive.is_err() {
                    break;
                }
            }
        });

        let mut ws = Self {
            focus: cx.focus_handle(),
            recent: store.recent_projects(),
            store,
            base_engine: engine.as_deref().cloned(),
            engine,
            registry,
            settings,
            openscad_version,
            project: None,
            files: Vec::new(),
            file_tree,
            open_file: None,
            editor,
            dirty: false,
            disk_mtime: None,
            diagnostics: Vec::new(),
            preview,
            customizer,
            agent,
            git,
            menu_bar,
            left_tab: LeftTab::Files,
            right_tab: RightTab::Customizer,
            show_left: true,
            show_console: true,
            show_agent: true,
            param_gen: 0,
            _poll: poll,
            _subs: subs,
        };

        let start = path.or_else(|| ws.recent.first().map(|r| r.root.clone()));
        if let Some(path) = start {
            ws.open_path(&path, window, cx);
        }
        ws
    }

    // ---------------------------------------------------------------------
    // Projects and files
    // ---------------------------------------------------------------------

    /// Open a folder as a project, or a `.scad` file inside its folder.
    pub fn open_path(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let (root, file) = if path.is_file() {
            (
                path.parent().unwrap_or(Path::new(".")).to_path_buf(),
                Some(path.to_path_buf()),
            )
        } else {
            (path.to_path_buf(), None)
        };
        let project = match Project::open(&root) {
            Ok(p) => p,
            Err(e) => {
                window.push_notification(
                    Notification::error(format!("Cannot open {}: {e}", root.display())),
                    cx,
                );
                return;
            }
        };
        if self.dirty {
            self.save(window, cx);
        }
        // Remember which thread was active in the project we are leaving.
        if let Some(old) = &self.project {
            let thread = self.agent.read(cx).current_thread_id();
            let _ = self.store.remember(old.root(), None, thread.as_deref());
        }
        let recent = self.store.touch_project(project.root()).ok();
        self.recent = self.store.recent_projects();
        let settings = project.settings();
        let name = project.name();
        let root = project.root().to_path_buf();
        self.project = Some(project);
        self.open_file = None;
        self.files.clear();
        self.preview.update(cx, |p, cx| p.clear(cx));
        self.refresh_files(cx);
        self.git.update(cx, |g, cx| g.set_project(Some(&root), cx));
        self.apply_backend(settings.openscad_backend.clone(), cx);
        let registry = self.registry.clone();
        let project = self.project.clone();
        self.settings.update(cx, |s, cx| {
            s.set_project(project.as_ref(), &registry, window, cx)
        });
        let last_thread = recent.as_ref().and_then(|r| r.last_thread.clone());
        self.agent.update(cx, |a, cx| {
            a.set_project(
                Some((root.clone(), name.clone(), settings.auto_checkpoint())),
                last_thread,
                settings.default_agent.clone(),
                window,
                cx,
            )
        });

        let target = file
            .or_else(|| {
                recent
                    .and_then(|r| r.last_file)
                    .map(|f| root.join(f))
                    .filter(|p| p.is_file())
            })
            .or_else(|| {
                self.project
                    .as_ref()
                    .and_then(|p| p.main_file())
                    .map(|f| root.join(f))
            });
        match target {
            Some(file) => self.open_file(&file, window, cx),
            None => {
                self.editor.update(cx, |e, cx| e.set_value("", window, cx));
                self.customizer
                    .update(cx, |c, cx| c.sync(Vec::new(), window, cx));
            }
        }
        window.set_window_title(&format!("{name} · OpenSuperCAD"));
        cx.notify();
    }

    /// Use the project's OpenSCAD backend for rendering and exports.
    fn apply_backend(&mut self, backend: Option<String>, cx: &mut Context<Self>) {
        let Some(base) = &self.base_engine else {
            return;
        };
        let mut engine = base.clone();
        engine.backend = backend;
        let engine = Some(Arc::new(engine));
        self.engine = engine.clone();
        self.preview.update(cx, |p, _| p.set_engine(engine));
    }

    fn refresh_files(&mut self, cx: &mut Context<Self>) {
        let Some(project) = &self.project else { return };
        let files = project.files();
        if files == self.files {
            return;
        }
        self.files = files;
        let items = build_tree(&self.files);
        self.file_tree.update(cx, |t, cx| t.set_items(items, cx));
        cx.notify();
    }

    fn open_file(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        if self.dirty {
            self.save(window, cx);
        }
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                window
                    .push_notification(Notification::error(format!("{}: {e}", path.display())), cx);
                return;
            }
        };
        self.open_file = Some(path.to_path_buf());
        self.disk_mtime = mtime(path);
        self.dirty = false;
        self.diagnostics.clear();
        self.editor
            .update(cx, |e, cx| e.set_value(text.clone(), window, cx));
        self.sync_customizer(&text, window, cx);
        if let Some(project) = &self.project {
            let rel = project.relative(path);
            let _ = self.store.remember(project.root(), Some(&rel), None);
        }
        if is_scad(path) {
            self.render(cx);
        }
        cx.notify();
    }

    fn current_text(&self, cx: &App) -> String {
        self.editor.read(cx).value().to_string()
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.open_file.clone() else {
            return;
        };
        let text = self.current_text(cx);
        if let Err(e) = std::fs::write(&path, &text) {
            window.push_notification(Notification::error(format!("Save failed: {e}")), cx);
            return;
        }
        self.dirty = false;
        self.disk_mtime = mtime(&path);
        self.git.update(cx, |g, cx| g.refresh(cx));
        if is_scad(&path) {
            self.render(cx);
        }
        cx.notify();
    }

    /// Pick up external changes (agent edits, other editors, restores).
    fn check_disk(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.open_file.clone() else {
            return;
        };
        let current = mtime(&path);
        if current.is_none() || current == self.disk_mtime {
            return;
        }
        if self.dirty {
            // Keep the user's unsaved edits; they win on the next save.
            self.disk_mtime = current;
            window.push_notification(
                Notification::warning(format!(
                    "{} changed on disk while you have unsaved edits",
                    path.file_name()
                        .map(|n| n.to_string_lossy())
                        .unwrap_or_default()
                )),
                cx,
            );
            return;
        }
        if let Ok(text) = std::fs::read_to_string(&path) {
            self.disk_mtime = current;
            if text != self.current_text(cx) {
                self.editor
                    .update(cx, |e, cx| e.set_value(text.clone(), window, cx));
                self.sync_customizer(&text, window, cx);
                self.render(cx);
            }
        }
        self.git.update(cx, |g, cx| g.refresh(cx));
    }

    fn sync_customizer(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let params = osc_syntax::customizer::parameters(text);
        self.customizer
            .update(cx, |c, cx| c.sync(params, window, cx));
    }

    fn on_editor_event(
        &mut self,
        _: &Entity<EditorState>,
        ev: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let InputEvent::Change = ev {
            self.dirty = true;
            let text = self.current_text(cx);
            self.sync_customizer(&text, window, cx);
            self.apply_editor_diagnostics(cx);
            cx.notify();
        }
    }

    /// Apply a customizer change to the source, save and re-render (debounced
    /// so dragging a slider doesn't queue a render per pixel).
    fn set_parameter(
        &mut self,
        name: &str,
        value: &osc_syntax::customizer::Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = self.current_text(cx);
        let Ok(updated) = osc_syntax::customizer::set_parameter(&text, name, value) else {
            return;
        };
        if updated == text {
            return;
        }
        self.editor
            .update(cx, |e, cx| e.replace_all(updated, window, cx));
        self.dirty = true;
        self.param_gen += 1;
        let generation = self.param_gen;
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(350))
                .await;
            this.update_in(cx, |this, window, cx| {
                if this.param_gen == generation {
                    this.save(window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn apply_editor_diagnostics(&mut self, cx: &mut Context<Self>) {
        use gpui_kit::component::highlighter::{Diagnostic as EdDiag, DiagnosticSeverity};
        let file_name = self
            .open_file
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned());
        let diags: Vec<EdDiag> = self
            .diagnostics
            .iter()
            .filter(|d| matches!(d.severity, Severity::Error | Severity::Warning))
            .filter(|d| match (&d.file, &file_name) {
                (Some(f), Some(n)) => f.ends_with(n.as_str()),
                _ => true,
            })
            .filter_map(|d| {
                let line = d.line?.saturating_sub(1) as u32;
                let sev = if d.severity == Severity::Error {
                    DiagnosticSeverity::Error
                } else {
                    DiagnosticSeverity::Warning
                };
                Some(
                    EdDiag::new(
                        Position::new(line, 0)..Position::new(line, 200),
                        d.message.clone(),
                    )
                    .with_severity(sev)
                    .with_source("openscad"),
                )
            })
            .collect();
        self.editor.update(cx, |e, cx| {
            if let Some(set) = e.diagnostics_mut() {
                set.clear();
                set.extend(diags);
            }
            cx.notify();
        });
    }

    fn render(&mut self, cx: &mut Context<Self>) {
        if let Some(file) = self.open_file.clone().filter(|f| is_scad(f)) {
            self.preview.update(cx, |p, cx| p.render(&file, cx));
        }
    }

    fn goto_line(&mut self, line: usize, window: &mut Window, cx: &mut Context<Self>) {
        let pos = Position::new(line.saturating_sub(1) as u32, 0);
        self.editor.update(cx, |e, cx| {
            e.set_cursor_position(pos, window, cx);
            e.focus(window, cx);
        });
    }

    // ---------------------------------------------------------------------
    // Actions
    // ---------------------------------------------------------------------

    fn open_folder(&mut self, _: &OpenFolder, _window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open Project".into()),
        });
        cx.spawn_in(_window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(path) = paths.into_iter().next()
            {
                this.update_in(cx, |this, window, cx| this.open_path(&path, window, cx))
                    .ok();
            }
        })
        .detach();
    }

    fn new_file(&mut self, _: &NewFile, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.project.as_ref().map(|p| p.root().to_path_buf()) else {
            self.open_folder(&OpenFolder, window, cx);
            return;
        };
        let rx = cx.prompt_for_new_path(&root, Some("untitled.scad"));
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(path))) = rx.await {
                if !path.exists() {
                    let _ = std::fs::write(&path, "// New OpenSCAD design\n\ncube(10);\n");
                }
                this.update_in(cx, |this, window, cx| {
                    this.refresh_files(cx);
                    this.open_file(&path, window, cx);
                })
                .ok();
            }
        })
        .detach();
    }

    fn export(&mut self, target: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(engine), Some(file)) = (self.engine.clone(), self.open_file.clone()) else {
            return;
        };
        self.save(window, cx);
        let job = cx.background_spawn(async move {
            engine
                .export(&osc_engine::Request::new(&file), &target)
                .map(|r| (r, target))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok((r, target)) if r.success => {
                    window.push_notification(
                        Notification::success(format!(
                            "Exported {} in {} ms",
                            target.display(),
                            r.duration_ms
                        )),
                        cx,
                    );
                    this.refresh_files(cx);
                }
                Ok((r, _)) => {
                    this.diagnostics = r.diagnostics;
                    this.right_tab = RightTab::Console;
                    window.push_notification(
                        Notification::error("Export failed, see the console"),
                        cx,
                    );
                }
                Err(e) => window.push_notification(Notification::error(e.to_string()), cx),
            })
            .ok();
        })
        .detach();
    }

    fn export_stl(&mut self, _: &ExportStl, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(file) = self.open_file.clone() {
            self.export(file.with_extension("stl"), window, cx);
        }
    }

    fn export_as(&mut self, _: &ExportAs, window: &mut Window, cx: &mut Context<Self>) {
        let Some(file) = self.open_file.clone() else {
            return;
        };
        let dir = file.parent().unwrap_or(Path::new(".")).to_path_buf();
        let name = format!(
            "{}.3mf",
            file.file_stem()
                .map(|s| s.to_string_lossy())
                .unwrap_or_default()
        );
        let rx = cx.prompt_for_new_path(&dir, Some(&name));
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(path))) = rx.await {
                this.update_in(cx, |this, window, cx| {
                    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                    if osc_engine::ExportFormat::from_extension(ext).is_err() {
                        window.push_notification(
                            Notification::error(
                                "Unsupported format. Use .stl, .3mf, .off, .amf, .obj, .wrl, .dxf, .svg, .pdf, .png or .csg",
                            ),
                            cx,
                        );
                    } else {
                        this.export(path, window, cx);
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    fn view(&mut self, view: View, cx: &mut Context<Self>) {
        self.preview.update(cx, |p, cx| p.set_view(view, cx));
    }

    // ---------------------------------------------------------------------
    // Rendering
    // ---------------------------------------------------------------------

    fn render_title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let project_label = self
            .project
            .as_ref()
            .map(|p| p.name())
            .unwrap_or_else(|| "Open a project".into());
        let recent: Vec<(usize, String)> = self
            .recent
            .iter()
            .enumerate()
            .map(|(ix, r)| (ix, format!("{}  —  {}", r.name, r.root.display())))
            .collect();
        let branch = self.git.read(cx).branch().map(str::to_owned);
        TitleBar::new().child(
            h_flex()
                .w_full()
                .pr_2()
                .gap_1()
                .child(self.menu_bar.clone())
                .child(
                    Button::new("project-switcher")
                        .label(project_label)
                        .icon(IconName::FolderOpen)
                        .ghost()
                        .small()
                        .tooltip("Switch project (recent projects)")
                        .dropdown_menu(move |menu, _, _| {
                            let mut menu =
                                menu.menu("Open Folder…", Box::new(OpenFolder)).separator();
                            for (ix, label) in &recent {
                                menu = menu.menu(label.clone(), Box::new(OpenRecent { ix: *ix }));
                            }
                            menu
                        }),
                )
                .when_some(branch, |el, b| {
                    el.child(
                        h_flex()
                            .gap_1()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(Icon::new(gpui_kit::assets::IconName::GitBranch).xsmall())
                            .child(b),
                    )
                })
                .child(div().flex_1())
                .child(
                    Button::new("toggle-left")
                        .icon(IconName::PanelLeft)
                        .ghost()
                        .small()
                        .selected(self.show_left)
                        .tooltip_with_action(
                            "Project panel",
                            &ToggleProjectPanel,
                            Some("Workspace"),
                        )
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.show_left = !this.show_left;
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("toggle-console")
                        .icon(IconName::PanelBottom)
                        .ghost()
                        .small()
                        .selected(self.show_console)
                        .tooltip_with_action(
                            "Customizer & console",
                            &ToggleConsole,
                            Some("Workspace"),
                        )
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.show_console = !this.show_console;
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("toggle-agent")
                        .icon(IconName::PanelRight)
                        .ghost()
                        .small()
                        .selected(self.show_agent)
                        .tooltip_with_action("Agent panel", &ToggleAgentPanel, Some("Workspace"))
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.show_agent = !this.show_agent;
                            cx.notify();
                        })),
                ),
        )
    }

    fn render_left(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let tabs = TabBar::new("left-tabs")
            .underline()
            .small()
            .selected_index(match self.left_tab {
                LeftTab::Files => 0,
                LeftTab::Outline => 1,
                LeftTab::Git => 2,
                LeftTab::Settings => 3,
            })
            .child(Tab::new().label("Files"))
            .child(Tab::new().label("Outline"))
            .child(Tab::new().label("Git"))
            .child(Tab::new().label("Settings"))
            .on_click(cx.listener(|this, ix: &usize, _, cx| {
                this.left_tab = match ix {
                    1 => LeftTab::Outline,
                    2 => LeftTab::Git,
                    3 => LeftTab::Settings,
                    _ => LeftTab::Files,
                };
                cx.notify();
            }));

        let body: AnyElement = match self.left_tab {
            LeftTab::Files => {
                let weak = cx.entity().downgrade();
                let root = self.project.as_ref().map(|p| p.root().to_path_buf());
                let open = self.open_file.clone();
                Tree::new(
                    &self.file_tree,
                    move |ix, entry: &TreeEntry, _selected, _, _| {
                        let id = entry.item().id.to_string();
                        let path = root.as_ref().map(|r| r.join(&id));
                        let is_open = path.is_some() && path == open;
                        let icon = if entry.is_folder() {
                            if entry.is_expanded() {
                                IconName::FolderOpen
                            } else {
                                IconName::Folder
                            }
                        } else {
                            IconName::File
                        };
                        let weak = weak.clone();
                        let is_folder = entry.is_folder();
                        ListItem::new(ix)
                            .selected(is_open)
                            .pl(px(8.) + px(12.) * entry.depth() as f32)
                            .child(
                                h_flex()
                                    .gap_1p5()
                                    .text_sm()
                                    .child(Icon::new(icon).small())
                                    .child(entry.item().label.clone()),
                            )
                            .on_click(move |_, window, cx| {
                                if is_folder {
                                    return;
                                }
                                if let (Some(view), Some(path)) = (weak.upgrade(), path.clone()) {
                                    view.update(cx, |this, cx| this.open_file(&path, window, cx));
                                }
                            })
                    },
                )
                .size_full()
                .into_any_element()
            }
            LeftTab::Outline => {
                let text = self.current_text(cx);
                let symbols = osc_syntax::outline(&text);
                let mut list = v_flex().p_1();
                if symbols.is_empty() {
                    list = list.child(
                        div()
                            .p_2()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child("No modules or functions"),
                    );
                }
                for (ix, s) in symbols.into_iter().enumerate() {
                    let line = s.line;
                    let kind = match s.kind {
                        osc_syntax::SymbolKind::Module => "module",
                        osc_syntax::SymbolKind::Function => "function",
                    };
                    list = list.child(
                        div()
                            .id(("sym", ix))
                            .px_2()
                            .py_0p5()
                            .pl(px(8.) + px(12.) * s.depth as f32)
                            .rounded_sm()
                            .cursor_pointer()
                            .hover(|st| st.bg(theme.list_hover))
                            .text_sm()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(theme.muted_foreground)
                                            .child(kind),
                                    )
                                    .child(s.name)
                                    .child(div().flex_1())
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(theme.muted_foreground)
                                            .child(format!("{line}")),
                                    ),
                            )
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                this.goto_line(line, window, cx)
                            })),
                    );
                }
                div()
                    .size_full()
                    .overflow_y_scrollbar()
                    .child(list)
                    .into_any_element()
            }
            LeftTab::Git => self.git.clone().into_any_element(),
            LeftTab::Settings => self.settings.clone().into_any_element(),
        };
        v_flex()
            .size_full()
            .bg(theme.sidebar)
            .child(tabs)
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element()
    }

    fn render_console(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let mut list = v_flex().p_2().gap_0p5();
        let shown: Vec<_> = self
            .diagnostics
            .iter()
            .filter(|d| d.severity != Severity::Info || self.diagnostics.len() < 40)
            .collect();
        if shown.is_empty() {
            list = list.child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("No output."),
            );
        }
        for (ix, d) in shown.into_iter().enumerate() {
            let color = match d.severity {
                Severity::Error => theme.danger,
                Severity::Warning => theme.warning,
                Severity::Echo => theme.info,
                Severity::Info => theme.muted_foreground,
            };
            let line = d.line;
            list = list.child(
                div()
                    .id(("diag", ix))
                    .text_xs()
                    .font_family(theme.mono_font_family.clone())
                    .text_color(color)
                    .when(line.is_some(), |el| {
                        el.cursor_pointer().hover(|s| s.bg(theme.list_hover))
                    })
                    .child(match d.severity {
                        Severity::Error => format!("ERROR: {}", d.message),
                        Severity::Warning => format!("WARNING: {}", d.message),
                        Severity::Echo => format!("ECHO: {}", d.message),
                        Severity::Info => d.message.clone(),
                    })
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        if let Some(line) = line {
                            this.goto_line(line, window, cx);
                        }
                    })),
            );
        }
        div()
            .size_full()
            .overflow_y_scrollbar()
            .child(list)
            .into_any_element()
    }

    fn render_right_bottom(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let errors = self
            .diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count();
        v_flex()
            .size_full()
            .child(
                TabBar::new("right-tabs")
                    .underline()
                    .small()
                    .selected_index(if self.right_tab == RightTab::Customizer {
                        0
                    } else {
                        1
                    })
                    .child(Tab::new().label("Customizer"))
                    .child(Tab::new().label(if errors > 0 {
                        format!(
                            "Console ({errors} error{})",
                            if errors == 1 { "" } else { "s" }
                        )
                    } else {
                        "Console".to_owned()
                    }))
                    .on_click(cx.listener(|this, ix: &usize, _, cx| {
                        this.right_tab = if *ix == 0 {
                            RightTab::Customizer
                        } else {
                            RightTab::Console
                        };
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .bg(theme.background)
                    .child(match self.right_tab {
                        RightTab::Customizer => self.customizer.clone().into_any_element(),
                        RightTab::Console => self.render_console(cx),
                    }),
            )
            .into_any_element()
    }

    fn render_welcome(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let mut recent = v_flex().gap_1().w(px(480.));
        for (ix, r) in self.recent.iter().take(10).enumerate() {
            let root = r.root.clone();
            recent = recent.child(
                div()
                    .id(("recent", ix))
                    .px_3()
                    .py_1p5()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.list_hover))
                    .child(div().text_sm().child(r.name.clone()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(r.root.display().to_string()),
                    )
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.open_path(&root, window, cx)
                    })),
            );
        }
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_4()
            .child(
                div()
                    .text_2xl()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("OpenSuperCAD"),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child("Design parametric 3D models in OpenSCAD with an AI agent."),
            )
            .child(
                Button::new("open-folder")
                    .label("Open Folder…")
                    .icon(IconName::FolderOpen)
                    .primary()
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.open_folder(&OpenFolder, window, cx)
                    })),
            )
            .when(!self.recent.is_empty(), |el| {
                el.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("RECENT PROJECTS"),
                )
                .child(recent)
            })
            .when(self.engine.is_none(), |el| {
                el.child(div().text_sm().text_color(theme.warning).child(
                    "OpenSCAD was not found. Rendering is disabled. Run `opensupercad doctor`.",
                ))
            })
            .into_any_element()
    }

    fn render_editor(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let title = match (&self.open_file, &self.project) {
            (Some(f), Some(p)) => p.relative(f),
            _ => "No file".into(),
        };
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .px_3()
                    .h(px(32.))
                    .gap_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .bg(theme.tab_bar)
                    .text_sm()
                    .child(
                        Icon::new(IconName::File)
                            .small()
                            .text_color(theme.muted_foreground),
                    )
                    .child(title)
                    .when(self.dirty, |el| {
                        el.child(div().text_color(theme.warning).child("●"))
                    })
                    .child(div().flex_1())
                    .child(
                        Button::new("btn-render")
                            .icon(IconName::Play)
                            .label("Render")
                            .ghost()
                            .xsmall()
                            .tooltip_with_action("Render (F6)", &RenderDesign, Some("Workspace"))
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.save(window, cx);
                                this.render(cx);
                            })),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(Editor::new(&self.editor).size_full().bordered(false)),
            )
            .into_any_element()
    }

    fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let cursor = self.editor.read(cx).cursor_position();
        let preview_msg = self.preview.read(cx).last_message.clone();
        h_flex()
            .h(px(26.))
            .px_3()
            .gap_4()
            .text_xs()
            .text_color(theme.muted_foreground)
            .bg(theme.status_bar)
            .border_t_1()
            .border_color(theme.status_bar_border)
            .child(
                self.openscad_version
                    .clone()
                    .unwrap_or_else(|| "OpenSCAD not found".into()),
            )
            .when_some(preview_msg, |el, m| el.child(m))
            .child(div().flex_1())
            .child(format!("Agent: {}", self.agent.read(cx).agent_name()))
            .child(format!(
                "Ln {}, Col {}",
                cursor.line + 1,
                cursor.character + 1
            ))
    }
}

type ControlMsg = (ControlRequest, std::sync::mpsc::Sender<ControlReply>);

/// Listen on a per-window socket and handle requests on the UI thread.
fn start_control(window: &mut Window, cx: &mut Context<Workspace>) -> Option<PathBuf> {
    let (tx, rx) = async_channel::unbounded::<ControlMsg>();
    let socket = crate::pipeline::scratch_dir().join("control.sock");
    #[cfg(unix)]
    osc_mcp::control::serve(&socket, move |req| {
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        if tx.send_blocking((req, reply_tx)).is_err() {
            return ControlReply::error("window closed");
        }
        reply_rx
            .recv_timeout(Duration::from_secs(4))
            .unwrap_or_else(|_| ControlReply::error("the app did not answer"))
    })
    .ok()?;
    cx.spawn_in(window, async move |this, cx| {
        while let Ok((req, reply)) = rx.recv().await {
            let answer = this
                .update_in(cx, |this, window, cx| this.on_control(req, window, cx))
                .unwrap_or_else(|_| ControlReply::error("window closed"));
            let _ = reply.send(answer);
        }
    })
    .detach();
    Some(socket)
}

impl Workspace {
    fn on_control(
        &mut self,
        req: ControlRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ControlReply {
        match req {
            ControlRequest::SaveAll => {
                if self.dirty {
                    self.save(window, cx);
                }
                ControlReply::ok()
            }
            ControlRequest::Snapshot { file, images } => {
                use base64::Engine as _;
                let images = images
                    .into_iter()
                    .filter_map(|i| {
                        base64::engine::general_purpose::STANDARD
                            .decode(i.png_base64)
                            .ok()
                            .map(|png| (i.label, png))
                    })
                    .collect();
                self.agent
                    .update(cx, |a, cx| a.add_snapshots(file, images, cx));
                ControlReply::ok()
            }
            ControlRequest::Camera { rotation } => {
                self.preview
                    .update(cx, |p, cx| p.set_rotation(rotation, cx));
                ControlReply::ok()
            }
        }
    }
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn is_scad(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "scad")
}

/// Turn sorted project-relative paths into a folder tree (folders first).
fn build_tree(files: &[String]) -> Vec<TreeItem> {
    #[derive(Default)]
    struct Node {
        dirs: std::collections::BTreeMap<String, Node>,
        files: Vec<String>,
    }
    let mut root = Node::default();
    for f in files {
        let parts: Vec<&str> = f.split('/').collect();
        let mut node = &mut root;
        for dir in &parts[..parts.len() - 1] {
            node = node.dirs.entry((*dir).to_owned()).or_default();
        }
        node.files.push(f.clone());
    }
    fn convert(node: &Node, prefix: &str) -> Vec<TreeItem> {
        let mut out = Vec::new();
        for (name, child) in &node.dirs {
            let id = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            out.push(
                TreeItem::new(id.clone(), name.clone())
                    .expanded(true)
                    .children(convert(child, &id)),
            );
        }
        for f in &node.files {
            let label = f.rsplit('/').next().unwrap_or(f).to_owned();
            out.push(TreeItem::new(f.clone(), label));
        }
        out
    }
    convert(&root, "")
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl gpui_kit::Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let _ = window;

        let main: AnyElement = if self.project.is_none() {
            self.render_welcome(cx)
        } else {
            let center = h_resizable("center")
                .child(resizable_panel().child(self.render_editor(cx)))
                .child(
                    resizable_panel().size(px(520.)).child(
                        v_resizable("preview-column")
                            .child(resizable_panel().child(self.preview.clone()))
                            .child(
                                resizable_panel()
                                    .size(px(300.))
                                    .visible(self.show_console)
                                    .child(self.render_right_bottom(cx)),
                            ),
                    ),
                );
            h_resizable("workspace")
                .child(
                    resizable_panel()
                        .size(px(240.))
                        .size_range(px(160.)..px(480.))
                        .visible(self.show_left)
                        .child(self.render_left(cx)),
                )
                .child(resizable_panel().child(center))
                .child(
                    resizable_panel()
                        .size(px(400.))
                        .size_range(px(280.)..px(800.))
                        .visible(self.show_agent)
                        .child(self.agent.clone()),
                )
                .into_any_element()
        };

        v_flex()
            .id("workspace")
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .key_context("Workspace")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::open_folder))
            .on_action(cx.listener(Self::new_file))
            .on_action(cx.listener(Self::export_stl))
            .on_action(cx.listener(Self::export_as))
            .on_action(cx.listener(|this, a: &OpenRecent, window, cx| {
                if let Some(r) = this.recent.get(a.ix).cloned() {
                    this.open_path(&r.root, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &Save, window, cx| this.save(window, cx)))
            .on_action(cx.listener(|this, _: &Reload, window, cx| {
                this.dirty = false;
                this.disk_mtime = None;
                if let Some(f) = this.open_file.clone() {
                    this.open_file(&f, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &RenderDesign, window, cx| {
                this.save(window, cx);
                this.render(cx);
            }))
            .on_action(cx.listener(|this, _: &PreviewDesign, window, cx| {
                this.save(window, cx);
                if let Some(f) = this.open_file.clone() {
                    this.preview.update(cx, |p, cx| p.openscad_preview(&f, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &Checkpoint, _, cx| {
                this.git
                    .update(cx, |g, cx| g.checkpoint("Manual checkpoint", cx));
            }))
            .on_action(cx.listener(|this, _: &NewThread, _, cx| {
                this.show_agent = true;
                this.agent.update(cx, |a, cx| a.new_thread(cx));
            }))
            .on_action(
                cx.listener(|this, _: &StopAgent, _, cx| this.agent.update(cx, |a, cx| a.stop(cx))),
            )
            .on_action(cx.listener(|this, _: &ToggleProjectPanel, _, cx| {
                this.show_left = !this.show_left;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleConsole, _, cx| {
                this.show_console = !this.show_console;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleAgentPanel, _, cx| {
                this.show_agent = !this.show_agent;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &FocusGit, _, cx| {
                this.show_left = true;
                this.left_tab = LeftTab::Git;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ViewTop, _, cx| this.view(View::Top, cx)))
            .on_action(cx.listener(|this, _: &ViewBottom, _, cx| this.view(View::Bottom, cx)))
            .on_action(cx.listener(|this, _: &ViewLeft, _, cx| this.view(View::Left, cx)))
            .on_action(cx.listener(|this, _: &ViewRight, _, cx| this.view(View::Right, cx)))
            .on_action(cx.listener(|this, _: &ViewFront, _, cx| this.view(View::Front, cx)))
            .on_action(cx.listener(|this, _: &ViewBack, _, cx| this.view(View::Back, cx)))
            .on_action(cx.listener(|this, _: &ViewDiagonal, _, cx| this.view(View::Diagonal, cx)))
            .on_action(cx.listener(|this, _: &ResetView, _, cx| {
                this.preview.update(cx, |p, cx| p.reset_view(cx))
            }))
            .child(self.render_title_bar(cx))
            .child(div().flex_1().min_h_0().child(main))
            .child(self.render_status_bar(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::build_tree;

    #[test]
    fn tree_groups_folders() {
        let files = vec![
            "a.scad".to_owned(),
            "lib/b.scad".to_owned(),
            "lib/x/c.scad".to_owned(),
        ];
        let tree = build_tree(&files);
        assert_eq!(tree.len(), 2);
        assert_eq!(tree[0].id.as_ref(), "lib");
        assert_eq!(tree[1].id.as_ref(), "a.scad");
    }
}
