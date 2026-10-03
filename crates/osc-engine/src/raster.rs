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

/// Extra things drawn on top of the shaded mesh.
#[derive(Clone, Copy, Debug, Default)]
pub struct Overlays<'a> {
    /// Feature edges to outline (see [`feature_edges`]).
    pub edges: Option<&'a [[Vec3; 2]]>,
    /// X/Y/Z axes from the origin (red/green/blue, as in OpenSCAD).
    pub axes: bool,
    /// Build-plate grid on the z = 0 plane.
    pub grid: bool,
}

/// Render the mesh with flat shading and a depth buffer.
pub fn render(mesh: &Mesh, opts: &RasterOptions) -> Image {
    render_with(mesh, &Overlays::default(), opts)
}

/// Mesh edges worth outlining: borders and creases whose faces meet at more
/// than `angle_deg` degrees. Smooth tessellation edges are skipped.
pub fn feature_edges(mesh: &Mesh, angle_deg: f32) -> Vec<[Vec3; 2]> {
    use std::collections::HashMap;
    let key = |v: Vec3| v.map(|c| (c * 1000.0).round() as i64);
    type EdgeKey = ([i64; 3], [i64; 3]);
    // Edge endpoints plus the normals of the faces sharing it.
    let mut map: HashMap<EdgeKey, (Vec3, Vec3, Vec<Vec3>)> = HashMap::new();
    for tri in &mesh.triangles {
        let n = normalize(cross(sub(tri[1], tri[0]), sub(tri[2], tri[0])));
        for (a, b) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
            let (ka, kb) = (key(a), key(b));
            let k = if ka <= kb { (ka, kb) } else { (kb, ka) };
            map.entry(k).or_insert_with(|| (a, b, Vec::new())).2.push(n);
        }
    }
    let threshold = angle_deg.to_radians().cos();
    map.into_values()
        .filter(|(_, _, normals)| match normals.as_slice() {
            [n1, n2] => dot(*n1, *n2) < threshold,
            _ => true, // border or non-manifold edge
        })
        .map(|(a, b, _)| [a, b])
        .collect()
}

/// A "nice" grid step (1, 2 or 5 × 10ⁿ) giving roughly ten cells over `size`.
fn grid_step(size: f32) -> f32 {
    let raw = (size / 10.0).max(0.1);
    let mag = 10f32.powf(raw.log10().floor());
    let norm = raw / mag;
    let nice = if norm < 1.5 {
        1.0
    } else if norm < 3.5 {
        2.0
    } else if norm < 7.5 {
        5.0
    } else {
        10.0
    };
    nice * mag
}

/// Render the mesh plus overlays.
pub fn render_with(mesh: &Mesh, overlays: &Overlays, opts: &RasterOptions) -> Image {
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
    // Screen: x right, y down; depth = rotated y (smaller = closer).
    let project = |v: Vec3| -> [f32; 3] {
        let r = rotate(
            [v[0] - center[0], v[1] - center[1], v[2] - center[2]],
            opts.rotation,
        );
        [
            w as f32 / 2.0 + r[0] * scale + opts.pan[0],
            h as f32 / 2.0 - r[2] * scale + opts.pan[1],
            r[1],
        ]
    };
    // Light from the viewer, slightly above and to the left.
    let light = normalize([-0.4, -1.0, 0.6]);

    for (ix, tri) in mesh.triangles.iter().enumerate() {
        let p = tri.map(project);
        let base = mesh
            .colors
            .get(ix)
            .map(|c| [c[0], c[1], c[2]])
            .unwrap_or(opts.color);
        let world = tri.map(|v| rotate(v, opts.rotation));
        let n = normalize(cross(sub(world[1], world[0]), sub(world[2], world[0])));
        let lambert = dot(n, light).abs();
        let shade = 0.25 + 0.75 * lambert;
        let rgb = base.map(|c| (c as f32 * shade).clamp(0.0, 255.0) as u8);
        fill_triangle(&mut pixels, &mut depth, w, h, p, rgb);
    }

    // Depth tolerance for lines lying on surfaces.
    let bias = radius.max(1.0) * 0.004;
    let mut line = |a: Vec3, b: Vec3, rgb: [u8; 3], mode: LineDepth| {
        draw_line(
            &mut pixels,
            &depth,
            w,
            h,
            project(a),
            project(b),
            rgb,
            mode,
            bias,
        );
    };

    if overlays.grid && !mesh.is_empty() {
        let size = (max[0] - min[0]).max(max[1] - min[1]).max(1.0);
        let step = grid_step(size * 1.4);
        let lo = |v: f32| ((v - size * 0.2) / step).floor() * step;
        let hi = |v: f32| ((v + size * 0.2) / step).ceil() * step;
        let (x0, x1, y0, y1) = (lo(min[0]), hi(max[0]), lo(min[1]), hi(max[1]));
        let bg = opts.background;
        let grid_rgb = [bg[0], bg[1], bg[2]].map(|c| if c > 128 { c - 40 } else { c + 40 });
        let mut x = x0;
        while x <= x1 + step * 0.5 {
            line([x, y0, 0.0], [x, y1, 0.0], grid_rgb, LineDepth::Behind);
            x += step;
        }
        let mut y = y0;
        while y <= y1 + step * 0.5 {
            line([x0, y, 0.0], [x1, y, 0.0], grid_rgb, LineDepth::Behind);
            y += step;
        }
    }
    if let Some(edges) = overlays.edges {
        let edge_rgb = opts.color.map(|c| (c as f32 * 0.18) as u8);
        for [a, b] in edges {
            line(*a, *b, edge_rgb, LineDepth::OnSurface);
        }
    }
    if overlays.axes && !mesh.is_empty() {
        let len = radius.max(1.0) * 1.3;
        line(
            [0.0; 3],
            [len, 0.0, 0.0],
            [0xe0, 0x4b, 0x4b],
            LineDepth::OnSurface,
        );
        line(
            [0.0; 3],
            [0.0, len, 0.0],
            [0x4b, 0xc0, 0x5a],
            LineDepth::OnSurface,
        );
        line(
            [0.0; 3],
            [0.0, 0.0, len],
            [0x4b, 0x7b, 0xe0],
            LineDepth::OnSurface,
        );
    }

    Image {
        width: w,
        height: h,
        pixels,
    }
}

#[derive(Clone, Copy, PartialEq)]
enum LineDepth {
    /// Visible on or in front of surfaces (edges, axes).
    OnSurface,
    /// Only where nothing is in front, even coplanar faces (the grid).
    Behind,
}

fn fill_triangle(
    pixels: &mut [u8],
    depth: &mut [f32],
    w: u32,
    h: u32,
    p: [[f32; 3]; 3],
    rgb: [u8; 3],
) {
    let min_x = p
        .iter()
        .map(|v| v[0])
        .fold(f32::INFINITY, f32::min)
        .floor()
        .max(0.0);
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
        .max(0.0);
    let max_y = p
        .iter()
        .map(|v| v[1])
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil()
        .min((h - 1) as f32);
    if max_x < min_x || max_y < min_y {
        return;
    }
    let area = edge(p[0], p[1], p[2]);
    if area.abs() < f32::EPSILON {
        return;
    }
    for y in min_y as u32..=max_y as u32 {
        for x in min_x as u32..=max_x as u32 {
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

/// Depth-tested line between projected points.
#[allow(clippy::too_many_arguments)]
fn draw_line(
    pixels: &mut [u8],
    depth: &[f32],
    w: u32,
    h: u32,
    a: [f32; 3],
    b: [f32; 3],
    rgb: [u8; 3],
    mode: LineDepth,
    bias: f32,
) {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let steps = dx
        .abs()
        .max(dy.abs())
        .ceil()
        .clamp(1.0, 4.0 * (w + h) as f32) as usize;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let (x, y, z) = (a[0] + dx * t, a[1] + dy * t, a[2] + (b[2] - a[2]) * t);
        // Brush grows with the frame so lines survive downscaling.
        let size = (w.max(h) / 400).max(2) as i32;
        for (ox, oy) in (0..size).flat_map(|i| (0..size).map(move |j| (i as f32, j as f32))) {
            let (px, py) = (x + ox - (size / 2) as f32, y + oy - (size / 2) as f32);
            if px < 0.0 || py < 0.0 || px >= w as f32 || py >= h as f32 {
                continue;
            }
            let idx = (py as u32 * w + px as u32) as usize;
            let visible = match mode {
                LineDepth::OnSurface => z <= depth[idx] + bias,
                LineDepth::Behind => z + bias < depth[idx],
            };
            if visible {
                pixels[idx * 4..idx * 4 + 3].copy_from_slice(&rgb);
                pixels[idx * 4 + 3] = 0xff;
            }
        }
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
            colors: Vec::new(),
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

    fn cube() -> Mesh {
        // Unit cube as 12 triangles.
        let v = |x: f32, y: f32, z: f32| [x, y, z];
        let quads = [
            [v(0., 0., 0.), v(1., 0., 0.), v(1., 1., 0.), v(0., 1., 0.)],
            [v(0., 0., 1.), v(1., 0., 1.), v(1., 1., 1.), v(0., 1., 1.)],
            [v(0., 0., 0.), v(1., 0., 0.), v(1., 0., 1.), v(0., 0., 1.)],
            [v(0., 1., 0.), v(1., 1., 0.), v(1., 1., 1.), v(0., 1., 1.)],
            [v(0., 0., 0.), v(0., 1., 0.), v(0., 1., 1.), v(0., 0., 1.)],
            [v(1., 0., 0.), v(1., 1., 0.), v(1., 1., 1.), v(1., 0., 1.)],
        ];
        Mesh {
            colors: Vec::new(),
            triangles: quads
                .iter()
                .flat_map(|q| [[q[0], q[1], q[2]], [q[0], q[2], q[3]]])
                .collect(),
        }
    }

    #[test]
    fn feature_edges_skip_face_diagonals() {
        // 12 cube edges; the 6 quad diagonals are coplanar and skipped.
        assert_eq!(feature_edges(&cube(), 30.0).len(), 12);
    }

    #[test]
    fn overlays_draw_something() {
        let opts = RasterOptions {
            width: 96,
            height: 96,
            ..Default::default()
        };
        let mesh = cube();
        let plain = render(&mesh, &opts);
        let edges = feature_edges(&mesh, 30.0);
        let with = render_with(
            &mesh,
            &Overlays {
                edges: Some(&edges),
                axes: true,
                grid: true,
            },
            &opts,
        );
        assert_ne!(plain.pixels, with.pixels);
        assert_eq!(grid_step(140.0), 10.0);
        assert_eq!(grid_step(14.0), 1.0);
        assert_eq!(grid_step(700.0), 50.0);
    }

    #[test]
    fn png_encoding_has_signature() {
        let img = render(&Mesh::default(), &RasterOptions::default());
        assert_eq!(&img.to_png()[..8], b"\x89PNG\r\n\x1a\n");
    }
}
