//! The OpenSCAD that OpenSuperCAD downloads for the user, and the one they
//! picked with *Locate…*. Both live in [`home`] (`<data>/openscad/`):
//!
//! * `<version>/`: an unpacked official build (installed by `osc-update`),
//! * `current`: the name of the version folder in use,
//! * `selected`: a path the user chose, which wins over every other location
//!   except `$OPENSUPERCAD_OPENSCAD`.
//!
//! Plain files rather than symlinks, so the layout works on Windows too.

use std::path::{Path, PathBuf};

/// Where downloaded builds and the user's choice are kept. Follows the
/// OpenSuperCAD data directory, including `$OPENSUPERCAD_DATA_DIR`.
pub fn home() -> PathBuf {
    std::env::var_os("OPENSUPERCAD_DATA_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            dirs::data_dir().map(|d| {
                d.join(if cfg!(target_os = "macos") {
                    "OpenSuperCAD"
                } else {
                    "opensupercad"
                })
            })
        })
        .unwrap_or_else(|| PathBuf::from(".opensupercad-data"))
        .join("openscad")
}

/// The OpenSCAD executable inside an unpacked build: an extracted AppImage,
/// an app bundle copied out of the dmg, or the Windows zip's contents.
pub fn binary_in(dir: &Path) -> Option<PathBuf> {
    [
        "AppDir/AppRun",
        "OpenSCAD.app/Contents/MacOS/OpenSCAD",
        // The console variant, so output reaches us on Windows.
        "openscad.com",
        "openscad.exe",
        "openscad",
    ]
    .iter()
    .map(|rel| dir.join(rel))
    .find(|p| p.is_file())
}

/// The downloaded build in use: its version and executable.
pub fn installed(home: &Path) -> Option<(String, PathBuf)> {
    let version = std::fs::read_to_string(home.join("current")).ok()?;
    let version = version.trim();
    // A version is a folder name, never a path.
    if version.is_empty() || version.contains(['/', '\\']) || version.starts_with('.') {
        return None;
    }
    binary_in(&home.join(version)).map(|b| (version.to_owned(), b))
}

/// Make `version` (a folder in `home`) the build in use.
pub fn set_current(home: &Path, version: &str) -> std::io::Result<()> {
    write_atomic(&home.join("current"), version)
}

/// The executable the user chose with *Locate…*, if it still exists.
pub fn selected(home: &Path) -> Option<PathBuf> {
    let path = std::fs::read_to_string(home.join("selected")).ok()?;
    let path = PathBuf::from(path.trim());
    path.is_file().then_some(path)
}

/// Remember `path` as the user's choice, or forget it with `None`.
pub fn select(home: &Path, path: Option<&Path>) -> std::io::Result<()> {
    let file = home.join("selected");
    match path {
        Some(p) => write_atomic(&file, &p.to_string_lossy()),
        None => match std::fs::remove_file(&file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        },
    }
}

fn write_atomic(file: &Path, contents: &str) -> std::io::Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = file.with_extension("tmp");
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_build_and_selection() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        assert!(installed(home).is_none());

        let app = home.join("2026.10.03/AppDir");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(app.join("AppRun"), "").unwrap();
        set_current(home, "2026.10.03").unwrap();
        let (version, binary) = installed(home).unwrap();
        assert_eq!(version, "2026.10.03");
        assert_eq!(binary, app.join("AppRun"));

        // `current` never escapes the folder.
        std::fs::write(home.join("current"), "../etc").unwrap();
        assert!(installed(home).is_none());

        let chosen = home.join("my-openscad");
        std::fs::write(&chosen, "").unwrap();
        select(home, Some(&chosen)).unwrap();
        assert_eq!(selected(home), Some(chosen.clone()));
        std::fs::remove_file(&chosen).unwrap();
        assert_eq!(selected(home), None, "a vanished choice is ignored");
        select(home, None).unwrap();
        select(home, None).unwrap();
    }

    #[test]
    fn finds_the_binary_in_each_layout() {
        let dir = tempfile::tempdir().unwrap();
        assert!(binary_in(dir.path()).is_none());
        let mac = dir.path().join("OpenSCAD.app/Contents/MacOS");
        std::fs::create_dir_all(&mac).unwrap();
        std::fs::write(mac.join("OpenSCAD"), "").unwrap();
        std::fs::write(dir.path().join("openscad.exe"), "").unwrap();
        assert_eq!(binary_in(dir.path()), Some(mac.join("OpenSCAD")));
    }
}
