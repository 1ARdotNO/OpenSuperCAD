# Contributing

Thanks for helping build OpenSuperCAD! Start with [CLAUDE.md](CLAUDE.md). It
describes the work process (every task is tracked as a todo **and** a GitHub
issue), the architecture and the gates your change must pass.

## Quick start

```sh
git clone https://github.com/1ARdotNO/OpenSuperCAD && cd OpenSuperCAD
cargo test --workspace --exclude opensupercad   # core crates, no GUI libs needed
cargo run -p opensupercad -- examples/phone-stand
```

See [docs/installation.md](docs/installation.md#building-from-source) for the
system libraries the GUI needs, and install `openscad` (plus `xvfb` on headless
Linux) to run the integration tests.

## Before opening a PR

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check
```

- Open or reference an issue (`Closes #N`).
- Use a Conventional Commit title: `feat:`, `fix:`, `docs:`, `chore:`, …
  Merges to `main` are released automatically, and the title decides the
  version bump.
- Update `docs/` and, if MCP tools change, `crates/osc-mcp/skill/SKILL.md`.

## Security

Please report vulnerabilities privately; see [SECURITY.md](SECURITY.md).
