# Architecture

OpenSuperCAD is a Cargo workspace. Only the app crate depends on the UI
framework; everything else is plain Rust with unit and integration tests.

```
                    ┌───────────────────────────────────────┐
                    │  opensupercad (GPUI app, gpui-kit)    │
                    │  workspace · editor · preview ·       │
                    │  customizer · agent panel · git panel │
                    └──┬──────┬──────┬───────┬───────┬──────┘
                       │      │      │       │       │
          ┌────────────▼┐ ┌───▼────┐ │ ┌─────▼─────┐ │
          │ osc-project │ │osc-git │ │ │ osc-agent │ │
          │ projects,   │ │status, │ │ │ ACP client│ │
          │ threads     │ │commit, │ │ │ + registry│ │
          └─────────────┘ │checkpts│ │ └─────┬─────┘ │
                          └────────┘ │       │ spawns agent; agent spawns ↓
                              ┌──────▼──┐  ┌─▼──────────────────────────────┐
                              │osc-engine│◄─┤ osc-mcp (opensupercad mcp)     │
                              │ openscad │  │ MCP tools + bundled SKILL.md   │
                              │ CLI,mesh,│  └────────────────────────────────┘
                              │ raster   │
                              └────┬─────┘
                              ┌────▼─────┐
                              │osc-syntax│ lexer · outline · customizer
                              └──────────┘
```

## The agent loop

```
user ──prompt──► agent panel ──ACP session/prompt──► agent process (Claude Code, Gemini, …)
                     ▲                                    │  model decides to use tools
                     │ session/update (text, tool calls)  ▼
                     │                         opensupercad MCP server (stdio)
                     │                           ├─ edit_file / set_parameters ──► project files
                     │                           ├─ snapshot ──► openscad --camera … → PNGs → model sees them
                     │                           └─ render / export ──► openscad -o …
                     │
             file watcher reloads editor + preview
             turn ends ──► osc-git checkpoint (osc/checkpoints/<branch>)
```

- **ACP** (`osc-agent`) uses the official `agent-client-protocol-schema` types
  over a small synchronous JSON-RPC transport. Each agent connection runs on
  its own threads and reports to the UI through an `async_channel`.
  OpenSuperCAD advertises `fs/read_text_file` and `fs/write_text_file`, so
  agent edits go through the app, confined to the project root.
- **MCP** (`osc-mcp`) is a dependency-light JSON-RPC server over stdio. The
  design skill is delivered through MCP `instructions` and as first-prompt
  context, so it reaches agents whether or not they honour MCP instructions.
- **Rendering** (`osc-engine`) always shells out to `openscad`, so behaviour
  matches upstream exactly. For an interactive viewport, the model is
  exported once as STL and rasterised in-process, which makes orbit and zoom
  instant. Snapshots for the agent use OpenSCAD's own renderer (`--camera`,
  `--viewall`, `--autocenter`), so the agent sees what OpenSCAD users see.
- **Checkpoints** (`osc-git`) are built with `GIT_INDEX_FILE` pointing at a
  temporary index: `git add -A` → `write-tree` → `commit-tree` →
  `update-ref refs/heads/osc/checkpoints/<branch>`. Restore uses
  `read-tree` + `checkout-index` the same way and deletes files absent from
  the target. The user's index and branch are never involved.
- **Updates** (`osc-update`) read GitHub's latest-release API and download
  with the system `curl` (HTTPS only). A download is accepted only if its
  SHA-256 matches both the release's `SHA256SUMS` and GitHub's asset digest.
  Tarball installs are swapped in place by renaming (old binaries kept as
  `*.old`); the macOS app opens the verified `.dmg`; package-manager installs
  are left to the package manager.
- **OpenSCAD downloads** (`osc-update::openscad`) fetch the official build
  pinned in `openscad-pins.json` (URL, size, SHA-256) when no OpenSCAD is
  installed. The pinned hash is compiled in; nothing is installed unless it
  matches. Builds are unpacked into `<data>/openscad/<version>` and switched
  with a `current` file; `osc_engine::managed` owns that layout and the
  user's *Locate…* choice, so the app, `doctor` and the MCP server all find
  the same OpenSCAD. The `OpenSCAD pins` workflow checks the pins weekly and
  smoke-tests the install on Linux, macOS and Windows.
- **Crash reports** come from a panic hook that writes a redacted text file.
  On the next start, the user is offered a prefilled GitHub issue to review.
  Nothing is sent automatically.

## Why these choices

| Choice | Reason |
| --- | --- |
| GPUI via `gpui-kit` | A GPU-accelerated Rust UI framework, packaged with a complete component set (code editor with tree-sitter, docks, trees, inputs) and pinned to a published GPUI snapshot, so the build is reproducible from crates.io. |
| `tree-sitter-openscad-ng` | The grammar maintained by the OpenSCAD organisation. |
| Drive the `openscad` binary | 100% language compatibility, and every option and backend (CGAL or Manifold) available today. A native evaluator could come later. |
| git CLI instead of libgit2 | The user's config, hooks, credentials and signing apply. |
| Hand-rolled MCP/ACP transports | Tiny, synchronous and fully testable, with no async runtime mixing with GPUI's executor. Protocol types come from the official ACP schema crate. |
