//! Getting an OpenSCAD to use: the verified download (see
//! `osc_update::openscad`), the user's own choice, and the
//! `opensupercad openscad` command.

use std::path::{Path, PathBuf};

use osc_engine::{Engine, managed};
use osc_update::openscad::{self as pins, Build, Progress};

/// "OpenSCAD 2026.10.03 (81 MB)" for the build this platform downloads.
pub fn describe(build: &Build) -> String {
    format!(
        "OpenSCAD {} ({} MB)",
        build.version,
        build.size.div_ceil(1024 * 1024)
    )
}

/// Download, verify and install the pinned build. Blocking.
pub fn install(progress: impl FnMut(Progress)) -> anyhow::Result<PathBuf> {
    let build = pins::build_for_this_platform().ok_or_else(|| {
        anyhow::anyhow!(
            "no OpenSCAD download for {}/{}; install it from https://openscad.org/downloads.html",
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    })?;
    let client = osc_update::Client::default();
    Ok(pins::install(&client, &build, &managed::home(), progress)?)
}

/// Use `path` from now on: an OpenSCAD executable, or an `OpenSCAD.app` or
/// unpacked build folder containing one. Checked by running it.
pub fn locate(path: &Path) -> anyhow::Result<Engine> {
    let binary = if path.is_dir() {
        if path.extension().is_some_and(|e| e == "app") {
            Some(path.join("Contents/MacOS/OpenSCAD"))
        } else {
            managed::binary_in(path)
        }
        .filter(|b| b.is_file())
        .ok_or_else(|| anyhow::anyhow!("no OpenSCAD executable in {}", path.display()))?
    } else {
        path.to_path_buf()
    };
    let version = Engine::new(&binary)
        .version()
        .map_err(|e| anyhow::anyhow!("{} doesn't run: {e}", binary.display()))?;
    if !version.contains("OpenSCAD") {
        let first = version.lines().next().unwrap_or_default();
        anyhow::bail!("{} is not OpenSCAD (it says: {first})", binary.display());
    }
    managed::select(&managed::home(), Some(&binary))?;
    Ok(Engine::discover()?)
}

/// Forget the user's choice and find OpenSCAD automatically again.
pub fn use_automatic() -> anyhow::Result<()> {
    Ok(managed::select(&managed::home(), None)?)
}

/// Where the OpenSCAD in use comes from, for Settings and `status`.
pub fn origin(engine: &Engine) -> &'static str {
    let home = managed::home();
    if std::env::var_os("OPENSUPERCAD_OPENSCAD").is_some() {
        "from $OPENSUPERCAD_OPENSCAD"
    } else if managed::selected(&home).is_some_and(|p| p == engine.binary) {
        "chosen by you"
    } else if engine.binary.starts_with(&home) {
        "downloaded by OpenSuperCAD"
    } else {
        "found on this system"
    }
}

const USAGE: &str = "\
USAGE:
    opensupercad openscad [status]      which OpenSCAD is used, and from where
    opensupercad openscad install       download the pinned official build (SHA-256 verified)
    opensupercad openscad locate PATH   use this OpenSCAD from now on
    opensupercad openscad auto          forget the chosen path, find OpenSCAD automatically";

/// `opensupercad openscad …`.
pub fn run_cli(args: &[String]) -> anyhow::Result<()> {
    match args.first().map(String::as_str) {
        None | Some("status") => status(),
        Some("install") => {
            let build = pins::build_for_this_platform();
            if let Some(b) = &build {
                println!("Downloading {} from {}", describe(b), b.url);
            }
            let mut shown = 0;
            let binary = install(|p| {
                if let Progress::Downloading { received, total } = p {
                    let pct = (received * 100).checked_div(total).unwrap_or(0);
                    if pct >= shown + 10 {
                        shown = pct - pct % 10;
                        println!("  {shown}%");
                    }
                }
            })?;
            println!("✓ Installed and verified (SHA-256): {}", binary.display());
            status()
        }
        Some("locate") => {
            let path = args
                .get(1)
                .ok_or_else(|| anyhow::anyhow!("locate needs a path\n\n{USAGE}"))?;
            let engine = locate(Path::new(path))?;
            println!("✓ Using {}", engine.binary.display());
            Ok(())
        }
        Some("auto") => {
            use_automatic()?;
            status()
        }
        Some(other) => anyhow::bail!("unknown command `{other}`\n\n{USAGE}"),
    }
}

fn status() -> anyhow::Result<()> {
    match Engine::discover() {
        Ok(engine) => println!(
            "{} ({})\n{}",
            engine.version().unwrap_or_else(|e| e.to_string()),
            origin(&engine),
            engine.binary.display()
        ),
        Err(_) => {
            println!("OpenSCAD was not found.");
            match pins::build_for_this_platform() {
                Some(b) => println!(
                    "Run `opensupercad openscad install` to download {}.",
                    describe(&b)
                ),
                None => println!("Install it from https://openscad.org/downloads.html"),
            }
        }
    }
    Ok(())
}
