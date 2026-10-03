//! `opensupercad doctor`: check the environment and explain what is missing.

use osc_engine::{Camera, Engine, RenderMode, Request, View};

pub fn run() -> anyhow::Result<()> {
    println!("OpenSuperCAD {}\n", env!("CARGO_PKG_VERSION"));
    let mut problems = 0;

    match Engine::discover() {
        Ok(engine) => {
            println!(
                "✓ OpenSCAD   {} ({})",
                engine.version().unwrap_or_else(|e| e.to_string()),
                engine.binary.display()
            );
            if !engine.png_wrapper.is_empty() {
                println!(
                    "  snapshots use `{}` (no display detected)",
                    engine.png_wrapper.join(" ")
                );
            }
            match snapshot_check(&engine) {
                Ok(()) => println!("✓ Snapshots  OpenSCAD can render PNG images"),
                Err(e) => {
                    problems += 1;
                    println!("✗ Snapshots  {e}");
                    if cfg!(target_os = "linux") {
                        println!("  on a headless machine install xvfb (xvfb-run)");
                    }
                }
            }
        }
        Err(e) => {
            problems += 1;
            println!("✗ OpenSCAD   {e}");
            println!("  see docs/installation.md#installing-openscad");
        }
    }

    match std::process::Command::new("git").arg("--version").output() {
        Ok(out) if out.status.success() => {
            println!(
                "✓ git        {}",
                String::from_utf8_lossy(&out.stdout).trim()
            )
        }
        _ => {
            problems += 1;
            println!("✗ git        not found: git integration and checkpoints are disabled");
        }
    }

    println!("\nAgents (ACP):");
    let registry = crate::agents::registry();
    for agent in registry.agents() {
        if agent.is_available() {
            println!(
                "✓ {:<12} {} {}",
                agent.name,
                agent.command,
                agent.args.join(" ")
            );
        } else {
            println!(
                "· {:<12} not found ({}). {}",
                agent.name,
                agent.command,
                agent.install_hint.as_deref().unwrap_or("")
            );
        }
    }
    if !registry.agents().iter().any(|a| a.is_available()) {
        problems += 1;
        println!("✗ no agent is installed; see docs/agents.md");
    }

    println!(
        "\nData directory: {}",
        osc_project::Store::default_location().dir().display()
    );
    println!(
        "Agent overrides: {}",
        crate::agents::config_dir().join("agents.json").display()
    );
    if problems == 0 {
        println!("\nAll good.");
    } else {
        println!("\n{problems} problem(s) found.");
    }
    Ok(())
}

fn snapshot_check(engine: &Engine) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("doctor.scad");
    std::fs::write(&file, "cube(10);\n")?;
    let out = engine.snapshot(
        &Request::new(&file),
        &dir.path().join("doctor.png"),
        &Camera::preset(View::Iso),
        (128, 96),
        RenderMode::Preview,
    )?;
    if out.success {
        Ok(())
    } else {
        anyhow::bail!("OpenSCAD could not produce a PNG:\n{}", out.console.trim())
    }
}
