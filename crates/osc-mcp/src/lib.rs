//! The OpenSuperCAD MCP server.
//!
//! Every agent session started from OpenSuperCAD gets this server attached
//! (via ACP's `mcpServers`), giving the agent tools to inspect and operate
//! the design: read/write sources, render, take snapshots from any angle,
//! tweak customizer parameters, export, and manage iteration checkpoints.
//!
//! The server speaks MCP (JSON-RPC 2.0, newline-delimited) over stdio. The
//! design skill ([`SKILL`]) is advertised through the `instructions` field of
//! the `initialize` result, which MCP clients put into the model's context
//! automatically, and is also exposed as a prompt and a resource.

pub mod control;
mod tools;

use std::io::{BufRead, Write};
use std::path::PathBuf;

use serde_json::{Value, json};

pub use tools::Tools;

/// The design skill shipped with OpenSuperCAD.
pub const SKILL: &str = include_str!("../skill/SKILL.md");

/// Name the server reports and agents see tools under.
pub const SERVER_NAME: &str = "opensupercad";

const PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

const SKILL_URI: &str = "opensupercad://skill";

/// Skill body without the YAML front matter.
pub fn skill_body() -> &'static str {
    SKILL
        .strip_prefix("---")
        .and_then(|rest| rest.find("\n---").map(|end| &rest[end + 4..]))
        .unwrap_or(SKILL)
        .trim_start()
}

pub struct Server {
    tools: Tools,
}

impl Server {
    pub fn new(project_root: PathBuf) -> anyhow::Result<Self> {
        Self::with_control(project_root, None)
    }

    /// A server that also talks to a running OpenSuperCAD window.
    pub fn with_control(project_root: PathBuf, control: Option<PathBuf>) -> anyhow::Result<Self> {
        Ok(Self {
            tools: Tools::new(project_root)?.with_control(control),
        })
    }

    /// Serve requests from stdin until EOF.
    pub fn serve_stdio(&mut self) -> anyhow::Result<()> {
        let stdin = std::io::stdin();
        let mut stdout = std::io::stdout().lock();
        for line in stdin.lock().lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let responses = self.handle_line(&line);
            for response in responses {
                serde_json::to_writer(&mut stdout, &response)?;
                stdout.write_all(b"\n")?;
                stdout.flush()?;
            }
        }
        Ok(())
    }

    /// Handle one line of input (a message or a batch); returns the responses.
    pub fn handle_line(&mut self, line: &str) -> Vec<Value> {
        match serde_json::from_str::<Value>(line) {
            Ok(Value::Array(batch)) => batch.into_iter().filter_map(|m| self.handle(m)).collect(),
            Ok(msg) => self.handle(msg).into_iter().collect(),
            Err(e) => vec![error_response(
                Value::Null,
                -32700,
                &format!("parse error: {e}"),
            )],
        }
    }

    /// Handle one JSON-RPC message. Notifications return `None`.
    pub fn handle(&mut self, msg: Value) -> Option<Value> {
        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let id = id?; // notifications (incl. notifications/initialized) need no reply

        let result = match method {
            "initialize" => Ok(self.initialize(&params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": self.tools.definitions() })),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                if !self.tools.exists(name) {
                    Err((-32602, format!("unknown tool `{name}`")))
                } else {
                    Ok(self.tools.call(name, &args))
                }
            }
            "prompts/list" => Ok(json!({
                "prompts": [{
                    "name": "opensupercad-skill",
                    "description": "How to design with OpenSCAD in OpenSuperCAD (the design loop and conventions).",
                }]
            })),
            "prompts/get" => Ok(json!({
                "description": "OpenSuperCAD design skill",
                "messages": [{ "role": "user", "content": { "type": "text", "text": skill_body() } }],
            })),
            "resources/list" => Ok(json!({
                "resources": [{
                    "uri": SKILL_URI,
                    "name": "OpenSuperCAD design skill",
                    "mimeType": "text/markdown",
                }]
            })),
            "resources/read" => {
                let uri = params.get("uri").and_then(Value::as_str).unwrap_or("");
                if uri == SKILL_URI {
                    Ok(
                        json!({ "contents": [{ "uri": SKILL_URI, "mimeType": "text/markdown", "text": SKILL }] }),
                    )
                } else {
                    Err((-32002, format!("resource not found: {uri}")))
                }
            }
            "resources/templates/list" => Ok(json!({ "resourceTemplates": [] })),
            "logging/setLevel" => Ok(json!({})),
            other => Err((-32601, format!("method not found: {other}"))),
        };
        Some(match result {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err((code, message)) => error_response(id, code, &message),
        })
    }

    fn initialize(&self, params: &Value) -> Value {
        let requested = params
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or(PROTOCOL_VERSIONS[0]);
        let version = PROTOCOL_VERSIONS
            .iter()
            .find(|v| **v == requested)
            .copied()
            .unwrap_or(PROTOCOL_VERSIONS[0]);
        json!({
            "protocolVersion": version,
            "capabilities": {
                "tools": { "listChanged": false },
                "prompts": { "listChanged": false },
                "resources": { "listChanged": false, "subscribe": false },
            },
            "serverInfo": {
                "name": SERVER_NAME,
                "title": "OpenSuperCAD",
                "version": env!("CARGO_PKG_VERSION"),
            },
            "instructions": skill_body(),
        })
    }
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> (tempfile::TempDir, Server) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("main.scad"),
            "/* [Size] */\nwidth = 10; // [1:50]\nlid = true;\nmodule box() { cube(width); }\nbox();\n",
        )
        .unwrap();
        let server = Server::new(dir.path().to_path_buf()).unwrap();
        (dir, server)
    }

    fn call(server: &mut Server, name: &str, args: Value) -> Value {
        let resp = server
            .handle(json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":name,"arguments":args}}))
            .unwrap();
        resp["result"].clone()
    }

    fn text(result: &Value) -> String {
        result["content"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|c| c["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn handshake_advertises_skill() {
        let (_d, mut s) = server();
        let r = s
            .handle(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}))
            .unwrap();
        assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
        assert!(
            r["result"]["instructions"]
                .as_str()
                .unwrap()
                .contains("design loop")
        );
        assert!(
            !r["result"]["instructions"]
                .as_str()
                .unwrap()
                .starts_with("---")
        );
        assert!(
            s.handle(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
                .is_none()
        );

        let r = s.handle(json!({"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"1999-01-01"}})).unwrap();
        assert_eq!(r["result"]["protocolVersion"], PROTOCOL_VERSIONS[0]);

        let r = s.handle(json!({"jsonrpc":"2.0","id":3,"method":"resources/read","params":{"uri":SKILL_URI}})).unwrap();
        assert!(
            r["result"]["contents"][0]["text"]
                .as_str()
                .unwrap()
                .starts_with("---")
        );
        let r = s
            .handle(json!({"jsonrpc":"2.0","id":4,"method":"bogus"}))
            .unwrap();
        assert_eq!(r["error"]["code"], -32601);
        assert_eq!(s.handle_line("{not json")[0]["error"]["code"], -32700);
    }

    #[test]
    fn lists_tools_with_schemas() {
        let (_d, mut s) = server();
        let r = s
            .handle(json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
            .unwrap();
        let tools = r["result"]["tools"].as_array().unwrap();
        for name in [
            "snapshot",
            "render",
            "write_file",
            "set_parameters",
            "checkpoint",
            "restore_checkpoint",
        ] {
            let t = tools
                .iter()
                .find(|t| t["name"] == name)
                .unwrap_or_else(|| panic!("{name}"));
            assert_eq!(t["inputSchema"]["type"], "object");
        }
        let r = s
            .handle(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"nope"}}))
            .unwrap();
        assert_eq!(r["error"]["code"], -32602);
    }

    #[test]
    fn file_and_parameter_tools() {
        let (dir, mut s) = server();
        let r = call(&mut s, "project_info", json!({}));
        assert!(text(&r).contains("main.scad"));

        let r = call(&mut s, "get_parameters", json!({}));
        assert!(text(&r).contains("\"width\""));

        let r = call(
            &mut s,
            "set_parameters",
            json!({"values": {"width": 25, "lid": false}}),
        );
        assert_eq!(r["isError"], false, "{}", text(&r));
        let src = std::fs::read_to_string(dir.path().join("main.scad")).unwrap();
        assert!(src.contains("width = 25; // [1:50]"));
        assert!(src.contains("lid = false;"));

        let r = call(
            &mut s,
            "set_parameters",
            json!({"values": {"width": "wide"}}),
        );
        assert_eq!(r["isError"], true);

        let r = call(
            &mut s,
            "edit_file",
            json!({"path":"main.scad","old_string":"cube(width)","new_string":"cube([width, 5, 5])"}),
        );
        assert_eq!(r["isError"], false, "{}", text(&r));
        let r = call(
            &mut s,
            "edit_file",
            json!({"path":"main.scad","old_string":"does not exist","new_string":"x"}),
        );
        assert_eq!(r["isError"], true);

        let r = call(
            &mut s,
            "write_file",
            json!({"path":"parts/new.scad","content":"sphere(2);\n"}),
        );
        assert_eq!(r["isError"], false, "{}", text(&r));
        assert!(dir.path().join("parts/new.scad").is_file());
        let r = call(&mut s, "read_file", json!({"path":"parts/new.scad"}));
        assert!(text(&r).contains("sphere(2);"));
        let r = call(&mut s, "read_file", json!({"path":"../../etc/passwd"}));
        assert_eq!(r["isError"], true);

        let r = call(&mut s, "outline", json!({"path":"main.scad"}));
        assert!(text(&r).contains("box"));
        let r = call(&mut s, "list_files", json!({}));
        assert!(text(&r).contains("parts/new.scad"));
    }

    #[test]
    fn checkpoint_tools() {
        let (dir, mut s) = server();
        if std::process::Command::new("git")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        // Not a repo yet: checkpoint initialises one.
        let r = call(&mut s, "checkpoint", json!({"message":"start"}));
        assert_eq!(r["isError"], false, "{}", text(&r));
        std::fs::write(dir.path().join("main.scad"), "cube(1);\n").unwrap();
        let r = call(&mut s, "checkpoint", json!({"message":"smaller"}));
        assert_eq!(r["isError"], false, "{}", text(&r));
        let r = call(&mut s, "list_checkpoints", json!({}));
        let listing: Value = serde_json::from_str(&text(&r)).unwrap();
        let first = listing.as_array().unwrap().last().unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let r = call(&mut s, "diff_checkpoint", json!({}));
        assert!(text(&r).contains("+cube(1);"), "{}", text(&r));
        let r = call(&mut s, "restore_checkpoint", json!({"id": first}));
        assert_eq!(r["isError"], false, "{}", text(&r));
        let src = std::fs::read_to_string(dir.path().join("main.scad")).unwrap();
        assert!(src.contains("width = 10"));
    }

    #[cfg(unix)]
    #[test]
    fn control_channel_forwards_snapshots_and_camera() {
        use crate::control::{ControlReply, ControlRequest, serve};
        use std::sync::{Arc, Mutex};
        let (dir, _) = server();
        let sock = dir.path().join("ctl.sock");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        serve(&sock, move |r| {
            log.lock().unwrap().push(r);
            ControlReply::ok()
        })
        .unwrap();
        let mut s = Server::with_control(dir.path().to_path_buf(), Some(sock)).unwrap();
        let r = call(&mut s, "set_view", json!({"view": "top"}));
        assert_eq!(r["isError"], false, "{}", text(&r));
        call(&mut s, "get_parameters", json!({}));
        let seen = seen.lock().unwrap();
        assert!(seen.contains(&ControlRequest::Camera {
            rotation: [0.0, 0.0, 0.0]
        }));
        assert!(seen.contains(&ControlRequest::SaveAll));
        // Without a control channel set_view explains itself.
        let (_d2, mut plain) = server();
        assert_eq!(
            call(&mut plain, "set_view", json!({"view": "top"}))["isError"],
            true
        );
    }

    #[test]
    fn rendering_tools_with_real_openscad() {
        let (_d, mut s) = server();
        if osc_engine::Engine::discover().is_err() {
            eprintln!("openscad not installed; skipping");
            return;
        }
        let r = call(&mut s, "render", json!({}));
        assert_eq!(r["isError"], false, "{}", text(&r));
        assert!(text(&r).contains("bounding_box"));

        let r = call(
            &mut s,
            "snapshot",
            json!({"views": ["iso", "top"], "size": [160, 120]}),
        );
        let images: Vec<_> = r["content"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["type"] == "image")
            .collect();
        if r["isError"] == true {
            // No OpenGL / virtual display available on this machine.
            eprintln!("snapshot unavailable: {}", text(&r));
            return;
        }
        assert_eq!(images.len(), 2, "{}", text(&r));
        assert_eq!(images[0]["mimeType"], "image/png");

        let r = call(&mut s, "export", json!({"output": "out/model.stl"}));
        assert_eq!(r["isError"], false, "{}", text(&r));
    }
}
