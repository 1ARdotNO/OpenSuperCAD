//! Git integration.
//!
//! Wraps the `git` command line (like Zed does for most operations) so the
//! user's own configuration, hooks, credentials and signing setup apply.
//!
//! Besides the usual status/commit/log operations, this crate implements
//! **checkpoints**: snapshots of the whole working tree taken after every AI
//! iteration. They are stored as commits on a dedicated branch,
//! `osc/checkpoints/<branch>`, built through a temporary index, so taking a
//! checkpoint never touches the user's branch, index or staged changes.
//! Any checkpoint can be restored, and the current state can be *promoted*
//! to a regular commit on the working branch when the user is happy with it.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

/// Prefix of the branches holding checkpoints.
pub const CHECKPOINT_PREFIX: &str = "osc/checkpoints/";

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("failed to run git: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("git {args} failed: {stderr}")]
    Command { args: String, stderr: String },
    #[error("{0} is not inside a git repository")]
    NotARepository(PathBuf),
    #[error("unknown checkpoint `{0}`")]
    UnknownCheckpoint(String),
}

pub type Result<T> = std::result::Result<T, GitError>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FileStatus {
    pub path: String,
    /// Two-letter porcelain code, e.g. ` M`, `??`, `A `.
    pub code: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CommitInfo {
    pub id: String,
    pub short_id: String,
    pub summary: String,
    pub author: String,
    /// Unix timestamp (seconds).
    pub time: i64,
}

/// A git repository (identified by its work tree root).
#[derive(Clone, Debug)]
pub struct Repo {
    root: PathBuf,
}

impl Repo {
    /// Open the repository containing `path`.
    pub fn discover(path: &Path) -> Result<Self> {
        let out = git_in(path, ["rev-parse", "--show-toplevel"])
            .map_err(|_| GitError::NotARepository(path.to_path_buf()))?;
        Ok(Self {
            root: PathBuf::from(out.trim()),
        })
    }

    /// `git init` a new repository at `path` (creating it if needed).
    pub fn init(path: &Path) -> Result<Self> {
        std::fs::create_dir_all(path)?;
        git_in(path, ["init", "--quiet"])?;
        Self::discover(path)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn git<I, S>(&self, args: I) -> Result<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        git_in(&self.root, args)
    }

    fn git_env<I, S>(&self, args: I, env: &[(&str, &OsStr)]) -> Result<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut cmd = Command::new("git");
        cmd.current_dir(&self.root).args(args);
        for (k, v) in env {
            cmd.env(k, v);
        }
        if !self.has_identity() {
            cmd.env("GIT_AUTHOR_NAME", "OpenSuperCAD")
                .env("GIT_AUTHOR_EMAIL", "opensupercad@localhost")
                .env("GIT_COMMITTER_NAME", "OpenSuperCAD")
                .env("GIT_COMMITTER_EMAIL", "opensupercad@localhost");
        }
        run(cmd)
    }

    fn has_identity(&self) -> bool {
        self.git(["config", "user.email"])
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
    }

    /// Name of the checked-out branch (`None` when detached).
    pub fn current_branch(&self) -> Result<Option<String>> {
        let out = self.git(["symbolic-ref", "--quiet", "--short", "HEAD"]);
        Ok(out
            .ok()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty()))
    }

    pub fn head(&self) -> Option<String> {
        self.git(["rev-parse", "--verify", "--quiet", "HEAD"])
            .ok()
            .map(|s| s.trim().to_owned())
    }

    pub fn branches(&self) -> Result<Vec<String>> {
        Ok(self
            .git(["for-each-ref", "--format=%(refname:short)", "refs/heads"])?
            .lines()
            .filter(|b| !b.starts_with(CHECKPOINT_PREFIX))
            .map(str::to_owned)
            .collect())
    }

    pub fn status(&self) -> Result<Vec<FileStatus>> {
        let out = self.git(["status", "--porcelain=v1", "-z", "--untracked-files=all"])?;
        let mut entries = Vec::new();
        let mut parts = out.split('\0').filter(|s| !s.is_empty());
        while let Some(entry) = parts.next() {
            if entry.len() < 4 {
                continue;
            }
            let code = entry[..2].to_owned();
            // Renames/copies are followed by the original path.
            if code.starts_with('R') || code.starts_with('C') {
                parts.next();
            }
            entries.push(FileStatus {
                path: entry[3..].to_owned(),
                code,
            });
        }
        Ok(entries)
    }

    /// Stage everything and commit. Returns the new commit id, or `None` if
    /// there was nothing to commit.
    pub fn commit_all(&self, message: &str) -> Result<Option<String>> {
        self.git(["add", "--all"])?;
        if self.git(["diff", "--cached", "--quiet"]).is_ok() && self.head().is_some() {
            return Ok(None);
        }
        self.git_env(["commit", "--quiet", "--allow-empty", "-m", message], &[])?;
        Ok(self.head())
    }

    pub fn log(&self, rev: &str, limit: usize) -> Result<Vec<CommitInfo>> {
        let out = self.git([
            "log",
            "-z",
            &format!("--max-count={limit}"),
            "--format=%H%x1f%h%x1f%s%x1f%an%x1f%ct",
            rev,
            "--",
        ]);
        let Ok(out) = out else {
            return Ok(Vec::new()); // no commits yet / unknown rev
        };
        Ok(out
            .split('\0')
            .filter(|s| !s.trim().is_empty())
            .filter_map(|rec| {
                let f: Vec<_> = rec.trim_start_matches('\n').split('\x1f').collect();
                Some(CommitInfo {
                    id: f.first()?.to_string(),
                    short_id: f.get(1)?.to_string(),
                    summary: f.get(2)?.to_string(),
                    author: f.get(3)?.to_string(),
                    time: f.get(4)?.trim().parse().unwrap_or(0),
                })
            })
            .collect())
    }

    /// Unified diff of the working tree against HEAD (or of one path).
    pub fn diff(&self, path: Option<&str>) -> Result<String> {
        let mut args = vec!["diff", "HEAD", "--"];
        if let Some(p) = path {
            args.push(p);
        }
        match self.git(&args) {
            Ok(d) => Ok(d),
            // No HEAD yet: everything is new.
            Err(_) => self
                .git(["diff", "--no-index", "/dev/null", path.unwrap_or(".")])
                .or(Ok(String::new())),
        }
    }

    // ---------------------------------------------------------------------
    // Checkpoints
    // ---------------------------------------------------------------------

    /// The checkpoint branch for the current branch (or `detached`).
    pub fn checkpoint_branch(&self) -> Result<String> {
        let branch = self.current_branch()?.unwrap_or_else(|| "detached".into());
        Ok(format!("{CHECKPOINT_PREFIX}{branch}"))
    }

    fn temp_index(&self) -> Result<TempIndex> {
        let git_dir = PathBuf::from(self.git(["rev-parse", "--absolute-git-dir"])?.trim());
        let path = git_dir.join(format!("osc-index-{}", std::process::id()));
        Ok(TempIndex(path))
    }

    /// Tree id of the working tree (respecting `.gitignore`), computed through
    /// a temporary index so the real index is untouched.
    fn snapshot_tree(&self) -> Result<String> {
        let index = self.temp_index()?;
        let env = [("GIT_INDEX_FILE", index.0.as_os_str())];
        self.git_env(["add", "--all", "--", "."], &env)?;
        Ok(self.git_env(["write-tree"], &env)?.trim().to_owned())
    }

    /// Record the current working tree as a checkpoint. Returns `None` when
    /// nothing changed since the last checkpoint.
    pub fn checkpoint(&self, message: &str) -> Result<Option<CommitInfo>> {
        let branch = self.checkpoint_branch()?;
        let refname = format!("refs/heads/{branch}");
        let parent = self
            .git(["rev-parse", "--verify", "--quiet", &refname])
            .ok()
            .map(|s| s.trim().to_owned());
        let tree = self.snapshot_tree()?;
        if let Some(parent) = &parent {
            let parent_tree = self.git(["rev-parse", &format!("{parent}^{{tree}}")])?;
            if parent_tree.trim() == tree {
                return Ok(None);
            }
        }
        let mut msg = message.trim().to_owned();
        if let Some(base) = self.head() {
            msg.push_str(&format!("\n\nBase: {base}"));
        }
        let mut args = vec!["commit-tree".to_owned(), tree, "-m".into(), msg];
        if let Some(p) = parent {
            args.push("-p".into());
            args.push(p);
        }
        let id = self.git_env(&args, &[])?.trim().to_owned();
        self.git(["update-ref", &refname, &id])?;
        Ok(self.log(&id, 1)?.into_iter().next())
    }

    /// Checkpoints of the current branch, newest first.
    pub fn checkpoints(&self, limit: usize) -> Result<Vec<CommitInfo>> {
        let branch = self.checkpoint_branch()?;
        self.log(&format!("refs/heads/{branch}"), limit)
    }

    /// Restore the working tree to a checkpoint (or any commit-ish). The
    /// current state is checkpointed first, so a restore can itself be undone.
    pub fn restore(&self, rev: &str) -> Result<()> {
        let target = self
            .git([
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{rev}^{{commit}}"),
            ])
            .map_err(|_| GitError::UnknownCheckpoint(rev.to_owned()))?
            .trim()
            .to_owned();
        self.checkpoint(&format!(
            "Before restoring {}",
            &target[..target.len().min(12)]
        ))?;

        let current_tree = self.snapshot_tree()?;
        let current = ls_tree(self, &current_tree)?;
        let wanted = ls_tree(self, &target)?;
        for path in current.iter().filter(|p| !wanted.contains(p)) {
            let full = self.root.join(path);
            if full.is_file() {
                std::fs::remove_file(&full)?;
            }
        }
        let index = self.temp_index()?;
        let env = [("GIT_INDEX_FILE", index.0.as_os_str())];
        self.git_env(["read-tree", &target], &env)?;
        self.git_env(["checkout-index", "--all", "--force"], &env)?;
        Ok(())
    }

    /// Commit the current working tree to the real branch, e.g. once an AI
    /// iteration is accepted.
    pub fn promote(&self, message: &str) -> Result<Option<String>> {
        self.commit_all(message)
    }
}

fn ls_tree(repo: &Repo, rev: &str) -> Result<Vec<String>> {
    Ok(repo
        .git(["ls-tree", "-r", "-z", "--name-only", rev])?
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect())
}

struct TempIndex(PathBuf);

impl Drop for TempIndex {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn git_in<I, S>(dir: &Path, args: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut cmd = Command::new("git");
    cmd.current_dir(dir).args(args);
    run(cmd)
}

fn run(mut cmd: Command) -> Result<String> {
    // Never prompt (credentials, editors) from inside the app.
    cmd.env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_EDITOR", "true");
    let out = cmd.output()?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(GitError::Command {
            args: cmd
                .get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(" "),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> (tempfile::TempDir, Repo) {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::init(dir.path()).unwrap();
        repo.git(["config", "user.email", "t@example.com"]).unwrap();
        repo.git(["config", "user.name", "Test"]).unwrap();
        repo.git(["config", "commit.gpgsign", "false"]).unwrap();
        (dir, repo)
    }

    fn write(repo: &Repo, path: &str, text: &str) {
        std::fs::write(repo.root().join(path), text).unwrap();
    }

    fn read(repo: &Repo, path: &str) -> Option<String> {
        std::fs::read_to_string(repo.root().join(path)).ok()
    }

    #[test]
    fn status_commit_log() {
        let (_d, repo) = repo();
        write(&repo, "a.scad", "cube(1);");
        let st = repo.status().unwrap();
        assert_eq!(
            st,
            [FileStatus {
                path: "a.scad".into(),
                code: "??".into()
            }]
        );
        let id = repo.commit_all("first").unwrap().unwrap();
        assert!(repo.status().unwrap().is_empty());
        assert_eq!(repo.commit_all("noop").unwrap(), None);
        let log = repo.log("HEAD", 10).unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].id, id);
        assert_eq!(log[0].summary, "first");
        assert!(!repo.branches().unwrap().is_empty());
    }

    #[test]
    fn checkpoints_do_not_touch_index_and_restore() {
        let (_d, repo) = repo();
        write(&repo, ".gitignore", "*.stl\n");
        write(&repo, "a.scad", "cube(1);");
        repo.commit_all("base").unwrap();

        write(&repo, "a.scad", "cube(2);");
        write(&repo, "b.scad", "sphere(1);");
        write(&repo, "out.stl", "ignored");
        let c1 = repo.checkpoint("iteration 1").unwrap().unwrap();
        // Nothing changed since: no new checkpoint.
        assert!(repo.checkpoint("again").unwrap().is_none());
        // The real index is untouched: b.scad is still untracked.
        assert!(
            repo.status()
                .unwrap()
                .iter()
                .any(|s| s.path == "b.scad" && s.code == "??")
        );

        write(&repo, "a.scad", "cube(3);");
        std::fs::remove_file(repo.root().join("b.scad")).unwrap();
        write(&repo, "c.scad", "cylinder(1);");
        repo.checkpoint("iteration 2").unwrap().unwrap();
        assert_eq!(repo.checkpoints(10).unwrap().len(), 2);
        // Unsaved-to-checkpoint work that a restore must not lose.
        write(&repo, "c.scad", "cylinder(2);");

        repo.restore(&c1.id).unwrap();
        assert_eq!(read(&repo, "a.scad").as_deref(), Some("cube(2);"));
        assert_eq!(read(&repo, "b.scad").as_deref(), Some("sphere(1);"));
        assert_eq!(read(&repo, "c.scad"), None);
        assert_eq!(read(&repo, "out.stl").as_deref(), Some("ignored"));
        // The pre-restore state was checkpointed, so the restore is undoable.
        let cps = repo.checkpoints(10).unwrap();
        assert_eq!(cps.len(), 3);
        assert!(cps[0].summary.starts_with("Before restoring"));
        repo.restore(&cps[0].id).unwrap();
        assert_eq!(read(&repo, "c.scad").as_deref(), Some("cylinder(2);"));

        assert!(
            repo.branches()
                .unwrap()
                .iter()
                .all(|b| !b.starts_with(CHECKPOINT_PREFIX))
        );
        assert!(matches!(
            repo.restore("nope"),
            Err(GitError::UnknownCheckpoint(_))
        ));
    }

    #[test]
    fn checkpoints_work_before_first_commit() {
        let (_d, repo) = repo();
        write(&repo, "a.scad", "cube(1);");
        let c = repo.checkpoint("first").unwrap().unwrap();
        assert_eq!(c.summary, "first");
        assert!(repo.head().is_none());
    }
}
