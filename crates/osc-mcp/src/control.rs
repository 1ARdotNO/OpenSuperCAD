//! Optional control channel between the MCP server and a running
//! OpenSuperCAD window, one JSON request per connection.
//!
//! On Unix it is a socket at the `--control` path. Elsewhere (Windows) the
//! app listens on a random loopback TCP port and writes `<address> <token>`
//! to that path; clients must send the token first, so other local users
//! can't drive the window.
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
        /// The app's agent run this server belongs to (`--run`), so the
        /// snapshots go to that run's thread.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run: Option<u64>,
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

    pub fn send(&self, request: &ControlRequest) -> std::io::Result<ControlReply> {
        let mut line = serde_json::to_vec(request)?;
        line.push(b'\n');
        let stream = transport::connect(&self.path)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        (&stream).write_all(&line)?;
        (&stream).flush()?;
        let mut reply = String::new();
        BufReader::new(&stream).read_line(&mut reply)?;
        serde_json::from_str(&reply).map_err(std::io::Error::other)
    }
}

/// Server side, used by the app: accept connections on `path` and answer each
/// request with `handler` (called on a background thread).
pub fn serve(
    path: &Path,
    handler: impl Fn(ControlRequest) -> ControlReply + Send + Sync + 'static,
) -> std::io::Result<()> {
    let listener = transport::Listener::bind(path)?;
    let handler = std::sync::Arc::new(handler);
    std::thread::Builder::new()
        .name("osc-control".into())
        .spawn(move || {
            for stream in listener.incoming() {
                let handler = handler.clone();
                std::thread::spawn(move || {
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                    let mut reader = BufReader::new(&stream);
                    if !transport::authorize(&stream.token, &mut reader) {
                        return;
                    }
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

#[cfg(unix)]
mod transport {
    use std::io::BufRead;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::Path;

    /// A connection. The socket's file permissions protect it, so there is
    /// no token on Unix.
    pub struct Stream {
        inner: UnixStream,
        pub token: String,
    }

    impl std::ops::Deref for Stream {
        type Target = UnixStream;
        fn deref(&self) -> &UnixStream {
            &self.inner
        }
    }

    impl std::io::Read for &Stream {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            (&self.inner).read(buf)
        }
    }

    impl std::io::Write for &Stream {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            (&self.inner).write(buf)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            (&self.inner).flush()
        }
    }

    pub fn connect(path: &Path) -> std::io::Result<Stream> {
        Ok(Stream {
            inner: UnixStream::connect(path)?,
            token: String::new(),
        })
    }

    pub struct Listener(UnixListener);

    impl Listener {
        pub fn bind(path: &Path) -> std::io::Result<Self> {
            let _ = std::fs::remove_file(path);
            Ok(Self(UnixListener::bind(path)?))
        }

        pub fn incoming(&self) -> impl Iterator<Item = Stream> + '_ {
            self.0.incoming().flatten().map(|inner| Stream {
                inner,
                token: String::new(),
            })
        }
    }

    pub fn authorize(_token: &str, _reader: &mut impl BufRead) -> bool {
        true
    }
}

#[cfg(not(unix))]
mod transport {
    use std::hash::{BuildHasher, Hasher};
    use std::io::{BufRead, Write};
    use std::net::{TcpListener, TcpStream};
    use std::path::Path;

    pub struct Stream {
        inner: TcpStream,
        /// The token this connection must present (server side).
        pub token: String,
    }

    impl std::ops::Deref for Stream {
        type Target = TcpStream;
        fn deref(&self) -> &TcpStream {
            &self.inner
        }
    }

    impl std::io::Read for &Stream {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            (&self.inner).read(buf)
        }
    }

    impl Write for &Stream {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            (&self.inner).write(buf)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            (&self.inner).flush()
        }
    }

    /// Read `<address> <token>` from `path`, connect and present the token.
    pub fn connect(path: &Path) -> std::io::Result<Stream> {
        let info = std::fs::read_to_string(path)?;
        let (addr, token) = info
            .trim()
            .split_once(' ')
            .ok_or_else(|| std::io::Error::other("malformed control file"))?;
        let inner = TcpStream::connect(addr)?;
        (&inner).write_all(format!("{token}\n").as_bytes())?;
        Ok(Stream {
            inner,
            token: token.to_owned(),
        })
    }

    pub struct Listener {
        inner: TcpListener,
        token: String,
    }

    impl Listener {
        pub fn bind(path: &Path) -> std::io::Result<Self> {
            let inner = TcpListener::bind("127.0.0.1:0")?;
            let token = random_token();
            std::fs::write(path, format!("{} {token}", inner.local_addr()?))?;
            Ok(Self { inner, token })
        }

        pub fn incoming(&self) -> impl Iterator<Item = Stream> + '_ {
            self.inner.incoming().flatten().map(|inner| Stream {
                inner,
                token: self.token.clone(),
            })
        }
    }

    pub fn authorize(token: &str, reader: &mut impl BufRead) -> bool {
        let mut line = String::new();
        reader.read_line(&mut line).is_ok() && line.trim() == token
    }

    /// 128 bits from the standard library's randomly seeded hasher.
    fn random_token() -> String {
        let mut out = String::new();
        for i in 0..2u64 {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u64(i);
            h.write_u128(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or_default(),
            );
            out.push_str(&format!("{:016x}", h.finish()));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn snapshots_name_their_run() {
        let req = ControlRequest::Snapshot {
            file: "main.scad".into(),
            images: Vec::new(),
            run: Some(3),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(serde_json::from_str::<ControlRequest>(&json).unwrap(), req);
        // Servers that predate runs omit the field.
        let old = json.replace(r#","run":3"#, "");
        assert!(matches!(
            serde_json::from_str::<ControlRequest>(&old).unwrap(),
            ControlRequest::Snapshot { run: None, .. }
        ));
    }

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
