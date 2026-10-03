//! A small, dependency-free software rasteriser used by the interactive
//! preview: OpenSCAD exports the mesh once, then orbiting/zooming is local.
//!
//! Camera conventions follow OpenSCAD: `rotation` is the viewport rotation
//! `$vpr` (so `[55, 0, 25]` is the default view, `[0, 0, 0]` looks down from
//! the top and `[90, 0, 0]` looks at the front).

use crate::mesh::{Mesh, Vec3};

#[derive(Clone, Debug)]
pub struct RasterOptions {
    pub width: u32,
    pub height: u32,
    /// Viewport rotation in degrees, OpenSCAD `$vpr` convention.
    pub rotation: [f32; 3],
    /// 1.0 frames the whole model; larger zooms in.
    pub zoom: f32,
    /// Screen-space pan in pixels.
    pub pan: [f32; 2],
    pub background: [u8; 4],
    pub color: [u8; 3],
}

impl Default for RasterOptions {
    fn default() -> Self {
        Self {
            width: 800,
            height: 600,
            rotation: [55.0, 0.0, 25.0],
            zoom: 1.0,
            pan: [0.0, 0.0],
            background: [0x1e, 0x1f, 0x22, 0xff],
            color: [0xf9, 0xd7, 0x5c],
        }
    }
}

/// An RGBA8 image.
#[derive(Clone, Debug)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Image {
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [
            self.pixels[i],
            self.pixels[i + 1],
            self.pixels[i + 2],
            self.pixels[i + 3],
        ]
    }

    /// Encode as PNG.
    pub fn to_png(&self) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, self.width, self.height);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            // Writing into a Vec cannot fail for a correctly sized buffer.
            let mut writer = enc.write_header().expect("png header");
            writer.write_image_data(&self.pixels).expect("png data");
        }
        out
    }
}

fn rotate(v: Vec3, rot: [f32; 3]) -> Vec3 {
    // OpenSCAD: object_rot = (90 - vpr.x, -vpr.y, -vpr.z), applied as
    // Rx * Ry * Rz with the eye on the -Y axis looking towards +Y.
    let (ax, ay, az) = (
        (90.0 - rot[0]).to_radians(),
        (-rot[1]).to_radians(),
        (-rot[2]).to_radians(),
    );
    let [x, y, z] = v;
    // Rz
    let (x, y) = (x * az.cos() - y * az.sin(), x * az.sin() + y * az.cos());
    // Ry
    let (x, z) = (x * ay.cos() + z * ay.sin(), -x * ay.sin() + z * ay.cos());
    // Rx
    let (y, z) = (y * ax.cos() - z * ax.sin(), y * ax.sin() + z * ax.cos());
    [x, y, z]
}

/// Render the mesh with flat shading and a depth buffer.
pub fn render(mesh: &Mesh, opts: &RasterOptions) -> Image {
    let (w, h) = (opts.width.max(1), opts.height.max(1));
    let mut pixels = Vec::with_capacity((w * h * 4) as usize);
    for _ in 0..w * h {
        pixels.extend_from_slice(&opts.background);
    }
    let mut depth = vec![f32::INFINITY; (w * h) as usize];

    let (min, max) = mesh.bounds();
    let center = [
        (min[0] + max[0]) / 2.0,
        (min[1] + max[1]) / 2.0,
        (min[2] + max[2]) / 2.0,
    ];
    let radius =
        ((max[0] - min[0]).powi(2) + (max[1] - min[1]).powi(2) + (max[2] - min[2]).powi(2)).sqrt()
            / 2.0;
    let scale = if radius > 0.0 {
        (w.min(h) as f32 / (2.2 * radius)) * opts.zoom.max(0.01)
    } else {
        1.0
    };
    // Light from the viewer, slightly above and to the left.
    let light = normalize([-0.4, -1.0, 0.6]);

    for tri in &mesh.triangles {
        let p = tri.map(|v| {
            let r = rotate(
                [v[0] - center[0], v[1] - center[1], v[2] - center[2]],
                opts.rotation,
            );
            // Screen: x right, y down; depth = r.y (smaller = closer).
            [
                w as f32 / 2.0 + r[0] * scale + opts.pan[0],
                h as f32 / 2.0 - r[2] * scale + opts.pan[1],
                r[1],
            ]
        });
        let world = tri.map(|v| rotate(v, opts.rotation));
        let n = normalize(cross(sub(world[1], world[0]), sub(world[2], world[0])));
        let lambert = dot(n, light).abs();
        let shade = 0.25 + 0.75 * lambert;
        let rgb = opts
            .color
            .map(|c| (c as f32 * shade).clamp(0.0, 255.0) as u8);

        let min_x = p
            .iter()
            .map(|v| v[0])
            .fold(f32::INFINITY, f32::min)
            .floor()
            .max(0.0) as u32;
        let max_x = p
            .iter()
            .map(|v| v[0])
            .fold(f32::NEG_INFINITY, f32::max)
            .ceil()
            .min((w - 1) as f32);
        let min_y = p
            .iter()
            .map(|v| v[1])
            .fold(f32::INFINITY, f32::min)
            .floor()
            .max(0.0) as u32;
        let max_y = p
            .iter()
            .map(|v| v[1])
            .fold(f32::NEG_INFINITY, f32::max)
            .ceil()
            .min((h - 1) as f32);
        if max_x < 0.0 || max_y < 0.0 {
            continue;
        }
        let area = edge(p[0], p[1], p[2]);
        if area.abs() < f32::EPSILON {
            continue;
        }
        for y in min_y..=max_y as u32 {
            for x in min_x..=max_x as u32 {
                let q = [x as f32 + 0.5, y as f32 + 0.5, 0.0];
                let w0 = edge(p[1], p[2], q) / area;
                let w1 = edge(p[2], p[0], q) / area;
                let w2 = edge(p[0], p[1], q) / area;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let z = w0 * p[0][2] + w1 * p[1][2] + w2 * p[2][2];
                let idx = (y * w + x) as usize;
                if z < depth[idx] {
                    depth[idx] = z;
                    pixels[idx * 4..idx * 4 + 3].copy_from_slice(&rgb);
                    pixels[idx * 4 + 3] = 0xff;
                }
            }
        }
    }
    Image {
        width: w,
        height: h,
        pixels,
    }
}

fn edge(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> f32 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalize(v: Vec3) -> Vec3 {
    let len = dot(v, v).sqrt();
    if len == 0.0 { v } else { v.map(|c| c / len) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat square in the XY plane at z = 0 spanning x 0..2, y 0..1 (wider
    /// than deep), so views can be told apart by the silhouette.
    fn plate() -> Mesh {
        Mesh {
            triangles: vec![
                [[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [2.0, 1.0, 0.0]],
                [[0.0, 0.0, 0.0], [2.0, 1.0, 0.0], [0.0, 1.0, 0.0]],
            ],
        }
    }

    fn coverage(img: &Image, bg: [u8; 4]) -> usize {
        img.pixels.chunks_exact(4).filter(|p| *p != bg).count()
    }

    #[test]
    fn top_view_shows_the_plate_front_view_does_not() {
        let opts = RasterOptions {
            width: 64,
            height: 64,
            rotation: [0.0, 0.0, 0.0],
            ..Default::default()
        };
        let top = render(&plate(), &opts);
        assert!(coverage(&top, opts.background) > 500);
        // Seen edge-on from the front, the plate is (nearly) invisible.
        let front = render(
            &plate(),
            &RasterOptions {
                rotation: [90.0, 0.0, 0.0],
                ..opts.clone()
            },
        );
        assert!(coverage(&front, opts.background) < 100);
    }

    #[test]
    fn png_encoding_has_signature() {
        let img = render(&Mesh::default(), &RasterOptions::default());
        assert_eq!(&img.to_png()[..8], b"\x89PNG\r\n\x1a\n");
    }
}
