//! The main window: Zed-style docks around the editor and the preview.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
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
    /// An OpenSCAD download in progress: bytes received and expected.
    openscad_download: Option<(Arc<AtomicU64>, u64)>,
    project: Option<Project>,
    recent: Vec<RecentProject>,
    files: Vec<String>,
    file_tree: Entity<TreeState>,
    open_file: Option<PathBuf>,
    /// The active tab's editor (a blank editor when no file is open). The
    /// active tab's `dirty`/`disk_mtime` live in the fields below and are
    /// stashed back into `tabs` when switching.
    editor: Entity<EditorState>,
    blank_editor: Entity<EditorState>,
    dirty: bool,
    disk_mtime: Option<SystemTime>,
    tabs: Vec<OpenTab>,
    /// Keeps the active tab scrolled into view when the tabs overflow.
    tab_scroll: ScrollHandle,
    active_tab: Option<usize>,
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
                    AgentPanelEvent::ShowCheckpointDiff(id) => {
                        let id = id.clone();
                        this.show_checkpoint_diff(&id, "AI turn", window, cx);
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
                    let settings = match ev {
                        SettingsEvent::Changed(settings) => settings,
                        SettingsEvent::DownloadOpenScad => {
                            return this.download_openscad(window, cx);
                        }
                        SettingsEvent::LocateOpenScad => {
                            return this.locate_openscad(window, cx);
                        }
                        SettingsEvent::AutoOpenScad => return this.auto_openscad(window, cx),
                    };
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
                    GitEvent::ShowDiff { id, title } => {
                        let (id, title) = (id.clone(), title.clone());
                        this.show_checkpoint_diff(&id, &title, window, cx);
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
            openscad_download: None,
            project: None,
            files: Vec::new(),
            file_tree,
            open_file: None,
            blank_editor: editor.clone(),
            editor,
            dirty: false,
            disk_mtime: None,
            tabs: Vec::new(),
            tab_scroll: ScrollHandle::new(),
            active_tab: None,
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

        ws.push_openscad_status(cx);
        let start = path.or_else(|| ws.recent.first().map(|r| r.root.clone()));
        if let Some(path) = start {
            ws.open_path(&path, window, cx);
        }
        // Focus the editor so shortcuts like Ctrl-P work before the first click.
        let editor = ws.editor.clone();
        window.defer(cx, move |window, cx| {
            editor.update(cx, |e, cx| e.focus(window, cx));
        });
        // After the window's root exists, so the dialog has somewhere to go.
        window.defer(cx, super::report::offer_crash_report);
        if ws.base_engine.is_none() {
            let this = cx.entity().downgrade();
            window.defer(cx, move |window, cx| {
                super::openscad_setup::offer(this, window, cx)
            });
        }
        window.defer(cx, super::updates::startup_check);
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
        self.save_all(window, cx);
        self.close_all_tabs(window, cx);
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

    /// Switch to another OpenSCAD (after a download or *Locate…*).
    fn set_base_engine(&mut self, engine: Option<Engine>, cx: &mut Context<Self>) {
        self.openscad_version = engine.as_ref().and_then(|e| e.version().ok());
        self.base_engine = engine;
        if self.base_engine.is_some() {
            let backend = self
                .project
                .as_ref()
                .and_then(|p| p.settings().openscad_backend);
            self.apply_backend(backend, cx);
        } else {
            self.engine = None;
            self.preview.update(cx, |p, _| p.set_engine(None));
        }
        self.push_openscad_status(cx);
        self.render(cx);
        cx.notify();
    }

    /// Tell the Settings panel which OpenSCAD is in use.
    fn push_openscad_status(&mut self, cx: &mut Context<Self>) {
        let status = super::settings_panel::OpenScadStatus {
            summary: match (&self.base_engine, &self.openscad_version) {
                (Some(e), v) => format!(
                    "{} ({})",
                    v.as_deref().unwrap_or("OpenSCAD"),
                    crate::openscad::origin(e)
                ),
                (None, _) => "Not found: rendering is disabled.".into(),
            },
            path: self
                .base_engine
                .as_ref()
                .map(|e| e.binary.display().to_string()),
            chosen: self
                .base_engine
                .as_ref()
                .is_some_and(|e| crate::openscad::origin(e) == "chosen by you"),
            download: osc_update::openscad::build_for_this_platform()
                .map(|b| crate::openscad::describe(&b)),
            downloading: self.openscad_download.is_some(),
        };
        self.settings
            .update(cx, |s, cx| s.set_openscad_status(status, cx));
    }

    /// Download, verify and install the pinned OpenSCAD, then use it.
    pub(super) fn download_openscad(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.openscad_download.is_some() {
            return;
        }
        let Some(build) = osc_update::openscad::build_for_this_platform() else {
            window.push_notification(
                Notification::error(
                    "There is no OpenSCAD download for this platform. Install it from \
                     openscad.org, then use Locate OpenSCAD….",
                ),
                cx,
            );
            return;
        };
        let label = crate::openscad::describe(&build);
        let received = Arc::new(AtomicU64::new(0));
        self.openscad_download = Some((received.clone(), build.size));
        self.push_openscad_status(cx);
        window.push_notification(Notification::info(format!("Downloading {label}…")), cx);
        let job = cx.background_spawn(async move {
            crate::openscad::install(move |p| {
                if let osc_update::openscad::Progress::Downloading { received: r, .. } = p {
                    received.store(r, Ordering::Relaxed);
                }
            })
        });
        // Repaint the status bar's progress while it runs.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(300))
                    .await;
                let running = this
                    .update(cx, |this, cx| {
                        cx.notify();
                        this.openscad_download.is_some()
                    })
                    .unwrap_or(false);
                if !running {
                    break;
                }
            }
        })
        .detach();
        cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            this.update_in(cx, |this, window, cx| {
                this.openscad_download = None;
                match result {
                    Ok(_) => {
                        this.set_base_engine(Engine::discover().ok(), cx);
                        window.push_notification(
                            Notification::success(format!(
                                "{label} is installed (SHA-256 verified)."
                            )),
                            cx,
                        );
                    }
                    Err(e) => {
                        this.push_openscad_status(cx);
                        window.push_notification(
                            Notification::error(format!("Downloading OpenSCAD failed: {e}")),
                            cx,
                        );
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    /// Let the user pick the OpenSCAD to use.
    pub(super) fn locate_openscad(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let start = if cfg!(target_os = "macos") {
            PathBuf::from("/Applications")
        } else {
            dirs::home_dir().unwrap_or_default()
        };
        let this = cx.entity().downgrade();
        super::path_prompt::file(
            "Locate OpenSCAD",
            "Path of the OpenSCAD program (or OpenSCAD.app)",
            &start,
            window,
            cx,
            move |path, window, cx| {
                let result = crate::openscad::locate(&path);
                this.update(cx, |this, cx| match result {
                    Ok(engine) => {
                        let binary = engine.binary.display().to_string();
                        this.set_base_engine(Some(engine), cx);
                        window.push_notification(
                            Notification::success(format!("Using {binary}")),
                            cx,
                        );
                    }
                    Err(e) => window.push_notification(
                        Notification::error(format!("Can't use that OpenSCAD: {e}")),
                        cx,
                    ),
                })
                .ok();
            },
        );
    }

    /// Forget the chosen OpenSCAD and find one automatically again.
    fn auto_openscad(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(e) = crate::openscad::use_automatic() {
            window.push_notification(Notification::error(e.to_string()), cx);
            return;
        }
        self.set_base_engine(Engine::discover().ok(), cx);
        let msg = match &self.base_engine {
            Some(e) => format!("Using {}", e.binary.display()),
            None => "OpenSCAD was not found.".into(),
        };
        window.push_notification(Notification::info(msg), cx);
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

    /// Open a file in a tab (or switch to its existing tab).
    fn open_file(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.tabs.iter().position(|t| t.path == path) {
            self.activate_tab(ix, window, cx);
            return;
        }
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                window
                    .push_notification(Notification::error(format!("{}: {e}", path.display())), cx);
                return;
            }
        };
        let language = match path.extension().and_then(|e| e.to_str()) {
            Some("scad") => "openscad",
            Some("json") => "json",
            _ => "plain",
        };
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language(language)
                .line_number(true)
                .soft_wrap(language == "plain")
                .default_value(text)
        });
        let sub = cx.subscribe_in(&editor, window, Self::on_editor_event);
        self.tabs.push(OpenTab {
            path: path.to_path_buf(),
            editor,
            dirty: false,
            disk_mtime: mtime(path),
            _sub: sub,
        });
        self.activate_tab(self.tabs.len() - 1, window, cx);
    }

    /// Write the active tab's state back into `tabs`.
    fn stash_active(&mut self) {
        if let Some(tab) = self.active_tab.and_then(|ix| self.tabs.get_mut(ix)) {
            tab.dirty = self.dirty;
            tab.disk_mtime = self.disk_mtime;
        }
    }

    fn activate_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(ix) else { return };
        let (path, editor, dirty, disk_mtime) = (
            tab.path.clone(),
            tab.editor.clone(),
            tab.dirty,
            tab.disk_mtime,
        );
        let previous_target = self.render_target();
        self.stash_active();
        self.active_tab = Some(ix);
        self.editor = editor;
        self.open_file = Some(path.clone());
        self.dirty = dirty;
        self.disk_mtime = disk_mtime;
        self.refresh_customizer(window, cx);
        if let Some(project) = &self.project {
            let rel = project.relative(&path);
            let _ = self.store.remember(project.root(), Some(&rel), None);
        }
        self.check_disk(window, cx);
        // Only re-render when the rendered file actually changes.
        if self.render_target() != previous_target {
            self.diagnostics.clear();
            self.render(cx);
        }
        self.apply_editor_diagnostics(cx);
        self.tab_scroll.scroll_to_item(ix);
        self.editor.update(cx, |e, cx| e.focus(window, cx));
        cx.notify();
    }

    /// Close a tab, saving it first if it has unsaved edits.
    fn close_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.tabs.len() {
            return;
        }
        if Some(ix) == self.active_tab {
            if self.dirty {
                self.save(window, cx);
            }
        } else if self.tabs[ix].dirty {
            let text = self.tabs[ix].editor.read(cx).value().to_string();
            let _ = std::fs::write(&self.tabs[ix].path, text);
        }
        self.stash_active();
        self.tabs.remove(ix);
        match self.active_tab {
            Some(active) if active == ix => {
                self.active_tab = None;
                if self.tabs.is_empty() {
                    self.show_blank(window, cx);
                } else {
                    self.activate_tab(ix.min(self.tabs.len() - 1), window, cx);
                }
            }
            Some(active) if active > ix => self.active_tab = Some(active - 1),
            _ => {}
        }
        cx.notify();
    }

    fn close_all_tabs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tabs.clear();
        self.active_tab = None;
        self.show_blank(window, cx);
    }

    fn show_blank(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor = self.blank_editor.clone();
        self.open_file = None;
        self.dirty = false;
        self.disk_mtime = None;
        self.customizer
            .update(cx, |c, cx| c.sync(Vec::new(), window, cx));
    }

    fn cycle_tab(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(active), n) = (self.active_tab, self.tabs.len()) else {
            return;
        };
        if n > 1 {
            let next = (active as isize + delta).rem_euclid(n as isize) as usize;
            self.activate_tab(next, window, cx);
        }
    }

    /// Save every tab with unsaved edits.
    fn save_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dirty {
            self.save(window, cx);
        }
        for tab in &mut self.tabs {
            if tab.dirty {
                let text = tab.editor.read(cx).value().to_string();
                if std::fs::write(&tab.path, text).is_ok() {
                    tab.dirty = false;
                    tab.disk_mtime = mtime(&tab.path);
                }
            }
        }
    }

    /// The file the preview shows: the active file, unless it is a library
    /// pulled in by the project's main file (`use`/`include`), in which case
    /// the main file is rendered so the change is seen in context.
    fn render_target(&self) -> Option<PathBuf> {
        let active = self.open_file.clone().filter(|f| is_scad(f))?;
        let project = self.project.as_ref()?;
        let main = project.main_file().map(|m| project.root().join(m))?;
        if main == active {
            return Some(active);
        }
        let main_src = std::fs::read_to_string(&main).unwrap_or_default();
        let rel = project.relative(&active);
        let name = active.file_name()?.to_string_lossy().into_owned();
        let referenced = main_src.lines().any(|l| {
            let l = l.trim_start();
            (l.starts_with("use") || l.starts_with("include"))
                && (l.contains(&rel) || l.contains(&name))
        });
        Some(if referenced { main } else { active })
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
        self.stash_active();
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
                self.refresh_customizer(window, cx);
                self.render(cx);
            }
        }
        self.git.update(cx, |g, cx| g.refresh(cx));
    }

    /// The file the customizer edits: the rendered file (see
    /// [`Self::render_target`]), with its editor if it is open in a tab.
    fn customizer_target(&self) -> Option<(PathBuf, Option<Entity<EditorState>>)> {
        let target = self.render_target().or_else(|| self.open_file.clone())?;
        let editor = if Some(&target) == self.open_file.as_ref() {
            Some(self.editor.clone())
        } else {
            self.tabs
                .iter()
                .find(|t| t.path == target)
                .map(|t| t.editor.clone())
        };
        Some((target, editor))
    }

    fn refresh_customizer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = match self.customizer_target() {
            Some((_, Some(editor))) => editor.read(cx).value().to_string(),
            Some((path, None)) => std::fs::read_to_string(path).unwrap_or_default(),
            None => String::new(),
        };
        self.sync_customizer(&text, window, cx);
    }

    fn sync_customizer(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let params = osc_syntax::customizer::parameters(text);
        self.customizer
            .update(cx, |c, cx| c.sync(params, window, cx));
    }

    fn on_editor_event(
        &mut self,
        editor: &Entity<EditorState>,
        ev: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if *editor != self.editor {
            if let InputEvent::Change = ev
                && let Some(tab) = self.tabs.iter_mut().find(|t| t.editor == *editor)
            {
                tab.dirty = true;
            }
            return;
        }
        if let InputEvent::Change = ev {
            self.dirty = true;
            self.refresh_customizer(window, cx);
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
        let Some((target, editor)) = self.customizer_target() else {
            return;
        };
        let active = Some(&target) == self.open_file.as_ref();
        let text = match &editor {
            Some(e) => e.read(cx).value().to_string(),
            None => std::fs::read_to_string(&target).unwrap_or_default(),
        };
        let Ok(updated) = osc_syntax::customizer::set_parameter(&text, name, value) else {
            return;
        };
        if updated == text {
            return;
        }
        self.param_gen += 1;
        let generation = self.param_gen;
        if !active {
            // The parameters belong to the main file while a library is being
            // edited: update it directly (and its tab, if open), then render.
            if let Some(e) = &editor {
                e.update(cx, |e, cx| e.set_value(updated.clone(), window, cx));
            }
            if std::fs::write(&target, &updated).is_ok()
                && let Some(tab) = self.tabs.iter_mut().find(|t| t.path == target)
            {
                tab.dirty = false;
                tab.disk_mtime = mtime(&target);
            }
            // No editor event fires for the active tab here, so refresh the
            // customizer's values (labels) from the new source directly.
            self.sync_customizer(&updated, window, cx);
            cx.spawn_in(window, async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(350))
                    .await;
                this.update_in(cx, |this, _, cx| {
                    if this.param_gen == generation {
                        this.render(cx);
                    }
                })
                .ok();
            })
            .detach();
            return;
        }
        self.editor
            .update(cx, |e, cx| e.replace_all(updated, window, cx));
        self.dirty = true;
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
        if let Some(file) = self.render_target() {
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

    fn open_folder(&mut self, _: &OpenFolder, window: &mut Window, cx: &mut Context<Self>) {
        let start = self
            .project
            .as_ref()
            .and_then(|p| p.root().parent().map(Path::to_path_buf))
            .or_else(dirs::home_dir)
            .unwrap_or_default();
        let this = cx.entity().downgrade();
        super::path_prompt::folder(
            "Open Project",
            &start,
            window,
            cx,
            move |path, window, cx| {
                this.update(cx, |this, cx| this.open_path(&path, window, cx))
                    .ok();
            },
        );
    }

    /// Open a read-only, highlighted diff of what a checkpoint changed.
    fn show_checkpoint_diff(
        &mut self,
        id: &str,
        title: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.project.as_ref().map(|p| p.root().to_path_buf()) else {
            return;
        };
        let diff = osc_git::Repo::discover(&root)
            .and_then(|repo| repo.diff_commit(id))
            .unwrap_or_else(|e| format!("Could not compute the diff: {e}"));
        let diff = if diff.trim().is_empty() {
            "No changes.".to_owned()
        } else {
            diff
        };
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("diff")
                .line_number(false)
                .default_value(diff)
        });
        let short: String = id.chars().take(8).collect();
        let heading = format!("{short} · {title}");
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .title(heading.clone())
                .w(px(920.))
                .margin_top(px(60.))
                .child(
                    div()
                        .h(px(560.))
                        .child(Editor::new(&editor).size_full().readonly(true)),
                )
        });
    }

    /// Settings belong to the app (and the open project), not to a sidebar
    /// tab: they open in a dialog from File → Settings… or Ctrl/Cmd-, (#84).
    fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let settings = self.settings.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .title("Settings")
                .w(px(560.))
                .margin_top(px(60.))
                .child(div().h(px(600.)).child(settings.clone()))
        });
    }

    fn command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let commands = super::picker::commands();
        let items = commands
            .iter()
            .map(|(label, action)| {
                let mut item = super::picker::PickerItem::new(*label);
                item.action = Some(action.clone());
                item
            })
            .collect();
        super::picker::open(
            "Run a command…",
            items,
            move |ix, _, window, cx| {
                if let Some((_, action)) = commands.get(ix) {
                    window.dispatch_action(action.boxed_clone(), cx);
                }
            },
            window,
            cx,
        );
    }

    fn find_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.project.as_ref().map(|p| p.root().to_path_buf()) else {
            return self.recent_projects(window, cx);
        };
        let files = self.files.clone();
        let items = files
            .iter()
            .map(|f| {
                let name = f.rsplit('/').next().unwrap_or(f).to_owned();
                super::picker::PickerItem::new(name).detail(f.clone())
            })
            .collect();
        super::picker::open(
            "Open a project file…",
            items,
            move |ix, ws, window, cx| {
                if let Some(f) = files.get(ix) {
                    ws.open_file(&root.join(f), window, cx);
                }
            },
            window,
            cx,
        );
    }

    fn recent_projects(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let recent = self.recent.clone();
        let mut items: Vec<_> = recent
            .iter()
            .map(|r| {
                super::picker::PickerItem::new(r.name.clone()).detail(r.root.display().to_string())
            })
            .collect();
        items.push(super::picker::PickerItem::new("Open Folder…"));
        super::picker::open(
            "Switch project…",
            items,
            move |ix, ws, window, cx| match recent.get(ix) {
                Some(r) => ws.open_path(&r.root.clone(), window, cx),
                None => ws.open_folder(&OpenFolder, window, cx),
            },
            window,
            cx,
        );
    }

    fn new_file(&mut self, _: &NewFile, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.project.as_ref().map(|p| p.root().to_path_buf()) else {
            self.open_folder(&OpenFolder, window, cx);
            return;
        };
        let this = cx.entity().downgrade();
        super::path_prompt::new_path(
            "New File",
            "Path of the new file",
            &root,
            "untitled.scad",
            window,
            cx,
            move |path, window, cx| {
                if !path.exists() {
                    let _ = std::fs::write(&path, "// New OpenSCAD design\n\ncube(10);\n");
                }
                this.update(cx, |this, cx| {
                    this.refresh_files(cx);
                    this.open_file(&path, window, cx);
                })
                .ok();
            },
        );
    }

    fn export(&mut self, target: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let Some(engine) = self.engine.clone() else {
            window.push_notification(
                Notification::error("OpenSCAD was not found; run `opensupercad doctor`"),
                cx,
            );
            return;
        };
        let Some(file) = self.render_target() else {
            return;
        };
        self.save(window, cx);
        window.push_notification(
            Notification::info(format!(
                "Exporting {}…",
                target.file_name().unwrap_or_default().to_string_lossy()
            )),
            cx,
        );
        let job = cx.background_spawn(async move {
            engine
                .export(&osc_engine::Request::new(&file), &target)
                .map(|r| (r, target))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok((r, target)) if r.success => {
                    let shown = this
                        .project
                        .as_ref()
                        .map(|p| p.relative(&target))
                        .unwrap_or_else(|| target.display().to_string());
                    window.push_notification(
                        Notification::success(format!("Exported {shown} in {} ms", r.duration_ms)),
                        cx,
                    );
                    this.refresh_files(cx);
                }
                Ok((r, _)) => {
                    let reason = r
                        .errors()
                        .next()
                        .map(|d| d.message.clone())
                        .unwrap_or_else(|| "see the console".into());
                    this.diagnostics = r.diagnostics;
                    this.right_tab = RightTab::Console;
                    window.push_notification(
                        Notification::error(format!("Export failed: {reason}")),
                        cx,
                    );
                }
                Err(e) => window.push_notification(Notification::error(e.to_string()), cx),
            })
            .ok();
        })
        .detach();
    }

    /// What an export renders: the same file as the preview, so exporting
    /// while editing a library exports the design that uses it.
    fn export_source(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<PathBuf> {
        let source = self.render_target();
        if source.is_none() {
            window.push_notification(Notification::warning("Open a .scad file to export it"), cx);
        }
        source
    }

    fn export_stl(&mut self, _: &ExportStl, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(file) = self.export_source(window, cx) {
            self.export(file.with_extension("stl"), window, cx);
        }
    }

    fn export_as(&mut self, _: &ExportAs, window: &mut Window, cx: &mut Context<Self>) {
        let Some(file) = self.export_source(window, cx) else {
            return;
        };
        let dir = file.parent().unwrap_or(Path::new(".")).to_path_buf();
        let name = format!(
            "{}.3mf",
            file.file_stem()
                .map(|s| s.to_string_lossy())
                .unwrap_or_default()
        );
        let this = cx.entity().downgrade();
        super::path_prompt::new_path(
            "Export",
            "File to export to. The extension picks the format: .stl, .3mf, .off, .amf, .obj, .wrl, .dxf, .svg, .pdf, .png or .csg",
            &dir,
            &name,
            window,
            cx,
            move |path, window, cx| {
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                if osc_engine::ExportFormat::from_extension(ext).is_err() {
                    window.push_notification(
                        Notification::error(
                            "Unsupported format. Use .stl, .3mf, .off, .amf, .obj, .wrl, .dxf, .svg, .pdf, .png or .csg",
                        ),
                        cx,
                    );
                } else {
                    this.update(cx, |this, cx| this.export(path, window, cx))
                        .ok();
                }
            },
        );
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
            })
            .child(Tab::new().label("Files"))
            .child(Tab::new().label("Outline"))
            .child(Tab::new().label("Git"))
            .on_click(cx.listener(|this, ix: &usize, _, cx| {
                this.left_tab = match ix {
                    1 => LeftTab::Outline,
                    2 => LeftTab::Git,
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
                el.child(
                    h_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.warning)
                                .child("OpenSCAD was not found. Rendering is disabled."),
                        )
                        .child(
                            Button::new("download-openscad")
                                .label("Download OpenSCAD")
                                .small()
                                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                    this.download_openscad(window, cx)
                                })),
                        )
                        .child(
                            Button::new("locate-openscad")
                                .label("Locate…")
                                .small()
                                .ghost()
                                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                    this.locate_openscad(window, cx)
                                })),
                        ),
                )
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
                    .pr_2()
                    .h(px(32.))
                    .gap_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .bg(theme.tab_bar)
                    .text_sm()
                    .child(self.render_tabs(&title, cx))
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

    fn render_tabs(&self, fallback: &str, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        if self.tabs.is_empty() {
            return h_flex()
                .px_3()
                .gap_2()
                .child(
                    Icon::new(IconName::File)
                        .small()
                        .text_color(theme.muted_foreground),
                )
                .child(fallback.to_owned())
                .into_any_element();
        }
        let mut bar = h_flex()
            .id("editor-tabs")
            .h_full()
            .min_w_0()
            .overflow_x_scroll()
            .track_scroll(&self.tab_scroll);
        for (ix, tab) in self.tabs.iter().enumerate() {
            let active = Some(ix) == self.active_tab;
            let dirty = if active { self.dirty } else { tab.dirty };
            let name = tab
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            bar = bar.child(
                h_flex()
                    .id(("tab", ix))
                    .h_full()
                    .flex_shrink_0()
                    .px_3()
                    .gap_1p5()
                    .border_r_1()
                    .border_color(theme.border)
                    .cursor_pointer()
                    .when(active, |el| {
                        el.bg(theme.background).text_color(theme.foreground)
                    })
                    .when(!active, |el| {
                        el.text_color(theme.muted_foreground)
                            .hover(|s| s.bg(theme.list_hover))
                    })
                    .child(name)
                    .child(
                        Button::new(("close-tab", ix))
                            .icon(if dirty {
                                Icon::new(gpui_kit::assets::IconName::Dot)
                            } else {
                                Icon::new(IconName::Close)
                            })
                            .ghost()
                            .xsmall()
                            .tooltip(if dirty {
                                "Unsaved: save and close"
                            } else {
                                "Close"
                            })
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                cx.stop_propagation();
                                this.close_tab(ix, window, cx)
                            })),
                    )
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.activate_tab(ix, window, cx)
                    })),
            );
        }
        bar.into_any_element()
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
            .child(match &self.openscad_download {
                Some((received, total)) => {
                    let received = received.load(Ordering::Relaxed);
                    if received >= *total {
                        "Verifying and unpacking OpenSCAD…".to_owned()
                    } else {
                        let pct = (received * 100).checked_div(*total).unwrap_or(0);
                        format!("Downloading OpenSCAD… {pct}%")
                    }
                }
                None => self
                    .openscad_version
                    .clone()
                    .unwrap_or_else(|| "OpenSCAD not found".into()),
            })
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
                self.save_all(window, cx);
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

/// A file open in the editor.
struct OpenTab {
    path: PathBuf,
    editor: Entity<EditorState>,
    dirty: bool,
    disk_mtime: Option<SystemTime>,
    _sub: Subscription,
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
            .on_action(
                cx.listener(|this, _: &CommandPalette, window, cx| {
                    this.command_palette(window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &FindFile, window, cx| this.find_file(window, cx)))
            .on_action(
                cx.listener(|this, _: &RecentProjects, window, cx| {
                    this.recent_projects(window, cx)
                }),
            )
            .on_action(cx.listener(Self::new_file))
            .on_action(cx.listener(Self::export_stl))
            .on_action(cx.listener(Self::export_as))
            // Handled here, not in the panel: the pickers' menus dispatch
            // to whatever has focus.
            .on_action(cx.listener(|this, a: &SetAgentOption, _, cx| {
                this.agent.update(cx, |agent, cx| {
                    agent.set_option(a.option.clone(), a.value.clone(), cx)
                })
            }))
            .on_action(
                cx.listener(|this, _: &OpenSettings, window, cx| this.open_settings(window, cx)),
            )
            .on_action(cx.listener(|this, _: &DownloadOpenScad, window, cx| {
                this.download_openscad(window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &LocateOpenScad, window, cx| {
                    this.locate_openscad(window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &AutoOpenScad, window, cx| this.auto_openscad(window, cx)),
            )
            .on_action(cx.listener(|this, a: &OpenRecent, window, cx| {
                if let Some(r) = this.recent.get(a.ix).cloned() {
                    this.open_path(&r.root, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &Save, window, cx| this.save(window, cx)))
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| {
                if let Some(ix) = this.active_tab {
                    this.close_tab(ix, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &NextTab, window, cx| this.cycle_tab(1, window, cx)))
            .on_action(cx.listener(|this, _: &PrevTab, window, cx| this.cycle_tab(-1, window, cx)))
            .on_action(cx.listener(|this, _: &Reload, window, cx| {
                // Drop unsaved edits of the active tab and re-read the disk.
                this.dirty = false;
                this.disk_mtime = None;
                this.check_disk(window, cx);
                this.render(cx);
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
