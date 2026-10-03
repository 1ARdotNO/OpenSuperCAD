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
    /// Feature edges of `mesh`, precomputed for outlining.
    pub edges: Option<Vec<[osc_engine::mesh::Vec3; 2]>>,
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
            edges: None,
            image: None,
        }
    }
}

/// Export an STL and load it.
pub fn render_mesh(engine: &Engine, file: &Path, scratch: &Path) -> JobResult {
    // Newer OpenSCAD keeps color() in 3MF; older releases get plain STL.
    let ext = if engine.exports_colors() {
        "3mf"
    } else {
        "stl"
    };
    let out = scratch.join(format!("render-{}.{ext}", unique()));
    let result = match engine.export(&Request::new(file), &out) {
        Ok(r) => r,
        Err(e) => return JobResult::failed(e.to_string()),
    };
    let mesh = if result.success {
        if ext == "3mf" {
            Mesh::load_3mf(&out).ok()
        } else {
            Mesh::load_stl(&out).ok()
        }
    } else {
        None
    };
    let _ = std::fs::remove_file(&out);
    let edges = mesh.as_ref().map(|m| raster::feature_edges(m, 30.0));
    JobResult {
        success: result.success,
        diagnostics: result.diagnostics,
        duration_ms: result.duration_ms,
        mesh,
        edges,
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
        edges: None,
        image,
    }
}

/// Rasterise a mesh for the viewport and return PNG bytes.
pub fn rasterize(mesh: &Mesh, overlays: &raster::Overlays, opts: &RasterOptions) -> Vec<u8> {
    raster::render_with(mesh, overlays, opts).to_png()
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
        let png = rasterize(
            &mesh,
            &raster::Overlays::default(),
            &RasterOptions::default(),
        );
        assert_eq!(&png[1..4], b"PNG");

        std::fs::write(&file, "cube(;").unwrap();
        let r = render_mesh(&engine, &file, dir.path());
        assert!(!r.success);
        assert!(r.mesh.is_none());
    }
}
