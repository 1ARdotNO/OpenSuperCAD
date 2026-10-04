//! Where agent commands are looked up.
//!
//! An app started from the desktop menu, the Dock or Explorer does not get
//! the `PATH` the user's shell builds: Node.js from nvm, Volta, fnm or
//! Homebrew is often missing, so `npx` "isn't installed" although it works
//! in a terminal (#69). The search path is therefore this process's `PATH`,
//! then the one the user's login shell reports, then the usual install
//! locations of Node.js and agent CLIs that exist on this machine. Agents
//! are started with the same `PATH`, so `npx` finds `node` as well.

use std::ffi::OsString;
use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How long the login shell may take to report its `PATH`.
const SHELL_TIMEOUT: Duration = Duration::from_secs(5);

/// The directories searched for agent commands, in order: the system's
/// (see the module docs), then the fallbacks added with [`add_fallback`].
/// The first call may run the user's login shell, see [`warm_up`].
pub fn search_path() -> Vec<PathBuf> {
    let mut dirs = system_path().to_vec();
    for dir in fallbacks().lock().map(|f| f.clone()).unwrap_or_default() {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs
}

/// Search `dir` after everything else, e.g. the Node.js OpenSuperCAD
/// downloaded: the user's own installs still win.
pub fn add_fallback(dir: PathBuf) {
    if let Ok(mut f) = fallbacks().lock()
        && !f.contains(&dir)
    {
        f.push(dir);
    }
}

fn fallbacks() -> &'static Mutex<Vec<PathBuf>> {
    static FALLBACKS: OnceLock<Mutex<Vec<PathBuf>>> = OnceLock::new();
    FALLBACKS.get_or_init(Mutex::default)
}

/// The process `PATH`, the login shell's and the usual install locations.
/// Computed once.
fn system_path() -> &'static [PathBuf] {
    static PATH: OnceLock<Vec<PathBuf>> = OnceLock::new();
    PATH.get_or_init(|| {
        let current = std::env::var_os("PATH").unwrap_or_default();
        let shell = login_shell_path().unwrap_or_default();
        merge(
            std::env::split_paths(&current)
                .chain(std::env::split_paths(&shell))
                .chain(well_known_dirs(home().as_deref())),
        )
    })
}

/// [`search_path`] as a `PATH` value for child processes.
pub fn search_path_var() -> OsString {
    std::env::join_paths(search_path())
        .unwrap_or_else(|_| std::env::var_os("PATH").unwrap_or_default())
}

/// Compute [`search_path`] on a background thread, so the UI never waits
/// for the login shell.
pub fn warm_up() {
    let _ = std::thread::Builder::new()
        .name("agent-path".into())
        .spawn(|| {
            system_path();
        });
}

/// First occurrence wins; empty entries and missing directories are dropped.
fn merge(dirs: impl Iterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for dir in dirs {
        if !dir.as_os_str().is_empty() && dir.is_dir() && !out.contains(&dir) {
            out.push(dir);
        }
    }
    out
}

fn home() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// Install locations of Node.js version managers, package managers and
/// agent CLIs. Only the ones that exist end up in the search path.
fn well_known_dirs(home: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if cfg!(windows) {
        for var in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(p) = std::env::var_os(var) {
                dirs.push(PathBuf::from(p).join("nodejs"));
            }
        }
        if let Some(p) = std::env::var_os("APPDATA") {
            dirs.push(PathBuf::from(p).join("npm"));
        }
        if let Some(home) = home {
            dirs.push(home.join(".volta/bin"));
            dirs.push(home.join("scoop/shims"));
        }
        return dirs;
    }
    if let Some(home) = home {
        for rel in [
            ".local/bin",
            ".volta/bin",
            ".bun/bin",
            ".npm-global/bin",
            ".asdf/shims",
            ".local/share/mise/shims",
            ".local/share/fnm/aliases/default/bin",
            ".fnm/aliases/default/bin",
            "n/bin",
        ] {
            dirs.push(home.join(rel));
        }
        dirs.extend(newest_nvm_node(&home.join(".nvm/versions/node")));
    }
    for dir in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"] {
        dirs.push(PathBuf::from(dir));
    }
    dirs
}

/// `~/.nvm/versions/node/<newest>/bin`.
fn newest_nvm_node(versions: &Path) -> Option<PathBuf> {
    let parse = |name: &str| -> Option<Vec<u64>> {
        name.strip_prefix('v')?
            .split('.')
            .map(|n| n.parse().ok())
            .collect()
    };
    std::fs::read_dir(versions)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|e| Some((parse(e.file_name().to_str()?)?, e.path())))
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, p)| p.join("bin"))
}

/// The `PATH` an interactive login shell ends up with, so version managers
/// set up in `.bashrc`/`.zshrc` count. `None` on Windows, when started from
/// a terminal (the `PATH` is already the shell's, and an interactive shell
/// could take over the terminal), without `$SHELL`, or when the shell fails
/// or takes longer than [`SHELL_TIMEOUT`].
fn login_shell_path() -> Option<OsString> {
    if cfg!(windows) || std::io::stdin().is_terminal() {
        return None;
    }
    let shell = std::env::var_os("SHELL").filter(|s| !s.is_empty())?;
    const MARKER: &str = "__OPENSUPERCAD_ENV__";
    // A bash login shell reads only `.profile`/`.bash_profile`, but nvm and
    // friends add themselves to `.bashrc`, which not every `.profile` sources.
    let rc = if Path::new(&shell).file_name().is_some_and(|n| n == "bash") {
        "[ -f ~/.bashrc ] && . ~/.bashrc >/dev/null 2>&1; "
    } else {
        ""
    };
    let mut child = Command::new(shell)
        .args(["-l", "-i", "-c", &format!("{rc}echo {MARKER}; env")])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.read_to_end(&mut out);
        out
    });
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < SHELL_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let out = reader.join().ok()?;
    parse_env_path(&String::from_utf8_lossy(&out), MARKER).map(OsString::from)
}

/// The `PATH=` line printed by `env` after `marker` (shell start-up files
/// may print anything before it).
fn parse_env_path(output: &str, marker: &str) -> Option<String> {
    output
        .split_once(marker)?
        .1
        .lines()
        .find_map(|l| l.strip_prefix("PATH="))
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_output_is_parsed_after_the_marker() {
        let out = "PATH=/noise\nWelcome!\n__M__\nHOME=/home/u\nPATH=/a:/b\nX=1\n";
        assert_eq!(parse_env_path(out, "__M__").as_deref(), Some("/a:/b"));
        assert_eq!(parse_env_path("PATH=/x\n", "__M__"), None);
    }

    #[test]
    fn merge_keeps_order_and_drops_duplicates_and_missing() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let merged = merge(
            [
                b.clone(),
                PathBuf::new(),
                dir.path().join("missing"),
                a.clone(),
                b.clone(),
            ]
            .into_iter(),
        );
        assert_eq!(merged, [b, a]);
    }

    #[test]
    fn picks_the_newest_nvm_node() {
        let dir = tempfile::tempdir().unwrap();
        for v in ["v9.11.2", "v22.3.0", "v18.20.1", "system"] {
            std::fs::create_dir_all(dir.path().join(v).join("bin")).unwrap();
        }
        assert_eq!(
            newest_nvm_node(dir.path()),
            Some(dir.path().join("v22.3.0/bin"))
        );
        assert_eq!(newest_nvm_node(&dir.path().join("none")), None);
    }

    #[cfg(unix)]
    #[test]
    fn well_known_dirs_include_version_managers() {
        let home = Path::new("/home/u");
        let dirs = well_known_dirs(Some(home));
        assert!(dirs.contains(&home.join(".volta/bin")));
        assert!(dirs.contains(&PathBuf::from("/usr/local/bin")));
    }
}
