//! The agent panel: AI threads for the current project.

use std::collections::HashMap;
use std::path::PathBuf;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::text::TextView;
use gpui_kit::component::{ActiveTheme, IconName, IndexPath, Selectable, Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use osc_agent::{AgentEvent, PermissionChoice, Registry, ToolStatus};
use osc_project::{Role, Store, Thread, ThreadSummary};

use crate::session::{Options, Session, SessionEvent, Status};

pub enum AgentPanelEvent {
    /// The agent changed files (fs write or end of turn).
    FilesChanged,
    TurnEnded,
    RestoreCheckpoint(String),
    ShowCheckpointDiff(String),
}

impl EventEmitter<AgentPanelEvent> for AgentPanel {}

struct PendingPermission {
    request: u64,
    title: String,
    choices: Vec<PermissionChoice>,
}

pub struct AgentPanel {
    focus: FocusHandle,
    store: Store,
    registry: Registry,
    project: Option<(PathBuf, String, bool)>,
    agent_select: Entity<SelectState<Vec<SharedString>>>,
    agent_id: String,
    threads: Vec<ThreadSummary>,
    thread: Option<Thread>,
    session: Option<Session>,
    status: Option<Status>,
    tool_index: HashMap<String, usize>,
    permissions: Vec<PendingPermission>,
    prompt: Entity<TextareaState>,
    show_threads: bool,
    scroll: ScrollHandle,
    stderr_tail: Vec<String>,
    control: Option<PathBuf>,
    _events: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl AgentPanel {
    pub fn new(
        store: Store,
        registry: Registry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let names: Vec<SharedString> = registry
            .agents()
            .iter()
            .map(|a| {
                if a.is_available() {
                    SharedString::from(a.name.clone())
                } else {
                    SharedString::from(format!("{} (not installed)", a.name))
                }
            })
            .collect();
        let default = registry.default_agent().id.clone();
        let default_ix = registry
            .agents()
            .iter()
            .position(|a| a.id == default)
            .unwrap_or(0);
        let agent_select = cx.new(|cx| {
            SelectState::new(names.clone(), Some(IndexPath::new(default_ix)), window, cx)
        });
        let prompt = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(2, 10)
                .submit_on_enter(true)
                // Short enough to fit the panel's default width.
                .placeholder("Describe a design or a change…")
        });
        let ids: Vec<String> = registry.agents().iter().map(|a| a.id.clone()).collect();
        let subs = vec![
            cx.subscribe_in(
                &prompt,
                window,
                |this, state, ev: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { shift: false, .. } = ev {
                        let text = state.read(cx).value().trim().to_owned();
                        if !text.is_empty() {
                            state.update(cx, |s, cx| s.set_value("", window, cx));
                            this.send(text, cx);
                        }
                    }
                },
            ),
            cx.subscribe(
                &agent_select,
                move |this, _, ev: &SelectEvent<Vec<SharedString>>, cx| {
                    if let SelectEvent::Confirm(Some(label)) = ev
                        && let Some(ix) = names.iter().position(|n| n == label)
                    {
                        this.agent_id = ids[ix].clone();
                        // The next prompt starts a fresh thread with this agent.
                        if this
                            .thread
                            .as_ref()
                            .is_some_and(|t| t.agent != this.agent_id)
                        {
                            this.new_thread(cx);
                        }
                        cx.notify();
                    }
                },
            ),
        ];
        Self {
            focus: cx.focus_handle(),
            store,
            registry,
            project: None,
            agent_select,
            agent_id: default,
            threads: Vec::new(),
            thread: None,
            session: None,
            status: None,
            tool_index: HashMap::new(),
            permissions: Vec::new(),
            prompt,
            show_threads: false,
            scroll: ScrollHandle::new(),
            stderr_tail: Vec::new(),
            control: None,
            _events: None,
            _subs: subs,
        }
    }

    /// The window's control socket, handed to agents' MCP servers.
    pub fn set_control(&mut self, socket: Option<PathBuf>) {
        self.control = socket;
    }

    /// Snapshots the agent took through the MCP server: keep them with the
    /// project's data and show them in the thread.
    pub fn add_snapshots(
        &mut self,
        file: String,
        images: Vec<(String, Vec<u8>)>,
        cx: &mut Context<Self>,
    ) {
        let (Some((root, ..)), Some(thread)) = (&self.project, &mut self.thread) else {
            return;
        };
        let dir = self.store.project_dir(root).join("snapshots");
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let mut text = format!("{SNAPSHOT_MARKER}{file}");
        for (ix, (label, png)) in images.into_iter().enumerate() {
            let safe: String = label
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            let path = dir.join(format!("{stamp}-{ix}-{safe}.png"));
            if std::fs::write(&path, png).is_ok() {
                text.push_str(&format!("\u{1f}{label}={}", path.display()));
            }
        }
        thread.push(Role::System, text);
        self.save_thread();
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    pub fn agent_name(&self) -> String {
        self.registry
            .get(&self.agent_id)
            .map(|a| a.name.clone())
            .unwrap_or_else(|| self.agent_id.clone())
    }

    pub fn is_busy(&self) -> bool {
        matches!(self.status, Some(Status::Busy | Status::Starting))
    }

    /// Switch project: drop the running session and load this project's threads.
    pub fn set_project(
        &mut self,
        project: Option<(PathBuf, String, bool)>,
        last_thread: Option<String>,
        default_agent: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.stop_session();
        self.project = project;
        self.thread = None;
        self.permissions.clear();
        if let Some(agent) = default_agent.filter(|a| self.registry.get(a).is_some()) {
            self.select_agent(&agent, window, cx);
        }
        self.reload_threads();
        if let (Some(id), Some((root, ..))) = (last_thread, &self.project) {
            self.thread = self.store.load_thread(root, &id).ok();
        }
        self.reindex();
        cx.notify();
    }

    pub fn current_thread_id(&self) -> Option<String> {
        self.thread.as_ref().map(|t| t.id.clone())
    }

    fn select_agent(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.registry.agents().iter().position(|a| a.id == id) {
            self.agent_id = id.to_owned();
            self.agent_select.update(cx, |s, cx| {
                s.set_selected_index(Some(IndexPath::new(ix)), window, cx)
            });
        }
    }

    fn reload_threads(&mut self) {
        self.threads = match &self.project {
            Some((root, ..)) => self.store.threads(root),
            None => Vec::new(),
        };
    }

    fn reindex(&mut self) {
        self.tool_index.clear();
        if let Some(t) = &self.thread {
            for (ix, m) in t.messages.iter().enumerate() {
                if m.role == Role::Tool
                    && let Some(id) = m.text.split('\u{1f}').next()
                {
                    self.tool_index.insert(id.to_owned(), ix);
                }
            }
        }
    }

    fn stop_session(&mut self) {
        self.session = None; // dropping kills the agent
        self._events = None;
        self.status = None;
    }

    pub fn new_thread(&mut self, cx: &mut Context<Self>) {
        self.stop_session();
        self.thread = None;
        self.permissions.clear();
        self.tool_index.clear();
        self.show_threads = false;
        cx.notify();
    }

    fn open_thread(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some((root, ..)) = &self.project else {
            return;
        };
        if let Ok(thread) = self.store.load_thread(root, id) {
            self.stop_session();
            let agent = thread.agent.clone();
            self.thread = Some(thread);
            self.select_agent(&agent, window, cx);
            self.reindex();
        }
        self.show_threads = false;
        cx.notify();
    }

    pub fn stop(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = &self.session {
            s.cancel();
        }
        cx.notify();
    }

    fn save_thread(&mut self) {
        if let (Some((root, ..)), Some(thread)) = (&self.project, &self.thread) {
            let _ = self.store.save_thread(root, thread);
            // Reopen this thread next time the project is opened.
            let _ = self.store.remember(root, None, Some(&thread.id));
        }
    }

    fn send(&mut self, text: String, cx: &mut Context<Self>) {
        let Some((root, name, auto_checkpoint)) = self.project.clone() else {
            return;
        };
        if self.is_busy() {
            return;
        }
        let thread = self
            .thread
            .get_or_insert_with(|| Thread::new(self.agent_id.clone()));
        let first_turn = thread.messages.iter().all(|m| m.role != Role::User);
        thread.push(Role::User, text.clone());
        self.save_thread();
        self.reload_threads();

        if self.session.is_none() {
            let Some(spec) = self.registry.get(&self.agent_id).cloned() else {
                return;
            };
            let resume = self.thread.as_ref().and_then(|t| t.session_id.clone());
            let session = Session::start(Options {
                spec,
                project_root: root,
                project_name: name,
                resume,
                auto_checkpoint,
                control: self.control.clone(),
            });
            let events = session.events.clone();
            self.session = Some(session);
            self.stderr_tail.clear();
            self._events = Some(cx.spawn(async move |this, cx| {
                while let Ok(ev) = events.recv().await {
                    if this
                        .update(cx, |this, cx| this.on_session_event(ev, cx))
                        .is_err()
                    {
                        break;
                    }
                }
            }));
        }
        if let Some(session) = &self.session {
            // A thread resumed into a fresh agent process gets the skill again.
            session.prompt(text, first_turn);
        }
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    fn append(&mut self, role: Role, text: &str) {
        let Some(thread) = &mut self.thread else {
            return;
        };
        match thread.messages.last_mut() {
            Some(last) if last.role == role && matches!(role, Role::Agent | Role::Thought) => {
                last.text.push_str(text)
            }
            _ => {
                thread.push(role, text.to_owned());
            }
        }
    }

    fn on_session_event(&mut self, ev: SessionEvent, cx: &mut Context<Self>) {
        match ev {
            SessionEvent::Status(status) => {
                if let Status::Failed(msg) = &status {
                    let msg = format!("⚠ {msg}");
                    self.append(Role::System, &msg);
                    self.session = None;
                }
                if matches!(status, Status::Exited) && self.is_busy() {
                    let tail = self.stderr_tail.join("\n");
                    self.append(
                        Role::System,
                        &format!("⚠ The agent exited unexpectedly.\n\n```\n{tail}\n```"),
                    );
                    self.session = None;
                }
                self.status = Some(status);
            }
            SessionEvent::SessionId(id) => {
                if let Some(t) = &mut self.thread {
                    t.session_id = Some(id);
                }
                self.save_thread();
            }
            SessionEvent::Agent(event) => self.on_agent_event(event, cx),
            SessionEvent::TurnEnded {
                stop_reason,
                checkpoint,
            } => {
                if stop_reason.starts_with("error") {
                    self.append(Role::System, &format!("⚠ {stop_reason}"));
                }
                if let Some(cp) = checkpoint
                    && let Some(thread) = &mut self.thread
                {
                    let msg = thread.push(
                        Role::System,
                        format!("Checkpoint {} · {}", cp.short_id, cp.summary),
                    );
                    msg.checkpoint = Some(cp.id);
                }
                self.permissions.clear();
                self.save_thread();
                self.reload_threads();
                cx.emit(AgentPanelEvent::TurnEnded);
            }
        }
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    fn on_agent_event(&mut self, event: AgentEvent, cx: &mut Context<Self>) {
        match event {
            AgentEvent::MessageChunk { text, .. } => self.append(Role::Agent, &text),
            AgentEvent::ThoughtChunk { text, .. } => self.append(Role::Thought, &text),
            AgentEvent::ToolCall {
                id, title, status, ..
            } => {
                if let Some(thread) = &mut self.thread {
                    thread.push(Role::Tool, encode_tool(&id, &title, status, None));
                    self.tool_index.insert(id, thread.messages.len() - 1);
                }
            }
            AgentEvent::ToolCallUpdate {
                id,
                title,
                status,
                text,
                ..
            } => {
                if let (Some(&ix), Some(thread)) = (self.tool_index.get(&id), &mut self.thread)
                    && let Some(msg) = thread.messages.get_mut(ix)
                {
                    let (_, old_title, old_status, old_text) = decode_tool(&msg.text);
                    msg.text = encode_tool(
                        &id,
                        title.as_deref().unwrap_or(&old_title),
                        status.unwrap_or(old_status),
                        text.or(old_text).as_deref(),
                    );
                }
            }
            AgentEvent::Plan { entries, .. } => {
                let plan = entries
                    .iter()
                    .map(|e| format!("- {e}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                self.append(Role::System, &format!("**Plan**\n{plan}"));
            }
            AgentEvent::PermissionRequest {
                request,
                title,
                choices,
                ..
            } => self.permissions.push(PendingPermission {
                request,
                title,
                choices,
            }),
            AgentEvent::FileWritten { .. } => cx.emit(AgentPanelEvent::FilesChanged),
            AgentEvent::Stderr(line) => {
                self.stderr_tail.push(line);
                if self.stderr_tail.len() > 20 {
                    self.stderr_tail.remove(0);
                }
            }
            AgentEvent::Exited => {}
        }
    }

    fn answer_permission(&mut self, request: u64, choice: Option<String>, cx: &mut Context<Self>) {
        if let Some(s) = &self.session {
            s.respond_permission(request, choice.as_deref());
        }
        self.permissions.retain(|p| p.request != request);
        cx.notify();
    }
}

/// System messages starting with this marker hold agent snapshots:
/// `␞file␟label=path␟label=path…`.
const SNAPSHOT_MARKER: char = '\u{1e}';

fn decode_snapshots(text: &str) -> Option<(String, Vec<(String, PathBuf)>)> {
    let rest = text.strip_prefix(SNAPSHOT_MARKER)?;
    let mut parts = rest.split('\u{1f}');
    let file = parts.next()?.to_owned();
    let images = parts
        .filter_map(|p| p.split_once('='))
        .map(|(l, p)| (l.to_owned(), PathBuf::from(p)))
        .collect();
    Some((file, images))
}

/// Tool-call messages are stored as `id␟title␟status␟output`.
fn encode_tool(id: &str, title: &str, status: ToolStatus, text: Option<&str>) -> String {
    let status = match status {
        ToolStatus::Pending => "pending",
        ToolStatus::InProgress => "in_progress",
        ToolStatus::Completed => "completed",
        ToolStatus::Failed => "failed",
    };
    format!(
        "{id}\u{1f}{title}\u{1f}{status}\u{1f}{}",
        text.unwrap_or("")
    )
}

fn decode_tool(s: &str) -> (String, String, ToolStatus, Option<String>) {
    let mut parts = s.splitn(4, '\u{1f}');
    let id = parts.next().unwrap_or("").to_owned();
    let title = parts.next().unwrap_or("").to_owned();
    let status = match parts.next().unwrap_or("") {
        "in_progress" => ToolStatus::InProgress,
        "completed" => ToolStatus::Completed,
        "failed" => ToolStatus::Failed,
        _ => ToolStatus::Pending,
    };
    let text = parts.next().filter(|t| !t.is_empty()).map(str::to_owned);
    (id, title, status, text)
}

impl Focusable for AgentPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for AgentPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let busy = self.is_busy();

        let header = h_flex()
            .gap_1()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.tab_bar)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Select::new(&self.agent_select).small()),
            )
            .child(
                Button::new("threads")
                    .icon(IconName::Menu)
                    .ghost()
                    .small()
                    .selected(self.show_threads)
                    .tooltip("Threads in this project")
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.show_threads = !this.show_threads;
                        this.reload_threads();
                        cx.notify();
                    })),
            )
            .child(
                Button::new("new-thread")
                    .icon(IconName::Plus)
                    .ghost()
                    .small()
                    .tooltip("New thread (Ctrl/Cmd-Shift-N)")
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.new_thread(cx))),
            );

        let body: AnyElement = if self.project.is_none() {
            div()
                .p_4()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child("Open a project to start designing with an agent.")
                .into_any_element()
        } else if self.show_threads {
            let mut list = v_flex().p_2().gap_1();
            if self.threads.is_empty() {
                list = list.child(
                    div()
                        .p_2()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("No threads yet."),
                );
            }
            for (ix, t) in self.threads.iter().enumerate() {
                let id = t.id.clone();
                let selected = self.thread.as_ref().is_some_and(|c| c.id == t.id);
                list = list.child(
                    div()
                        .id(("thread", ix))
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .cursor_pointer()
                        .when(selected, |el| el.bg(theme.list_active))
                        .hover(|s| s.bg(theme.list_hover))
                        .child(div().text_sm().truncate().child(t.title.clone()))
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(t.agent.clone()),
                        )
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            this.open_thread(&id, window, cx)
                        })),
                );
            }
            div()
                .flex_1()
                .overflow_y_scrollbar()
                .child(list)
                .into_any_element()
        } else {
            self.render_messages(&theme, cx)
        };

        let mut permissions = v_flex().gap_2();
        for p in &self.permissions {
            let mut buttons = h_flex().gap_1().flex_wrap();
            for (ix, c) in p.choices.iter().enumerate() {
                let request = p.request;
                let id = c.id.clone();
                let button = Button::new(SharedString::from(format!("perm-{request}-{ix}")))
                    .label(c.name.clone())
                    .small();
                let button = if c.kind.starts_with("allow") {
                    button.primary()
                } else {
                    button
                };
                buttons = buttons.child(button.on_click(cx.listener(
                    move |this, _: &ClickEvent, _, cx| {
                        this.answer_permission(request, Some(id.clone()), cx)
                    },
                )));
            }
            permissions = permissions.child(
                v_flex()
                    .gap_1()
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.warning)
                    .child(div().text_sm().child(format!("Allow: {}?", p.title)))
                    .child(buttons),
            );
        }

        let composer = v_flex()
            .gap_1()
            .p_2()
            .border_t_1()
            .border_color(theme.border)
            .child(permissions)
            .child(Textarea::new(&self.prompt).w_full())
            .child(
                h_flex()
                    .gap_2()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(match &self.status {
                        Some(Status::Starting) => format!("Starting {}…", self.agent_name()),
                        Some(Status::Busy) => format!("{} is working…", self.agent_name()),
                        Some(Status::Failed(_)) => "Agent failed to start".to_owned(),
                        Some(Status::Exited) => "Agent exited".to_owned(),
                        _ => self.agent_name(),
                    })
                    .when(busy, |el| el.child(Spinner::new().xsmall()))
                    .child(div().flex_1())
                    .when(busy, |el| {
                        el.child(
                            Button::new("stop")
                                .label("Stop")
                                .danger()
                                .xsmall()
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.stop(cx))),
                        )
                    }),
            );

        v_flex()
            .size_full()
            .key_context("AgentPanel")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &super::StopAgent, _, cx| this.stop(cx)))
            .child(header)
            .child(div().flex_1().min_h_0().child(body))
            .child(composer)
    }
}

impl AgentPanel {
    fn render_messages(
        &self,
        theme: &gpui_kit::component::Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(thread) = &self.thread else {
            return v_flex()
                .size_full()
                .p_4()
                .gap_2()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(div().text_base().text_color(theme.foreground).child("Design with AI"))
                .child("Describe what you want to build, e.g. “a wall bracket for a 25 mm pipe with two M4 holes”.")
                .child("The agent edits the .scad files, renders them and checks snapshots from several angles. Every turn is checkpointed, so you can roll back.")
                .into_any_element();
        };
        let mut list = v_flex().gap_3().p_3();
        for (ix, m) in thread.messages.iter().enumerate() {
            let el: AnyElement = match m.role {
                Role::User => div()
                    .p_2()
                    .rounded_md()
                    .bg(theme.secondary)
                    .text_sm()
                    .child(m.text.clone())
                    .into_any_element(),
                Role::Agent => TextView::markdown(("msg", ix), m.text.clone())
                    .selectable(true)
                    .text_sm()
                    .into_any_element(),
                Role::Thought => div()
                    .text_xs()
                    .italic()
                    .text_color(theme.muted_foreground)
                    .line_clamp(4)
                    .child(m.text.clone())
                    .into_any_element(),
                Role::Tool => {
                    let (_, title, status, output) = decode_tool(&m.text);
                    let (icon, color) = match status {
                        ToolStatus::Completed => (IconName::Check, theme.success),
                        ToolStatus::Failed => (IconName::CircleAlert, theme.danger),
                        _ => (IconName::LoaderCircle, theme.muted_foreground),
                    };
                    v_flex()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .border_1()
                        .border_color(theme.border)
                        .child(
                            h_flex()
                                .gap_2()
                                .text_xs()
                                .child(
                                    gpui_kit::component::Icon::new(icon)
                                        .xsmall()
                                        .text_color(color),
                                )
                                .child(div().truncate().child(title)),
                        )
                        .when_some(output, |el, out| {
                            el.child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .line_clamp(3)
                                    .child(out),
                            )
                        })
                        .into_any_element()
                }
                Role::System if decode_snapshots(&m.text).is_some() => {
                    let (file, images) = decode_snapshots(&m.text).unwrap_or_default();
                    let mut grid = h_flex().gap_2().flex_wrap();
                    for (label, path) in images {
                        grid = grid.child(
                            v_flex()
                                .gap_0p5()
                                .child(
                                    img(path)
                                        .w(px(168.))
                                        .h(px(126.))
                                        .rounded_md()
                                        .border_1()
                                        .border_color(theme.border)
                                        .object_fit(ObjectFit::Contain),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child(label),
                                ),
                        );
                    }
                    v_flex()
                        .gap_1()
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(format!("Agent snapshot of {file}")),
                        )
                        .child(grid)
                        .into_any_element()
                }
                Role::System => {
                    let checkpoint = m.checkpoint.clone();
                    h_flex()
                        .gap_2()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(TextView::markdown(("sys", ix), m.text.clone())),
                        )
                        .when_some(checkpoint, |el, id| {
                            let diff_id = id.clone();
                            el.child(
                                Button::new(("changes", ix))
                                    .icon(IconName::Eye)
                                    .label("Changes")
                                    .ghost()
                                    .xsmall()
                                    .tooltip("View what this turn changed")
                                    .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                                        cx.emit(AgentPanelEvent::ShowCheckpointDiff(
                                            diff_id.clone(),
                                        ))
                                    })),
                            )
                            .child(
                                Button::new(("rollback", ix))
                                    .icon(IconName::Undo2)
                                    .label("Restore")
                                    .ghost()
                                    .xsmall()
                                    .tooltip("Restore all files to this checkpoint")
                                    .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                                        cx.emit(AgentPanelEvent::RestoreCheckpoint(id.clone()))
                                    })),
                            )
                        })
                        .into_any_element()
                }
            };
            list = list.child(el);
        }
        div()
            .id("messages")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(list)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{SNAPSHOT_MARKER, ToolStatus, decode_snapshots, decode_tool, encode_tool};

    #[test]
    fn tool_messages_roundtrip() {
        let s = encode_tool("t1", "snapshot", ToolStatus::Completed, Some("4 views"));
        let (id, title, status, text) = decode_tool(&s);
        assert_eq!(
            (id.as_str(), title.as_str(), status),
            ("t1", "snapshot", ToolStatus::Completed)
        );
        assert_eq!(text.as_deref(), Some("4 views"));
        assert_eq!(
            decode_tool(&encode_tool("a", "b", ToolStatus::Pending, None)).3,
            None
        );
    }

    #[test]
    fn snapshot_messages_roundtrip() {
        let text =
            format!("{SNAPSHOT_MARKER}main.scad\u{1f}iso=/d/0-iso.png\u{1f}top=/d/1-top.png");
        let (file, images) = decode_snapshots(&text).unwrap();
        assert_eq!(file, "main.scad");
        assert_eq!(images[1].0, "top");
        assert_eq!(images[1].1, std::path::PathBuf::from("/d/1-top.png"));
        assert!(decode_snapshots("Checkpoint abc").is_none());
    }
}
