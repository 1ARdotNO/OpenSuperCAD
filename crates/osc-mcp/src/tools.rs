use std::path::{Path, PathBuf};

use base64::Engine as _;
use osc_engine::{Camera, Diagnostic, Engine, RenderMode, Request, Severity, View, mesh::Mesh};
use osc_git::Repo;
use osc_project::Project;
use osc_syntax::customizer::{self, Value as ParamValue};
use serde_json::{Value, json};

use crate::control::{ControlClient, ControlReply, ControlRequest, SnapshotImage};

enum Content {
    Text(String),
    Png(Vec<u8>),
}

type ToolResult = Result<Vec<Content>, String>;

/// The tool implementations, bound to one project.
pub struct Tools {
    project: Project,
    engine: Option<Result<Engine, String>>,
    scratch: tempfile::TempDir,
    control: Option<ControlClient>,
}

struct ToolDef {
    name: &'static str,
    description: &'static str,
    schema: fn() -> Value,
}

const DEFAULT_VIEWS: [View; 4] = [View::Iso, View::Front, View::Top, View::Right];

fn path_prop() -> Value {
    json!({ "type": "string", "description": "Project-relative path to a .scad file. Defaults to the project's main file." })
}

fn defines_prop() -> Value {
    json!({
        "type": "object",
        "description": "Customizer overrides for this call only (like `openscad -D`), e.g. {\"width\": 40}. The file is not modified.",
        "additionalProperties": { "type": ["number", "string", "boolean", "array"] }
    })
}

const TOOLS: &[ToolDef] = &[
    ToolDef {
        name: "project_info",
        description: "Overview of the open project: root, main file, OpenSCAD sources, OpenSCAD version and git/checkpoint state. Call this first.",
        schema: || json!({ "type": "object", "properties": {} }),
    },
    ToolDef {
        name: "list_files",
        description: "List all files in the project (project-relative paths).",
        schema: || json!({ "type": "object", "properties": {} }),
    },
    ToolDef {
        name: "read_file",
        description: "Read a project file as text.",
        schema: || {
            json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            })
        },
    },
    ToolDef {
        name: "write_file",
        description: "Create or overwrite a project file. For .scad files the result includes OpenSCAD's syntax check (errors, warnings, echo output).",
        schema: || {
            json!({
                "type": "object",
                "properties": { "path": { "type": "string" }, "content": { "type": "string" } },
                "required": ["path", "content"]
            })
        },
    },
    ToolDef {
        name: "edit_file",
        description: "Replace an exact string in a project file. `old_string` must match exactly once unless `replace_all` is true. For .scad files the result includes OpenSCAD's syntax check.",
        schema: || {
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "old_string": { "type": "string" },
                    "new_string": { "type": "string" },
                    "replace_all": { "type": "boolean", "default": false }
                },
                "required": ["path", "old_string", "new_string"]
            })
        },
    },
    ToolDef {
        name: "outline",
        description: "List the modules and functions defined in a .scad file with their line numbers.",
        schema: || json!({ "type": "object", "properties": { "path": path_prop() } }),
    },
    ToolDef {
        name: "get_parameters",
        description: "Read the OpenSCAD Customizer parameters of a file: name, value, group, description and widget (slider range, dropdown options, …).",
        schema: || json!({ "type": "object", "properties": { "path": path_prop() } }),
    },
    ToolDef {
        name: "set_parameters",
        description: "Change Customizer parameter values in the source file, keeping comments and annotations intact. Values must keep their type (number, string, boolean, numeric vector).",
        schema: || {
            json!({
                "type": "object",
                "properties": {
                    "path": path_prop(),
                    "values": {
                        "type": "object",
                        "description": "Map of parameter name to new value, e.g. {\"width\": 42, \"style\": \"round\"}.",
                        "additionalProperties": { "type": ["number", "string", "boolean", "array"] }
                    }
                },
                "required": ["values"]
            })
        },
    },
    ToolDef {
        name: "snapshot",
        description: "Render PNG images of the model so you can SEE it. Defaults to the iso, front, top and right views. Named views: iso, front, back, left, right, top, bottom, diagonal (from below). Use `cameras` for arbitrary angles.",
        schema: || {
            json!({
                "type": "object",
                "properties": {
                    "path": path_prop(),
                    "views": {
                        "type": "array",
                        "items": { "type": "string", "enum": ["iso", "front", "back", "left", "right", "top", "bottom", "diagonal"] }
                    },
                    "cameras": {
                        "type": "array",
                        "description": "Custom cameras. {\"type\":\"gimbal\",\"rotation\":[rx,ry,rz],\"distance\":d?} (OpenSCAD $vpr/$vpd) or {\"type\":\"look_at\",\"eye\":[x,y,z],\"center\":[x,y,z]}.",
                        "items": { "type": "object" }
                    },
                    "size": { "type": "array", "items": { "type": "integer" }, "minItems": 2, "maxItems": 2, "description": "[width, height] in pixels, default [640, 480]." },
                    "mode": { "type": "string", "enum": ["preview", "render"], "description": "preview (fast, F5) or render (full geometry, F6). Default preview." },
                    "defines": defines_prop()
                }
            })
        },
    },
    ToolDef {
        name: "render",
        description: "Fully render the model (like F6). Reports errors/warnings/echo output, the bounding box in mm and the triangle count. Use to verify real dimensions and that the geometry is valid.",
        schema: || {
            json!({
                "type": "object",
                "properties": { "path": path_prop(), "defines": defines_prop() }
            })
        },
    },
    ToolDef {
        name: "export",
        description: "Export the model to a file in the project. The format follows the output extension: stl, 3mf, off, amf, obj, wrl, dxf, svg, pdf, png, csg.",
        schema: || {
            json!({
                "type": "object",
                "properties": {
                    "path": path_prop(),
                    "output": { "type": "string", "description": "Project-relative output path, e.g. exports/part.stl" },
                    "defines": defines_prop()
                },
                "required": ["output"]
            })
        },
    },
    ToolDef {
        name: "checkpoint",
        description: "Save the current state of all project files as a checkpoint (on the osc/checkpoints/<branch> git branch; the user's branch and index are untouched).",
        schema: || {
            json!({
                "type": "object",
                "properties": { "message": { "type": "string", "description": "What this iteration changed." } },
                "required": ["message"]
            })
        },
    },
    ToolDef {
        name: "set_view",
        description: "Point the user's 3D viewport in the OpenSuperCAD window at the model from a named view or rotation, e.g. to show them the detail you are talking about. Does not return an image (use `snapshot` to see).",
        schema: || {
            json!({
                "type": "object",
                "properties": {
                    "view": { "type": "string", "enum": ["iso", "front", "back", "left", "right", "top", "bottom", "diagonal"] },
                    "rotation": { "type": "array", "items": { "type": "number" }, "minItems": 3, "maxItems": 3, "description": "OpenSCAD $vpr rotation in degrees" }
                }
            })
        },
    },
    ToolDef {
        name: "list_checkpoints",
        description: "List checkpoints, newest first.",
        schema: || json!({ "type": "object", "properties": { "limit": { "type": "integer", "default": 20 } } }),
    },
    ToolDef {
        name: "restore_checkpoint",
        description: "Restore all project files to a checkpoint. The current state is checkpointed first, so this can be undone.",
        schema: || {
            json!({
                "type": "object",
                "properties": { "id": { "type": "string", "description": "Checkpoint commit id (full or abbreviated)." } },
                "required": ["id"]
            })
        },
    },
];

impl Tools {
    pub fn new(root: PathBuf) -> anyhow::Result<Self> {
        Ok(Self {
            project: Project::open(root)?,
            engine: None,
            scratch: tempfile::Builder::new()
                .prefix("opensupercad-mcp")
                .tempdir()?,
            control: None,
        })
    }

    /// Connect to a running OpenSuperCAD window (see [`crate::control`]).
    pub fn with_control(mut self, socket: Option<PathBuf>) -> Self {
        self.control = socket.map(ControlClient::new);
        self
    }

    fn notify_app(&self, request: &ControlRequest) -> Option<ControlReply> {
        self.control.as_ref()?.send(request).ok()
    }

    pub fn definitions(&self) -> Vec<Value> {
        TOOLS
            .iter()
            .map(|t| json!({ "name": t.name, "description": t.description, "inputSchema": (t.schema)() }))
            .collect()
    }

    pub fn exists(&self, name: &str) -> bool {
        TOOLS.iter().any(|t| t.name == name)
    }

    /// Run a tool and build the MCP `CallToolResult`.
    pub fn call(&mut self, name: &str, args: &Value) -> Value {
        // Let the app flush unsaved edits so tools see what the user sees.
        if !matches!(name, "list_checkpoints" | "set_view") {
            self.notify_app(&ControlRequest::SaveAll);
        }
        let result = match name {
            "project_info" => self.project_info(),
            "list_files" => Ok(vec![Content::Text(self.project.files().join("\n"))]),
            "read_file" => self.read_file(args),
            "write_file" => self.write_file(args),
            "edit_file" => self.edit_file(args),
            "outline" => self.outline(args),
            "get_parameters" => self.get_parameters(args),
            "set_parameters" => self.set_parameters(args),
            "snapshot" => self.snapshot(args),
            "render" => self.render(args),
            "export" => self.export(args),
            "checkpoint" => self.checkpoint(args),
            "list_checkpoints" => self.list_checkpoints(args),
            "restore_checkpoint" => self.restore_checkpoint(args),
            "set_view" => self.set_view(args),
            _ => Err(format!("unknown tool `{name}`")),
        };
        let (content, is_error) = match result {
            Ok(c) => (c, false),
            Err(e) => (vec![Content::Text(e)], true),
        };
        let content: Vec<Value> = content
            .into_iter()
            .map(|c| match c {
                Content::Text(text) => json!({ "type": "text", "text": text }),
                Content::Png(bytes) => json!({
                    "type": "image",
                    "mimeType": "image/png",
                    "data": base64::engine::general_purpose::STANDARD.encode(bytes),
                }),
            })
            .collect();
        json!({ "content": content, "isError": is_error })
    }

    fn engine(&mut self) -> Result<&Engine, String> {
        let engine = self.engine.get_or_insert_with(|| {
            Engine::discover()
                .map(|mut e| {
                    e.backend = self.project.settings().openscad_backend;
                    e
                })
                .map_err(|e| e.to_string())
        });
        engine.as_ref().map_err(Clone::clone)
    }

    fn str_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
        args.get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("missing string argument `{key}`"))
    }

    fn resolve(&self, rel: &str) -> Result<PathBuf, String> {
        self.project.resolve(rel).map_err(|e| e.to_string())
    }

    /// The .scad file a tool operates on: `path` or the project's main file.
    fn scad_file(&self, args: &Value) -> Result<PathBuf, String> {
        let rel = match args.get("path").and_then(Value::as_str) {
            Some(p) => p.to_owned(),
            None => self
                .project
                .main_file()
                .ok_or("the project has no .scad file yet; create one with write_file")?,
        };
        let path = self.resolve(&rel)?;
        if !path.is_file() {
            return Err(format!("no such file: {rel}"));
        }
        Ok(path)
    }

    fn request(&self, args: &Value) -> Result<Request, String> {
        let mut req = Request::new(self.scad_file(args)?);
        if let Some(defines) = args.get("defines").and_then(Value::as_object) {
            for (name, v) in defines {
                let value: ParamValue = serde_json::from_value(v.clone())
                    .map_err(|_| format!("unsupported value for define `{name}`: {v}"))?;
                req.defines.push((name.clone(), value));
            }
        }
        Ok(req)
    }

    fn project_info(&mut self) -> ToolResult {
        let openscad = match self.engine() {
            Ok(e) => e.version().unwrap_or_else(|e| e.to_string()),
            Err(e) => format!("not available: {e}"),
        };
        let git = match Repo::discover(self.project.root()) {
            Ok(repo) => json!({
                "branch": repo.current_branch().ok().flatten(),
                "changed_files": repo.status().map(|s| s.len()).unwrap_or(0),
                "checkpoints": repo.checkpoints(1000).map(|c| c.len()).unwrap_or(0),
            }),
            Err(_) => Value::Null,
        };
        let info = json!({
            "name": self.project.name(),
            "root": self.project.root(),
            "main_file": self.project.main_file(),
            "scad_files": self.project.scad_files(),
            "openscad": openscad,
            "git": git,
        });
        Ok(vec![Content::Text(pretty(&info))])
    }

    fn read_file(&self, args: &Value) -> ToolResult {
        let path = self.resolve(Self::str_arg(args, "path")?)?;
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(vec![Content::Text(text)])
    }

    fn after_write(&mut self, path: &Path, summary: String) -> ToolResult {
        let mut out = vec![Content::Text(summary)];
        if path.extension().is_some_and(|e| e == "scad")
            && let Ok(engine) = self.engine()
        {
            let engine = engine.clone();
            match engine.check(&Request::new(path), self.scratch.path()) {
                Ok(r) => out.push(Content::Text(format_diagnostics(&r.diagnostics, r.success))),
                Err(e) => out.push(Content::Text(format!("syntax check failed to run: {e}"))),
            }
        }
        Ok(out)
    }

    fn write_file(&mut self, args: &Value) -> ToolResult {
        let rel = Self::str_arg(args, "path")?;
        let content = Self::str_arg(args, "content")?;
        let path = self.resolve(rel)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, content).map_err(|e| format!("{rel}: {e}"))?;
        self.after_write(&path, format!("Wrote {rel} ({} bytes).", content.len()))
    }

    fn edit_file(&mut self, args: &Value) -> ToolResult {
        let rel = Self::str_arg(args, "path")?;
        let old = Self::str_arg(args, "old_string")?;
        let new = Self::str_arg(args, "new_string")?;
        let all = args
            .get("replace_all")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if old.is_empty() {
            return Err("old_string must not be empty".into());
        }
        let path = self.resolve(rel)?;
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{rel}: {e}"))?;
        let count = text.matches(old).count();
        let updated = match (count, all) {
            (0, _) => return Err(format!("old_string not found in {rel}")),
            (1, _) | (_, true) => text.replace(old, new),
            (n, false) => {
                return Err(format!(
                    "old_string occurs {n} times in {rel}; add context to make it unique or set replace_all"
                ));
            }
        };
        std::fs::write(&path, updated).map_err(|e| format!("{rel}: {e}"))?;
        self.after_write(&path, format!("Edited {rel} ({count} replacement(s))."))
    }

    fn outline(&self, args: &Value) -> ToolResult {
        let path = self.scad_file(args)?;
        let src = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        Ok(vec![Content::Text(pretty(&osc_syntax::outline(&src)))])
    }

    fn get_parameters(&self, args: &Value) -> ToolResult {
        let path = self.scad_file(args)?;
        let src = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        Ok(vec![Content::Text(pretty(&customizer::parameters(&src)))])
    }

    fn set_parameters(&mut self, args: &Value) -> ToolResult {
        let path = self.scad_file(args)?;
        let values = args
            .get("values")
            .and_then(Value::as_object)
            .ok_or("missing object argument `values`")?;
        let mut src = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        // Apply all changes in memory first so a bad value changes nothing.
        for (name, v) in values {
            let value: ParamValue = serde_json::from_value(v.clone())
                .map_err(|_| format!("unsupported value for `{name}`: {v}"))?;
            src = customizer::set_parameter(&src, name, &value).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, &src).map_err(|e| e.to_string())?;
        let rel = self.project.relative(&path);
        self.after_write(
            &path,
            format!("Updated {} parameter(s) in {rel}.", values.len()),
        )
    }

    fn snapshot(&mut self, args: &Value) -> ToolResult {
        let req = self.request(args)?;
        let mut cameras: Vec<Camera> = Vec::new();
        if let Some(views) = args.get("views").and_then(Value::as_array) {
            for v in views {
                let name = v.as_str().unwrap_or_default();
                let view = View::parse(name).ok_or_else(|| format!("unknown view `{name}`"))?;
                cameras.push(Camera::preset(view));
            }
        }
        if let Some(custom) = args.get("cameras").and_then(Value::as_array) {
            for c in custom {
                cameras.push(
                    serde_json::from_value(c.clone())
                        .map_err(|e| format!("invalid camera {c}: {e}"))?,
                );
            }
        }
        if cameras.is_empty() {
            cameras = DEFAULT_VIEWS.iter().map(|v| Camera::preset(*v)).collect();
        }
        if cameras.len() > 12 {
            return Err("at most 12 cameras per snapshot".into());
        }
        let size = args
            .get("size")
            .and_then(Value::as_array)
            .and_then(|s| Some((s.first()?.as_u64()? as u32, s.get(1)?.as_u64()? as u32)))
            .unwrap_or((640, 480));
        let size = (size.0.clamp(64, 2048), size.1.clamp(64, 2048));
        let mode = match args.get("mode").and_then(Value::as_str) {
            Some("render") => RenderMode::Render,
            _ => RenderMode::Preview,
        };
        let engine = self.engine()?.clone();
        let results = engine.snapshots(&req, self.scratch.path(), &cameras, size, mode);

        let mut content = Vec::new();
        let mut notes = Vec::new();
        let mut images = Vec::new();
        let mut diagnostics: Option<Vec<Diagnostic>> = None;
        for (camera, result) in results {
            match result {
                Ok(out) => {
                    if diagnostics.is_none() {
                        diagnostics = Some(out.diagnostics.clone());
                    }
                    match out.output.as_ref().map(std::fs::read) {
                        Some(Ok(png)) => {
                            notes.push(camera.label());
                            images.push(SnapshotImage {
                                label: camera.label(),
                                png_base64: base64::engine::general_purpose::STANDARD.encode(&png),
                            });
                            content.push(Content::Png(png));
                        }
                        _ => notes.push(format!("{}: failed", camera.label())),
                    }
                }
                Err(e) => notes.push(format!("{}: {e}", camera.label())),
            }
        }
        if content.is_empty() {
            let diag = diagnostics
                .map(|d| format_diagnostics(&d, false))
                .unwrap_or_default();
            return Err(format!(
                "no snapshot could be rendered ({}).\n{diag}\nOn headless Linux, OpenSCAD needs a display; install xvfb (xvfb-run).",
                notes.join(", ")
            ));
        }
        let mut header = format!(
            "{} view(s) of {}: {}",
            content.len(),
            self.project.relative(&req.file),
            notes.join(", ")
        );
        if let Some(d) = diagnostics {
            header.push('\n');
            header.push_str(&format_diagnostics(&d, true));
        }
        // Show the snapshots in the app's agent thread too.
        self.notify_app(&ControlRequest::Snapshot {
            file: self.project.relative(&req.file),
            images,
        });
        content.insert(0, Content::Text(header));
        Ok(content)
    }

    fn set_view(&mut self, args: &Value) -> ToolResult {
        let rotation = match (
            args.get("view").and_then(Value::as_str),
            args.get("rotation"),
        ) {
            (Some(name), _) => View::parse(name)
                .ok_or_else(|| format!("unknown view `{name}`"))?
                .rotation(),
            (None, Some(r)) => serde_json::from_value::<[f64; 3]>(r.clone())
                .map_err(|_| "rotation must be [x, y, z] in degrees".to_owned())?,
            (None, None) => return Err("give `view` or `rotation`".into()),
        };
        match self.notify_app(&ControlRequest::Camera { rotation }) {
            Some(reply) if reply.ok => Ok(vec![Content::Text(format!(
                "The user's viewport now shows rotation {rotation:?}."
            ))]),
            Some(reply) => Err(reply.message.unwrap_or_else(|| "the app refused".into())),
            None => Err(
                "not connected to an OpenSuperCAD window; use `snapshot` to look at the model"
                    .into(),
            ),
        }
    }

    fn render(&mut self, args: &Value) -> ToolResult {
        let req = self.request(args)?;
        let out_path = self.scratch.path().join("render.stl");
        let _ = std::fs::remove_file(&out_path);
        let engine = self.engine()?.clone();
        let out = engine.export(&req, &out_path).map_err(|e| e.to_string())?;
        let mut report = json!({
            "success": out.success,
            "duration_ms": out.duration_ms,
        });
        if out.success {
            match Mesh::load_stl(&out_path) {
                Ok(mesh) => {
                    let (min, max) = mesh.bounds();
                    report["triangles"] = json!(mesh.triangles.len());
                    report["bounding_box"] = json!({
                        "min": min, "max": max,
                        "size": [max[0] - min[0], max[1] - min[1], max[2] - min[2]],
                    });
                }
                Err(e) => report["mesh_error"] = json!(e.to_string()),
            }
        }
        let text = format!(
            "{}\n{}",
            pretty(&report),
            format_diagnostics(&out.diagnostics, out.success)
        );
        if out.success {
            Ok(vec![Content::Text(text)])
        } else {
            Err(text)
        }
    }

    fn export(&mut self, args: &Value) -> ToolResult {
        let req = self.request(args)?;
        let rel = Self::str_arg(args, "output")?;
        let out_path = self.resolve(rel)?;
        if let Some(dir) = out_path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let engine = self.engine()?.clone();
        let out = engine.export(&req, &out_path).map_err(|e| e.to_string())?;
        let text = format!(
            "{} {rel} in {} ms\n{}",
            if out.success {
                "Exported"
            } else {
                "Failed to export"
            },
            out.duration_ms,
            format_diagnostics(&out.diagnostics, out.success)
        );
        if out.success {
            Ok(vec![Content::Text(text)])
        } else {
            Err(text)
        }
    }

    fn repo(&self, create: bool) -> Result<Repo, String> {
        match Repo::discover(self.project.root()) {
            Ok(r) => Ok(r),
            Err(_) if create => Repo::init(self.project.root()).map_err(|e| e.to_string()),
            Err(e) => Err(e.to_string()),
        }
    }

    fn checkpoint(&mut self, args: &Value) -> ToolResult {
        let message = Self::str_arg(args, "message")?;
        let repo = self.repo(true)?;
        Ok(vec![Content::Text(
            match repo.checkpoint(message).map_err(|e| e.to_string())? {
                Some(c) => format!("Checkpoint {} saved: {}", c.short_id, c.summary),
                None => "No changes since the last checkpoint.".into(),
            },
        )])
    }

    fn list_checkpoints(&mut self, args: &Value) -> ToolResult {
        let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(20) as usize;
        let list = match self.repo(false) {
            Ok(repo) => repo.checkpoints(limit).map_err(|e| e.to_string())?,
            Err(_) => Vec::new(),
        };
        Ok(vec![Content::Text(pretty(&list))])
    }

    fn restore_checkpoint(&mut self, args: &Value) -> ToolResult {
        let id = Self::str_arg(args, "id")?;
        let repo = self.repo(false)?;
        repo.restore(id).map_err(|e| e.to_string())?;
        Ok(vec![Content::Text(format!(
            "Restored checkpoint {id}. The previous state was checkpointed first."
        ))])
    }
}

fn pretty<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string_pretty(v).unwrap_or_else(|e| e.to_string())
}

fn format_diagnostics(diags: &[Diagnostic], success: bool) -> String {
    let mut lines = Vec::new();
    for d in diags {
        let tag = match d.severity {
            Severity::Error => "ERROR",
            Severity::Warning => "WARNING",
            Severity::Echo => "ECHO",
            Severity::Info => continue,
        };
        lines.push(match d.line {
            Some(line) => format!("{tag} (line {line}): {}", d.message),
            None => format!("{tag}: {}", d.message),
        });
    }
    let status = if success {
        "OpenSCAD: OK"
    } else {
        "OpenSCAD: FAILED"
    };
    if lines.is_empty() {
        status.to_owned()
    } else {
        format!("{status}\n{}", lines.join("\n"))
    }
}
