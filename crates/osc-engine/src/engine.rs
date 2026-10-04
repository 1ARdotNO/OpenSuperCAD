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
            };
            &effective
        };
        let mut cmd = self.command(wrapper);
        cmd.args(self.args(req, out, extra));
        if let Some(dir) = req.file.parent().filter(|d| !d.as_os_str().is_empty()) {
            cmd.current_dir(dir);
        }
        let output = cmd.output().map_err(|source| EngineError::Spawn {
            binary: self.binary.clone(),
            source,
        })?;
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let mut console = stderr.clone();
        console.push_str(&String::from_utf8_lossy(&output.stdout));
        let mut diagnostics = parse_console(&console);
        let has_errors = diagnostics.iter().any(|d| d.severity == Severity::Error);
        let success = output.status.success() && !has_errors && out.exists();
        if !success && !has_errors {
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
                    None => format!("OpenSCAD did not produce {name} ({})", output.status),
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
        })
    }
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
            count += 1;
        }
        assert!(count >= 5, "found {count} examples");
    }
}
