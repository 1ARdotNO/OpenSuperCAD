# Security Policy

## Reporting a vulnerability

Please report security issues **privately**. Do not open a public issue for
anything exploitable.

- Preferred: use GitHub's **[Report a vulnerability](https://github.com/1ARdotNO/OpenSuperCAD/security/advisories/new)**
  form (Security → Advisories).
- We aim to acknowledge reports within a few days, and to agree on a fix and
  disclosure timeline with you.

Please include the affected version or commit, a description, reproduction
steps and the impact.

## Supported versions

OpenSuperCAD is pre-1.0 and released continuously from `main`. Only the
latest release is supported.

## Threat model and handling notes

- **AI agents run with your privileges.** OpenSuperCAD launches the ACP agent
  you choose (Claude Code, Gemini CLI, Codex, …) as a local process in the
  project directory. The agent's *own* tools (shell, web) are governed by that
  agent's permission system. OpenSuperCAD forwards its permission prompts to
  you and never auto-approves them.
- **OpenSuperCAD's tools are sandboxed to the project.** The built-in MCP
  server and the ACP `fs/*` handlers reject any path outside the project
  root, including `..` traversal and symlinks that point outside it.
- **OpenSCAD runs your design.** `.scad` files are programs. OpenSCAD can
  read files referenced by `import()`, `include` and `use`. Only open projects
  you trust, as you would with OpenSCAD itself.
- **Checkpoints are local git commits** on `osc/checkpoints/*` branches. They
  are never pushed unless you push them yourself. Remember this before
  pushing `--all`.
- **No telemetry, no network calls** from OpenSuperCAD itself. Network access
  comes only from the agent you run.
- **Automated hardening.** Renovate monitors dependencies and GitHub Actions,
  and each change has to pass CI (fmt, clippy `-D warnings`, tests,
  cargo-deny, dependency-review, CodeQL, Trivy, MegaLinter) before it is
  merged automatically. Workflows are SHA-pinned and analysed by
  [zizmor](https://docs.zizmor.sh).
