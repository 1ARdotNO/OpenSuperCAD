//! Checking for and applying updates (see `osc-update`), for the CLI and UI.

use std::path::PathBuf;

use osc_update::{Client, Install, Release, Version};

/// What applying an update did.
pub enum Outcome {
    /// The binaries were replaced; restart `binary` to use them.
    Installed { version: Version, binary: PathBuf },
    /// The verified disk image was opened for the user to install.
    OpenedDmg { path: PathBuf },
    /// The user has to update through their package manager.
    Manual { how: String },
}

/// The latest release, if it is newer than this build.
pub fn check() -> anyhow::Result<Option<Release>> {
    let release = Client::default().latest()?;
    Ok(release.is_newer_than(Version::current()).then_some(release))
}

pub fn install_kind() -> Install {
    std::env::current_exe()
        .map(|exe| Install::detect(&exe.canonicalize().unwrap_or(exe)))
        .unwrap_or(Install::Unsupported {
            reason: "cannot locate the executable".into(),
        })
}

/// Download, verify and install `release`. Blocking; run it off the UI thread.
pub fn apply(release: &Release) -> anyhow::Result<Outcome> {
    let install = install_kind();
    let name = match &install {
        Install::Package { manager, command } => {
            return Ok(Outcome::Manual {
                how: format!("OpenSuperCAD was installed with {manager}: {command}"),
            });
        }
        Install::Unsupported { reason } => {
            return Ok(Outcome::Manual {
                how: format!(
                    "Can't update automatically: {reason}. Download it from {}",
                    release.html_url
                ),
            });
        }
        _ => match osc_update::asset_name(
            release.version,
            &install,
            std::env::consts::OS,
            std::env::consts::ARCH,
        ) {
            Some(name) => name,
            // Windows installs update through the installer for now.
            None => {
                return Ok(Outcome::Manual {
                    how: format!(
                        "Download and run the new installer from {}",
                        release.html_url
                    ),
                });
            }
        },
    };
    let asset = release
        .asset(&name)
        .ok_or_else(|| osc_update::UpdateError::NoAsset(name.clone()))?;
    let client = Client::default();
    match install {
        Install::Tarball { dir } => {
            let download = tempfile::tempdir()?;
            let archive = client.download_verified(release, asset, download.path())?;
            osc_update::install_tarball(&archive, &dir)?;
            Ok(Outcome::Installed {
                version: release.version,
                // The running executable is now `opensupercad.old`.
                binary: dir.join("opensupercad"),
            })
        }
        Install::MacApp { .. } => {
            let dir = dirs::download_dir()
                .or_else(dirs::home_dir)
                .unwrap_or_else(std::env::temp_dir);
            let dmg = client.download_verified(release, asset, &dir)?;
            std::process::Command::new("open").arg(&dmg).status()?;
            Ok(Outcome::OpenedDmg { path: dmg })
        }
        _ => unreachable!("handled above"),
    }
}

/// `opensupercad update [--check]`.
pub fn run_cli(check_only: bool) -> anyhow::Result<()> {
    let current = Version::current();
    println!("OpenSuperCAD {current}: checking for updates…");
    let Some(release) = check()? else {
        println!("You're up to date.");
        return Ok(());
    };
    println!("{} is available: {}", release.tag, release.html_url);
    if check_only {
        return Ok(());
    }
    match apply(&release)? {
        Outcome::Installed { version, .. } => {
            println!("Updated to {version} (SHA-256 verified). Restart OpenSuperCAD to use it.")
        }
        Outcome::OpenedDmg { path } => println!(
            "Downloaded and verified {}. Drag OpenSuperCAD to Applications to finish.",
            path.display()
        ),
        Outcome::Manual { how } => println!("{how}"),
    }
    Ok(())
}
