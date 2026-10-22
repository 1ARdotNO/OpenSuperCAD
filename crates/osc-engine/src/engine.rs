use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use osc_syntax::customizer::Value;
use serde::Serialize;

use crate::{BoundingBox, Camera, Diagnostic, Severity, parse_console};

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error(
        "could not find OpenSCAD; download it from OpenSuperCAD's Settings or with `opensupercad openscad install`, or set OPENSUPERCAD_OPENSCAD to its path"
    )]
    NotFound,
    #[error("failed to run {binary}: {source}")]
    Spawn {
        binary: PathBuf,
        source: std::io::Error,
    },
    #[error("unsupported export format `{0}`")]
    UnsupportedFormat(String),
}

/// File formats OpenSCAD can export.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    Stl,
    ThreeMf,
    Off,
    Amf,
    Obj,
    Wrl,
    Dxf,
    Svg,
    Pdf,
    Png,
    Csg,
    Echo,
}

impl ExportFormat {
    pub const ALL: [ExportFormat; 12] = [
        ExportFormat::Stl,
        ExportFormat::ThreeMf,
        ExportFormat::Off,
        ExportFormat::Amf,
        ExportFormat::Obj,
        ExportFormat::Wrl,
        ExportFormat::Dxf,
        ExportFormat::Svg,
        ExportFormat::Pdf,
        ExportFormat::Png,
        ExportFormat::Csg,
        ExportFormat::Echo,
    ];

    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Stl => "stl",
            ExportFormat::ThreeMf => "3mf",
            ExportFormat::Off => "off",
            ExportFormat::Amf => "amf",
            ExportFormat::Obj => "obj",
            ExportFormat::Wrl => "wrl",
            ExportFormat::Dxf => "dxf",
            ExportFormat::Svg => "svg",
            ExportFormat::Pdf => "pdf",
            ExportFormat::Png => "png",
            ExportFormat::Csg => "csg",
            ExportFormat::Echo => "echo",
        }
    }

    pub fn from_extension(ext: &str) -> Result<Self, EngineError> {
        let ext = ext.trim_start_matches('.').to_ascii_lowercase();
        ExportFormat::ALL
            .into_iter()
            .find(|f| f.extension() == ext)
            .ok_or(EngineError::UnsupportedFormat(ext))
    }

    /// Whether the format is a 2D format (requires a 2D top-level object).
    pub fn is_2d(self) -> bool {
        matches!(
            self,
            ExportFormat::Dxf | ExportFormat::Svg | ExportFormat::Pdf
        )
    }
}

/// OpenSCAD's two evaluation modes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderMode {
    /// Fast OpenCSG preview (F5).
    #[default]
    Preview,
    /// Full geometry evaluation (F6).
    Render,
}

/// What to evaluate: a file plus customizer overrides (`-D name=value`).
#[derive(Clone, Debug, Default)]
pub struct Request {
    pub file: PathBuf,
    pub defines: Vec<(String, Value)>,
    /// Cancellation, time limit and progress for the OpenSCAD process.
    pub control: RunControl,
}

impl Request {
    pub fn new(file: impl Into<PathBuf>) -> Self {
        Self {
            file: file.into(),
            ..Self::default()
        }
    }

    pub fn define(mut self, name: impl Into<String>, value: Value) -> Self {
        self.defines.push((name.into(), value));
        self
    }

    pub fn with_control(mut self, control: RunControl) -> Self {
        self.control = control;
        self
    }
}

/// Stops a running OpenSCAD from another thread; clones share the flag.
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Called about once a second with the time OpenSCAD has been running.
pub type ProgressFn = Arc<dyn Fn(Duration) + Send + Sync>;

/// How an OpenSCAD run may be interrupted or observed.
#[derive(Clone, Default)]
pub struct RunControl {
    pub cancel: Option<CancelToken>,
    /// Kill OpenSCAD once it has run this long.
    pub timeout: Option<Duration>,
    pub progress: Option<ProgressFn>,
}

impl std::fmt::Debug for RunControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunControl")
            .field("cancel", &self.cancel)
            .field("timeout", &self.timeout)
            .field("progress", &self.progress.is_some())
            .finish()
    }
}

/// Why a run was stopped before OpenSCAD finished.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Interrupt {
    Cancelled,
    TimedOut,
}

/// The result of one `openscad` invocation.
#[derive(Clone, Debug, Serialize)]
pub struct RenderOutput {
    pub success: bool,
    pub diagnostics: Vec<Diagnostic>,
    pub console: String,
    pub duration_ms: u128,
    pub output: Option<PathBuf>,
    /// Set when OpenSCAD was killed (cancelled or over its time limit).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interrupted: Option<Interrupt>,
}

impl RenderOutput {
    pub fn errors(&self) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
    }

    pub fn warnings(&self) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Warning)
    }

    pub fn echoes(&self) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Echo)
    }
}

/// Handle to an installed OpenSCAD.
#[derive(Clone, Debug)]
pub struct Engine {
    pub binary: PathBuf,
    /// Command prefix used for image exports on headless Linux (`xvfb-run -a`).
    pub png_wrapper: Vec<String>,
    /// `--backend=` value, e.g. `manifold` (fast) or `cgal`.
    pub backend: Option<String>,
    /// `--colorscheme=` for snapshots.
    pub colorscheme: Option<String>,
    /// Release year of the OpenSCAD build (e.g. 2021 for 2021.01), if known.
    pub year: Option<u32>,
}

impl Engine {
    pub fn new(binary: impl Into<PathBuf>) -> Self {
        Self {
            binary: binary.into(),
            png_wrapper: Vec::new(),
            backend: None,
            colorscheme: None,
            year: None,
        }
    }

    /// `--backend=…`, only for releases that understand it (2024+); older
    /// OpenSCAD rejects unknown options and would fail every render.
    fn backend_arg(&self) -> Option<String> {
        let b = self.backend.as_ref()?;
        let supported = self.year.is_none_or(|y| y >= 2024);
        supported.then(|| format!("--backend={b}"))
    }

    /// Whether this OpenSCAD exports `color()` into 3MF files (2024 and
    /// later; 2021.01 writes plain geometry).
    pub fn exports_colors(&self) -> bool {
        self.year.is_some_and(|y| y >= 2024)
    }

    /// Locate OpenSCAD: `$OPENSUPERCAD_OPENSCAD`, the user's choice, the
    /// build OpenSuperCAD downloaded (see [`managed`](crate::managed)), then
    /// `PATH` and the usual install locations.
    pub fn discover() -> Result<Self, EngineError> {
        Self::discover_in(&crate::managed::home())
    }

    /// [`discover`](Self::discover) with downloaded builds kept in `home`.
    pub fn discover_in(home: &Path) -> Result<Self, EngineError> {
        let binary = locate(home).ok_or(EngineError::NotFound)?;
        let mut engine = Engine::new(binary);
        engine.png_wrapper = default_png_wrapper();
        engine.year = engine.version().ok().as_deref().and_then(parse_year);
        Ok(engine)
    }

    /// `openscad --version` (OpenSCAD prints it on stderr).
    pub fn version(&self) -> Result<String, EngineError> {
        let out = self
            .command(&[])
            .arg("--version")
            .output()
            .map_err(|source| EngineError::Spawn {
                binary: self.binary.clone(),
                source,
            })?;
        let text = String::from_utf8_lossy(if out.stderr.is_empty() {
            &out.stdout
        } else {
            &out.stderr
        });
        Ok(text.trim().to_owned())
    }

    /// Evaluate the file without building geometry — fast syntax/echo check.
    pub fn check(&self, req: &Request, scratch: &Path) -> Result<RenderOutput, EngineError> {
        // `.csg` evaluates the program without building geometry and, unlike
        // `.echo`, is supported by every OpenSCAD release in the wild.
        let out = scratch.join("check.csg");
        self.run(req, &out, &[], &[])
    }

    /// Export the model to `out`; the format follows the file extension.
    pub fn export(&self, req: &Request, out: &Path) -> Result<RenderOutput, EngineError> {
        let ext = out.extension().and_then(|e| e.to_str()).unwrap_or("");
        ExportFormat::from_extension(ext)?;
        let mut extra = Vec::new();
        if let Some(b) = self.backend_arg() {
            extra.push(b);
        }
        self.run(req, out, &extra, &[])
    }

    /// Render a PNG snapshot from the given camera.
    pub fn snapshot(
        &self,
        req: &Request,
        out: &Path,
        camera: &Camera,
        size: (u32, u32),
        mode: RenderMode,
    ) -> Result<RenderOutput, EngineError> {
        let mut extra = camera.args();
        extra.push(format!("--imgsize={},{}", size.0, size.1));
        extra.push("--projection=perspective".into());
        if mode == RenderMode::Render {
            extra.push("--render".into());
            if let Some(b) = self.backend_arg() {
                extra.push(b);
            }
        }
        if let Some(scheme) = &self.colorscheme {
            extra.push(format!("--colorscheme={scheme}"));
        }
        let wrapper = self.png_wrapper.clone();
        self.run(req, out, &extra, &wrapper)
    }

    /// Render several snapshots in parallel, named `<stem>-<view>.png` in `dir`.
    pub fn snapshots(
        &self,
        req: &Request,
        dir: &Path,
        cameras: &[Camera],
        size: (u32, u32),
        mode: RenderMode,
    ) -> Vec<(Camera, Result<RenderOutput, EngineError>)> {
        let stem = req
            .file
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("model")
            .to_owned();
        let auto = mode == RenderMode::Preview && cameras.iter().any(Camera::is_auto_framed);
        let bbox = auto.then(|| self.final_bbox(req, dir)).flatten();
        std::thread::scope(|scope| {
            let handles: Vec<_> = cameras
                .iter()
                .enumerate()
                .map(|(i, cam)| {
                    let out = dir.join(format!("{stem}-{i:02}-{}.png", cam.label()));
                    let framed = bbox.as_ref().map_or_else(|| cam.clone(), |b| cam.framed(b));
                    scope
                        .spawn(move || (cam.clone(), self.snapshot(req, &out, &framed, size, mode)))
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().expect("snapshot thread panicked"))
                .collect()
        })
    }

    /// Bounding box of the final geometry, for framing preview snapshots;
    /// OpenSCAD's preview `--viewall` also fits operands that are cut away.
    fn final_bbox(&self, req: &Request, dir: &Path) -> Option<BoundingBox> {
        let csg = tempfile::Builder::new()
            .prefix("frame-")
            .suffix(".csg")
            .tempfile_in(dir)
            .ok()?;
        let out = self.export(req, csg.path()).ok()?;
        out.success.then_some(())?;
        crate::bbox::from_csg(&std::fs::read_to_string(csg.path()).ok()?)
    }

    fn command(&self, wrapper: &[String]) -> Command {
        match wrapper.split_first() {
            Some((program, args)) => {
                let mut c = Command::new(program);
                c.args(args).arg(&self.binary);
                c
            }
            None => Command::new(&self.binary),
        }
    }

    /// The full argument list for an invocation (exposed for testing/logging).
    pub fn args(&self, req: &Request, out: &Path, extra: &[String]) -> Vec<OsString> {
        let mut args: Vec<OsString> = vec!["-o".into(), out.into()];
        for (name, value) in &req.defines {
            args.push("-D".into());
            args.push(format!("{name}={}", value.to_scad()).into());
        }
        args.extend(extra.iter().map(OsString::from));
        args.push(req.file.clone().into());
        args
    }

    fn run(
        &self,
        req: &Request,
        out: &Path,
        extra: &[String],
        wrapper: &[String],
    ) -> Result<RenderOutput, EngineError> {
        let start = Instant::now();
        // OpenSCAD ignores `-D` for special variables such as `$t`, so those
        // are set in a wrapper that includes the design.
        let (special, plain): (Vec<_>, Vec<_>) = req
            .defines
            .iter()
            .cloned()
            .partition(|(name, _)| name.starts_with('$'));
        let wrapper_file;
        let effective;
        let req = if special.is_empty() {
            req
        } else {
            let file = req.file.canonicalize().unwrap_or_else(|_| req.file.clone());
            let mut source = String::new();
            for (name, value) in &special {
                source.push_str(&format!("{name} = {};\n", value.to_scad()));
            }
            source.push_str(&format!("include <{}>\n", scad_include_path(&file)));
            wrapper_file = tempfile::Builder::new()
                .prefix("osc-wrapper-")
                .suffix(".scad")
                .tempfile()
                .map_err(|source| EngineError::Spawn {
                    binary: self.binary.clone(),
                    source,
                })?;
            std::fs::write(wrapper_file.path(), source).map_err(|source| EngineError::Spawn {
                binary: self.binary.clone(),
                source,
            })?;
            effective = Request {
                file: wrapper_file.path().to_path_buf(),
                defines: plain,
                control: req.control.clone(),
            };
            &effective
        };
        let mut cmd = self.command(wrapper);
        cmd.args(self.args(req, out, extra));
        if let Some(dir) = req.file.parent().filter(|d| !d.as_os_str().is_empty()) {
            cmd.current_dir(dir);
        }
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // xvfb-run starts OpenSCAD and an X server as children; a process
        // group lets a timeout kill all of them, not just the wrapper.
        #[cfg(unix)]
        if !wrapper.is_empty() {
            std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
        }
        let mut child = cmd.spawn().map_err(|source| EngineError::Spawn {
            binary: self.binary.clone(),
            source,
        })?;
        let stdout = drain(child.stdout.take());
        let stderr = drain(child.stderr.take());
        let (status, interrupted) = supervise(&mut child, &req.control, start, !wrapper.is_empty())
            .map_err(|source| EngineError::Spawn {
                binary: self.binary.clone(),
                source,
            })?;
        let grace = interrupted.map(|_| Duration::from_secs(2));
        let stdout = stdout.finish(grace);
        let stderr = String::from_utf8_lossy(&stderr.finish(grace)).into_owned();
        let mut console = stderr.clone();
        console.push_str(&String::from_utf8_lossy(&stdout));
        let mut diagnostics = parse_console(&console);
        let has_errors = diagnostics.iter().any(|d| d.severity == Severity::Error);
        let success = interrupted.is_none() && status.success() && !has_errors && out.exists();
        if let Some(reason) = interrupted {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                message: match reason {
                    Interrupt::Cancelled => "OpenSCAD was cancelled".to_owned(),
                    Interrupt::TimedOut => format!(
                        "OpenSCAD was stopped after the {} s time limit",
                        req.control.timeout.unwrap_or_default().as_secs()
                    ),
                },
                file: None,
                line: None,
            });
        } else if !success && !has_errors {
            // Never fail silently: say what happened even without output.
            // Some failures (e.g. a 3D object exported as SVG) are printed
            // without an ERROR: prefix, as the last top-level line.
            let name = out
                .file_name()
                .map(|n| n.to_string_lossy())
                .unwrap_or_default();
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                message: match last_message(&stderr) {
                    Some(reason) => format!("OpenSCAD did not produce {name}: {reason}"),
                    None => format!("OpenSCAD did not produce {name} ({status})"),
                },
                file: None,
                line: None,
            });
        }
        Ok(RenderOutput {
            success,
            diagnostics,
            console,
            duration_ms: start.elapsed().as_millis(),
            output: success.then(|| out.to_path_buf()),
            interrupted,
        })
    }
}

/// A child's pipe read to the end on its own thread, so a chatty OpenSCAD
/// can't fill the pipe and block while it is being supervised.
struct Drain {
    buf: Arc<std::sync::Mutex<Vec<u8>>>,
    thread: std::thread::JoinHandle<()>,
}

fn drain(pipe: Option<impl Read + Send + 'static>) -> Drain {
    let buf = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = buf.clone();
    let thread = std::thread::spawn(move || {
        let Some(mut pipe) = pipe else { return };
        let mut chunk = [0u8; 8192];
        while let Ok(n @ 1..) = pipe.read(&mut chunk) {
            if let Ok(mut buf) = sink.lock() {
                buf.extend_from_slice(&chunk[..n]);
            }
        }
    });
    Drain { buf, thread }
}

impl Drain {
    /// Everything read once the pipe closes. After a kill, a grandchild may
    /// still hold the pipe open, so give up waiting after `grace`.
    fn finish(self, grace: Option<Duration>) -> Vec<u8> {
        if let Some(grace) = grace {
            let until = Instant::now() + grace;
            while !self.thread.is_finished() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        if grace.is_none() || self.thread.is_finished() {
            let _ = self.thread.join();
        }
        self.buf.lock().map(|b| b.clone()).unwrap_or_default()
    }
}

/// Wait for OpenSCAD, killing it when cancelled or over its time limit.
fn supervise(
    child: &mut Child,
    control: &RunControl,
    start: Instant,
    group: bool,
) -> std::io::Result<(ExitStatus, Option<Interrupt>)> {
    // Poll fast at first so quick syntax checks don't wait on the poll.
    let mut poll = Duration::from_millis(2);
    let mut next_tick = Duration::from_secs(1);
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok((status, None));
        }
        let elapsed = start.elapsed();
        let reason = if control
            .cancel
            .as_ref()
            .is_some_and(CancelToken::is_cancelled)
        {
            Some(Interrupt::Cancelled)
        } else if control.timeout.is_some_and(|t| elapsed >= t) {
            Some(Interrupt::TimedOut)
        } else {
            None
        };
        if let Some(reason) = reason {
            kill(child, group);
            return Ok((child.wait()?, Some(reason)));
        }
        if elapsed >= next_tick {
            if let Some(progress) = &control.progress {
                progress(elapsed);
            }
            next_tick = elapsed + Duration::from_secs(1);
        }
        std::thread::sleep(poll);
        poll = (poll * 2).min(Duration::from_millis(100));
    }
}

fn kill(child: &mut Child, group: bool) {
    #[cfg(unix)]
    if group {
        let _ = Command::new("kill")
            .args(["-KILL", "--", &format!("-{}", child.id())])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(not(unix))]
    let _ = group;
    let _ = child.kill();
}

/// `OpenSCAD version 2021.01` → 2021.
fn parse_year(version: &str) -> Option<u32> {
    version
        .split(|c: char| !c.is_ascii_digit())
        .find(|part| part.len() == 4)
        .and_then(|y| y.parse().ok())
}

/// Find an executable on `PATH`. On Windows `name.com` comes first (the
/// console build of OpenSCAD, which prints to stdout/stderr), then `.exe`.
fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let names: Vec<String> = if cfg!(windows) {
        vec![
            format!("{name}.com"),
            format!("{name}.exe"),
            name.to_owned(),
        ]
    } else {
        vec![name.to_owned()]
    };
    std::env::split_paths(&path)
        .flat_map(|dir| names.iter().map(move |n| dir.join(n)))
        .find(|p| p.is_file())
}

fn locate(home: &Path) -> Option<PathBuf> {
    std::env::var_os("OPENSUPERCAD_OPENSCAD")
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .or_else(|| crate::managed::selected(home))
        .or_else(|| crate::managed::installed(home).map(|(_, binary)| binary))
        .or_else(|| find_in_path("openscad"))
        .or_else(|| find_in_path("openscad-nightly"))
        .or_else(|| {
            let flatpak_user =
                dirs::data_dir().map(|d| d.join("flatpak/exports/bin/org.openscad.OpenSCAD"));
            [
                // Apps started from the macOS Finder don't get the shell's PATH.
                "/Applications/OpenSCAD.app/Contents/MacOS/OpenSCAD",
                "/Applications/OpenSCAD-2021.01.app/Contents/MacOS/OpenSCAD",
                "/opt/homebrew/bin/openscad",
                "/usr/local/bin/openscad",
                "/snap/bin/openscad",
                "/var/lib/flatpak/exports/bin/org.openscad.OpenSCAD",
            ]
            .into_iter()
            .map(PathBuf::from)
            .chain(flatpak_user)
            .chain(windows_install_dirs())
            .find(|p| p.is_file())
        })
}

/// Where the official installers put OpenSCAD on Windows.
fn windows_install_dirs() -> Vec<PathBuf> {
    [
        "ProgramFiles",
        "ProgramW6432",
        "ProgramFiles(x86)",
        "LOCALAPPDATA",
    ]
    .iter()
    .filter_map(std::env::var_os)
    .flat_map(|base| {
        let base = PathBuf::from(base);
        ["OpenSCAD", "OpenSCAD (Nightly)", "Programs/OpenSCAD"].map(|d| base.join(d))
    })
    .flat_map(|dir| [dir.join("openscad.com"), dir.join("openscad.exe")])
    .collect()
}

/// A path OpenSCAD's `include <…>` accepts: no `\\?\` verbatim prefix (which
/// `canonicalize` adds on Windows) and forward slashes.
fn scad_include_path(path: &Path) -> String {
    let s = path.to_string_lossy();
    if !cfg!(windows) {
        return s.into_owned();
    }
    let s = s.strip_prefix(r"\\?\").unwrap_or(&s);
    s.replace('\\', "/")
}

/// The last unindented, non-empty line of OpenSCAD's stderr. Statistics
/// lines (`   Volumes: 2`) are indented, so this is usually the reason.
fn last_message(stderr: &str) -> Option<&str> {
    stderr
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty() && !l.starts_with(char::is_whitespace))
        .map(str::trim)
}

/// On Linux without a display server, OpenSCAD needs a virtual X server to
/// produce PNGs. `OPENSUPERCAD_PNG_WRAPPER` overrides the detection (an empty
/// value or `none` disables it).
fn default_png_wrapper() -> Vec<String> {
    if let Ok(v) = std::env::var("OPENSUPERCAD_PNG_WRAPPER") {
        if v.trim().is_empty() || v.trim() == "none" {
            return Vec::new();
        }
        return v.split_whitespace().map(str::to_owned).collect();
    }
    let headless = cfg!(target_os = "linux")
        && std::env::var_os("DISPLAY").is_none()
        && std::env::var_os("WAYLAND_DISPLAY").is_none();
    if headless && find_in_path("xvfb-run").is_some() {
        vec!["xvfb-run".into(), "-a".into()]
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_choice_beats_the_downloaded_build() {
        if std::env::var_os("OPENSUPERCAD_OPENSCAD").is_some() {
            return;
        }
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let build = home.join("2026.10.03");
        std::fs::create_dir_all(&build).unwrap();
        std::fs::write(build.join("openscad"), "").unwrap();
        crate::managed::set_current(home, "2026.10.03").unwrap();
        assert_eq!(locate(home), Some(build.join("openscad")));

        let chosen = home.join("chosen-openscad");
        std::fs::write(&chosen, "").unwrap();
        crate::managed::select(home, Some(&chosen)).unwrap();
        assert_eq!(locate(home), Some(chosen));
    }

    #[test]
    fn builds_arguments() {
        let engine = Engine::new("/usr/bin/openscad");
        let req = Request::new("/p/main.scad")
            .define("width", Value::Number(12.5))
            .define("label", Value::String("a b".into()));
        let args = engine.args(&req, Path::new("/tmp/o.stl"), &["--render".into()]);
        let args: Vec<_> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "-o",
                "/tmp/o.stl",
                "-D",
                "width=12.5",
                "-D",
                "label=\"a b\"",
                "--render",
                "/p/main.scad"
            ]
        );
    }

    /// A stand-in for OpenSCAD that runs `script` (unix only).
    #[cfg(unix)]
    fn fake_openscad(dir: &Path, script: &str) -> Engine {
        use std::os::unix::fs::PermissionsExt;
        let bin = dir.join("openscad");
        std::fs::write(&bin, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        Engine::new(bin)
    }

    #[cfg(unix)]
    #[test]
    fn time_limit_kills_openscad() {
        let dir = tempfile::tempdir().unwrap();
        let engine = fake_openscad(dir.path(), "echo 'Rendering...' >&2; sleep 30");
        let req = Request::new(dir.path().join("m.scad")).with_control(RunControl {
            timeout: Some(Duration::from_secs(1)),
            ..RunControl::default()
        });
        let start = Instant::now();
        let out = engine.export(&req, &dir.path().join("m.stl")).unwrap();
        assert!(start.elapsed() < Duration::from_secs(10));
        assert!(!out.success);
        assert_eq!(out.interrupted, Some(Interrupt::TimedOut));
        assert!(out.console.contains("Rendering..."), "{}", out.console);
        assert!(out.errors().any(|d| d.message.contains("time limit")));
    }

    #[cfg(unix)]
    #[test]
    fn cancel_and_progress() {
        let dir = tempfile::tempdir().unwrap();
        let engine = fake_openscad(dir.path(), "sleep 30");
        let cancel = CancelToken::new();
        let ticks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let req = Request::new(dir.path().join("m.scad")).with_control(RunControl {
            cancel: Some(cancel.clone()),
            progress: Some({
                let ticks = ticks.clone();
                Arc::new(move |_| {
                    ticks.fetch_add(1, Ordering::Relaxed);
                })
            }),
            ..RunControl::default()
        });
        let stopper = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(1500));
            cancel.cancel();
        });
        let out = engine.export(&req, &dir.path().join("m.stl")).unwrap();
        stopper.join().unwrap();
        assert_eq!(out.interrupted, Some(Interrupt::Cancelled));
        assert!(ticks.load(Ordering::Relaxed) >= 1);

        // A run that finishes on its own is not interrupted.
        let engine = fake_openscad(dir.path(), "exit 0");
        let req = Request::new(dir.path().join("m.scad")).with_control(RunControl {
            cancel: Some(CancelToken::new()),
            ..RunControl::default()
        });
        let out = engine.export(&req, &dir.path().join("m.stl")).unwrap();
        assert_eq!(out.interrupted, None);
    }

    #[test]
    fn backend_only_for_new_releases() {
        let mut e = Engine::new("openscad");
        e.backend = Some("manifold".into());
        e.year = Some(2021);
        assert_eq!(e.backend_arg(), None);
        e.year = Some(2026);
        assert_eq!(e.backend_arg().as_deref(), Some("--backend=manifold"));
    }

    #[test]
    fn include_paths_for_openscad() {
        if cfg!(windows) {
            assert_eq!(
                scad_include_path(Path::new(r"\\?\C:\Users\a b\main.scad")),
                "C:/Users/a b/main.scad"
            );
        } else {
            assert_eq!(scad_include_path(Path::new("/p/a\\b.scad")), "/p/a\\b.scad");
        }
    }

    #[test]
    fn last_message_skips_statistics() {
        let stderr = "Top level object is a 3D object:\n   Volumes:  2\nCurrent top level object is not a 2D object.\n\n";
        assert_eq!(
            last_message(stderr),
            Some("Current top level object is not a 2D object.")
        );
        assert_eq!(last_message("   Facets: 3\n"), None);
    }

    #[test]
    fn version_year() {
        assert_eq!(parse_year("OpenSCAD version 2021.01"), Some(2021));
        assert_eq!(
            parse_year("OpenSCAD version 2026.10.02 (git 1a2b)"),
            Some(2026)
        );
        assert_eq!(parse_year("garbage"), None);
    }

    #[test]
    fn formats() {
        assert_eq!(
            ExportFormat::from_extension(".3MF").unwrap(),
            ExportFormat::ThreeMf
        );
        assert!(ExportFormat::from_extension("step").is_err());
        assert!(ExportFormat::Svg.is_2d());
    }

    /// Integration test against a real OpenSCAD, skipped when not installed.
    #[test]
    fn real_openscad_roundtrip() {
        let Ok(engine) = Engine::discover() else {
            eprintln!("openscad not installed; skipping");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("t.scad");
        std::fs::write(&file, "size = 10; // [1:20]\necho(\"hi\");\ncube(size);\n").unwrap();
        let req = Request::new(&file).define("size", Value::Number(4.0));

        let out = engine.export(&req, &dir.path().join("t.stl")).unwrap();
        assert!(out.success, "{}", out.console);
        let mesh = crate::mesh::Mesh::load_stl(&dir.path().join("t.stl")).unwrap();
        let (min, max) = mesh.bounds();
        assert!((max[0] - min[0] - 4.0).abs() < 1e-3);
        assert_eq!(out.echoes().count(), 1);

        // Special variables ($t) take effect through the wrapper.
        std::fs::write(&file, "translate([10 * $t, 0, 0]) cube(1);\n").unwrap();
        let req_t = Request::new(&file).define("$t", Value::Number(0.5));
        let out = engine.export(&req_t, &dir.path().join("t5.stl")).unwrap();
        assert!(out.success, "{}", out.console);
        let mesh = crate::mesh::Mesh::load_stl(&dir.path().join("t5.stl")).unwrap();
        assert!(
            (mesh.bounds().0[0] - 5.0).abs() < 1e-3,
            "{:?}",
            mesh.bounds()
        );

        if engine.exports_colors() {
            std::fs::write(
                &file,
                "color(\"red\") cube(2);\ntranslate([5,0,0]) cube(1);\n",
            )
            .unwrap();
            let out = engine
                .export(&Request::new(&file), &dir.path().join("t.3mf"))
                .unwrap();
            assert!(out.success, "{}", out.console);
            let mesh = crate::mesh::Mesh::load_3mf(&dir.path().join("t.3mf")).unwrap();
            assert_eq!(mesh.colors.len(), mesh.triangles.len());
            assert!(mesh.colors.contains(&[0xff, 0, 0, 0xff]));
        }

        std::fs::write(&file, "cube(;\n").unwrap();
        let out = engine.check(&Request::new(&file), dir.path()).unwrap();
        assert!(!out.success);
        assert_eq!(out.errors().next().and_then(|d| d.line), Some(1));
    }

    /// Every bundled example renders cleanly, skipped without OpenSCAD.
    #[test]
    fn examples_render() {
        let Ok(engine) = Engine::discover() else {
            eprintln!("openscad not installed; skipping");
            return;
        };
        let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
        let dir = tempfile::tempdir().unwrap();
        let mut count = 0;
        for entry in std::fs::read_dir(&examples).unwrap() {
            let main = entry.unwrap().path().join("main.scad");
            if !main.exists() {
                continue;
            }
            let out = engine
                .export(&Request::new(&main), &dir.path().join("out.stl"))
                .unwrap();
            assert!(out.success, "{}: {}", main.display(), out.console);
            assert_eq!(
                out.warnings().count(),
                0,
                "{}: {}",
                main.display(),
                out.console
            );
            assert!(
                engine
                    .final_bbox(&Request::new(&main), dir.path())
                    .is_some(),
                "{}: no bounding box from the CSG tree",
                main.display()
            );
            count += 1;
        }
        assert!(count >= 5, "found {count} examples");
    }

    /// Share of pixels that differ from the background (the corner pixel).
    fn coverage(png: &Path) -> f64 {
        let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(png).unwrap()));
        let mut reader = decoder.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        let px = info.color_type.samples();
        let pixels: Vec<&[u8]> = buf[..info.buffer_size()].chunks(px).collect();
        let bg = pixels[0];
        pixels.iter().filter(|p| **p != bg).count() as f64 / pixels.len() as f64
    }

    /// Preview snapshots frame the final geometry, not the helper operands
    /// OpenSCAD's normalised CSG tree keeps. Skipped without OpenSCAD.
    #[test]
    fn preview_frames_the_final_geometry() {
        let Ok(engine) = Engine::discover() else {
            eprintln!("openscad not installed; skipping");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("part.scad");
        let iso = Camera::preset(crate::View::Iso);
        for source in [
            "intersection() {\n  cube([20, 10, 5]);\n  translate([-100, -100, -100]) cube(200);\n}\n",
            "difference() {\n  cube([20, 10, 5]);\n  difference() {\n    translate([-1, -1, 3]) cube(30);\n    translate([-100, -100, -100]) cube(200);\n  }\n}\n",
        ] {
            std::fs::write(&file, source).unwrap();
            let req = Request::new(&file);
            let bbox = engine.final_bbox(&req, dir.path()).expect("bbox");
            assert_eq!((bbox.min, bbox.max), ([0.0; 3], [20.0, 10.0, 5.0]));

            let unframed = dir.path().join("unframed.png");
            let out = engine
                .snapshot(&req, &unframed, &iso, (320, 240), RenderMode::Preview)
                .unwrap();
            assert!(out.success, "{}", out.console);
            let cameras = [iso.clone()];
            let results =
                engine.snapshots(&req, dir.path(), &cameras, (320, 240), RenderMode::Preview);
            let (camera, out) = results.into_iter().next().unwrap();
            let out = out.unwrap();
            assert!(out.success, "{}", out.console);
            assert_eq!(camera.label(), "iso");
            let (before, after) = (coverage(&unframed), coverage(&out.output.unwrap()));
            eprintln!("model covers {before:.4} of the image unframed, {after:.4} framed");
            assert!(after > 0.2 && after >= before * 0.95, "{before} -> {after}");
        }
    }
}
