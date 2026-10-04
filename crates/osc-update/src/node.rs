//! Download and install an official Node.js build for users who don't have
//! one, to run the Claude Code and Codex ACP adapters (`npx -y …`). Those
//! adapters are Node.js programs, separate from the `claude` and `codex`
//! CLIs, so a native install of either CLI is not enough.
//!
//! Works like [`crate::openscad`]: every release pins one build per platform
//! in `node-pins.json` (URL, size and SHA-256 from nodejs.org's signed
//! `SHASUMS256.txt`), a download is used only when it matches, and builds are
//! unpacked per user into [`home`] (`<data>/node/`):
//!
//! * `<version>/`: the unpacked build (`bin/node`, `bin/npx`; on Windows
//!   `node.exe` and `npx.cmd` at the top),
//! * `current`: the name of the version folder in use.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

pub use crate::openscad::Progress;
use crate::openscad::{remove_other_versions, unpack_zip_into};
use crate::{Client, Result, UpdateError, sha256_file};

const PINS: &str = include_str!("../node-pins.json");

/// How a build is packaged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum Kind {
    #[serde(rename = "tar.gz")]
    TarGz,
    #[serde(rename = "zip")]
    Zip,
}

/// One pinned official Node.js build.
#[derive(Debug, Clone, Deserialize)]
pub struct Build {
    /// `std::env::consts::OS` value: `linux`, `macos` or `windows`.
    pub os: String,
    /// `std::env::consts::ARCH` value: `x86_64` or `aarch64`.
    pub arch: String,
    pub kind: Kind,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Deserialize)]
struct Pins {
    version: String,
    builds: Vec<Build>,
}

fn pins() -> Option<Pins> {
    serde_json::from_str(PINS).ok()
}

/// The pinned Node.js version, also the folder it's installed in.
pub fn version() -> String {
    pins().map(|p| p.version).unwrap_or_default()
}

/// The pinned build for this platform, if there is one.
pub fn build_for_this_platform() -> Option<Build> {
    build_for(std::env::consts::OS, std::env::consts::ARCH)
}

pub fn build_for(os: &str, arch: &str) -> Option<Build> {
    pins()?
        .builds
        .into_iter()
        .find(|b| b.os == os && b.arch == arch)
}

/// "Node.js 24.21.0 (56 MB)".
pub fn describe(build: &Build) -> String {
    format!(
        "Node.js {} ({} MB)",
        version(),
        build.size.div_ceil(1024 * 1024)
    )
}

/// Where downloaded builds are kept: next to the downloaded OpenSCAD, in
/// the OpenSuperCAD data directory.
pub fn home() -> PathBuf {
    osc_engine::managed::home().with_file_name("node")
}

/// The folder holding `node` and `npx` inside an unpacked build.
fn bin_dir(build_dir: &Path) -> PathBuf {
    if cfg!(windows) {
        build_dir.to_path_buf()
    } else {
        build_dir.join("bin")
    }
}

fn node_in(bin: &Path) -> PathBuf {
    bin.join(if cfg!(windows) { "node.exe" } else { "node" })
}

/// The `bin` folder of the downloaded build in use, for the agents' `PATH`.
pub fn installed(home: &Path) -> Option<PathBuf> {
    let version = std::fs::read_to_string(home.join("current")).ok()?;
    let version = version.trim();
    // A version is a folder name, never a path.
    if version.is_empty() || version.contains(['/', '\\']) || version.starts_with('.') {
        return None;
    }
    let bin = bin_dir(&home.join(version));
    node_in(&bin).is_file().then_some(bin)
}

/// Download `build`, verify it and install it into `home`, making it the
/// build in use. Returns its `bin` folder. Blocking.
///
/// Nothing in `home` changes unless every step succeeds: the build is
/// unpacked into a hidden staging folder, checked by running
/// `node --version`, and only then moved into place.
pub fn install(
    client: &Client,
    build: &Build,
    home: &Path,
    mut progress: impl FnMut(Progress),
) -> Result<PathBuf> {
    let version = version();
    check_pin(build, &version)?;
    std::fs::create_dir_all(home)?;
    let work = home.join(format!(".install-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work)?;
    let result = (|| {
        let file_name = build.url.rsplit('/').next().unwrap_or("node-download");
        let download = work.join(file_name);
        client.download(&build.url, &download, |received| {
            progress(Progress::Downloading {
                received,
                total: build.size,
            })
        })?;
        verify_download(&download, build)?;
        progress(Progress::Unpacking);
        let root = work.join("node");
        match build.kind {
            Kind::TarGz => untar(&download, &root)?,
            Kind::Zip => unpack_zip_into(&download, &root)?,
        }
        let node = node_in(&bin_dir(&root));
        let out = Command::new(&node)
            .arg("--version")
            .output()
            .map_err(|e| UpdateError::Unpack(format!("the downloaded Node.js doesn't run: {e}")))?;
        let reported = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        if !out.status.success() || reported != format!("v{version}") {
            return Err(UpdateError::Unpack(format!(
                "the downloaded Node.js doesn't run (it says: {reported})"
            )));
        }
        let target = home.join(&version);
        let _ = std::fs::remove_dir_all(&target);
        std::fs::rename(&root, &target)?;
        let current = home.join("current.tmp");
        std::fs::write(&current, &version)?;
        std::fs::rename(&current, home.join("current"))?;
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&work);
    result?;
    remove_other_versions(home, &version);
    installed(home).ok_or_else(|| UpdateError::Unpack("the installed Node.js went missing".into()))
}

/// Unpack with the system `tar` (every Linux and macOS has one), dropping
/// the archive's top-level folder (`node-v<version>-linux-x64/`). It keeps
/// the `npx` and `npm` symlinks the archive relies on.
fn untar(file: &Path, into: &Path) -> Result<()> {
    std::fs::create_dir_all(into)?;
    let status = Command::new("tar")
        .args(["--no-same-owner", "--strip-components=1", "-xzf"])
        .arg(file)
        .arg("-C")
        .arg(into)
        .status()
        .map_err(|source| UpdateError::Spawn {
            tool: "tar",
            source,
        })?;
    if !status.success() {
        return Err(UpdateError::Unpack(format!(
            "tar failed on {}",
            file.display()
        )));
    }
    Ok(())
}

/// Refuse pins that could fetch from anywhere but nodejs.org over HTTPS or
/// name a folder outside `home`.
fn check_pin(build: &Build, version: &str) -> Result<()> {
    let valid_version =
        !version.is_empty() && version.chars().all(|c| c.is_ascii_digit() || c == '.');
    let valid_hash =
        build.sha256.len() == 64 && build.sha256.chars().all(|c| c.is_ascii_hexdigit());
    if !build.url.starts_with("https://nodejs.org/dist/")
        || !valid_version
        || !valid_hash
        || build.size == 0
    {
        return Err(UpdateError::Release(format!(
            "invalid Node.js pin for {}/{}",
            build.os, build.arch
        )));
    }
    Ok(())
}

/// The file must have the pinned size and SHA-256.
fn verify_download(file: &Path, build: &Build) -> Result<()> {
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let size = std::fs::metadata(file)?.len();
    if size != build.size {
        return Err(UpdateError::Checksum {
            name,
            expected: format!("{} bytes", build.size),
            actual: format!("{size} bytes"),
        });
    }
    let actual = sha256_file(file)?;
    if !actual.eq_ignore_ascii_case(&build.sha256) {
        return Err(UpdateError::Checksum {
            name,
            expected: build.sha256.to_ascii_lowercase(),
            actual,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pins_are_well_formed() {
        let v = version();
        assert!(v.starts_with("24."), "Node.js {v}: the adapters need 22+");
        for (os, arch) in [
            ("linux", "x86_64"),
            ("linux", "aarch64"),
            ("macos", "aarch64"),
            ("macos", "x86_64"),
            ("windows", "x86_64"),
        ] {
            let b = build_for(os, arch).unwrap_or_else(|| panic!("no pin for {os}/{arch}"));
            check_pin(&b, &v).unwrap();
            assert!(b.url.contains(&format!("/v{v}/node-v{v}-")), "{}", b.url);
            assert_eq!(b.kind == Kind::Zip, os == "windows");
        }
    }

    #[test]
    fn rejects_bad_pins() {
        let b = build_for("linux", "x86_64").unwrap();
        assert!(check_pin(&b, "../x").is_err());
        let mut bad = b.clone();
        bad.url = "https://example.com/node.tar.gz".into();
        assert!(check_pin(&bad, "24.0.0").is_err());
        let mut bad = b;
        bad.sha256 = "abc".into();
        assert!(check_pin(&bad, "24.0.0").is_err());
    }

    #[test]
    fn verifies_size_and_hash() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("node.tar.gz");
        std::fs::write(&file, b"hello").unwrap();
        let mut b = build_for("linux", "x86_64").unwrap();
        b.size = 5;
        b.sha256 = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824".into();
        verify_download(&file, &b).unwrap();
        b.sha256 = "0".repeat(64);
        assert!(matches!(
            verify_download(&file, &b),
            Err(UpdateError::Checksum { .. })
        ));
    }

    #[test]
    fn finds_the_current_build() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        assert!(installed(home).is_none());
        let bin = bin_dir(&home.join("24.21.0"));
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(node_in(&bin), "").unwrap();
        std::fs::write(home.join("current"), "24.21.0\n").unwrap();
        assert_eq!(installed(home), Some(bin));
        std::fs::write(home.join("current"), "../x").unwrap();
        assert!(installed(home).is_none());
    }
}
