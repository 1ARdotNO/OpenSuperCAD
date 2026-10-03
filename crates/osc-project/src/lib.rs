//! Projects and their state.
//!
//! Like Zed, OpenSuperCAD is organised around *projects*: a folder (usually a
//! git repository) with its files, settings and its own set of AI threads.
//! Switching project switches all of it at once.
//!
//! * [`Store`] persists the recent-project list and per-project threads in the
//!   user's data directory (never inside the project, so repositories stay
//!   clean).
//! * [`Project`] gives access to files and the optional, committable
//!   `.opensupercad/settings.json`.

mod store;
mod thread;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub use store::{RecentProject, Store};
pub use thread::{Message, Role, Thread, ThreadSummary};

#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("i/o error on {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid JSON in {path}: {source}")]
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("path {0} escapes the project root")]
    OutsideProject(PathBuf),
    #[error("unknown thread {0}")]
    UnknownThread(String),
}

pub type Result<T> = std::result::Result<T, ProjectError>;

pub(crate) fn io_err(path: &Path) -> impl FnOnce(std::io::Error) -> ProjectError + '_ {
    move |source| ProjectError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Settings stored in `<project>/.opensupercad/settings.json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectSettings {
    /// The file rendered by default (relative to the project root).
    pub main_file: Option<String>,
    /// Agent id from the agent registry to use for new threads.
    pub default_agent: Option<String>,
    /// Checkpoint the working tree after every AI turn (default: on).
    pub auto_checkpoint: Option<bool>,
    /// OpenSCAD `--backend` (e.g. `manifold`).
    pub openscad_backend: Option<String>,
}

impl ProjectSettings {
    pub fn auto_checkpoint(&self) -> bool {
        self.auto_checkpoint.unwrap_or(true)
    }
}

/// File extensions shown in the project panel and accessible to the agent.
pub const DESIGN_EXTENSIONS: &[&str] = &[
    "scad", "json", "md", "txt", "csv", "dxf", "svg", "stl", "3mf", "off", "png",
];

/// Directories never listed.
const IGNORED_DIRS: &[&str] = &[".git", "target", "node_modules", ".opensupercad"];

#[derive(Clone, Debug)]
pub struct Project {
    root: PathBuf,
}

impl Project {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        let root = root.canonicalize().map_err(io_err(&root))?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn name(&self) -> String {
        self.root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.root.display().to_string())
    }

    fn settings_path(&self) -> PathBuf {
        self.root.join(".opensupercad").join("settings.json")
    }

    pub fn settings(&self) -> ProjectSettings {
        let path = self.settings_path();
        std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save_settings(&self, settings: &ProjectSettings) -> Result<()> {
        let path = self.settings_path();
        write_json(&path, settings)
    }

    /// Resolve a project-relative path, refusing anything that escapes the
    /// project (`..`, absolute paths elsewhere, symlinks pointing out).
    pub fn resolve(&self, relative: &str) -> Result<PathBuf> {
        let candidate = Path::new(relative);
        let joined = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            self.root.join(candidate)
        };
        // Normalise lexically first so non-existent files can be resolved.
        let mut normal = PathBuf::new();
        for comp in joined.components() {
            match comp {
                std::path::Component::ParentDir => {
                    if !normal.pop() {
                        return Err(ProjectError::OutsideProject(joined));
                    }
                }
                std::path::Component::CurDir => {}
                other => normal.push(other),
            }
        }
        if !normal.starts_with(&self.root) {
            return Err(ProjectError::OutsideProject(joined));
        }
        // Then check the nearest existing ancestor for symlink escapes.
        let mut existing = normal.as_path();
        while !existing.exists() {
            match existing.parent() {
                Some(p) => existing = p,
                None => break,
            }
        }
        if let Ok(real) = existing.canonicalize()
            && !real.starts_with(&self.root)
        {
            return Err(ProjectError::OutsideProject(joined));
        }
        Ok(normal)
    }

    pub fn relative(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }

    /// All files in the project (sorted, project-relative), skipping hidden
    /// files and build/VCS directories.
    pub fn files(&self) -> Vec<String> {
        let mut out = Vec::new();
        walk(&self.root, &mut |p| out.push(self.relative(p)));
        out.sort();
        out
    }

    /// OpenSCAD sources in the project.
    pub fn scad_files(&self) -> Vec<String> {
        self.files()
            .into_iter()
            .filter(|f| f.ends_with(".scad"))
            .collect()
    }

    /// The file to render by default: the configured main file, else
    /// `main.scad`, else the only/first `.scad` file.
    pub fn main_file(&self) -> Option<String> {
        if let Some(m) = self.settings().main_file
            && self.root.join(&m).is_file()
        {
            return Some(m);
        }
        let scad = self.scad_files();
        scad.iter()
            .find(|f| f.as_str() == "main.scad")
            .or_else(|| scad.first())
            .cloned()
    }
}

fn walk(dir: &Path, f: &mut dyn FnMut(&Path)) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || IGNORED_DIRS.contains(&name.as_ref()) {
            continue;
        }
        let path = entry.path();
        match entry.file_type() {
            Ok(t) if t.is_dir() => walk(&path, f),
            Ok(t) if t.is_file() => f(&path),
            _ => {}
        }
    }
}

pub(crate) fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io_err(dir))?;
    }
    let json = serde_json::to_vec_pretty(value).map_err(|source| ProjectError::Json {
        path: path.to_path_buf(),
        source,
    })?;
    // Write atomically so a crash never leaves a truncated file behind.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).map_err(io_err(&tmp))?;
    std::fs::rename(&tmp, path).map_err(io_err(path))
}

pub(crate) fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Option<T>> {
    match std::fs::read(path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|source| ProjectError::Json {
                    path: path.to_path_buf(),
                    source,
                })
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io_err(path)(e)),
    }
}

pub(crate) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_settings_and_sandbox() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("parts")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join("parts/a.scad"), "").unwrap();
        std::fs::write(root.join("main.scad"), "").unwrap();
        std::fs::write(root.join(".git/HEAD"), "").unwrap();
        std::fs::write(root.join(".hidden"), "").unwrap();

        let p = Project::open(root).unwrap();
        assert_eq!(p.files(), ["main.scad", "parts/a.scad"]);
        assert_eq!(p.main_file().as_deref(), Some("main.scad"));

        let mut s = p.settings();
        assert!(s.auto_checkpoint());
        s.main_file = Some("parts/a.scad".into());
        p.save_settings(&s).unwrap();
        assert_eq!(p.main_file().as_deref(), Some("parts/a.scad"));
        // Settings dir is not listed.
        assert!(!p.files().iter().any(|f| f.contains("settings")));

        assert!(p.resolve("parts/new.scad").is_ok());
        assert!(p.resolve("parts/../main.scad").is_ok());
        assert!(matches!(
            p.resolve("../x"),
            Err(ProjectError::OutsideProject(_))
        ));
        assert!(p.resolve("/etc/passwd").is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc", root.join("link")).unwrap();
            assert!(p.resolve("link/passwd").is_err());
        }
    }
}
