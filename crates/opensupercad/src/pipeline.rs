//! Background rendering jobs for the preview, independent of the UI.
//!
//! * **Render** (on save and F6): OpenSCAD exports an STL. The mesh is loaded
//!   and the viewport rasterises it locally, so orbit and zoom are instant.
//! * **Preview** (F5): OpenSCAD's own OpenCSG preview rendered to a PNG from
//!   the viewport's current camera. It shows colours and `#`/`%` debug
//!   modifiers exactly as OpenSCAD does.

use std::path::{Path, PathBuf};

use osc_engine::mesh::Mesh;
use osc_engine::raster::{self, RasterOptions};
use osc_engine::{Camera, Diagnostic, Engine, RenderMode, Request};

#[derive(Clone, Debug)]
pub struct JobResult {
    pub success: bool,
    pub diagnostics: Vec<Diagnostic>,
    pub duration_ms: u128,
    pub mesh: Option<Mesh>,
    /// PNG bytes of an OpenSCAD preview image.
    pub image: Option<Vec<u8>>,
}

impl JobResult {
    fn failed(message: String) -> Self {
        Self {
            success: false,
            diagnostics: vec![Diagnostic {
                severity: osc_engine::Severity::Error,
                message,
                file: None,
                line: None,
            }],
            duration_ms: 0,
            mesh: None,
            image: None,
        }
    }
}

/// Export an STL and load it.
pub fn render_mesh(engine: &Engine, file: &Path, scratch: &Path) -> JobResult {
    let out = scratch.join(format!("render-{}.stl", unique()));
    let result = match engine.export(&Request::new(file), &out) {
        Ok(r) => r,
        Err(e) => return JobResult::failed(e.to_string()),
    };
    let mesh = if result.success {
        Mesh::load_stl(&out).ok()
    } else {
        None
    };
    let _ = std::fs::remove_file(&out);
    JobResult {
        success: result.success,
        diagnostics: result.diagnostics,
        duration_ms: result.duration_ms,
        mesh,
        image: None,
    }
}

/// OpenSCAD's own preview of the model from a camera.
pub fn preview_image(
    engine: &Engine,
    file: &Path,
    camera: &Camera,
    size: (u32, u32),
    scratch: &Path,
) -> JobResult {
    let out = scratch.join(format!("preview-{}.png", unique()));
    let result = match engine.snapshot(&Request::new(file), &out, camera, size, RenderMode::Preview)
    {
        Ok(r) => r,
        Err(e) => return JobResult::failed(e.to_string()),
    };
    let image = result.success.then(|| std::fs::read(&out).ok()).flatten();
    let _ = std::fs::remove_file(&out);
    JobResult {
        success: result.success && image.is_some(),
        diagnostics: result.diagnostics,
        duration_ms: result.duration_ms,
        mesh: None,
        image,
    }
}

/// Rasterise a mesh for the viewport and return PNG bytes.
pub fn rasterize(mesh: &Mesh, opts: &RasterOptions) -> Vec<u8> {
    raster::render(mesh, opts).to_png()
}

fn unique() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    )
}

/// A scratch directory for render outputs that lives as long as the app.
pub fn scratch_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("opensupercad-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_mesh_with_real_openscad() {
        let Ok(engine) = Engine::discover() else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("m.scad");
        std::fs::write(&file, "cube([10, 20, 5]);").unwrap();
        let r = render_mesh(&engine, &file, dir.path());
        assert!(r.success);
        let mesh = r.mesh.unwrap();
        assert_eq!(mesh.bounds().1, [10.0, 20.0, 5.0]);
        let png = rasterize(&mesh, &RasterOptions::default());
        assert_eq!(&png[1..4], b"PNG");

        std::fs::write(&file, "cube(;").unwrap();
        let r = render_mesh(&engine, &file, dir.path());
        assert!(!r.success);
        assert!(r.mesh.is_none());
    }
}
