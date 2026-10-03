//! The 3D preview: an interactive viewport over the last rendered mesh, with
//! OpenSCAD's view presets, plus OpenSCAD's own preview image (F5).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme, IconName, Sizable, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use osc_engine::mesh::Mesh;
use osc_engine::raster::RasterOptions;
use osc_engine::{Camera, Engine, View};

use crate::pipeline::{self, JobResult};

const FRAME: (u32, u32) = (1200, 900);

pub enum PreviewEvent {
    /// A render or preview job finished (for the console/status bar).
    Finished(JobResult),
}

impl EventEmitter<PreviewEvent> for Preview {}

pub struct Preview {
    engine: Option<Arc<Engine>>,
    scratch: PathBuf,
    mesh: Option<Arc<Mesh>>,
    /// Viewport rotation, OpenSCAD `$vpr` convention.
    rotation: [f32; 3],
    zoom: f32,
    pan: [f32; 2],
    frame: Option<Arc<Image>>,
    /// OpenSCAD's own preview image; shown until the user orbits.
    openscad_image: Option<Arc<Image>>,
    drag_from: Option<Point<Pixels>>,
    busy: usize,
    raster_gen: u64,
    render_gen: u64,
    pub last_message: Option<SharedString>,
}

impl Preview {
    pub fn new(engine: Option<Arc<Engine>>) -> Self {
        Self {
            engine,
            scratch: pipeline::scratch_dir(),
            mesh: None,
            rotation: View::Iso.rotation().map(|v| v as f32),
            zoom: 1.0,
            pan: [0.0, 0.0],
            frame: None,
            openscad_image: None,
            drag_from: None,
            busy: 0,
            raster_gen: 0,
            render_gen: 0,
            last_message: None,
        }
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.mesh = None;
        self.frame = None;
        self.openscad_image = None;
        cx.notify();
    }

    /// Full render (F6 / on save): export a mesh and show it.
    pub fn render(&mut self, file: &Path, cx: &mut Context<Self>) {
        let Some(engine) = self.engine.clone() else {
            self.last_message = Some("OpenSCAD not found. Run `opensupercad doctor`.".into());
            cx.notify();
            return;
        };
        self.render_gen += 1;
        let generation = self.render_gen;
        self.busy += 1;
        cx.notify();
        let file = file.to_path_buf();
        let scratch = self.scratch.clone();
        let job =
            cx.background_spawn(async move { pipeline::render_mesh(&engine, &file, &scratch) });
        cx.spawn(async move |this, cx| {
            let result = job.await;
            this.update(cx, |this, cx| {
                this.busy = this.busy.saturating_sub(1);
                // A newer render supersedes this one.
                if generation == this.render_gen {
                    if let Some(mesh) = result.mesh.clone() {
                        this.mesh = Some(Arc::new(mesh));
                        this.openscad_image = None;
                        this.rasterize(cx);
                    }
                    this.last_message = Some(summary(&result).into());
                }
                cx.emit(PreviewEvent::Finished(result));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// OpenSCAD's own preview (F5) from the current camera.
    pub fn openscad_preview(&mut self, file: &Path, cx: &mut Context<Self>) {
        let Some(engine) = self.engine.clone() else {
            return;
        };
        self.busy += 1;
        cx.notify();
        let camera = Camera::Gimbal {
            rotation: self.rotation.map(f64::from),
            distance: None,
            translate: [0.0; 3],
        };
        let file = file.to_path_buf();
        let scratch = self.scratch.clone();
        let job = cx.background_spawn(async move {
            pipeline::preview_image(&engine, &file, &camera, FRAME, &scratch)
        });
        cx.spawn(async move |this, cx| {
            let result = job.await;
            this.update(cx, |this, cx| {
                this.busy = this.busy.saturating_sub(1);
                if let Some(png) = result.image.clone() {
                    this.openscad_image = Some(Arc::new(Image::from_bytes(ImageFormat::Png, png)));
                }
                this.last_message = Some(summary(&result).into());
                cx.emit(PreviewEvent::Finished(result));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn set_view(&mut self, view: View, cx: &mut Context<Self>) {
        self.rotation = view.rotation().map(|v| v as f32);
        self.openscad_image = None;
        self.rasterize(cx);
    }

    pub fn reset_view(&mut self, cx: &mut Context<Self>) {
        self.zoom = 1.0;
        self.pan = [0.0, 0.0];
        self.set_view(View::Iso, cx);
    }

    /// Rasterise the mesh off the main thread; stale frames are dropped.
    fn rasterize(&mut self, cx: &mut Context<Self>) {
        let Some(mesh) = self.mesh.clone() else {
            return;
        };
        self.raster_gen += 1;
        let generation = self.raster_gen;
        let theme = cx.theme();
        let bg = theme.background.to_rgb();
        let opts = RasterOptions {
            width: FRAME.0,
            height: FRAME.1,
            rotation: self.rotation,
            zoom: self.zoom,
            pan: self.pan,
            background: [
                (bg.r * 255.0) as u8,
                (bg.g * 255.0) as u8,
                (bg.b * 255.0) as u8,
                0xff,
            ],
            color: [0xf9, 0xd7, 0x5c],
        };
        let job = cx.background_spawn(async move { pipeline::rasterize(&mesh, &opts) });
        cx.spawn(async move |this, cx| {
            let png = job.await;
            this.update(cx, |this, cx| {
                if generation == this.raster_gen {
                    this.frame = Some(Arc::new(Image::from_bytes(ImageFormat::Png, png)));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn on_drag(&mut self, e: &MouseMoveEvent, cx: &mut Context<Self>) {
        if e.pressed_button != Some(MouseButton::Left) {
            self.drag_from = None;
            return;
        }
        let Some(prev) = self.drag_from.replace(e.position) else {
            return;
        };
        let dx = f32::from(e.position.x - prev.x);
        let dy = f32::from(e.position.y - prev.y);
        if e.modifiers.shift {
            // Pan in frame pixels (the frame is scaled to fit the element).
            self.pan[0] += dx * 1.5;
            self.pan[1] += dy * 1.5;
        } else {
            self.rotation[2] -= dx * 0.4;
            self.rotation[0] = (self.rotation[0] - dy * 0.4).clamp(0.0, 180.0);
        }
        self.openscad_image = None;
        self.rasterize(cx);
    }

    fn view_button(
        &self,
        id: &'static str,
        label: &'static str,
        view: View,
        cx: &mut Context<Self>,
    ) -> Button {
        Button::new(id)
            .label(label)
            .ghost()
            .xsmall()
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.set_view(view, cx)))
    }
}

fn summary(result: &JobResult) -> String {
    let errors = result
        .diagnostics
        .iter()
        .filter(|d| d.severity == osc_engine::Severity::Error)
        .count();
    let warnings = result
        .diagnostics
        .iter()
        .filter(|d| d.severity == osc_engine::Severity::Warning)
        .count();
    if result.success {
        let tris = result
            .mesh
            .as_ref()
            .map(|m| format!(" · {} triangles", m.triangles.len()))
            .unwrap_or_default();
        format!(
            "Done in {} ms{tris} · {warnings} warning(s)",
            result.duration_ms
        )
    } else {
        format!("Failed: {errors} error(s), {warnings} warning(s)")
    }
}

impl Render for Preview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let image = self.openscad_image.clone().or_else(|| self.frame.clone());
        let toolbar = h_flex()
            .gap_0p5()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.tab_bar)
            .child(self.view_button("v-iso", "Iso", View::Iso, cx))
            .child(self.view_button("v-top", "Top", View::Top, cx))
            .child(self.view_button("v-bottom", "Bottom", View::Bottom, cx))
            .child(self.view_button("v-front", "Front", View::Front, cx))
            .child(self.view_button("v-back", "Back", View::Back, cx))
            .child(self.view_button("v-left", "Left", View::Left, cx))
            .child(self.view_button("v-right", "Right", View::Right, cx))
            .child(div().flex_1())
            .when(self.busy > 0, |el| el.child(Spinner::new().small()))
            .child(
                Button::new("v-reset")
                    .icon(IconName::RefreshCw)
                    .ghost()
                    .xsmall()
                    .tooltip("Reset view")
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.reset_view(cx))),
            );

        let viewport = div()
            .id("viewport")
            .flex_1()
            .size_full()
            .overflow_hidden()
            .bg(theme.background)
            .cursor_grab()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, e: &MouseDownEvent, _, _| this.drag_from = Some(e.position)),
            )
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| this.on_drag(e, cx)))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, _| this.drag_from = None),
            )
            .on_scroll_wheel(cx.listener(|this, e: &ScrollWheelEvent, _, cx| {
                let d = e.delta.pixel_delta(px(16.));
                this.zoom = (this.zoom * (1.0 - f32::from(d.y) * 0.002)).clamp(0.05, 50.0);
                this.openscad_image = None;
                this.rasterize(cx);
            }))
            .map(|el| match image {
                Some(image) => el.child(img(image).size_full().object_fit(ObjectFit::Contain)),
                None => el.child(
                    v_flex()
                        .size_full()
                        .items_center()
                        .justify_center()
                        .text_color(theme.muted_foreground)
                        .text_sm()
                        .child(if self.engine.is_none() {
                            "OpenSCAD was not found. Install it, then restart (see `opensupercad doctor`)."
                        } else if self.busy > 0 {
                            "Rendering…"
                        } else {
                            "Save (Ctrl/Cmd-S) or press F6 to render."
                        }),
                ),
            });

        v_flex()
            .size_full()
            .child(toolbar)
            .child(viewport)
            .when_some(self.last_message.clone(), |el, msg| {
                el.child(
                    div()
                        .px_2()
                        .py_0p5()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .border_t_1()
                        .border_color(theme.border)
                        .child(msg),
                )
            })
    }
}
