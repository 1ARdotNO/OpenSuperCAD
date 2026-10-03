# CLAUDE.md: how we work on OpenSuperCAD

OpenSuperCAD is an AI-powered, Zed-style editor for OpenSCAD, written in Rust
on GPUI. This file is the contract for anyone, human or AI agent, who works
on the repository. Read it before you start.

## 1. Work process (mandatory)

### Every task is tracked twice: in the todo list AND as a GitHub issue

Nothing may exist in only one place, so that nothing gets lost between
sessions, agents and people.

1. **Before starting work**, break the request into tasks and create:
   - a todo-list entry for each task (the agent's TaskCreate/TodoWrite list),
     and
   - a GitHub issue for each task in `1ARdotNO/OpenSuperCAD`. Larger efforts
     get an *epic* issue with the tasks as sub-issues.
   Search existing issues first (`search_issues`) to avoid duplicates.
2. **Cross-reference them.** Put the issue number in the todo subject, e.g.
   `Add snapshot tool (#6)`, and keep issue checklists in sync with progress.
3. **While working**, mark the todo `in_progress`. When you discover new
   work (follow-ups, bugs, tech debt), create **both** a todo and an issue
   right away. Never leave a "TODO" only in code or only in chat.
4. **When done**, mark the todo `completed` and close the issue, or reference
   it with `Closes #N` in the PR or commit so it closes on merge. Leave a
   short comment with what was done if it isn't obvious from the PR.
5. If a task is dropped, close its issue as *not planned* with a reason and
   delete the todo.
6. Every GitHub comment, issue or PR body written by an agent ends with the
   attribution footer required by the agent's environment.

### Branches, commits and PRs

- Work on a feature branch and merge to `main` via PR. `main` is protected
  by the `ci-ok` check and is released automatically (see §4).
- Use **Conventional Commits**; the release version is computed from them:
  `feat:` → minor, `fix:`/`perf:`/`refactor:`/`chore:` → patch,
  `feat!:` / `BREAKING CHANGE:` → major (minor while < 1.0).
  `docs:`-only changes to `*.md`/`docs/` do not trigger a release.
- Reference issues in commits and PRs (`Refs #5`, `Closes #6`).
- Keep PRs focused. Use the PR template in `.github/pull_request_template.md`.

## 2. Before you push: run the same gates as CI

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings   # warnings are errors
cargo test --workspace                                   # core + app
cargo deny check                                         # advisories, licenses, bans, sources
```

The integration tests in `osc-engine` and `osc-mcp` use a real `openscad`
binary when one is installed, and skip themselves otherwise. On a headless
Linux box, install `openscad xvfb xauth` to run them as CI does. The GUI crate
(`crates/opensupercad`) needs the GPUI system libraries listed in
`docs/installation.md#building-from-source`.

## 3. Architecture

```
crates/
  osc-syntax    OpenSCAD lexer, outline and Customizer parameters (parse + rewrite)
  osc-engine    drives the `openscad` CLI: export, PNG snapshots from cameras,
                diagnostics; STL loader + software rasteriser for the viewport
  osc-git       git CLI wrapper; AI checkpoints on `osc/checkpoints/<branch>`
                built with a temporary index (never touches the user's index)
  osc-project   projects, recent projects, settings, per-project agent threads
  osc-agent     ACP client (official schema types, sync stdio transport) and the
                agent registry (Claude Code, Gemini CLI, Codex, Goose, OpenCode, custom)
  osc-mcp       built-in MCP server + `opensupercad-mcp` binary; ships the agent
                skill (skill/SKILL.md) via MCP `instructions`
  opensupercad  the GPUI desktop app (gpui-kit); `opensupercad mcp` runs the MCP server
```

Design principles, borrowed from Zed:

- **Project-centric.** A project is a folder, usually a git repo, with its own
  files, settings and AI threads. Switching project switches all of them.
- **Agents are external.** OpenSuperCAD is an ACP *client*, so any model or
  agent can drive it. Every session gets the `opensupercad` MCP server attached,
  so the agent can see (snapshots) and act (render, edit, parameters,
  checkpoints).
- **OpenSCAD stays the source of truth.** We drive the upstream `openscad`
  binary, so every language feature and option keeps working. We re-implement
  only what the UI needs to be fast: highlighting, outline, customizer and
  viewport orbiting.
- **Safety by default.** Tool and fs access is sandboxed to the project root,
  agent permission prompts always go to the user, and checkpoints let every AI
  iteration be undone.
- Keep crates UI-free and unit-tested. Only `crates/opensupercad` depends on GPUI.

Conventions: Rust 2024 edition, `unsafe_code = "deny"`, no `dbg!`/`todo!`, and
errors via `thiserror` in libraries (`anyhow` only in binaries). Match the
comment density and naming of the surrounding code.

## 4. CI, security and releases

Mirrors the `1ARdotNO/wyrm` and `1ARdotNO/Jync` setups:

| Workflow | Gate |
| --- | --- |
| `ci.yml` | fmt, clippy `-D warnings`, tests (real OpenSCAD via xvfb), MCP smoke test, app build on Linux + macOS, cargo-deny, dependency-review → **`ci-ok`** (the single required check) |
| `codeql.yml` | CodeQL security-and-quality (Rust) |
| `trivy.yml` | Trivy fs scan, blocking on fixable HIGH/CRITICAL |
| `zizmor.yml` | GitHub Actions hardening (SARIF) |
| `rust-clippy.yml` | clippy SARIF to the Security tab |
| `sbom.yml` | SPDX SBOM to the dependency graph |
| `mega-linter.yml` | actionlint, yamllint, jsonlint, markdownlint, gitleaks |
| `release.yml` | every code push to `main` → versioned GitHub Release: Linux x86_64/aarch64 tarballs + `.deb`, Arch `.pkg.tar.zst`, macOS universal `.dmg` |

- **Renovate** (`renovate.json`) automerges everything that is green, majors
  included, with lockfile maintenance and vulnerability alerts. CI is the
  safety net, so keep it strict. Never weaken a gate to make a bump pass: fix
  the code, or pin and open an issue.
- All actions are pinned by commit SHA with `persist-credentials: false` and
  least-privilege `permissions`.
- Repository settings required for automerge (one-time, by a maintainer):
  "Allow auto-merge" on, and branch protection on `main` requiring `ci-ok`.

## 5. Docs

User docs live in `README.md` and `docs/`: `installation.md`, `usage.md`,
`agents.md` and `architecture.md`. Maintainer docs live in `docs/releasing.md`
(release secrets and channels). Update them in the same PR as any
user-visible change. The agent skill lives in `crates/osc-mcp/skill/SKILL.md`.
Keep it accurate when tools change; it is what agents read.
