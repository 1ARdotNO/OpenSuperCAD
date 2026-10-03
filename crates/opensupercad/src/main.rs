//! OpenSuperCAD: an AI-powered, Zed-style editor for OpenSCAD.

mod agents;
mod doctor;
mod pipeline;
mod session;
mod ui;

use std::path::PathBuf;

const USAGE: &str = "\
USAGE:
    opensupercad [PATH]                 open a project folder (or the folder of a .scad file)
    opensupercad mcp [--project DIR]    run the OpenSuperCAD MCP server on stdio
    opensupercad doctor                 check OpenSCAD, git, snapshots and agents
    opensupercad --version | --help";

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("mcp") => {
            let mut project = std::env::current_dir()?;
            let mut it = args.iter().skip(1);
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--project" | "-p" => {
                        project = it
                            .next()
                            .map(PathBuf::from)
                            .ok_or_else(|| anyhow::anyhow!("--project needs a value"))?
                    }
                    other => anyhow::bail!("unknown argument `{other}`\n\n{USAGE}"),
                }
            }
            osc_mcp::Server::new(project)?.serve_stdio()
        }
        Some("doctor") => doctor::run(),
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
            ui::run(path)
        }
    }
}
