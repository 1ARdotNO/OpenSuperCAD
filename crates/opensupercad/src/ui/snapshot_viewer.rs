//! A dialog that shows an agent snapshot large, with zoom and save.

use std::path::{Path, PathBuf};

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::{ActiveTheme, Disableable, IconName, Sizable, WindowExt, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

/// Zoom is relative to "fit": 1.0 shows the whole image in the viewer.
const FIT: f32 = 1.0;
const MIN_ZOOM: f32 = 0.25;
const MAX_ZOOM: f32 = 8.0;
const ZOOM_STEP: f32 = 1.25;

fn clamp_zoom(zoom: f32) -> f32 {
    if zoom.is_nan() {
        FIT
    } else {
        zoom.clamp(MIN_ZOOM, MAX_ZOOM)
    }
}

fn zoom_in(zoom: f32) -> f32 {
    clamp_zoom(zoom * ZOOM_STEP)
}

fn zoom_out(zoom: f32) -> f32 {
    clamp_zoom(zoom / ZOOM_STEP)
}

/// Exponential, so a trackpad pinch and a wheel notch zoom out exactly as
/// far as they zoomed in.
fn zoom_by_scroll(zoom: f32, delta_y: f32) -> f32 {
    clamp_zoom(zoom * (delta_y * 0.0025).exp())
}

/// `<file stem>-<label>.png`, keeping only characters that are safe in file
/// names on every platform.
fn default_file_name(file: &str, label: &str) -> String {
    let clean = |s: &str| {
        let mut out = String::new();
        for c in s.chars() {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                out.push(c);
            } else if !out.ends_with('-') {
                out.push('-');
            }
        }
        out.trim_matches('-').to_owned()
    };
    let stem = Path::new(file)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(clean)
        .unwrap_or_default();
    let label = clean(label);
    let name = [stem, label]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if name.is_empty() {
        "snapshot.png".to_owned()
    } else {
        format!("{name}.png")
    }
}

/// Size of the image area for a window of `viewport` size, leaving room for
/// the dialog chrome and toolbar.
fn viewer_size(viewport: Size<Pixels>) -> Size<Pixels> {
    let w = (f32::from(viewport.width) - 120.).clamp(320., 1200.);
    let h = (f32::from(viewport.height) * 0.65 - 40.).clamp(240., 900.);
    size(px(w), px(h))
}

pub struct SnapshotViewer {
    file: String,
    label: String,
    path: PathBuf,
    save_dir: PathBuf,
    zoom: f32,
}

impl SnapshotViewer {
    /// Open `path` (the snapshot `label` of `file`) in a dialog. Save prompts
    /// start in `save_dir`.
    pub fn open(
        file: String,
        label: String,
        path: PathBuf,
        save_dir: PathBuf,
        window: &mut Window,
        cx: &mut App,
    ) {
        let title: SharedString = format!("{label} · {file}").into();
        let viewer = cx.new(|_| Self {
            file,
            label,
            path,
            save_dir,
            zoom: FIT,
        });
        window.open_dialog(cx, move |dialog, window, _| {
            let width = viewer_size(window.viewport_size()).width + px(32.);
            dialog.title(title.clone()).w(width).child(viewer.clone())
        });
    }

    fn set_zoom(&mut self, zoom: f32, cx: &mut Context<Self>) {
        if zoom != self.zoom {
            self.zoom = zoom;
            cx.notify();
        }
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let src = self.path.clone();
        let name = default_file_name(&self.file, &self.label);
        super::path_prompt::new_path(
            "Save Snapshot",
            "File to save the snapshot PNG to",
            &self.save_dir,
            &name,
            window,
            cx,
            move |dest, window, cx| {
                let note = match std::fs::copy(&src, &dest) {
                    Ok(_) => Notification::success(format!("Saved {}", dest.display())),
                    Err(e) => Notification::error(format!(
                        "Could not save the snapshot to {}: {e}",
                        dest.display()
                    )),
                };
                window.push_notification(note, cx);
            },
        );
    }
}

impl Render for SnapshotViewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let area = viewer_size(window.viewport_size());
        let zoom = self.zoom;
        let toolbar = h_flex()
            .gap_1()
            .child(
                Button::new("snapshot-zoom-out")
                    .icon(IconName::Minus)
                    .ghost()
                    .small()
                    .tooltip("Zoom out (Ctrl/Cmd + scroll)")
                    .disabled(zoom <= MIN_ZOOM)
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.set_zoom(zoom_out(this.zoom), cx)
                    })),
            )
            .child(
                div()
                    .w(px(48.))
                    .text_center()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(format!("{:.0}%", zoom * 100.)),
            )
            .child(
                Button::new("snapshot-zoom-in")
                    .icon(IconName::Plus)
                    .ghost()
                    .small()
                    .tooltip("Zoom in (Ctrl/Cmd + scroll)")
                    .disabled(zoom >= MAX_ZOOM)
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.set_zoom(zoom_in(this.zoom), cx)
                    })),
            )
            .child(
                Button::new("snapshot-fit")
                    .label("Fit")
                    .ghost()
                    .small()
                    .tooltip("Fit the image to the window")
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.set_zoom(FIT, cx))),
            )
            .child(div().flex_1())
            .child(
                Button::new("snapshot-save")
                    .icon(IconName::ArrowDown)
                    .label("Save…")
                    .small()
                    .tooltip("Save this snapshot as a PNG file")
                    .on_click(
                        cx.listener(|this, _: &ClickEvent, window, cx| this.save(window, cx)),
                    ),
            );
        let image = img(self.path.clone())
            .flex_shrink_0()
            .w(area.width * zoom)
            .h(area.height * zoom)
            .object_fit(ObjectFit::Contain);
        let viewport = div()
            .id("snapshot-viewport")
            .w(area.width)
            .h(area.height)
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .bg(theme.secondary)
            .child(
                // At least the viewport's size, so Ctrl/Cmd-scroll zooms
                // anywhere and stops propagating before the parent scrolls.
                h_flex()
                    .flex_shrink_0()
                    .min_w(area.width)
                    .min_h(area.height)
                    .items_center()
                    .justify_center()
                    .on_scroll_wheel(cx.listener(|this, e: &ScrollWheelEvent, _, cx| {
                        let m = e.modifiers;
                        if m.control || m.platform {
                            let d = e.delta.pixel_delta(px(16.));
                            this.set_zoom(zoom_by_scroll(this.zoom, f32::from(d.y)), cx);
                            cx.stop_propagation();
                        }
                    }))
                    .child(image),
            )
            .overflow_scrollbar();
        v_flex().gap_2().child(toolbar).child(viewport)
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::{px, size};

    use super::{
        FIT, MAX_ZOOM, MIN_ZOOM, clamp_zoom, default_file_name, viewer_size, zoom_by_scroll,
        zoom_in, zoom_out,
    };

    #[test]
    fn zoom_steps_are_clamped() {
        assert_eq!(zoom_in(FIT), 1.25);
        assert_eq!(zoom_out(FIT), 0.8);
        assert_eq!(zoom_in(MAX_ZOOM), MAX_ZOOM);
        assert_eq!(zoom_out(MIN_ZOOM), MIN_ZOOM);
        assert_eq!(clamp_zoom(f32::NAN), FIT);
        assert!((zoom_out(zoom_in(FIT)) - FIT).abs() < 1e-6);
    }

    #[test]
    fn scroll_zoom_is_symmetric_and_clamped() {
        let z = zoom_by_scroll(zoom_by_scroll(FIT, 40.), -40.);
        assert!((z - FIT).abs() < 1e-5);
        assert!(zoom_by_scroll(FIT, 50.) > FIT);
        assert_eq!(zoom_by_scroll(FIT, 1e6), MAX_ZOOM);
        assert_eq!(zoom_by_scroll(FIT, -1e6), MIN_ZOOM);
    }

    #[test]
    fn default_file_name_is_safe() {
        assert_eq!(
            default_file_name("parts/bracket.scad", "front"),
            "bracket-front.png"
        );
        assert_eq!(
            default_file_name("a b.scad", "Top / iso"),
            "a-b-Top-iso.png"
        );
        assert_eq!(default_file_name("", "diag"), "diag.png");
        assert_eq!(default_file_name("", "///"), "snapshot.png");
    }

    #[test]
    fn viewer_size_stays_in_bounds() {
        let small = viewer_size(size(px(200.), px(100.)));
        assert_eq!((small.width, small.height), (px(320.), px(240.)));
        let big = viewer_size(size(px(4000.), px(3000.)));
        assert_eq!((big.width, big.height), (px(1200.), px(900.)));
    }
}
