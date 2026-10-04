//! A running agent session for one thread, independent of the UI.
//!
//! [`Session::start`] spawns the agent and a worker thread that performs the
//! ACP handshake, attaches the OpenSuperCAD MCP server and then executes
//! prompts one at a time. After every turn it takes a git checkpoint (if
//! enabled), so each AI iteration can be rolled back. Cancelling and answering
//! permission prompts go straight to the agent and work mid-turn.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};

use osc_agent::{AgentClient, AgentEvent, AgentSpec, first_turn_prompt, text_block};
use osc_git::{CommitInfo, Repo};

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Starting,
    Ready,
    Busy,
    Failed(String),
    Exited,
}

#[derive(Clone, Debug)]
pub enum SessionEvent {
    Status(Status),
    Agent(AgentEvent),
    /// The ACP session id (persist it to resume the thread later).
    SessionId(String),
    TurnEnded {
        stop_reason: String,
        checkpoint: Option<CommitInfo>,
    },
    /// Whether the agent accepts images in prompts (ACP prompt capability).
    AcceptsImages(bool),
}

enum Command {
    Prompt {
        text: String,
        first_turn: bool,
        images: Vec<PathBuf>,
    },
}

pub struct Session {
    commands: mpsc::Sender<Command>,
    client: Arc<Mutex<Option<AgentClient>>>,
    session_id: Arc<Mutex<Option<String>>>,
    pub events: async_channel::Receiver<SessionEvent>,
}

pub struct Options {
    pub spec: AgentSpec,
    pub project_root: PathBuf,
    pub project_name: String,
    /// ACP session to resume with `session/load`, if the agent supports it.
    pub resume: Option<String>,
    pub auto_checkpoint: bool,
    /// Control socket of this window, handed to the MCP server.
    pub control: Option<PathBuf>,
}

impl Session {
    pub fn start(opts: Options) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (ev_tx, ev_rx) = async_channel::unbounded();
        let client = Arc::new(Mutex::new(None));
        let session_id = Arc::new(Mutex::new(None));
        let worker_client = client.clone();
        let worker_session = session_id.clone();
        std::thread::Builder::new()
            .name(format!("agent-{}", opts.spec.id))
            .spawn(move || {
                if let Err(e) = worker(opts, cmd_rx, &ev_tx, &worker_client, &worker_session) {
                    let _ = ev_tx.send_blocking(SessionEvent::Status(Status::Failed(e)));
                }
            })
            .expect("spawn agent worker");
        Self {
            commands: cmd_tx,
            client,
            session_id,
            events: ev_rx,
        }
    }

    /// Send `text` with the attached `images` (saved image files).
    pub fn prompt(&self, text: String, first_turn: bool, images: Vec<PathBuf>) {
        let _ = self.commands.send(Command::Prompt {
            text,
            first_turn,
            images,
        });
    }

    /// Change one of the agent's settings (mode, model, effort…). Runs on
    /// its own thread, so it works while a turn is in progress; the result
    /// arrives as an `AgentEvent::ConfigOptions`.
    pub fn set_option(&self, option: String, value: String) {
        let client = self.client.lock().expect("lock").clone();
        let session = self.session_id.lock().expect("lock").clone();
        if let (Some(client), Some(session)) = (client, session) {
            let _ = std::thread::Builder::new()
                .name("agent-option".into())
                .spawn(move || {
                    let _ = client.set_config_option(&session, &option, &value);
                });
        }
    }

    pub fn cancel(&self) {
        let client = self.client.lock().expect("lock").clone();
        let session = self.session_id.lock().expect("lock").clone();
        if let (Some(client), Some(session)) = (client, session) {
            let _ = client.cancel(&session);
        }
    }

    pub fn respond_permission(&self, request: u64, choice: Option<&str>) {
        if let Some(client) = self.client.lock().expect("lock").clone() {
            let _ = client.respond_permission(request, choice);
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(client) = self.client.lock().expect("lock").take() {
            client.kill();
        }
    }
}

/// Command for the MCP server handed to the agent: this executable in `mcp`
/// mode (falls back to a sibling `opensupercad-mcp`).
fn mcp_command(
    root: &std::path::Path,
    control: Option<&std::path::Path>,
) -> (PathBuf, Vec<String>) {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("opensupercad"));
    let mut args = vec!["--project".to_owned(), root.display().to_string()];
    if let Some(socket) = control {
        args.push("--control".to_owned());
        args.push(socket.display().to_string());
    }
    let is_app = exe.file_stem().is_some_and(|s| {
        s.to_string_lossy().starts_with("opensupercad") && !s.to_string_lossy().ends_with("-mcp")
    });
    if is_app {
        let mut full = vec!["mcp".to_owned()];
        full.extend(args);
        (exe, full)
    } else {
        (exe.with_file_name("opensupercad-mcp"), args)
    }
}

fn worker(
    opts: Options,
    commands: mpsc::Receiver<Command>,
    events: &async_channel::Sender<SessionEvent>,
    shared_client: &Mutex<Option<AgentClient>>,
    shared_session: &Mutex<Option<String>>,
) -> Result<(), String> {
    let send = |e: SessionEvent| {
        let _ = events.send_blocking(e);
    };
    send(SessionEvent::Status(Status::Starting));

    let (client, agent_events) =
        AgentClient::spawn(&opts.spec, &opts.project_root).map_err(|e| {
            match &opts.spec.install_hint {
                Some(hint) => format!("{e}. {hint}"),
                None => e.to_string(),
            }
        })?;
    *shared_client.lock().expect("lock") = Some(client.clone());

    // Forward agent events to the UI.
    let forward = events.clone();
    std::thread::Builder::new()
        .name("agent-events".into())
        .spawn(move || {
            while let Ok(ev) = agent_events.recv_blocking() {
                let exited = matches!(ev, AgentEvent::Exited);
                let _ = forward.send_blocking(SessionEvent::Agent(ev));
                if exited {
                    let _ = forward.send_blocking(SessionEvent::Status(Status::Exited));
                    break;
                }
            }
        })
        .map_err(|e| e.to_string())?;

    let init = client
        .initialize()
        .map_err(|e| format!("initialize failed: {e}"))?;
    send(SessionEvent::AcceptsImages(
        init.agent_capabilities.prompt_capabilities.image,
    ));
    let (cmd, args) = mcp_command(&opts.project_root, opts.control.as_deref());
    let mcp = vec![osc_agent::opensupercad_mcp_server(&cmd, args)];

    let mut resumed = false;
    if let Some(id) = &opts.resume
        && init.agent_capabilities.load_session
        && client
            .load_session(id, &opts.project_root, mcp.clone())
            .is_ok()
    {
        *shared_session.lock().expect("lock") = Some(id.clone());
        resumed = true;
    }
    if !resumed {
        let id = client
            .new_session(&opts.project_root, mcp)
            .map_err(|e| format!("could not start a session: {e}"))?;
        *shared_session.lock().expect("lock") = Some(id.clone());
        send(SessionEvent::SessionId(id));
    }
    send(SessionEvent::Status(Status::Ready));

    let session = shared_session
        .lock()
        .expect("lock")
        .clone()
        .unwrap_or_default();
    // A resumed session already had its skill context.
    let mut skill_sent = resumed;
    while let Ok(Command::Prompt {
        text,
        first_turn,
        images,
    }) = commands.recv()
    {
        send(SessionEvent::Status(Status::Busy));
        // Agents only recognise a slash command at the very start of the
        // prompt, so commands go alone; the skill rides the next message.
        let mut blocks = if text.starts_with('/') {
            vec![text_block(text.clone())]
        } else if first_turn || !skill_sent {
            skill_sent = true;
            first_turn_prompt(osc_mcp::skill_body(), &opts.project_name, &text)
        } else {
            vec![text_block(text.clone())]
        };
        blocks.extend(images.iter().filter_map(|path| image_block(path)));
        let stop_reason = match client.prompt(&session, blocks) {
            Ok(reason) => serde_json::to_value(reason)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_else(|| "end_turn".into()),
            Err(e) => format!("error: {e}"),
        };
        let checkpoint = if opts.auto_checkpoint {
            checkpoint_after_turn(&opts.project_root, &text)
        } else {
            None
        };
        send(SessionEvent::TurnEnded {
            stop_reason,
            checkpoint,
        });
        send(SessionEvent::Status(Status::Ready));
    }
    client.kill();
    Ok(())
}

fn checkpoint_after_turn(root: &std::path::Path, prompt: &str) -> Option<CommitInfo> {
    let repo = Repo::discover(root).or_else(|_| Repo::init(root)).ok()?;
    let summary: String = prompt
        .lines()
        .next()
        .unwrap_or("")
        .chars()
        .take(72)
        .collect();
    repo.checkpoint(&format!("AI: {summary}")).ok().flatten()
}

/// An attached image file as an ACP image block.
fn image_block(path: &Path) -> Option<osc_agent::acp::v1::ContentBlock> {
    use base64::Engine as _;
    let mime = crate::attachments::mime_for(path)?;
    let bytes = std::fs::read(path).ok()?;
    let data = base64::engine::general_purpose::STANDARD.encode(bytes);
    Some(osc_agent::acp::v1::ContentBlock::Image(
        osc_agent::acp::v1::ImageContent::new(data, mime),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attached_images_become_acp_image_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("a.png");
        std::fs::write(&png, b"\x89PNG").unwrap();
        let Some(osc_agent::acp::v1::ContentBlock::Image(block)) = image_block(&png) else {
            panic!("expected an image block");
        };
        assert_eq!(block.mime_type, "image/png");
        assert_eq!(block.data, "iVBORw==");
        // Unknown types and missing files are skipped, not sent.
        assert!(image_block(&dir.path().join("a.bmp")).is_none());
        assert!(image_block(&dir.path().join("gone.png")).is_none());
    }

    #[test]
    fn mcp_command_points_back_at_us() {
        let (cmd, args) = mcp_command(
            std::path::Path::new("/p"),
            Some(std::path::Path::new("/s.sock")),
        );
        // Under `cargo test` the executable is the test harness, so we fall
        // back to the sibling standalone server.
        assert!(cmd.ends_with("opensupercad-mcp") || args[0] == "mcp");
        assert!(args.iter().any(|a| a == "/p"));
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--control" && w[1] == "/s.sock")
        );
    }

    #[test]
    fn checkpoint_initialises_repo() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.scad"), "cube(1);").unwrap();
        let cp = checkpoint_after_turn(dir.path(), "make a cube\nwith details").unwrap();
        assert_eq!(cp.summary, "AI: make a cube");
        assert!(checkpoint_after_turn(dir.path(), "nothing changed").is_none());
    }
}
