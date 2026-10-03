# Using OpenSuperCAD

## The window

The layout follows Zed: one window per project, with docks around a central
editor.

```
┌ Title bar: project ▾ · branch · agent ▾ ──────────────────────────────────────┐
│ Project panel │ Editor (main.scad)            │ Preview (3D)    │ Agent panel  │
│  Files        │                               │  [iso][top]...  │  Threads ▾   │
│  Outline      │                               │ Customizer      │  messages    │
│  Git          │                               │  width ──●──    │  tool calls  │
│               ├───────────────────────────────┴─────────────────┤  [prompt…]   │
│               │ Console: errors · warnings · ECHO               │              │
└ Status bar: OpenSCAD version · render time · cursor ──────────────────────────┘
```

- **Project panel** (left): file tree, outline of the open file, and git
  (changes, commit, checkpoints).
- **Editor** (center): OpenSCAD source with tree-sitter highlighting and line
  numbers. Errors from OpenSCAD are shown in the console; click one to jump
  to its line.
- **Preview**: the rendered model. Drag to orbit, Shift-drag to pan, scroll
  to zoom. The buttons switch between OpenSCAD's view presets.
- **Customizer**: OpenSCAD's customizer, with the same groups, sliders,
  dropdowns, checkboxes and text fields, driven by the comments in your file.
  Changing a value edits the source in place and re-renders.
- **Agent panel** (right): AI threads for the current project.
- **Console** (bottom): OpenSCAD output.

## Projects

A project is a folder, ideally a git repository. Open one with
**File → Open Folder…** (`Ctrl/Cmd-O`), switch between recent projects
from the project switcher in the title bar (`Ctrl/Cmd-Alt-O`), or run
`opensupercad path/to/folder` from a terminal.

Switching projects switches everything: the file tree, open file, preview,
customizer and the set of AI threads. Threads belong to a project and are
remembered per project, so you can come back to a conversation later.

The **main file** is what gets previewed and what the agent works on by
default: `main.scad` if present, otherwise the first `.scad` file. Change it
from the file's context menu ("Set as main file") or in the project settings.

### Project settings

Optional, and committable: `<project>/.opensupercad/settings.json`

```json
{
  "main_file": "parts/enclosure.scad",
  "default_agent": "claude-code",
  "auto_checkpoint": true,
  "openscad_backend": "manifold"
}
```

## The AI design loop

1. Open the agent panel (`Ctrl/Cmd-?`) and pick an agent (see [agents.md](agents.md)).
2. Describe what you want: *"A wall-mount bracket for a 25 mm pipe, two M4
   screw holes, printable without supports."*
3. The agent edits the `.scad` files, renders, and **looks at snapshots from
   several angles** through the built-in MCP tools before reporting back.
   You see each tool call, and the preview updates live as files change.
4. Iterate: *"make the walls 3 mm"*, *"add a fillet where the arm meets the
   plate"*. For simple tweaks the agent uses the customizer parameters
   directly.
5. Every turn ends with a **checkpoint**. If an iteration goes wrong, roll
   back from the checkpoint marker in the thread or from the git panel.

Agent permission prompts appear inline. Choose *Allow once*, *Always*, or
*Reject*. Press `Esc` in the agent panel or click **Stop** to cancel a turn.

## OpenSCAD features

Everything OpenSCAD does still works, because OpenSuperCAD runs OpenSCAD
itself.

| Action | Shortcut (Linux / macOS) | OpenSCAD equivalent |
| --- | --- | --- |
| Preview | `F5` | Design → Preview |
| Render (full geometry) | `F6` | Design → Render |
| Export STL | `F7` | File → Export → STL |
| Export… (3MF, OFF, AMF, OBJ, DXF, SVG, PDF, PNG, CSG) | `Ctrl/Cmd-Shift-E` | File → Export |
| Reload from disk | `Ctrl/Cmd-R` | Design → Reload and Preview |
| View: top / bottom / left / right / front / back / diagonal | `Ctrl/Cmd-4` … `9`, `0` | View menu |
| Reset view | `Ctrl/Cmd-Shift-0` | View → Reset View |
| Save | `Ctrl/Cmd-S` | File → Save (also triggers preview, as with OpenSCAD's auto-reload) |

Customizer annotations follow the OpenSCAD conventions:

```openscad
/* [Dimensions] */            // starts a group (tab)
// Outer width in mm          // description shown under the parameter
width = 60;      // [20:1:200]  slider: min:step:max
style = "round"; // [round:Rounded, sharp:Sharp]  dropdown with labels
wall = 2;        // [1, 2, 3]   dropdown
label = "Hi";    // 8           text field, max length 8
fillet = 0.4;    // 0.1         spin box step
lid = true;                     // checkbox
size = [10, 20, 30];            // vector editor
/* [Hidden] */                  // not shown
$fn = 64;
```

Only literal assignments before the first `module`/`function` are
parameters, as in OpenSCAD.

## Git and checkpoints

OpenSuperCAD manages git for you, Zed-style, through the git panel in the
project panel:

- **Changes**: modified and untracked files.
- **Commit**: write a message and commit everything (`Ctrl/Cmd-Enter` in the
  message box).
- **Checkpoints**: one per AI turn, plus any you create yourself
  (`Ctrl/Cmd-Alt-S`).

Checkpoints are real git commits on a separate branch,
`osc/checkpoints/<your-branch>`. They are built through a temporary index,
so taking one **never touches your branch, your staged changes or your
working tree**. That means:

- **Restore** brings all project files back to a checkpoint. The current
  state is checkpointed first, so a restore can itself be undone.
- **Commit** ("promote") turns the current state into a normal commit on your
  branch once you are happy with an iteration.
- The checkpoint branch can be inspected with any git tool:
  `git log osc/checkpoints/main`.
- Checkpoints stay local unless you push them. To clean up, delete the
  branch: `git branch -D osc/checkpoints/main`.

If a project is not a git repository yet, the first checkpoint runs
`git init` for you.

## Keyboard shortcuts

`Ctrl` on Linux, `Cmd` on macOS:

| Shortcut | Action |
| --- | --- |
| `Ctrl-O` | Open folder (project) |
| `Ctrl-Alt-O` | Recent projects |
| `Ctrl-N` | New `.scad` file |
| `Ctrl-S` | Save |
| `Ctrl-B` | Toggle project panel |
| `Ctrl-J` | Toggle console |
| `Ctrl-?` | Toggle agent panel |
| `Ctrl-Shift-N` | New agent thread |
| `Enter` / `Shift-Enter` | Send prompt / new line (agent panel) |
| `Esc` | Stop the agent (agent panel) |
| `Ctrl-Alt-S` | Take a checkpoint |
| `Ctrl-Shift-G` | Focus git panel |
| `F5` / `F6` / `F7` | Preview / Render / Export STL |
| `Ctrl-4` … `Ctrl-0` | View presets |
| `Ctrl-Q` | Quit |

## Command line

```text
opensupercad [PATH]                 open a project folder (or the folder of a .scad file)
opensupercad mcp [--project DIR]    run the MCP server on stdio (for external agents)
opensupercad doctor                 check OpenSCAD, git, snapshots and agents
opensupercad --version
```

## Data locations

| What | Linux | macOS |
| --- | --- | --- |
| Recent projects and threads | `~/.local/share/opensupercad/` | `~/Library/Application Support/OpenSuperCAD/` |
| Agent overrides (`agents.json`) | `~/.config/opensupercad/` | `~/Library/Application Support/OpenSuperCAD/` |
| Project settings | `<project>/.opensupercad/settings.json` | same |

Override the data directory with `OPENSUPERCAD_DATA_DIR`.
