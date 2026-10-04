//! OpenSuperCAD: an AI-powered, Zed-style editor for OpenSCAD.

mod agents;
mod crash;
mod doctor;
mod openscad;
mod pipeline;
mod session;
mod ui;
mod update;

use std::path::PathBuf;

const USAGE: &str = "\
USAGE:
    opensupercad [PATH]                 open a project folder (or the folder of a .scad file)
    opensupercad mcp [--project DIR]    run the OpenSuperCAD MCP server on stdio
    opensupercad doctor                 check OpenSCAD, git, snapshots and agents
    opensupercad openscad [install]     show or download the OpenSCAD in use (SHA-256 verified)
    opensupercad update [--check]       update from GitHub Releases (SHA-256 verified)
    opensupercad --version | --help";

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("mcp") => {
            crash::install("mcp");
            let mut project = std::env::current_dir()?;
            let mut control = None;
            let mut it = args.iter().skip(1);
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--project" | "-p" => {
                        project = it
                            .next()
                            .map(PathBuf::from)
                            .ok_or_else(|| anyhow::anyhow!("--project needs a value"))?
                    }
                    "--control" => control = it.next().map(PathBuf::from),
                    other => anyhow::bail!("unknown argument `{other}`\n\n{USAGE}"),
                }
            }
            osc_mcp::Server::with_control(project, control)?.serve_stdio()
        }
        Some("doctor") => doctor::run(),
        Some("openscad") => openscad::run_cli(&args[1..]),
        Some("update") => update::run_cli(args.get(1).is_some_and(|a| a == "--check")),
        Some("--version" | "-V") => {
            println!("opensupercad {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("--help" | "-h") => {
            println!("opensupercad {}\n\n{USAGE}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some(flag) if flag.starts_with('-') => anyhow::bail!("unknown option `{flag}`\n\n{USAGE}"),
        path => {
            let path = path.map(PathBuf::from);
            if let Some(p) = path.as_ref().filter(|p| !p.exists()) {
                anyhow::bail!("{}: no such file or folder", p.display());
            }
            if !has_display() {
                anyhow::bail!(
                    "no display found (DISPLAY and WAYLAND_DISPLAY are unset).\n\
                     OpenSuperCAD needs a graphical session; for headless use, run \
                     `opensupercad mcp --project DIR`."
                );
            }
            crash::install("app");
            ui::run(path)
        }
    }
}

/// Whether a window can be opened. Only X11/Wayland desktops can lack one.
fn has_display() -> bool {
    if cfg!(any(target_os = "macos", windows)) {
        return true;
    }
    ["DISPLAY", "WAYLAND_DISPLAY"]
        .iter()
        .any(|v| std::env::var_os(v).is_some_and(|s| !s.is_empty()))
}
