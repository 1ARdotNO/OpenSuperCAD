//! Agent Client Protocol (ACP) support.
//!
//! OpenSuperCAD is an ACP *client*, like Zed: any agent that speaks ACP
//! (Claude Code, Gemini CLI, Codex, Goose, OpenCode, or your own) can drive
//! the design loop. Each session gets the built-in `opensupercad` MCP server
//! attached, so whichever model is behind the agent can render, look at
//! snapshots and operate the app.
//!
//! The protocol types come from the official `agent-client-protocol-schema`
//! crate. The transport is a small, synchronous JSON-RPC implementation over
//! the agent's stdio, running on its own threads. That keeps it independent
//! of the UI's executor. Events are delivered through an [`async_channel`],
//! which can be consumed from async UI code or blocking threads alike.

mod client;
mod registry;

pub use acp;
pub use client::{AgentClient, AgentError, AgentEvent, PermissionChoice, ToolStatus};
pub use registry::{AgentSpec, Registry};

use std::path::Path;

/// The MCP server entry handed to agents in `session/new`.
pub fn opensupercad_mcp_server(command: &Path, args: Vec<String>) -> acp::v1::McpServer {
    acp::v1::McpServer::Stdio(acp::v1::McpServerStdio::new("opensupercad", command).args(args))
}

/// Content for the first prompt of a thread: the design skill as context,
/// followed by the user's request. Agents that already loaded the skill via
/// the MCP server's `instructions` simply see it twice; agents that ignore
/// MCP instructions still get it.
pub fn first_turn_prompt(
    skill: &str,
    project_name: &str,
    user_text: &str,
) -> Vec<acp::v1::ContentBlock> {
    vec![
        text_block(format!(
            "<opensupercad-context>\nYou are working in the OpenSuperCAD project \"{project_name}\". \
             Use the `opensupercad` MCP tools to inspect, render and snapshot the design.\n\n{skill}\n</opensupercad-context>"
        )),
        text_block(user_text.to_owned()),
    ]
}

pub fn text_block(text: impl Into<String>) -> acp::v1::ContentBlock {
    acp::v1::ContentBlock::Text(acp::v1::TextContent::new(text.into()))
}
