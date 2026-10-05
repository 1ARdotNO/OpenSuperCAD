# AI agents

OpenSuperCAD does not bundle a model. It is an
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
| Claude Code | `npx -y @agentclientprotocol/claude-agent-acp@latest` | [Node.js](https://nodejs.org) 22+, plus a Claude subscription (`claude /login`) or `ANTHROPIC_API_KEY` |
| Gemini CLI | `gemini --experimental-acp` | `npm i -g @google/gemini-cli`, Google login or `GEMINI_API_KEY` |
| Codex | `npx -y @agentclientprotocol/codex-acp@latest` | [Node.js](https://nodejs.org) 22+, plus a ChatGPT login or `OPENAI_API_KEY` |
| Goose | `goose acp` | [Goose](https://block.github.io/goose/), configured with any provider, including local models |
| OpenCode | `opencode acp` | [OpenCode](https://opencode.ai), configured with any provider |

Pick the agent from the selector in the agent panel. Agents whose command
isn't found are listed with an install hint. `opensupercad doctor` shows which
agents are available.

Claude Code and Codex talk ACP through an adapter that `npx` downloads on first
use. The adapter is a Node.js program that runs next to the CLI, so it needs
Node.js 22 or newer even when the `claude` or `codex` CLI is installed natively.
It uses the same login.

OpenSuperCAD looks for `npx` on your `PATH`, on the `PATH` your login shell
sets up (nvm, fnm, Volta, asdf, mise), and in the usual install locations
(`~/.volta/bin`, `~/.nvm`, Homebrew, `/usr/local/bin`). So it works when the
app is started from the desktop menu or the Dock, not only from a terminal.

If you have no Node.js, sending your first message offers to **download
Node.js**: the official nodejs.org build (24 LTS, about 55 MB). Its SHA-256 is
checked against a checksum pinned in OpenSuperCAD, and it is installed just for
OpenSuperCAD in its data folder, without admin rights. Your own Node.js always
wins when you have one. From a terminal:

```sh
opensupercad node            # which npx the agents use
opensupercad node install    # download the pinned Node.js (SHA-256 verified)
```

## Sending while the agent works

You can keep typing while the agent works. Messages you send meanwhile are
queued above the prompt and sent one by one as the agent finishes each turn
(also after **Stop**), with their images. Click × on a queued message to drop
it.

## Several threads at once

Switching to another thread, or starting a new one, doesn't stop the agent
in the thread you leave: it keeps working in the background, and its replies,
tool calls and checkpoints are saved to that thread. The thread list (☰)
shows which threads are *working…*, have *queued* messages, or are waiting
for *your OK* on a permission prompt; open the thread to answer it. When a
background agent finishes and has nothing left to do, it's stopped to save
resources; reopening the thread resumes the conversation.

## Mode, model and effort

Agents that offer settings get small dropdowns under the prompt, one per
setting, showing only what the current agent supports. Claude Code offers:

- **Mode**: *Manual* (ask before every change), *Accept edits*, *Plan* (plan
  first, then ask), and *Auto* (Claude decides which actions need your OK).
  Modes that skip permission prompts are shown in the warning colour.
- **Model**: Default, Sonnet, Opus, Fable, Haiku (whatever your account has).
- **Effort**: how hard the model thinks, from *Low* to *Max*.

A change applies right away, even mid-turn, and is remembered for the project
(in OpenSuperCAD's data folder, not the shared `.opensupercad/settings.json`),
so the agent starts with it next time. When the agent switches mode itself,
for example leaving plan mode, the dropdown follows. The dropdowns appear
once the agent has started in this session (your first message, or typing
`/`).

## Images

Show the agent what you mean: a photo of an object to copy, a sketch, or a
screenshot. **Paste** an image into the prompt (`Ctrl/Cmd-V`), **drop** image
files onto it, or click the image button under the prompt to pick files. PNG,
JPEG, GIF and WebP are accepted. Attached images show as thumbnails above the
prompt; click × to remove one before sending.

Images larger than 1568 px on their longest side are scaled down first, which
is as much detail as models use, and keeps the prompt small. They're saved
with the project's data, so they appear in the thread when you come back to
it. The button is disabled when an agent says it doesn't accept images
(Claude Code does).

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
