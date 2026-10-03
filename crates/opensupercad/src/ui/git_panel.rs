//! Git panel: changed files, commit, and the AI checkpoint timeline.

use std::path::{Path, PathBuf};

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::{ActiveTheme, IconName, Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use osc_git::{CommitInfo, FileStatus, Repo};

pub enum GitEvent {
    /// Files on disk changed because of a restore.
    Restored(String),
    Message(String),
}

impl EventEmitter<GitEvent> for GitPanel {}

pub struct GitPanel {
    root: Option<PathBuf>,
    repo: Option<Repo>,
    branch: Option<String>,
    status: Vec<FileStatus>,
    checkpoints: Vec<CommitInfo>,
    history: Vec<CommitInfo>,
    message: Entity<InputState>,
    _subs: Vec<Subscription>,
}

impl GitPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let message = cx
            .new(|cx| InputState::new(window, cx).placeholder("Commit message (Enter to commit)"));
        let subs =
            vec![
                cx.subscribe_in(&message, window, |this, _, ev: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = ev {
                        this.commit(window, cx);
                    }
                }),
            ];
        Self {
            root: None,
            repo: None,
            branch: None,
            status: Vec::new(),
            checkpoints: Vec::new(),
            history: Vec::new(),
            message,
            _subs: subs,
        }
    }

    pub fn branch(&self) -> Option<&str> {
        self.branch.as_deref()
    }

    pub fn set_project(&mut self, root: Option<&Path>, cx: &mut Context<Self>) {
        self.root = root.map(Path::to_path_buf);
        self.refresh(cx);
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.repo = self.root.as_deref().and_then(|r| Repo::discover(r).ok());
        match &self.repo {
            Some(repo) => {
                self.branch = repo.current_branch().ok().flatten();
                self.status = repo.status().unwrap_or_default();
                self.checkpoints = repo.checkpoints(50).unwrap_or_default();
                self.history = repo.log("HEAD", 20).unwrap_or_default();
            }
            None => {
                self.branch = None;
                self.status.clear();
                self.checkpoints.clear();
                self.history.clear();
            }
        }
        cx.notify();
    }

    fn ensure_repo(&mut self) -> Option<Repo> {
        if self.repo.is_none() {
            let root = self.root.as_deref()?;
            self.repo = Repo::init(root).ok();
        }
        self.repo.clone()
    }

    pub fn checkpoint(&mut self, message: &str, cx: &mut Context<Self>) {
        let Some(repo) = self.ensure_repo() else {
            return;
        };
        let msg = match repo.checkpoint(message) {
            Ok(Some(c)) => format!("Checkpoint {} saved", c.short_id),
            Ok(None) => "No changes since the last checkpoint".into(),
            Err(e) => format!("Checkpoint failed: {e}"),
        };
        cx.emit(GitEvent::Message(msg));
        self.refresh(cx);
    }

    pub fn restore(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        match repo.restore(id) {
            Ok(()) => cx.emit(GitEvent::Restored(id.to_owned())),
            Err(e) => cx.emit(GitEvent::Message(format!("Restore failed: {e}"))),
        }
        self.refresh(cx);
    }

    fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.message.read(cx).value().trim().to_owned();
        if text.is_empty() {
            cx.emit(GitEvent::Message("Write a commit message first".into()));
            return;
        }
        let Some(repo) = self.ensure_repo() else {
            return;
        };
        let msg = match repo.promote(&text) {
            Ok(Some(id)) => {
                self.message.update(cx, |m, cx| m.set_value("", window, cx));
                format!("Committed {}", &id[..id.len().min(8)])
            }
            Ok(None) => "Nothing to commit".into(),
            Err(e) => format!("Commit failed: {e}"),
        };
        cx.emit(GitEvent::Message(msg));
        self.refresh(cx);
    }
}

fn ago(time: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(time);
    let s = (now - time).max(0);
    match s {
        0..60 => "just now".into(),
        60..3600 => format!("{}m ago", s / 60),
        3600..86400 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86400),
    }
}

impl Render for GitPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let _ = window;
        let heading = |t: &str| {
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.muted_foreground)
                .child(t.to_uppercase())
        };

        if self.root.is_none() {
            return div().p_3().child("No project open").into_any_element();
        }

        let mut changes = v_flex().gap_0p5();
        if self.status.is_empty() {
            changes = changes.child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("No changes"),
            );
        }
        for s in self.status.iter().take(200) {
            let color = match s.code.trim() {
                "??" | "A" => theme.success,
                "D" => theme.danger,
                _ => theme.warning,
            };
            changes = changes.child(
                h_flex()
                    .gap_2()
                    .text_sm()
                    .child(
                        div()
                            .w(px(20.))
                            .text_color(color)
                            .child(s.code.trim().to_owned()),
                    )
                    .child(div().truncate().child(s.path.clone())),
            );
        }

        let mut checkpoints = v_flex().gap_1();
        if self.checkpoints.is_empty() {
            checkpoints =
                checkpoints.child(div().text_xs().text_color(theme.muted_foreground).child(
                    "Checkpoints appear after each AI turn, or take one with Ctrl/Cmd-Alt-S.",
                ));
        }
        for (ix, c) in self.checkpoints.iter().enumerate() {
            let id = c.id.clone();
            checkpoints = checkpoints.child(
                h_flex()
                    .gap_2()
                    .items_start()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_sm().truncate().child(c.summary.clone()))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(format!("{} · {}", c.short_id, ago(c.time))),
                            ),
                    )
                    .child(
                        Button::new(("restore", ix))
                            .icon(IconName::Undo2)
                            .ghost()
                            .xsmall()
                            .tooltip("Restore all files to this checkpoint")
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.restore(&id, cx)
                            })),
                    ),
            );
        }

        let mut history = v_flex().gap_1();
        for c in &self.history {
            history = history.child(
                v_flex()
                    .child(div().text_sm().truncate().child(c.summary.clone()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(format!("{} · {} · {}", c.short_id, c.author, ago(c.time))),
                    ),
            );
        }

        div()
            .size_full()
            .overflow_y_scrollbar()
            .child(
                v_flex()
                    .p_3()
                    .gap_3()
                    .child(
                        h_flex()
                            .gap_2()
                            .text_sm()
                            .child(
                                Icon::new(gpui_kit::assets::IconName::GitBranch)
                                    .small()
                                    .text_color(theme.muted_foreground),
                            )
                            .child(match (&self.repo, &self.branch) {
                                (None, _) => "Not a git repository".to_owned(),
                                (Some(_), Some(b)) => b.clone(),
                                (Some(_), None) => "detached HEAD".to_owned(),
                            })
                            .child(div().flex_1())
                            .child(
                                Button::new("git-refresh")
                                    .icon(IconName::RefreshCw)
                                    .ghost()
                                    .xsmall()
                                    .on_click(
                                        cx.listener(|this, _: &ClickEvent, _, cx| this.refresh(cx)),
                                    ),
                            ),
                    )
                    .child(heading("Changes"))
                    .child(changes)
                    .child(Input::new(&self.message).small())
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("commit")
                                    .label("Commit all")
                                    .primary()
                                    .small()
                                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                        this.commit(window, cx)
                                    })),
                            )
                            .child(
                                Button::new("checkpoint")
                                    .label("Checkpoint")
                                    .small()
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                        this.checkpoint("Manual checkpoint", cx)
                                    })),
                            ),
                    )
                    .child(heading("Checkpoints"))
                    .child(checkpoints)
                    .when(!self.history.is_empty(), |el| {
                        el.child(heading("History")).child(history)
                    }),
            )
            .into_any_element()
    }
}

use gpui_kit::component::Icon;
