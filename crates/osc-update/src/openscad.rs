//! Download and install an official OpenSCAD build for users who don't have
//! one.
//!
//! Every OpenSuperCAD release pins one build per platform in
//! `openscad-pins.json`: URL, size and SHA-256. A download is used only when
//! it matches the pinned hash, which is compiled into the binary, so a
//! tampered mirror or a swapped upstream file is rejected. Users fetch
//! OpenSCAD from openscad.org themselves; we never redistribute it.
//!
//! Builds are unpacked per user into [`osc_engine::managed::home`], where
//! `Engine::discover` finds them:
//!
//! * AppImage (Linux): extracted, so FUSE isn't needed to run it,
//! * dmg (macOS): `OpenSCAD.app` copied out with `hdiutil` and `ditto`,
//! * zip (Windows): unpacked, dropping the archive's top-level folder.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use crate::{Client, Result, UpdateError, sha256_file};

const PINS: &str = include_str!("../openscad-pins.json");

/// How a build is packaged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    AppImage,
    Dmg,
    Zip,
}

/// One pinned official OpenSCAD build.
#[derive(Debug, Clone, Deserialize)]
pub struct Build {
    /// `std::env::consts::OS` value: `linux`, `macos` or `windows`.
    pub os: String,
    /// `std::env::consts::ARCH` value: `x86_64` or `aarch64`.
    pub arch: String,
    /// OpenSCAD's version, also the folder it's installed in.
    pub version: String,
    pub kind: Kind,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Deserialize)]
struct Pins {
    builds: Vec<Build>,
}

/// Every pinned build.
pub fn builds() -> Vec<Build> {
    serde_json::from_str::<Pins>(PINS)
        .map(|p| p.builds)
        .unwrap_or_default()
}

/// The pinned build for this platform, if there is one.
pub fn build_for_this_platform() -> Option<Build> {
    build_for(std::env::consts::OS, std::env::consts::ARCH)
}

pub fn build_for(os: &str, arch: &str) -> Option<Build> {
    builds().into_iter().find(|b| b.os == os && b.arch == arch)
}

/// Where an install is: downloading, then unpacking.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Progress {
    Downloading { received: u64, total: u64 },
    Unpacking,
}

/// Download `build`, verify it and install it into `home`, making it the
/// build in use. Returns the OpenSCAD executable. Blocking.
///
/// Nothing in `home` changes unless every step succeeds: the build is
/// unpacked into a hidden staging folder, checked by running
/// `openscad --version`, and only then moved into place.
pub fn install(
    client: &Client,
    build: &Build,
    home: &Path,
    mut progress: impl FnMut(Progress),
) -> Result<PathBuf> {
    check_pin(build)?;
    std::fs::create_dir_all(home)?;
    let work = home.join(format!(".install-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work)?;
    let result = (|| {
        let file_name = build.url.rsplit('/').next().unwrap_or("openscad-download");
        let download = work.join(file_name);
        client.download(&build.url, &download, |received| {
            progress(Progress::Downloading {
                received,
                total: build.size,
            })
        })?;
        verify_download(&download, build)?;
        progress(Progress::Unpacking);
        let unpacked = work.join("unpacked");
        std::fs::create_dir_all(&unpacked)?;
        unpack(build.kind, &download, &unpacked)?;
        let root = single_folder(&unpacked)?;
        let binary = osc_engine::managed::binary_in(&root)
            .ok_or_else(|| UpdateError::Unpack(format!("no OpenSCAD executable in {file_name}")))?;
        let version = osc_engine::Engine::new(&binary).version().map_err(|e| {
            UpdateError::Unpack(format!("the downloaded OpenSCAD doesn't run: {e}"))
        })?;
        if !version.contains("OpenSCAD") {
            return Err(UpdateError::Unpack(format!(
                "the downloaded OpenSCAD doesn't run: {version}"
            )));
        }
        let target = home.join(&build.version);
        let _ = std::fs::remove_dir_all(&target);
        std::fs::rename(&root, &target)?;
        osc_engine::managed::set_current(home, &build.version)?;
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&work);
    result?;
    remove_other_versions(home, &build.version);
    osc_engine::managed::installed(home)
        .map(|(_, binary)| binary)
        .ok_or_else(|| UpdateError::Unpack("the installed OpenSCAD went missing".into()))
}

/// Refuse pins that could fetch from anywhere but HTTPS or name a folder
/// outside `home`.
fn check_pin(build: &Build) -> Result<()> {
    let valid_version = !build.version.is_empty()
        && !build.version.starts_with('.')
        && build
            .version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    let valid_hash =
        build.sha256.len() == 64 && build.sha256.chars().all(|c| c.is_ascii_hexdigit());
    if !build.url.starts_with("https://") || !valid_version || !valid_hash || build.size == 0 {
        return Err(UpdateError::Release(format!(
            "invalid OpenSCAD pin for {}/{}",
            build.os, build.arch
        )));
    }
    Ok(())
}

/// The file must have the pinned size and SHA-256.
pub fn verify_download(file: &Path, build: &Build) -> Result<()> {
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

fn unpack(kind: Kind, file: &Path, into: &Path) -> Result<()> {
    match kind {
        Kind::AppImage => unpack_appimage(file, into),
        Kind::Dmg => unpack_dmg(file, into),
        Kind::Zip => unpack_zip(file, into),
    }
}

/// `--appimage-extract` writes `squashfs-root/` into the working directory.
fn unpack_appimage(file: &Path, into: &Path) -> Result<()> {
    make_executable(file)?;
    let out = Command::new(file)
        .arg("--appimage-extract")
        .current_dir(into)
        .output()
        .map_err(|source| UpdateError::Spawn {
            tool: "AppImage",
            source,
        })?;
    let extracted = into.join("squashfs-root");
    if !out.status.success() || !extracted.is_dir() {
        return Err(UpdateError::Unpack(format!(
            "extracting the AppImage failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    // One folder holding `AppDir/AppRun`, the layout `binary_in` expects.
    let root = into.join("openscad");
    std::fs::create_dir_all(&root)?;
    std::fs::rename(extracted, root.join("AppDir"))?;
    Ok(())
}

#[cfg(unix)]
fn make_executable(file: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o755))?;
    Ok(())
}

#[cfg(not(unix))]
fn make_executable(_: &Path) -> Result<()> {
    Ok(())
}

/// Mount the image read-only, copy the app bundle out, always unmount.
fn unpack_dmg(file: &Path, into: &Path) -> Result<()> {
    let mount = into.join("mnt");
    std::fs::create_dir_all(&mount)?;
    let attach = Command::new("hdiutil")
        .args([
            "attach",
            "-nobrowse",
            "-readonly",
            "-noautoopen",
            "-mountpoint",
        ])
        .arg(&mount)
        .arg(file)
        // A licence prompt would otherwise wait for input forever.
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|source| UpdateError::Spawn {
            tool: "hdiutil",
            source,
        })?;
    if !attach.status.success() {
        return Err(UpdateError::Unpack(format!(
            "mounting the disk image failed: {}",
            String::from_utf8_lossy(&attach.stderr).trim()
        )));
    }
    let copied = (|| {
        let app = std::fs::read_dir(&mount)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .find(|p| p.extension().is_some_and(|e| e == "app"))
            .ok_or_else(|| UpdateError::Unpack("no .app in the disk image".into()))?;
        let root = into.join("openscad");
        std::fs::create_dir_all(&root)?;
        // `ditto` keeps the bundle's symlinks, permissions and signature.
        let status = Command::new("ditto")
            .arg(&app)
            .arg(root.join("OpenSCAD.app"))
            .status()
            .map_err(|source| UpdateError::Spawn {
                tool: "ditto",
                source,
            })?;
        if !status.success() {
            return Err(UpdateError::Unpack("copying OpenSCAD.app failed".into()));
        }
        Ok(())
    })();
    let _ = Command::new("hdiutil")
        .args(["detach", "-quiet", "-force"])
        .arg(&mount)
        .status();
    let _ = std::fs::remove_dir(&mount);
    copied
}

/// Unpack with the `zip` crate, which rejects entries escaping `into`.
fn unpack_zip(file: &Path, into: &Path) -> Result<()> {
    let bad = |e: zip::result::ZipError| UpdateError::Unpack(format!("bad zip archive: {e}"));
    let mut archive = zip::ZipArchive::new(std::fs::File::open(file)?).map_err(bad)?;
    let root = into.join("openscad");
    std::fs::create_dir_all(&root)?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(bad)?;
        let Some(rel) = entry.enclosed_name() else {
            return Err(UpdateError::Unpack(format!(
                "unsafe path in the archive: {}",
                entry.name()
            )));
        };
        // Drop the top-level folder (`OpenSCAD-<version>-x86-64/`).
        let rel: PathBuf = rel.components().skip(1).collect();
        if rel.as_os_str().is_empty() {
            continue;
        }
        let path = root.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&path)?;
        } else {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::io::copy(&mut entry, &mut std::fs::File::create(&path)?)?;
        }
    }
    Ok(())
}

/// The one folder every unpacker leaves in `dir`.
fn single_folder(dir: &Path) -> Result<PathBuf> {
    let root = dir.join("openscad");
    if root.is_dir() {
        Ok(root)
    } else {
        Err(UpdateError::Unpack("nothing was unpacked".into()))
    }
}

/// Drop builds replaced by `keep`, best effort. Hidden folders (staging,
/// other installs in progress) and files are left alone.
fn remove_other_versions(home: &Path, keep: &str) {
    let Ok(entries) = std::fs::read_dir(home) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.path().is_dir() && name != keep && !name.starts_with('.') {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn pins_are_well_formed() {
        let builds = builds();
        assert!(!builds.is_empty(), "openscad-pins.json must parse");
        for b in &builds {
            check_pin(b).unwrap();
            assert!(
                b.url.starts_with("https://files.openscad.org/"),
                "{}",
                b.url
            );
            assert!(["linux", "macos", "windows"].contains(&b.os.as_str()));
        }
        for (os, arch) in [
            ("linux", "x86_64"),
            ("macos", "aarch64"),
            ("windows", "x86_64"),
        ] {
            assert!(build_for(os, arch).is_some(), "no pin for {os}/{arch}");
        }
    }

    #[test]
    fn rejects_bad_pins() {
        let mut b = build_for("linux", "x86_64").unwrap();
        b.url = "http://files.openscad.org/x".into();
        assert!(check_pin(&b).is_err());
        let mut b = build_for("linux", "x86_64").unwrap();
        b.version = "../../etc".into();
        assert!(check_pin(&b).is_err());
        let mut b = build_for("linux", "x86_64").unwrap();
        b.sha256 = "abc".into();
        assert!(check_pin(&b).is_err());
    }

    #[test]
    fn verifies_size_and_hash() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("x.zip");
        std::fs::write(&file, b"hello").unwrap();
        let mut b = build_for("windows", "x86_64").unwrap();
        b.size = 5;
        // sha256("hello")
        b.sha256 = "2CF24DBA5FB0A30E26E83B2AC5B9E29E1B161E5C1FA7425E73043362938B9824".into();
        verify_download(&file, &b).unwrap();
        b.size = 6;
        assert!(matches!(
            verify_download(&file, &b),
            Err(UpdateError::Checksum { .. })
        ));
        b.size = 5;
        b.sha256 = "0".repeat(64);
        assert!(matches!(
            verify_download(&file, &b),
            Err(UpdateError::Checksum { .. })
        ));
    }

    fn zip_with(entries: &[(&str, &[u8])]) -> tempfile::NamedTempFile {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut w = zip::ZipWriter::new(file.reopen().unwrap());
        for (name, data) in entries {
            w.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap();
        file
    }

    #[test]
    fn unpacks_the_windows_zip_without_its_top_folder() {
        let zip = zip_with(&[
            ("OpenSCAD-2026.10.03-x86-64/openscad.com", b"com"),
            ("OpenSCAD-2026.10.03-x86-64/openscad.exe", b"exe"),
            ("OpenSCAD-2026.10.03-x86-64/libraries/README", b"r"),
        ]);
        let dir = tempfile::tempdir().unwrap();
        unpack_zip(zip.path(), dir.path()).unwrap();
        let root = single_folder(dir.path()).unwrap();
        assert_eq!(
            osc_engine::managed::binary_in(&root),
            Some(root.join("openscad.com"))
        );
        assert!(root.join("libraries/README").is_file());
    }

    #[test]
    fn refuses_zip_entries_that_escape() {
        let zip = zip_with(&[("../../evil", b"x")]);
        let dir = tempfile::tempdir().unwrap();
        assert!(unpack_zip(zip.path(), dir.path()).is_err());
        assert!(!dir.path().join("../../evil").exists());
    }

    #[test]
    fn keeps_only_the_current_version() {
        let home = tempfile::tempdir().unwrap();
        for d in ["2021.01", "2026.10.03", ".install-1"] {
            std::fs::create_dir_all(home.path().join(d)).unwrap();
        }
        std::fs::write(home.path().join("current"), "2026.10.03").unwrap();
        remove_other_versions(home.path(), "2026.10.03");
        assert!(!home.path().join("2021.01").exists());
        assert!(home.path().join("2026.10.03").exists());
        assert!(home.path().join(".install-1").exists());
        assert!(home.path().join("current").exists());
    }
}
