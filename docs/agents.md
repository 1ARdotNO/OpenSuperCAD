# AI agents

OpenSuperCAD does not bundle a model. Like Zed, it is an
[Agent Client Protocol (ACP)](https://agentclientprotocol.com) client, so any
ACP agent can design with it, whichever model is behind it.

When you start a thread, OpenSuperCAD:

1. launches the agent as a local process in the project folder,
2. attaches the built-in **`opensupercad` MCP server** to the session, which
   gives the agent tools to render, take snapshots, edit sources, tweak
   customizer parameters, export and manage checkpoints,
3. loads the **design skill** automatically: through the MCP server's
   `instructions` (which agents put into the model's context) and as context
   on the first prompt of every thread,
4. streams the agent's messages, thoughts, plans and tool calls into the
   agent panel, and sends its permission prompts to you,
5. takes a **checkpoint** after every turn, so any iteration can be rolled
   back.

## Built-in agents

| Agent | Command OpenSuperCAD runs | You need |
| --- | --- | --- |
| Claude Code | `npx -y @zed-industries/claude-code-acp@latest` | Node.js, plus a Claude subscription (`claude /login`) or `ANTHROPIC_API_KEY` |
| Gemini CLI | `gemini --experimental-acp` | `npm i -g @google/gemini-cli`, Google login or `GEMINI_API_KEY` |
| Codex | `npx -y @zed-industries/codex-acp@latest` | Node.js, plus a ChatGPT login or `OPENAI_API_KEY` |
| Goose | `goose acp` | [Goose](https://block.github.io/goose/), configured with any provider, including local models |
| OpenCode | `opencode acp` | [OpenCode](https://opencode.ai), configured with any provider |

Pick the agent from the selector in the agent panel. Agents whose command
isn't found are listed with an install hint. `opensupercad doctor` shows which
agents are available.

## Slash commands

Agents can offer slash commands over ACP, such as Claude Code's custom
commands. Type `/` at the start of the prompt to list the current agent's
commands with their descriptions, filtered as you type. Use `↑`/`↓` to pick
one, then `Tab` or `Enter` to complete it. If the command takes input, the
panel shows what to type. Press `Enter` again to send it.

The agent starts when you type `/`, because agents only announce their
commands once a session is running. A command is sent on its own: the
OpenSuperCAD design context goes with your next ordinary message. Features
of an agent's interactive terminal UI that aren't slash commands over ACP,
such as Claude Code's Remote Control, aren't available in the panel. For
those, run the agent in a terminal with the MCP server attached (see
[Using the MCP server outside OpenSuperCAD](#using-the-mcp-server-outside-opensupercad)).

> ACP adapters evolve quickly. If an adapter is renamed or a CLI changes its
> ACP flag, override the built-in entry as shown below. No new release needed.

## Adding or overriding agents

Create `agents.json` in the OpenSuperCAD config directory:

- Linux: `~/.config/opensupercad/agents.json`
- macOS: `~/Library/Application Support/OpenSuperCAD/agents.json`

It holds a JSON array. An entry with a built-in `id` replaces that agent;
other entries are added:

```json
[
  {
    "id": "gemini",
    "name": "Gemini CLI",
    "command": "gemini",
    "args": ["--acp"]
  },
  {
    "id": "local-qwen",
    "name": "Goose + local Qwen",
    "command": "goose",
    "args": ["acp"],
    "env": { "GOOSE_PROVIDER": "ollama", "GOOSE_MODEL": "qwen3-coder" },
    "install_hint": "Run `ollama pull qwen3-coder` first."
  }
]
```

## Using the MCP server outside OpenSuperCAD

The same tools work from any MCP client, which is handy for terminal agents:

```sh
# Claude Code
claude mcp add opensupercad -- opensupercad-mcp --project /path/to/project

# anything else: stdio server
opensupercad-mcp --project /path/to/project
# or, equivalently
opensupercad mcp --project /path/to/project
```

Print the bundled skill (for agents that take a system prompt or skill file):

```sh
opensupercad-mcp --print-skill > SKILL.md
```

## MCP tools

| Tool | What it does |
| --- | --- |
| `project_info` | Project root, main file, `.scad` files, OpenSCAD version, git branch and checkpoint count |
| `list_files` / `read_file` | Explore the project |
| `write_file` / `edit_file` | Change files. `.scad` writes return OpenSCAD's syntax check |
| `outline` | Modules and functions of a file with line numbers |
| `get_parameters` / `set_parameters` | Read or tune Customizer parameters without disturbing comments and annotations |
| `snapshot` | PNG images from `iso`, `front`, `back`, `left`, `right`, `top`, `bottom`, `diagonal`, or custom gimbal/look-at cameras. Preview or full render; optional `-D` overrides and animation time `t` (`$t`) |
| `render` | Full render: errors, warnings, echo output, bounding box (mm) and triangle count |
| `export` | STL, 3MF, OFF, AMF, OBJ, WRL, DXF, SVG, PDF, PNG, CSG |
| `set_view` | Turn the user's viewport (in the OpenSuperCAD window) to a named view or rotation |
| `diff_checkpoint` | Unified diff of what a checkpoint changed, or what changed since it |
| `checkpoint` / `list_checkpoints` / `restore_checkpoint` | Iteration history (see [usage.md](usage.md#git-and-checkpoints)) |

All file access is confined to the project folder.

### Connection to the app window

Inside OpenSuperCAD, the MCP server also gets a private control socket
(`--control`) to the window it was launched from:

- before each tool runs, the app saves unsaved edits, so the agent sees
  exactly what you see,
- snapshots the agent takes appear inline in the thread,
- `set_view` turns your viewport.

Run standalone (`opensupercad-mcp`), the server works the same, without
these extras.

## Permissions and safety

- Permission requests from the agent (for example "run this shell command")
  show up as a card in the thread with the agent's options. OpenSuperCAD
  never auto-approves them.
- OpenSuperCAD's own tools and the ACP `fs/*` requests cannot reach outside
  the project root.
- Every turn ends with a checkpoint, so you can always go back.
