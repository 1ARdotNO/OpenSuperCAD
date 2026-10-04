//! The 3D preview: an interactive viewport over the last rendered mesh, with
//! OpenSCAD's view presets, plus OpenSCAD's own preview image (F5).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme, IconName, Selectable, Sizable, h_flex, v_flex};
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
    edges: Arc<Vec<[osc_engine::mesh::Vec3; 2]>>,
    show_edges: bool,
    show_axes: bool,
    show_grid: bool,
    /// Viewport rotation, OpenSCAD `$vpr` convention.
    rotation: [f32; 3],
    zoom: f32,
    pan: [f32; 2],
    /// The rasterised view, handed to the GPU as is: no PNG round trip, so
    /// a new frame paints at once and the old one stays until it does.
    frame: Option<Arc<RenderImage>>,
    /// OpenSCAD's own preview image; shown until the user orbits.
    openscad_image: Option<Arc<Image>>,
    drag_from: Option<Point<Pixels>>,
    busy: usize,
    raster_gen: u64,
    /// A raster is running; requests meanwhile only set `raster_dirty`.
    raster_busy: bool,
    /// The view changed while a raster was running: draw it once it ends.
    raster_dirty: bool,
    /// Generation of the frame on screen. Any newer frame is shown, so a
    /// stream of requests (playback, orbiting) can't starve the display.
    shown_raster: u64,
    /// The background the current frame was drawn on, to redraw after a
    /// theme change.
    raster_bg: Option<[u8; 4]>,
    render_gen: u64,
    pub last_message: Option<SharedString>,
    /// File of the last render (animation frames are rendered from it).
    last_file: Option<PathBuf>,
    anim: Option<Animation>,
}

/// A rendered animation frame: mesh plus its outline edges.
type Frame = (Arc<Mesh>, Arc<Vec<[osc_engine::mesh::Vec3; 2]>>);

/// OpenSCAD animation: frames rendered with `$t = i / steps`, cached so
/// playback and scrubbing are instant.
struct Animation {
    steps: usize,
    fps: u32,
    frame: usize,
    playing: bool,
    frames: Vec<Option<Frame>>,
    generation: u64,
}

impl Animation {
    /// Union of the rendered frames' bounds, so playback doesn't jitter.
    fn bounds(&self) -> Option<([f32; 3], [f32; 3])> {
        self.frames
            .iter()
            .flatten()
            .map(|(m, _)| m.bounds())
            .reduce(|(a0, a1), (b0, b1)| {
                (
                    [a0[0].min(b0[0]), a0[1].min(b0[1]), a0[2].min(b0[2])],
                    [a1[0].max(b1[0]), a1[1].max(b1[1]), a1[2].max(b1[2])],
                )
            })
    }

    fn t(&self, frame: usize) -> f64 {
        frame as f64 / self.steps as f64
    }
}

impl Preview {
    pub fn new(engine: Option<Arc<Engine>>) -> Self {
        Self {
            engine,
            scratch: pipeline::scratch_dir(),
            mesh: None,
            edges: Arc::new(Vec::new()),
            show_edges: true,
            show_axes: true,
            show_grid: true,
            rotation: View::Iso.rotation().map(|v| v as f32),
            zoom: 1.0,
            pan: [0.0, 0.0],
            frame: None,
            openscad_image: None,
            drag_from: None,
            busy: 0,
            raster_gen: 0,
            raster_busy: false,
            raster_dirty: false,
            shown_raster: 0,
            raster_bg: None,
            render_gen: 0,
            last_message: None,
            last_file: None,
            anim: None,
        }
    }

    /// Swap the OpenSCAD configuration (e.g. a different backend).
    pub fn set_engine(&mut self, engine: Option<Arc<Engine>>) {
        self.engine = engine;
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.mesh = None;
        self.set_frame(None, cx);
        self.openscad_image = None;
        cx.notify();
    }

    /// Full render (F6 / on save): export a mesh and show it.
    pub fn render(&mut self, file: &Path, cx: &mut Context<Self>) {
        let Some(engine) = self.engine.clone() else {
            self.last_message =
                Some("OpenSCAD not found: download it or locate it in Settings.".into());
            cx.notify();
            return;
        };
        self.last_file = Some(file.to_path_buf());
        if self.anim.is_some() {
            // The design changed: re-render every animation frame.
            self.render_frames(cx);
            return;
        }
        self.render_gen += 1;
        let generation = self.render_gen;
        self.busy += 1;
        cx.notify();
        let file = file.to_path_buf();
        let scratch = self.scratch.clone();
        let job = cx
            .background_spawn(async move { pipeline::render_mesh(&engine, &file, &[], &scratch) });
        cx.spawn(async move |this, cx| {
            let result = job.await;
            this.update(cx, |this, cx| {
                this.busy = this.busy.saturating_sub(1);
                // A newer render supersedes this one.
                if generation == this.render_gen {
                    if let Some(mesh) = result.mesh.clone() {
                        this.mesh = Some(Arc::new(mesh));
                        this.edges = Arc::new(result.edges.clone().unwrap_or_default());
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

    pub fn toggle_animation(&mut self, cx: &mut Context<Self>) {
        if self.anim.take().is_some() {
            if let Some(file) = self.last_file.clone() {
                self.render(&file, cx);
            }
        } else {
            self.anim = Some(Animation {
                steps: 24,
                fps: 12,
                frame: 0,
                playing: false,
                frames: Vec::new(),
                generation: 0,
            });
            self.render_frames(cx);
        }
        cx.notify();
    }

    /// Render all frames in the background, one after another.
    fn render_frames(&mut self, cx: &mut Context<Self>) {
        let (Some(engine), Some(file), Some(anim)) = (
            self.engine.clone(),
            self.last_file.clone(),
            self.anim.as_mut(),
        ) else {
            return;
        };
        anim.generation += 1;
        anim.frames = vec![None; anim.steps];
        let (generation, steps) = (anim.generation, anim.steps);
        let scratch = self.scratch.clone();
        self.busy += 1;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let mut failed = None;
            for i in 0..steps {
                let (engine, file, scratch) = (engine.clone(), file.clone(), scratch.clone());
                let t = i as f64 / steps as f64;
                let result = cx
                    .background_spawn(async move {
                        let defines = [("$t".to_owned(), osc_syntax::customizer::Value::Number(t))];
                        pipeline::render_mesh(&engine, &file, &defines, &scratch)
                    })
                    .await;
                let keep_going = this
                    .update(cx, |this, cx| {
                        let Some(anim) = this.anim.as_mut().filter(|a| a.generation == generation)
                        else {
                            return false;
                        };
                        if let Some(mesh) = result.mesh.clone() {
                            anim.frames[i] = Some((
                                Arc::new(mesh),
                                Arc::new(result.edges.clone().unwrap_or_default()),
                            ));
                            if i == anim.frame {
                                this.show_frame(i, cx);
                            }
                        } else if failed.is_none() {
                            failed = Some(result.clone());
                        }
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !keep_going {
                    break;
                }
            }
            this.update(cx, |this, cx| {
                this.busy = this.busy.saturating_sub(1);
                if let Some(result) = failed {
                    this.last_message = Some(summary(&result).into());
                    cx.emit(PreviewEvent::Finished(result));
                } else if let Some(anim) = &this.anim {
                    this.last_message = Some(
                        format!("Animation: {} frames at {} fps", anim.steps, anim.fps).into(),
                    );
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn show_frame(&mut self, frame: usize, cx: &mut Context<Self>) {
        let Some(anim) = self.anim.as_mut() else {
            return;
        };
        anim.frame = frame.min(anim.steps.saturating_sub(1));
        if let Some(Some((mesh, edges))) = anim.frames.get(anim.frame) {
            self.mesh = Some(mesh.clone());
            self.edges = edges.clone();
            self.openscad_image = None;
            self.rasterize(cx);
        }
        cx.notify();
    }

    fn toggle_play(&mut self, cx: &mut Context<Self>) {
        let Some(anim) = self.anim.as_mut() else {
            return;
        };
        anim.playing = !anim.playing;
        if !anim.playing {
            cx.notify();
            return;
        }
        let interval = std::time::Duration::from_millis(1000 / u64::from(anim.fps.max(1)));
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(interval).await;
                let playing = this
                    .update(cx, |this, cx| {
                        let Some(anim) = this.anim.as_ref().filter(|a| a.playing) else {
                            return false;
                        };
                        // Wait for the last frame to be drawn, so playback
                        // slows down rather than queueing rasters.
                        if this.shown_raster < this.raster_gen {
                            return true;
                        }
                        let next = (anim.frame + 1) % anim.steps.max(1);
                        this.show_frame(next, cx);
                        true
                    })
                    .unwrap_or(false);
                if !playing {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    fn render_animation_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let anim = self.anim.as_ref()?;
        let theme = cx.theme().clone();
        let mut cells = h_flex().flex_1().h(px(14.)).gap_px();
        for i in 0..anim.steps {
            let ready = anim.frames.get(i).is_some_and(Option::is_some);
            let current = i == anim.frame;
            cells = cells.child(
                div()
                    .id(("frame", i))
                    .flex_1()
                    .h_full()
                    .rounded_sm()
                    .cursor_pointer()
                    .bg(if current {
                        theme.primary
                    } else if ready {
                        theme.muted
                    } else {
                        theme.background
                    })
                    .border_1()
                    .border_color(theme.border)
                    .on_click(
                        cx.listener(move |this, _: &ClickEvent, _, cx| this.show_frame(i, cx)),
                    ),
            );
        }
        Some(
            h_flex()
                .gap_2()
                .px_2()
                .py_1()
                .border_t_1()
                .border_color(theme.border)
                .child(
                    Button::new("anim-play")
                        .icon(if anim.playing {
                            IconName::Pause
                        } else {
                            IconName::Play
                        })
                        .ghost()
                        .xsmall()
                        .tooltip(if anim.playing { "Pause" } else { "Play" })
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.toggle_play(cx))),
                )
                .child(cells)
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(format!("$t = {:.3}", anim.t(anim.frame))),
                )
                .into_any_element(),
        )
    }

    /// Point the camera at an arbitrary rotation (from the agent).
    pub fn set_rotation(&mut self, rotation: [f64; 3], cx: &mut Context<Self>) {
        self.rotation = rotation.map(|v| v as f32);
        self.openscad_image = None;
        self.rasterize(cx);
    }

    pub fn reset_view(&mut self, cx: &mut Context<Self>) {
        self.zoom = 1.0;
        self.pan = [0.0, 0.0];
        self.set_view(View::Iso, cx);
    }

    /// Rasterise the mesh off the main thread. One raster runs at a time:
    /// requests while it runs (every mouse move of an orbit) are coalesced
    /// into one more raster of the latest view when it ends.
    fn rasterize(&mut self, cx: &mut Context<Self>) {
        let Some(mesh) = self.mesh.clone() else {
            return;
        };
        if self.raster_busy {
            self.raster_dirty = true;
            return;
        }
        self.raster_busy = true;
        self.raster_dirty = false;
        self.raster_gen += 1;
        let generation = self.raster_gen;
        let background = theme_background(cx);
        self.raster_bg = Some(background);
        let opts = RasterOptions {
            width: FRAME.0,
            height: FRAME.1,
            rotation: self.rotation,
            zoom: self.zoom,
            pan: self.pan,
            background,
            color: [0xf9, 0xd7, 0x5c],
            bounds: self.anim.as_ref().and_then(Animation::bounds),
        };
        let edges = self.edges.clone();
        let (show_edges, axes, grid) = (self.show_edges, self.show_axes, self.show_grid);
        let job = cx.background_spawn(async move {
            let overlays = osc_engine::raster::Overlays {
                edges: show_edges.then_some(edges.as_slice()),
                axes,
                grid,
            };
            render_image(pipeline::rasterize(&mesh, &overlays, &opts))
        });
        cx.spawn(async move |this, cx| {
            let frame = job.await;
            this.update(cx, |this, cx| {
                this.raster_busy = false;
                if generation > this.shown_raster {
                    this.shown_raster = generation;
                    this.set_frame(frame.map(Arc::new), cx);
                    cx.notify();
                }
                if this.raster_dirty {
                    this.rasterize(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Show `frame`, releasing the GPU texture of the one it replaces (each
    /// frame is a new texture; orbiting would otherwise fill the atlas).
    fn set_frame(&mut self, frame: Option<Arc<RenderImage>>, cx: &mut Context<Self>) {
        if let Some(old) = std::mem::replace(&mut self.frame, frame) {
            // Deferred: windows are borrowed while they draw.
            cx.defer(move |cx| cx.drop_image(old, None));
        }
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

    fn toggle(
        &self,
        id: &'static str,
        label: &'static str,
        on: bool,
        field: fn(&mut Self) -> &mut bool,
        cx: &mut Context<Self>,
    ) -> Button {
        Button::new(id)
            .label(label)
            .ghost()
            .xsmall()
            .selected(on)
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                let flag = field(this);
                *flag = !*flag;
                this.rasterize(cx);
            }))
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
        // The frame bakes in the theme's background: redraw after a switch.
        if self.raster_bg.is_some_and(|bg| bg != theme_background(cx)) {
            self.rasterize(cx);
        }
        let theme = cx.theme().clone();
        let image: Option<ImageSource> = match (&self.openscad_image, &self.frame) {
            (Some(png), _) => Some(png.clone().into()),
            (None, Some(frame)) => Some(ImageSource::Render(frame.clone())),
            (None, None) => None,
        };
        // Wraps rather than clipping, so every toggle stays reachable in a
        // narrow window.
        let toolbar = h_flex()
            .flex_wrap()
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
            .child(div().w(px(8.)))
            .child(self.toggle(
                "t-edges",
                "Edges",
                self.show_edges,
                |p| &mut p.show_edges,
                cx,
            ))
            .child(self.toggle("t-axes", "Axes", self.show_axes, |p| &mut p.show_axes, cx))
            .child(self.toggle("t-grid", "Grid", self.show_grid, |p| &mut p.show_grid, cx))
            .child(
                Button::new("t-anim")
                    .label("Animate")
                    .ghost()
                    .xsmall()
                    .selected(self.anim.is_some())
                    .tooltip("Render $t frames (OpenSCAD animation)")
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.toggle_animation(cx))),
            )
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

        let anim_bar = self.render_animation_bar(cx);
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
            .when_some(anim_bar, |el, bar| el.child(bar))
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

/// The rasteriser's RGBA pixels as a GPUI image, which wants BGRA.
fn render_image(image: osc_engine::raster::Image) -> Option<RenderImage> {
    let mut pixels = image.pixels;
    for [r, _, b, _] in pixels.as_chunks_mut::<4>().0 {
        std::mem::swap(r, b);
    }
    let buffer = image::RgbaImage::from_raw(image.width, image.height, pixels)?;
    Some(RenderImage::new(vec![image::Frame::new(buffer)]))
}

/// The theme background as RGBA bytes for the rasteriser.
fn theme_background(cx: &App) -> [u8; 4] {
    let bg = cx.theme().background.to_rgb();
    [
        (bg.r * 255.0) as u8,
        (bg.g * 255.0) as u8,
        (bg.b * 255.0) as u8,
        0xff,
    ]
}

#[cfg(test)]
mod tests {
    use super::render_image;

    #[test]
    fn frames_go_to_the_gpu_as_bgra() {
        let image = osc_engine::raster::Image {
            width: 2,
            height: 1,
            pixels: vec![1, 2, 3, 255, 10, 20, 30, 128],
        };
        let frame = render_image(image).unwrap();
        assert_eq!(frame.as_bytes(0).unwrap(), &[3, 2, 1, 255, 30, 20, 10, 128]);
        let short = osc_engine::raster::Image {
            width: 2,
            height: 2,
            pixels: vec![0; 4],
        };
        assert!(render_image(short).is_none());
    }
}
