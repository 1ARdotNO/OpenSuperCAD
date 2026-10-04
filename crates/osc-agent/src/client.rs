use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use acp::v1 as schema;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::AgentSpec;

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("failed to start agent `{command}`: {source}")]
    Spawn {
        command: String,
        source: std::io::Error,
    },
    #[error("the agent exited")]
    Exited,
    #[error("agent error {code}: {message}")]
    Rpc { code: i64, message: String },
    #[error("unexpected response from agent: {0}")]
    Protocol(String),
    #[error("i/o error talking to the agent: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

impl From<schema::ToolCallStatus> for ToolStatus {
    fn from(s: schema::ToolCallStatus) -> Self {
        match s {
            schema::ToolCallStatus::InProgress => ToolStatus::InProgress,
            schema::ToolCallStatus::Completed => ToolStatus::Completed,
            schema::ToolCallStatus::Failed => ToolStatus::Failed,
            _ => ToolStatus::Pending,
        }
    }
}

/// One option offered in a permission prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionChoice {
    pub id: String,
    pub name: String,
    /// `allow_once`, `allow_always`, `reject_once`, `reject_always`.
    pub kind: String,
}

/// Everything the UI needs to render an agent session.
#[derive(Clone, Debug, PartialEq)]
pub enum AgentEvent {
    MessageChunk {
        session: String,
        text: String,
    },
    ThoughtChunk {
        session: String,
        text: String,
    },
    ToolCall {
        session: String,
        id: String,
        title: String,
        status: ToolStatus,
    },
    ToolCallUpdate {
        session: String,
        id: String,
        title: Option<String>,
        status: Option<ToolStatus>,
        text: Option<String>,
    },
    Plan {
        session: String,
        entries: Vec<String>,
    },
    /// The slash commands the agent accepts in this session (the whole
    /// list, replacing any earlier one).
    AvailableCommands {
        session: String,
        commands: Vec<SlashCommand>,
    },
    /// The agent asks for permission; answer with
    /// [`AgentClient::respond_permission`].
    PermissionRequest {
        request: u64,
        session: String,
        title: String,
        choices: Vec<PermissionChoice>,
    },
    /// The agent wrote a file through `fs/write_text_file`.
    FileWritten {
        path: PathBuf,
    },
    /// A line the agent printed on stderr (logs, auth hints).
    Stderr(String),
    Exited,
}

/// A slash command an agent advertises. Run it by sending `/name input` as
/// an ordinary prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlashCommand {
    pub name: String,
    pub description: String,
    /// What to type after the name, if the command takes input.
    pub hint: Option<String>,
}

type Pending = HashMap<i64, mpsc::Sender<Result<Value, AgentError>>>;

struct Inner {
    writer: Mutex<Option<Box<dyn Write + Send>>>,
    next_id: AtomicI64,
    pending: Mutex<Pending>,
    /// Permission prompts awaiting the user, keyed by our handle.
    permissions: Mutex<HashMap<u64, Value>>,
    next_permission: AtomicI64,
    root: PathBuf,
    events: async_channel::Sender<AgentEvent>,
    child: Mutex<Option<Child>>,
}

/// A connection to one ACP agent process.
#[derive(Clone)]
pub struct AgentClient {
    inner: Arc<Inner>,
}

const PROTOCOL_VERSION: u16 = 1;

impl AgentClient {
    /// Launch an agent. `root` is the project directory: the agent's working
    /// directory and the sandbox for its `fs/*` requests.
    pub fn spawn(
        spec: &AgentSpec,
        root: &Path,
    ) -> Result<(Self, async_channel::Receiver<AgentEvent>), AgentError> {
        let program = spec
            .resolve()
            .map(|p| p.into_os_string())
            .unwrap_or_else(|| spec.command.clone().into());
        let mut child = Command::new(program)
            .args(&spec.args)
            .envs(&spec.env)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|source| AgentError::Spawn {
                command: spec.command.clone(),
                source,
            })?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let (client, events) = Self::from_io(stdout, stdin, root);
        *client.inner.child.lock().expect("lock") = Some(child);

        let tx = client.inner.events.clone();
        std::thread::Builder::new()
            .name("acp-stderr".into())
            .spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    let _ = tx.send_blocking(AgentEvent::Stderr(line));
                }
            })?;
        Ok((client, events))
    }

    /// Connect over arbitrary streams (used for tests and custom transports).
    pub fn from_io(
        reader: impl Read + Send + 'static,
        writer: impl Write + Send + 'static,
        root: &Path,
    ) -> (Self, async_channel::Receiver<AgentEvent>) {
        let (tx, rx) = async_channel::unbounded();
        let inner = Arc::new(Inner {
            writer: Mutex::new(Some(Box::new(writer))),
            next_id: AtomicI64::new(1),
            pending: Mutex::new(HashMap::new()),
            permissions: Mutex::new(HashMap::new()),
            next_permission: AtomicI64::new(1),
            root: root.to_path_buf(),
            events: tx,
            child: Mutex::new(None),
        });
        let reader_inner = inner.clone();
        std::thread::Builder::new()
            .name("acp-reader".into())
            .spawn(move || read_loop(reader_inner, reader))
            .expect("spawn reader thread");
        (Self { inner }, rx)
    }

    fn send(&self, msg: &Value) -> Result<(), AgentError> {
        let mut guard = self.inner.writer.lock().expect("writer lock");
        let w = guard.as_mut().ok_or(AgentError::Exited)?;
        serde_json::to_writer(&mut *w, msg).map_err(|e| AgentError::Protocol(e.to_string()))?;
        w.write_all(b"\n")?;
        w.flush()?;
        Ok(())
    }

    /// Send a request and block until the response arrives.
    pub fn request<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: &P,
    ) -> Result<R, AgentError> {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        self.inner.pending.lock().expect("lock").insert(id, tx);
        let params =
            serde_json::to_value(params).map_err(|e| AgentError::Protocol(e.to_string()))?;
        if let Err(e) =
            self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
        {
            self.inner.pending.lock().expect("lock").remove(&id);
            return Err(e);
        }
        let value = rx.recv().map_err(|_| AgentError::Exited)??;
        serde_json::from_value(value).map_err(|e| AgentError::Protocol(format!("{method}: {e}")))
    }

    pub fn notify<P: Serialize>(&self, method: &str, params: &P) -> Result<(), AgentError> {
        let params =
            serde_json::to_value(params).map_err(|e| AgentError::Protocol(e.to_string()))?;
        self.send(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))
    }

    /// `initialize`, advertising file-system support so agent edits flow
    /// through OpenSuperCAD (and open buffers can be reloaded).
    pub fn initialize(&self) -> Result<schema::InitializeResponse, AgentError> {
        let req = schema::InitializeRequest::new(acp::ProtocolVersion::V1)
            .client_capabilities(
                schema::ClientCapabilities::new().fs(schema::FileSystemCapabilities::new()
                    .read_text_file(true)
                    .write_text_file(true)),
            )
            .client_info(
                schema::Implementation::new("opensupercad", env!("CARGO_PKG_VERSION"))
                    .title("OpenSuperCAD".to_string()),
            );
        let resp: schema::InitializeResponse = self.request("initialize", &req)?;
        if resp.protocol_version != acp::ProtocolVersion::V1 {
            return Err(AgentError::Protocol(format!(
                "agent speaks ACP {:?}, expected {PROTOCOL_VERSION}",
                resp.protocol_version
            )));
        }
        Ok(resp)
    }

    pub fn new_session(
        &self,
        cwd: &Path,
        mcp: Vec<schema::McpServer>,
    ) -> Result<String, AgentError> {
        let resp: schema::NewSessionResponse = self.request(
            "session/new",
            &schema::NewSessionRequest::new(cwd).mcp_servers(mcp),
        )?;
        Ok(resp.session_id.0.to_string())
    }

    /// Resume a previous session (agents advertising `loadSession`). The agent
    /// replays the history as `session/update` notifications.
    pub fn load_session(
        &self,
        session: &str,
        cwd: &Path,
        mcp: Vec<schema::McpServer>,
    ) -> Result<(), AgentError> {
        let _: Value = self.request(
            "session/load",
            &schema::LoadSessionRequest::new(session.to_owned(), cwd).mcp_servers(mcp),
        )?;
        Ok(())
    }

    /// Send a prompt; blocks until the turn ends. Updates stream as events.
    pub fn prompt(
        &self,
        session: &str,
        content: Vec<schema::ContentBlock>,
    ) -> Result<schema::StopReason, AgentError> {
        let resp: schema::PromptResponse = self.request(
            "session/prompt",
            &schema::PromptRequest::new(session.to_owned(), content),
        )?;
        Ok(resp.stop_reason)
    }

    /// Ask the agent to stop the current turn.
    pub fn cancel(&self, session: &str) -> Result<(), AgentError> {
        self.notify(
            "session/cancel",
            &schema::CancelNotification::new(session.to_owned()),
        )
    }

    /// Answer a [`AgentEvent::PermissionRequest`]; `None` cancels.
    pub fn respond_permission(&self, request: u64, choice: Option<&str>) -> Result<(), AgentError> {
        let Some(id) = self
            .inner
            .permissions
            .lock()
            .expect("lock")
            .remove(&request)
        else {
            return Ok(());
        };
        let outcome = match choice {
            Some(option) => schema::RequestPermissionOutcome::Selected(
                schema::SelectedPermissionOutcome::new(option.to_owned()),
            ),
            None => schema::RequestPermissionOutcome::Cancelled,
        };
        let result = serde_json::to_value(schema::RequestPermissionResponse::new(outcome))
            .map_err(|e| AgentError::Protocol(e.to_string()))?;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "result": result }))
    }

    /// Close the agent's stdin; well-behaved agents exit on EOF.
    pub fn shutdown(&self) {
        self.inner.writer.lock().expect("writer lock").take();
    }

    /// Terminate the agent process.
    pub fn kill(&self) {
        self.shutdown();
        if let Some(mut child) = self.inner.child.lock().expect("lock").take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn read_loop(inner: Arc<Inner>, reader: impl Read) {
    let client = AgentClient {
        inner: inner.clone(),
    };
    for line in BufReader::new(reader).lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            let _ = inner.events.send_blocking(AgentEvent::Stderr(format!(
                "invalid JSON from agent: {line}"
            )));
            continue;
        };
        let method = msg.get("method").and_then(Value::as_str).map(str::to_owned);
        let id = msg.get("id").cloned().filter(|v| !v.is_null());
        match (method, id) {
            (Some(method), Some(id)) => handle_request(&client, &method, id, msg.get("params")),
            (Some(method), None) => handle_notification(&inner, &method, msg.get("params")),
            (None, Some(id)) => {
                let Some(id) = id.as_i64() else { continue };
                let Some(tx) = inner.pending.lock().expect("lock").remove(&id) else {
                    continue;
                };
                let result = match msg.get("error") {
                    Some(err) => Err(AgentError::Rpc {
                        code: err.get("code").and_then(Value::as_i64).unwrap_or(0),
                        message: err
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown error")
                            .to_owned(),
                    }),
                    None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
                };
                let _ = tx.send(result);
            }
            (None, None) => {}
        }
    }
    // Dropping the pending senders wakes every waiter with `Exited`.
    inner.pending.lock().expect("lock").clear();
    let _ = inner.events.send_blocking(AgentEvent::Exited);
}

fn content_text(block: &schema::ContentBlock) -> Option<String> {
    match block {
        schema::ContentBlock::Text(t) => Some(t.text.clone()),
        schema::ContentBlock::ResourceLink(l) => Some(format!("[{}]({})", l.name, l.uri)),
        schema::ContentBlock::Image(_) => Some("[image]".into()),
        _ => None,
    }
}

fn tool_content_text(content: &[schema::ToolCallContent]) -> Option<String> {
    let parts: Vec<String> = content
        .iter()
        .filter_map(|c| match c {
            schema::ToolCallContent::Content(c) => content_text(&c.content),
            schema::ToolCallContent::Diff(d) => Some(format!("Edited {}", d.path.display())),
            _ => None,
        })
        .collect();
    (!parts.is_empty()).then(|| parts.join("\n"))
}

fn handle_notification(inner: &Inner, method: &str, params: Option<&Value>) {
    if method != "session/update" {
        return;
    }
    let Some(Ok(note)) =
        params.map(|p| serde_json::from_value::<schema::SessionNotification>(p.clone()))
    else {
        return;
    };
    let session = note.session_id.0.to_string();
    let event = match note.update {
        schema::SessionUpdate::AgentMessageChunk(c) => {
            content_text(&c.content).map(|text| AgentEvent::MessageChunk { session, text })
        }
        schema::SessionUpdate::AgentThoughtChunk(c) => {
            content_text(&c.content).map(|text| AgentEvent::ThoughtChunk { session, text })
        }
        schema::SessionUpdate::ToolCall(call) => Some(AgentEvent::ToolCall {
            session,
            id: call.tool_call_id.0.to_string(),
            title: call.title,
            status: call.status.into(),
        }),
        schema::SessionUpdate::ToolCallUpdate(update) => Some(AgentEvent::ToolCallUpdate {
            session,
            id: update.tool_call_id.0.to_string(),
            title: update.fields.title,
            status: update.fields.status.map(Into::into),
            text: update.fields.content.as_deref().and_then(tool_content_text),
        }),
        schema::SessionUpdate::Plan(plan) => Some(AgentEvent::Plan {
            session,
            entries: plan.entries.into_iter().map(|e| e.content).collect(),
        }),
        schema::SessionUpdate::AvailableCommandsUpdate(update) => {
            Some(AgentEvent::AvailableCommands {
                session,
                commands: update
                    .available_commands
                    .into_iter()
                    .map(|c| SlashCommand {
                        hint: match c.input {
                            Some(schema::AvailableCommandInput::Unstructured(i)) => Some(i.hint),
                            _ => None,
                        },
                        name: c.name,
                        description: c.description,
                    })
                    .collect(),
            })
        }
        _ => None,
    };
    if let Some(event) = event {
        let _ = inner.events.send_blocking(event);
    }
}

fn handle_request(client: &AgentClient, method: &str, id: Value, params: Option<&Value>) {
    let inner = &client.inner;
    let params = params.cloned().unwrap_or(Value::Null);
    let reply = |result: Result<Value, (i64, String)>| {
        let msg = match result {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err((code, message)) => {
                json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
            }
        };
        let _ = client.send(&msg);
    };
    match method {
        "session/request_permission" => {
            let Ok(req) = serde_json::from_value::<schema::RequestPermissionRequest>(params) else {
                return reply(Err((-32602, "invalid params".into())));
            };
            let handle = inner.next_permission.fetch_add(1, Ordering::Relaxed) as u64;
            inner
                .permissions
                .lock()
                .expect("lock")
                .insert(handle, id.clone());
            let choices = req
                .options
                .iter()
                .map(|o| PermissionChoice {
                    id: o.option_id.0.to_string(),
                    name: o.name.clone(),
                    kind: serde_json::to_value(o.kind)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_owned))
                        .unwrap_or_default(),
                })
                .collect();
            let _ = inner.events.send_blocking(AgentEvent::PermissionRequest {
                request: handle,
                session: req.session_id.0.to_string(),
                title: req
                    .tool_call
                    .fields
                    .title
                    .unwrap_or_else(|| "Tool call".into()),
                choices,
            });
        }
        "fs/read_text_file" => {
            let Ok(req) = serde_json::from_value::<schema::ReadTextFileRequest>(params) else {
                return reply(Err((-32602, "invalid params".into())));
            };
            reply(sandboxed(&inner.root, &req.path).and_then(|path| {
                let text = std::fs::read_to_string(&path)
                    .map_err(|e| (-32603, format!("{}: {e}", path.display())))?;
                let start = req.line.map(|l| l.saturating_sub(1) as usize).unwrap_or(0);
                let content = match (req.line, req.limit) {
                    (None, None) => text,
                    _ => text
                        .lines()
                        .skip(start)
                        .take(req.limit.map(|l| l as usize).unwrap_or(usize::MAX))
                        .collect::<Vec<_>>()
                        .join("\n"),
                };
                Ok(json!({ "content": content }))
            }));
        }
        "fs/write_text_file" => {
            let Ok(req) = serde_json::from_value::<schema::WriteTextFileRequest>(params) else {
                return reply(Err((-32602, "invalid params".into())));
            };
            reply(sandboxed(&inner.root, &req.path).and_then(|path| {
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir).map_err(|e| (-32603, e.to_string()))?;
                }
                std::fs::write(&path, &req.content)
                    .map_err(|e| (-32603, format!("{}: {e}", path.display())))?;
                let _ = inner.events.send_blocking(AgentEvent::FileWritten { path });
                Ok(Value::Null)
            }));
        }
        other => reply(Err((
            -32601,
            format!("method not supported by OpenSuperCAD: {other}"),
        ))),
    }
}

/// Only allow file access inside the project root.
fn sandboxed(root: &Path, path: &Path) -> Result<PathBuf, (i64, String)> {
    let deny = || (-32602, format!("{} is outside the project", path.display()));
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let mut normal = PathBuf::new();
    for comp in joined.components() {
        match comp {
            std::path::Component::ParentDir => {
                if !normal.pop() {
                    return Err(deny());
                }
            }
            std::path::Component::CurDir => {}
            c => normal.push(c),
        }
    }
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut existing = normal.as_path();
    while !existing.exists() {
        existing = existing.parent().ok_or_else(deny)?;
    }
    let real = existing.canonicalize().map_err(|_| deny())?;
    if !real.starts_with(&root) {
        return Err(deny());
    }
    Ok(normal)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scripted fake agent on the other end of two pipes.
    fn fake_agent(
        root: &Path,
    ) -> (
        AgentClient,
        async_channel::Receiver<AgentEvent>,
        std::thread::JoinHandle<Vec<Value>>,
    ) {
        let (client_read, agent_write) = std::io::pipe().unwrap();
        let (agent_read, client_write) = std::io::pipe().unwrap();
        let file = root.join("main.scad");
        let agent = std::thread::spawn(move || {
            let mut out = agent_write;
            let mut seen = Vec::new();
            let mut send = |v: Value| {
                serde_json::to_writer(&mut out, &v).unwrap();
                out.write_all(b"\n").unwrap();
                out.flush().unwrap();
            };
            let mut lines = BufReader::new(agent_read).lines();
            while let Some(Ok(line)) = lines.next() {
                let msg: Value = serde_json::from_str(&line).unwrap();
                seen.push(msg.clone());
                let id = msg["id"].clone();
                match msg["method"].as_str() {
                    Some("initialize") => send(json!({"jsonrpc":"2.0","id":id,"result":{
                        "protocolVersion":1,"agentCapabilities":{"loadSession":true}}})),
                    Some("session/new") => {
                        send(json!({"jsonrpc":"2.0","id":id,"result":{"sessionId":"s1"}}));
                        send(
                            json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1",
                            "update":{"sessionUpdate":"available_commands_update","availableCommands":[
                                {"name":"review","description":"Review the design","input":{"hint":"what to focus on"}},
                                {"name":"compact","description":"Summarise the conversation"}]}}}),
                        );
                    }
                    Some("session/prompt") => {
                        send(
                            json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1",
                            "update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Making a cube"}}}}),
                        );
                        send(
                            json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1",
                            "update":{"sessionUpdate":"tool_call","toolCallId":"t1","title":"snapshot","status":"pending"}}}),
                        );
                        send(
                            json!({"jsonrpc":"2.0","id":100,"method":"fs/write_text_file","params":{
                            "sessionId":"s1","path":file,"content":"cube(5);"}}),
                        );
                        send(
                            json!({"jsonrpc":"2.0","id":101,"method":"fs/read_text_file","params":{
                            "sessionId":"s1","path":"/etc/passwd"}}),
                        );
                        send(
                            json!({"jsonrpc":"2.0","id":102,"method":"session/request_permission","params":{
                            "sessionId":"s1","toolCall":{"toolCallId":"t1","title":"Run snapshot"},
                            "options":[{"optionId":"yes","name":"Allow","kind":"allow_once"},
                                       {"optionId":"no","name":"Reject","kind":"reject_once"}]}}),
                        );
                        // Wait for the replies to 100, 101 and 102 before ending the turn.
                        let mut replies = 0;
                        while replies < 3 {
                            let Some(Ok(l)) = lines.next() else { break };
                            let m: Value = serde_json::from_str(&l).unwrap();
                            seen.push(m.clone());
                            if m.get("method").is_none() {
                                replies += 1;
                            }
                        }
                        send(
                            json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1",
                            "update":{"sessionUpdate":"tool_call_update","toolCallId":"t1","status":"completed",
                                      "content":[{"type":"content","content":{"type":"text","text":"4 views"}}]}}}),
                        );
                        send(json!({"jsonrpc":"2.0","id":id,"result":{"stopReason":"end_turn"}}));
                    }
                    Some("session/cancel") => {}
                    _ => send(
                        json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"nope"}}),
                    ),
                }
            }
            seen
        });
        let (client, events) = AgentClient::from_io(client_read, client_write, root);
        (client, events, agent)
    }

    #[test]
    fn full_turn_against_fake_agent() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let (client, events, agent) = fake_agent(&root);

        let init = client.initialize().unwrap();
        assert!(init.agent_capabilities.load_session);
        let mcp = crate::opensupercad_mcp_server(
            Path::new("/usr/bin/opensupercad-mcp"),
            vec!["--project".into(), root.display().to_string()],
        );
        let session = client.new_session(&root, vec![mcp]).unwrap();
        assert_eq!(session, "s1");

        // Answer the permission prompt from another thread, like the UI would.
        let responder = client.clone();
        let collector = std::thread::spawn(move || {
            let mut seen = Vec::new();
            while let Ok(ev) = events.recv_blocking() {
                if let AgentEvent::PermissionRequest {
                    request, choices, ..
                } = &ev
                {
                    assert_eq!(choices[0].kind, "allow_once");
                    responder
                        .respond_permission(*request, Some(&choices[0].id))
                        .unwrap();
                }
                let done = matches!(ev, AgentEvent::Exited);
                seen.push(ev);
                if done {
                    break;
                }
            }
            seen
        });

        let stop = client
            .prompt(
                &session,
                crate::first_turn_prompt("SKILL", "demo", "make a cube"),
            )
            .unwrap();
        assert_eq!(stop, schema::StopReason::EndTurn);
        assert_eq!(
            std::fs::read_to_string(root.join("main.scad")).unwrap(),
            "cube(5);"
        );
        assert!(matches!(
            client.request::<_, Value>("bogus", &json!({})),
            Err(AgentError::Rpc { code: -32601, .. })
        ));
        client.cancel(&session).unwrap();

        // Close our end so the fake agent finishes and the reader sees EOF.
        client.shutdown();
        let seen = agent.join().unwrap();
        let events = collector.join().unwrap();

        assert!(events.contains(&AgentEvent::MessageChunk {
            session: "s1".into(),
            text: "Making a cube".into()
        }));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::FileWritten { .. }))
        );
        assert!(events.iter().any(|e| matches!(e, AgentEvent::ToolCallUpdate { text: Some(t), status: Some(ToolStatus::Completed), .. } if t == "4 views")));
        assert!(events.contains(&AgentEvent::AvailableCommands {
            session: "s1".into(),
            commands: vec![
                SlashCommand {
                    name: "review".into(),
                    description: "Review the design".into(),
                    hint: Some("what to focus on".into()),
                },
                SlashCommand {
                    name: "compact".into(),
                    description: "Summarise the conversation".into(),
                    hint: None,
                },
            ],
        }));

        // The MCP server was offered to the agent.
        let new = seen.iter().find(|m| m["method"] == "session/new").unwrap();
        assert_eq!(new["params"]["mcpServers"][0]["name"], "opensupercad");
        // Client capabilities advertise fs support.
        let init = seen.iter().find(|m| m["method"] == "initialize").unwrap();
        assert_eq!(
            init["params"]["clientCapabilities"]["fs"]["writeTextFile"],
            true
        );
        // Reading outside the project was refused; permission answered.
        let refused = seen.iter().find(|m| m["id"] == 101).unwrap();
        assert!(
            refused["error"]["message"]
                .as_str()
                .unwrap()
                .contains("outside the project")
        );
        let permission = seen.iter().find(|m| m["id"] == 102).unwrap();
        assert_eq!(permission["result"]["outcome"]["optionId"], "yes");
        assert_eq!(permission["result"]["outcome"]["outcome"], "selected");
        // The skill went out with the first prompt.
        let prompt = seen
            .iter()
            .find(|m| m["method"] == "session/prompt")
            .unwrap();
        assert!(
            prompt["params"]["prompt"][0]["text"]
                .as_str()
                .unwrap()
                .contains("SKILL")
        );
    }

    #[test]
    fn sandbox_rules() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        assert!(sandboxed(&root, &root.join("a/b.scad")).is_ok());
        assert!(sandboxed(&root, Path::new("rel.scad")).is_ok());
        assert!(sandboxed(&root, &root.join("../x")).is_err());
        assert!(sandboxed(&root, Path::new("/etc/passwd")).is_err());
    }

    #[test]
    fn spawn_failure_is_reported() {
        let spec = AgentSpec {
            id: "x".into(),
            name: "x".into(),
            command: "definitely-not-an-agent-binary".into(),
            args: vec![],
            env: Default::default(),
            install_hint: None,
        };
        assert!(matches!(
            AgentClient::spawn(&spec, Path::new(".")),
            Err(AgentError::Spawn { .. })
        ));
    }
}
