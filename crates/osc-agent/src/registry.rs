use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// How to launch an ACP agent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSpec {
    /// Stable identifier, stored with threads.
    pub id: String,
    /// Display name.
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Shown when the command cannot be found.
    #[serde(default)]
    pub install_hint: Option<String>,
}

impl AgentSpec {
    fn builtin(id: &str, name: &str, command: &str, args: &[&str], hint: &str) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            command: command.into(),
            args: args.iter().map(|s| (*s).to_owned()).collect(),
            env: BTreeMap::new(),
            install_hint: Some(hint.into()),
        }
    }

    /// Whether the command can be found (absolute path or on the
    /// [search path](crate::path::search_path)).
    pub fn is_available(&self) -> bool {
        self.resolve().is_some()
    }

    /// The executable to run. On Windows, Node-based agents are `.cmd`
    /// shims (`npx.cmd`), which `Command` does not find by bare name.
    pub fn resolve(&self) -> Option<PathBuf> {
        let cmd = Path::new(&self.command);
        if cmd.is_absolute() {
            return cmd.is_file().then(|| cmd.to_path_buf());
        }
        let names: Vec<String> = if cfg!(windows) {
            [".exe", ".cmd", ".bat", ""]
                .iter()
                .map(|ext| format!("{}{ext}", self.command))
                .collect()
        } else {
            vec![self.command.clone()]
        };
        crate::path::search_path()
            .iter()
            .flat_map(|d| names.iter().map(move |n| d.join(n)))
            .find(|p| p.is_file())
    }
}

/// The set of known agents: built-ins plus user entries from `agents.json`.
#[derive(Clone, Debug)]
pub struct Registry {
    agents: Vec<AgentSpec>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            agents: Self::builtins(),
        }
    }
}

impl Registry {
    /// Agents known out of the box. The adapters for Claude Code and Codex are
    /// fetched on demand with `npx`; Gemini CLI, Goose and OpenCode ship ACP
    /// support in their own binaries.
    pub fn builtins() -> Vec<AgentSpec> {
        vec![
            AgentSpec::builtin(
                "claude-code",
                "Claude Code",
                "npx",
                &["-y", "@agentclientprotocol/claude-agent-acp@latest"],
                "Claude Code's ACP adapter runs on Node.js 22 or newer (npx): install it from \
                 https://nodejs.org. It signs in like Claude Code (Claude subscription or \
                 ANTHROPIC_API_KEY).",
            ),
            AgentSpec::builtin(
                "gemini",
                "Gemini CLI",
                "gemini",
                &["--experimental-acp"],
                "Install with `npm install -g @google/gemini-cli`.",
            ),
            AgentSpec::builtin(
                "codex",
                "Codex",
                "npx",
                &["-y", "@agentclientprotocol/codex-acp@latest"],
                "Codex's ACP adapter runs on Node.js 22 or newer (npx): install it from \
                 https://nodejs.org. It needs an OpenAI account or OPENAI_API_KEY.",
            ),
            AgentSpec::builtin(
                "goose",
                "Goose",
                "goose",
                &["acp"],
                "Install Goose from https://block.github.io/goose/.",
            ),
            AgentSpec::builtin(
                "opencode",
                "OpenCode",
                "opencode",
                &["acp"],
                "Install OpenCode from https://opencode.ai/.",
            ),
        ]
    }

    /// Built-ins overridden/extended by `<config_dir>/agents.json`, a JSON
    /// array of [`AgentSpec`]s. Entries with a built-in `id` replace it.
    pub fn load(config_dir: &Path) -> Result<Self, String> {
        let mut registry = Self::default();
        let path = config_dir.join("agents.json");
        match std::fs::read(&path) {
            Ok(bytes) => {
                let custom: Vec<AgentSpec> = serde_json::from_slice(&bytes)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                for spec in custom {
                    registry.upsert(spec);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{}: {e}", path.display())),
        }
        Ok(registry)
    }

    pub fn upsert(&mut self, spec: AgentSpec) {
        match self.agents.iter_mut().find(|a| a.id == spec.id) {
            Some(existing) => *existing = spec,
            None => self.agents.push(spec),
        }
    }

    pub fn agents(&self) -> &[AgentSpec] {
        &self.agents
    }

    pub fn get(&self, id: &str) -> Option<&AgentSpec> {
        self.agents.iter().find(|a| a.id == id)
    }

    /// The default agent: the first available one, else the first entry.
    pub fn default_agent(&self) -> &AgentSpec {
        self.agents
            .iter()
            .find(|a| a.is_available())
            .unwrap_or(&self.agents[0])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_config_overrides_and_extends() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("agents.json"),
            r#"[
              {"id": "gemini", "name": "Gemini (custom)", "command": "/opt/gemini", "args": ["--acp"]},
              {"id": "local", "name": "Local agent", "command": "my-agent", "env": {"MODEL": "qwen"}}
            ]"#,
        )
        .unwrap();
        let reg = Registry::load(dir.path()).unwrap();
        assert_eq!(reg.get("gemini").unwrap().args, ["--acp"]);
        assert_eq!(reg.get("local").unwrap().env["MODEL"], "qwen");
        assert!(reg.get("claude-code").is_some());
        assert_eq!(reg.agents().len(), Registry::builtins().len() + 1);

        std::fs::write(dir.path().join("agents.json"), "{oops").unwrap();
        assert!(Registry::load(dir.path()).is_err());
        assert!(Registry::load(&dir.path().join("missing")).is_ok());
    }
}
