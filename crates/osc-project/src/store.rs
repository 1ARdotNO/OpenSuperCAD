use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{ProjectError, Result, Thread, ThreadSummary, read_json, write_json};

const MAX_RECENT: usize = 30;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecentProject {
    pub root: PathBuf,
    pub name: String,
    pub last_opened: u64,
    /// File that was focused when the project was last closed.
    #[serde(default)]
    pub last_file: Option<String>,
    /// Thread that was active when the project was last closed.
    #[serde(default)]
    pub last_thread: Option<String>,
}

/// Application state on disk (`~/.local/share/opensupercad` on Linux,
/// `~/Library/Application Support/OpenSuperCAD` on macOS).
#[derive(Clone, Debug)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    /// The default per-user store. `$OPENSUPERCAD_DATA_DIR` overrides it.
    pub fn default_location() -> Self {
        let dir = std::env::var_os("OPENSUPERCAD_DATA_DIR")
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
            .unwrap_or_else(|| PathBuf::from(".opensupercad-data"));
        Self { dir }
    }

    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn recent_path(&self) -> PathBuf {
        self.dir.join("recent-projects.json")
    }

    pub fn recent_projects(&self) -> Vec<RecentProject> {
        let mut list: Vec<RecentProject> = read_json(&self.recent_path())
            .ok()
            .flatten()
            .unwrap_or_default();
        list.retain(|p| p.root.is_dir());
        list.sort_by_key(|p| std::cmp::Reverse(p.last_opened));
        list
    }

    /// Record that a project was opened (moves it to the top of the list).
    pub fn touch_project(&self, root: &Path) -> Result<RecentProject> {
        let mut list = self.recent_projects();
        let previous = list
            .iter()
            .position(|p| p.root == root)
            .map(|i| list.remove(i));
        let entry = RecentProject {
            root: root.to_path_buf(),
            name: root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| root.display().to_string()),
            last_opened: crate::now(),
            last_file: previous.as_ref().and_then(|p| p.last_file.clone()),
            last_thread: previous.and_then(|p| p.last_thread),
        };
        list.insert(0, entry.clone());
        list.truncate(MAX_RECENT);
        write_json(&self.recent_path(), &list)?;
        Ok(entry)
    }

    /// Remember the focused file / thread of a project.
    pub fn remember(&self, root: &Path, file: Option<&str>, thread: Option<&str>) -> Result<()> {
        let mut list = self.recent_projects();
        if let Some(p) = list.iter_mut().find(|p| p.root == root) {
            if file.is_some() {
                p.last_file = file.map(str::to_owned);
            }
            if thread.is_some() {
                p.last_thread = thread.map(str::to_owned);
            }
            write_json(&self.recent_path(), &list)?;
        }
        Ok(())
    }

    pub fn forget_project(&self, root: &Path) -> Result<()> {
        let mut list = self.recent_projects();
        list.retain(|p| p.root != root);
        write_json(&self.recent_path(), &list)
    }

    /// Directory holding the state of one project (keyed by a stable hash of
    /// its root path).
    pub fn project_dir(&self, root: &Path) -> PathBuf {
        self.dir
            .join("projects")
            .join(format!("{:016x}", fnv1a(root.to_string_lossy().as_bytes())))
    }

    fn threads_dir(&self, root: &Path) -> PathBuf {
        self.project_dir(root).join("threads")
    }

    fn thread_path(&self, root: &Path, id: &str) -> Result<PathBuf> {
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err(ProjectError::UnknownThread(id.to_owned()));
        }
        Ok(self.threads_dir(root).join(format!("{id}.json")))
    }

    pub fn save_thread(&self, root: &Path, thread: &Thread) -> Result<()> {
        write_json(&self.thread_path(root, &thread.id)?, thread)
    }

    pub fn load_thread(&self, root: &Path, id: &str) -> Result<Thread> {
        read_json(&self.thread_path(root, id)?)?
            .ok_or_else(|| ProjectError::UnknownThread(id.to_owned()))
    }

    pub fn delete_thread(&self, root: &Path, id: &str) -> Result<()> {
        let path = self.thread_path(root, id)?;
        std::fs::remove_file(&path).map_err(crate::io_err(&path))
    }

    /// Threads of a project, most recently updated first.
    pub fn threads(&self, root: &Path) -> Vec<ThreadSummary> {
        let Ok(entries) = std::fs::read_dir(self.threads_dir(root)) else {
            return Vec::new();
        };
        let mut list: Vec<ThreadSummary> = entries
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| read_json::<Thread>(&e.path()).ok().flatten())
            .map(|t| t.summary())
            .collect();
        list.sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| b.id.cmp(&a.id)));
        list
    }
}

/// 64-bit FNV-1a: stable across platforms and Rust versions (unlike
/// `DefaultHasher`), which matters because it names on-disk directories.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Role;

    #[test]
    fn recent_projects_and_threads() {
        let data = tempfile::tempdir().unwrap();
        let p1 = tempfile::tempdir().unwrap();
        let p2 = tempfile::tempdir().unwrap();
        let store = Store::at(data.path());

        store.touch_project(p1.path()).unwrap();
        store.touch_project(p2.path()).unwrap();
        store.remember(p1.path(), Some("main.scad"), None).unwrap();
        store.touch_project(p1.path()).unwrap();
        let recent = store.recent_projects();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].root, p1.path());
        assert_eq!(recent[0].last_file.as_deref(), Some("main.scad"));

        let mut t = Thread::new("claude-code");
        t.push(Role::User, "make a gear");
        store.save_thread(p1.path(), &t).unwrap();
        assert_eq!(store.threads(p1.path()).len(), 1);
        assert!(
            store.threads(p2.path()).is_empty(),
            "threads are per project"
        );
        assert_eq!(store.load_thread(p1.path(), &t.id).unwrap(), t);
        assert!(store.load_thread(p1.path(), "../../etc").is_err());
        store.delete_thread(p1.path(), &t.id).unwrap();
        assert!(store.threads(p1.path()).is_empty());

        assert_eq!(fnv1a(b"a"), 0xaf63dc4c8601ec8c);
    }
}
