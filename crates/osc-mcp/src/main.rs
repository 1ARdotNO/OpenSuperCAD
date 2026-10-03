//! `opensupercad-mcp` — the OpenSuperCAD MCP server as a standalone binary,
//! usable from any MCP client:
//!
//! ```sh
//! claude mcp add opensupercad -- opensupercad-mcp --project .
//! ```

use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let mut project = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--project" | "-p" => project = args.next().map(PathBuf::from),
            "--print-skill" => {
                print!("{}", osc_mcp::SKILL);
                return Ok(());
            }
            "--version" | "-V" => {
                println!("opensupercad-mcp {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--help" | "-h" => {
                println!(
                    "opensupercad-mcp {}\n\nMCP server (stdio) exposing OpenSuperCAD tools for a project.\n\n\
                     USAGE: opensupercad-mcp [--project <DIR>] [--print-skill]\n\n\
                     --project <DIR>  Project root (default: current directory)\n\
                     --print-skill    Print the bundled agent skill (SKILL.md) and exit",
                    env!("CARGO_PKG_VERSION")
                );
                return Ok(());
            }
            other => anyhow::bail!("unknown argument `{other}` (see --help)"),
        }
    }
    let project = match project {
        Some(p) => p,
        None => std::env::current_dir()?,
    };
    osc_mcp::Server::new(project)?.serve_stdio()
}
