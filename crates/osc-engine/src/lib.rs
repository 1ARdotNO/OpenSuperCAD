//! The geometry backend of OpenSuperCAD.
//!
//! OpenSuperCAD drives the upstream `openscad` binary for evaluation and
//! export, so every language feature and option OpenSCAD supports keeps
//! working. On top of that this crate provides:
//!
//! * [`Camera`] presets matching OpenSCAD's view menu, used to take
//!   snapshots from several angles (the agent's "eyes"),
//! * structured [`Diagnostic`]s parsed from OpenSCAD's console output,
//! * an STL [`mesh`] loader and a small software [`raster`]iser, so the UI can
//!   orbit a rendered model interactively without re-invoking OpenSCAD.

mod camera;
mod diagnostics;
mod engine;
pub mod managed;
pub mod mesh;
pub mod raster;

pub use camera::{Camera, View};
pub use diagnostics::{Diagnostic, Severity, parse_console};
pub use engine::{Engine, EngineError, ExportFormat, RenderMode, RenderOutput, Request};
