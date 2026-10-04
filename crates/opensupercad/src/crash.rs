//! Crash reports and bug-report links.
//!
//! A panic hook writes a plain-text report to `<data dir>/crashes/`. On the
//! next start the app offers to report it: the user reviews a prefilled
//! GitHub issue in the browser and decides whether to submit. Nothing is
//! sent anywhere automatically, and the report never includes project
//! contents. Paths under the home directory are shortened to `~`.

use std::backtrace::Backtrace;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const REPO_URL: &str = "https://github.com/1ARdotNO/OpenSuperCAD";

/// Prefilled issue bodies are kept below this many bytes, so the URL stays
/// well inside what browsers and GitHub accept.
const MAX_FIELD: usize = 4000;

/// Where crash reports live.
pub fn crash_dir() -> PathBuf {
    osc_project::Store::default_location().dir().join("crashes")
}

/// Install the panic hook. `component` names the process (`app` or `mcp`).
/// The default hook still runs afterwards, so stderr output is unchanged.
pub fn install(component: &'static str) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "(non-string panic payload)".into());
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_default();
        let thread = std::thread::current()
            .name()
            .unwrap_or("<unnamed>")
            .to_owned();
        let report = Report {
            component,
            message,
            location,
            thread,
            backtrace: Backtrace::force_capture().to_string(),
        };
        if let Ok(path) = report.write_to(&crash_dir()) {
            eprintln!(
                "OpenSuperCAD crashed. A report was saved to {}",
                path.display()
            );
        }
        previous(info);
    }));
}

/// The facts about one crash.
pub struct Report {
    pub component: &'static str,
    pub message: String,
    pub location: String,
    pub thread: String,
    pub backtrace: String,
}

impl Report {
    /// Render the report as text, with the home directory redacted.
    pub fn render(&self, home: Option<&Path>) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "OpenSuperCAD crash report");
        let _ = writeln!(out, "component: {}", self.component);
        let _ = writeln!(out, "{}", environment());
        let _ = writeln!(out, "thread: {}", self.thread);
        let _ = writeln!(out, "location: {}", self.location);
        let _ = writeln!(out, "message: {}", self.message);
        let _ = writeln!(out, "\nbacktrace:\n{}", self.backtrace);
        redact(&out, home)
    }

    pub fn write_to(&self, dir: &Path) -> std::io::Result<PathBuf> {
        std::fs::create_dir_all(dir)?;
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or_default();
        let path = dir.join(format!("crash-{stamp}-{}.txt", self.component));
        std::fs::write(&path, self.render(dirs::home_dir().as_deref()))?;
        Ok(path)
    }
}

/// Version, OS and architecture, one line each.
pub fn environment() -> String {
    format!(
        "version: {}\nos: {} ({})\narch: {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::FAMILY,
        std::env::consts::ARCH
    )
}

/// Replace the home directory with `~` so reports don't leak user names.
pub fn redact(text: &str, home: Option<&Path>) -> String {
    match home.and_then(Path::to_str) {
        Some(h) if h.len() > 1 => text.replace(h, "~"),
        _ => text.to_owned(),
    }
}

/// The newest crash report the user hasn't been asked about yet.
pub fn pending(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension().is_some_and(|e| e == "txt")
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("crash-"))
        })
        .max()
}

/// Mark every report as handled by renaming `*.txt` to `*.log`. The files
/// stay on disk, so they can still be attached to an issue later.
pub fn mark_seen(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for path in entries.filter_map(|e| e.ok().map(|e| e.path())) {
        if path.extension().is_some_and(|e| e == "txt") {
            let _ = std::fs::rename(&path, path.with_extension("log"));
        }
    }
}

/// A prefilled "new bug report" URL for a crash file's contents.
pub fn crash_issue_url(report: &str) -> String {
    let message = report
        .lines()
        .find_map(|l| l.strip_prefix("message: "))
        .unwrap_or("crash");
    let title = format!("Crash: {}", truncate(message, 80));
    let what = format!(
        "OpenSuperCAD crashed.\n\n<!-- Describe what you were doing. -->\n\n```text\n{}\n```",
        truncate(report, MAX_FIELD)
    );
    issue_url(
        "bug_report.yml",
        &[
            ("title", &title),
            ("what", &what),
            ("version", &environment().replace('\n', ", ")),
        ],
    )
}

/// A prefilled bug report for the current environment.
pub fn bug_report_url() -> String {
    issue_url(
        "bug_report.yml",
        &[("version", &environment().replace('\n', ", "))],
    )
}

pub fn feature_request_url() -> String {
    issue_url("feature_request.yml", &[])
}

pub fn issues_url() -> String {
    format!("{REPO_URL}/issues")
}

fn issue_url(template: &str, fields: &[(&str, &str)]) -> String {
    let mut url = format!("{REPO_URL}/issues/new?template={}", encode(template));
    for (key, value) in fields {
        let _ = write!(url, "&{}={}", encode(key), encode(value));
    }
    url
}

/// Cut `s` to at most `max` bytes on a char boundary, marking the cut.
fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n… (truncated; please attach the full report file)",
        &s[..end]
    )
}

/// Percent-encode for a URL query value.
fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => {
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{Report, crash_issue_url, encode, mark_seen, pending, redact, truncate};
    use std::path::Path;

    fn report() -> Report {
        Report {
            component: "app",
            message: "index out of bounds".into(),
            location: "/home/alice/src/x.rs:1:2".into(),
            thread: "main".into(),
            backtrace: "0: foo\n1: bar".into(),
        }
    }

    #[test]
    fn redacts_home() {
        let text = report().render(Some(Path::new("/home/alice")));
        assert!(!text.contains("alice"), "{text}");
        assert!(text.contains("~/src/x.rs:1:2"));
        assert!(text.contains("message: index out of bounds"));
        assert_eq!(redact("/x", Some(Path::new("/"))), "/x");
    }

    #[test]
    fn issue_url_is_prefilled_and_encoded() {
        let url = crash_issue_url(&report().render(None));
        assert!(url.starts_with(
            "https://github.com/1ARdotNO/OpenSuperCAD/issues/new?template=bug_report.yml&title=Crash%3A%20index%20out%20of%20bounds"
        ));
        assert!(url.contains("&what="));
        assert!(url.contains("&version=version%3A%20"));
        assert!(!url.contains(' ') && !url.contains('\n'));
        assert_eq!(encode("a b&c=é"), "a%20b%26c%3D%C3%A9");
    }

    #[test]
    fn truncates_on_char_boundary() {
        let s = "é".repeat(10);
        let t = truncate(&s, 5);
        assert!(t.starts_with("éé\n"));
        assert_eq!(truncate("short", 10), "short");
    }

    #[test]
    fn pending_until_seen() {
        let dir = tempfile::tempdir().unwrap();
        assert!(pending(dir.path()).is_none());
        let first = report().write_to(dir.path()).unwrap();
        std::fs::write(dir.path().join("notes.md"), "").unwrap();
        assert_eq!(pending(dir.path()), Some(first.clone()));
        mark_seen(dir.path());
        assert!(pending(dir.path()).is_none());
        assert!(first.with_extension("log").exists());
    }
}
