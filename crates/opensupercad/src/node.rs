//! The Node.js that runs the Claude Code and Codex ACP adapters, downloaded
//! when none is installed (see `osc_update::node`), and the
//! `opensupercad node` command.

use std::path::PathBuf;

use osc_update::node::{self as pins, Progress};

/// Let agents find the downloaded Node.js, if there is one. The user's own
/// Node.js still comes first.
pub fn register() {
    if let Some(bin) = pins::installed(&pins::home()) {
        osc_agent::path::add_fallback(bin);
    }
}

/// Download, verify and install the pinned build, then [`register`] it.
/// Returns its `bin` folder. Blocking.
pub fn install(progress: impl FnMut(Progress)) -> anyhow::Result<PathBuf> {
    let build = pins::build_for_this_platform().ok_or_else(|| {
        anyhow::anyhow!(
            "no Node.js download for {}/{}; install Node.js 22 or newer from https://nodejs.org",
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    })?;
    let bin = pins::install(
        &osc_update::Client::default(),
        &build,
        &pins::home(),
        progress,
    )?;
    osc_agent::path::add_fallback(bin.clone());
    Ok(bin)
}

const USAGE: &str = "\
USAGE:
    opensupercad node [status]    which Node.js the Claude Code and Codex agents use
    opensupercad node install     download the pinned official Node.js (SHA-256 verified)";

/// `opensupercad node …`.
pub fn run_cli(args: &[String]) -> anyhow::Result<()> {
    match args.first().map(String::as_str) {
        None | Some("status") => status(),
        Some("install") => {
            if let Some(b) = pins::build_for_this_platform() {
                println!("Downloading {} from {}", pins::describe(&b), b.url);
            }
            let mut shown = 0;
            let bin = install(|p| {
                if let Progress::Downloading { received, total } = p {
                    let pct = (received * 100).checked_div(total).unwrap_or(0);
                    if pct >= shown + 10 {
                        shown = pct - pct % 10;
                        println!("  {shown}%");
                    }
                }
            })?;
            println!("✓ Installed and verified (SHA-256): {}", bin.display());
            Ok(())
        }
        Some(other) => anyhow::bail!("unknown command `{other}`\n\n{USAGE}"),
    }
}

fn status() -> anyhow::Result<()> {
    let npx = osc_agent::AgentSpec {
        id: "npx".into(),
        name: "npx".into(),
        command: "npx".into(),
        args: Vec::new(),
        env: Default::default(),
        install_hint: None,
    };
    match npx.resolve() {
        Some(path) => println!("npx: {}", path.display()),
        None => {
            println!("Node.js (npx) was not found.");
            if let Some(b) = pins::build_for_this_platform() {
                println!(
                    "Run `opensupercad node install` to download {}.",
                    pins::describe(&b)
                );
            }
        }
    }
    Ok(())
}
