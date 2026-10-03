//! Optional control channel between the MCP server and a running
//! OpenSuperCAD window (a Unix socket, one JSON request per connection).
//!
//! When the app spawns an agent it passes `--control <socket>` to the MCP
//! server, so that:
//! * tools see the user's unsaved edits (the app saves before each tool),
//! * snapshots the agent takes are shown inline in the agent thread,
//! * the agent can point the app's viewport camera at something.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlRequest {
    /// Save unsaved editor buffers before a tool reads or writes files.
    SaveAll,
    /// Snapshots the agent just took.
    Snapshot {
        file: String,
        images: Vec<SnapshotImage>,
    },
    /// Move the app's viewport camera (OpenSCAD `$vpr` rotation).
    Camera { rotation: [f64; 3] },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SnapshotImage {
    pub label: String,
    /// PNG, base64-encoded.
    pub png_base64: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ControlReply {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl ControlReply {
    pub fn ok() -> Self {
        Self {
            ok: true,
            message: None,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            message: Some(message.into()),
        }
    }
}

/// Client side, used by the MCP server.
#[derive(Clone, Debug)]
pub struct ControlClient {
    path: PathBuf,
}

impl ControlClient {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    #[cfg(unix)]
    pub fn send(&self, request: &ControlRequest) -> std::io::Result<ControlReply> {
        let mut stream = std::os::unix::net::UnixStream::connect(&self.path)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        let mut line = serde_json::to_vec(request)?;
        line.push(b'\n');
        stream.write_all(&line)?;
        stream.flush()?;
        let mut reply = String::new();
        BufReader::new(stream).read_line(&mut reply)?;
        serde_json::from_str(&reply).map_err(std::io::Error::other)
    }

    #[cfg(not(unix))]
    pub fn send(&self, _request: &ControlRequest) -> std::io::Result<ControlReply> {
        Err(std::io::Error::other(
            "control channel requires Unix sockets",
        ))
    }
}

/// Server side, used by the app: accept connections on `path` and answer each
/// request with `handler` (called on a background thread).
#[cfg(unix)]
pub fn serve(
    path: &Path,
    handler: impl Fn(ControlRequest) -> ControlReply + Send + Sync + 'static,
) -> std::io::Result<()> {
    let _ = std::fs::remove_file(path);
    let listener = std::os::unix::net::UnixListener::bind(path)?;
    let handler = std::sync::Arc::new(handler);
    std::thread::Builder::new()
        .name("osc-control".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let handler = handler.clone();
                std::thread::spawn(move || {
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                    let mut reader = BufReader::new(&stream);
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() {
                        return;
                    }
                    let reply = match serde_json::from_str::<ControlRequest>(&line) {
                        Ok(req) => handler(req),
                        Err(e) => ControlReply::error(format!("bad request: {e}")),
                    };
                    let mut out = serde_json::to_vec(&reply).unwrap_or_default();
                    out.push(b'\n');
                    let _ = (&stream).write_all(&out);
                });
            }
        })?;
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("control.sock");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        serve(&sock, move |req| {
            log.lock().unwrap().push(req.clone());
            match req {
                ControlRequest::Camera { .. } => ControlReply::error("no viewport"),
                _ => ControlReply::ok(),
            }
        })
        .unwrap();
        let client = ControlClient::new(&sock);
        assert!(client.send(&ControlRequest::SaveAll).unwrap().ok);
        let reply = client
            .send(&ControlRequest::Camera {
                rotation: [1.0, 2.0, 3.0],
            })
            .unwrap();
        assert_eq!(reply.message.as_deref(), Some("no viewport"));
        assert_eq!(seen.lock().unwrap().len(), 2);
        assert!(
            ControlClient::new(dir.path().join("missing"))
                .send(&ControlRequest::SaveAll)
                .is_err()
        );
    }
}
