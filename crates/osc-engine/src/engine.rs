use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use osc_syntax::customizer::Value;
use serde::Serialize;

use crate::{Camera, Diagnostic, Severity, parse_console};

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error(
        "could not find the `openscad` binary; install OpenSCAD or set OPENSUPERCAD_OPENSCAD to its path"
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
}

impl Request {
    pub fn new(file: impl Into<PathBuf>) -> Self {
        Self {
            file: file.into(),
            defines: Vec::new(),
        }
    }

    pub fn define(mut self, name: impl Into<String>, value: Value) -> Self {
        self.defines.push((name.into(), value));
        self
    }
}

/// The result of one `openscad` invocation.
#[derive(Clone, Debug, Serialize)]
pub struct RenderOutput {
    pub success: bool,
    pub diagnostics: Vec<Diagnostic>,
    pub console: String,
    pub duration_ms: u128,
    pub output: Option<PathBuf>,
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
}

impl Engine {
    pub fn new(binary: impl Into<PathBuf>) -> Self {
        Self {
            binary: binary.into(),
            png_wrapper: Vec::new(),
            backend: None,
            colorscheme: None,
        }
    }

    /// Locate OpenSCAD: `$OPENSUPERCAD_OPENSCAD`, then `PATH`, then the usual
    /// macOS application bundle locations.
    pub fn discover() -> Result<Self, EngineError> {
        let binary = std::env::var_os("OPENSUPERCAD_OPENSCAD")
            .map(PathBuf::from)
            .filter(|p| p.is_file())
            .or_else(|| find_in_path("openscad"))
            .or_else(|| find_in_path("openscad-nightly"))
            .or_else(|| {
                [
                    "/Applications/OpenSCAD.app/Contents/MacOS/OpenSCAD",
                    "/Applications/OpenSCAD-2021.01.app/Contents/MacOS/OpenSCAD",
                    "/opt/homebrew/bin/openscad",
                ]
                .into_iter()
                .map(PathBuf::from)
                .find(|p| p.is_file())
            })
            .ok_or(EngineError::NotFound)?;
        let mut engine = Engine::new(binary);
        engine.png_wrapper = default_png_wrapper();
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
        if let Some(b) = &self.backend {
            extra.push(format!("--backend={b}"));
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
            if let Some(b) = &self.backend {
                extra.push(format!("--backend={b}"));
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
        std::thread::scope(|scope| {
            let handles: Vec<_> = cameras
                .iter()
                .enumerate()
                .map(|(i, cam)| {
                    let out = dir.join(format!("{stem}-{i:02}-{}.png", cam.label()));
                    scope.spawn(move || (cam.clone(), self.snapshot(req, &out, cam, size, mode)))
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().expect("snapshot thread panicked"))
                .collect()
        })
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
        let mut cmd = self.command(wrapper);
        cmd.args(self.args(req, out, extra));
        if let Some(dir) = req.file.parent().filter(|d| !d.as_os_str().is_empty()) {
            cmd.current_dir(dir);
        }
        let output = cmd.output().map_err(|source| EngineError::Spawn {
            binary: self.binary.clone(),
            source,
        })?;
        let mut console = String::from_utf8_lossy(&output.stderr).into_owned();
        console.push_str(&String::from_utf8_lossy(&output.stdout));
        let diagnostics = parse_console(&console);
        let has_errors = diagnostics.iter().any(|d| d.severity == Severity::Error);
        let success = output.status.success() && !has_errors && out.exists();
        Ok(RenderOutput {
            success,
            diagnostics,
            console,
            duration_ms: start.elapsed().as_millis(),
            output: success.then(|| out.to_path_buf()),
        })
    }
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
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

        std::fs::write(&file, "cube(;\n").unwrap();
        let out = engine.check(&Request::new(&file), dir.path()).unwrap();
        assert!(!out.success);
        assert_eq!(out.errors().next().and_then(|d| d.line), Some(1));
    }
}
