//! Triangle meshes loaded from OpenSCAD exports.

use std::path::Path;

pub type Vec3 = [f32; 3];

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    pub triangles: Vec<[Vec3; 3]>,
}

#[derive(Debug, thiserror::Error)]
pub enum MeshError {
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("malformed STL: {0}")]
    Malformed(&'static str),
}

impl Mesh {
    pub fn load_stl(path: &Path) -> Result<Self, MeshError> {
        Self::parse_stl(&std::fs::read(path)?)
    }

    /// Parse binary or ASCII STL.
    pub fn parse_stl(data: &[u8]) -> Result<Self, MeshError> {
        if data.len() >= 84 {
            let count = u32::from_le_bytes([data[80], data[81], data[82], data[83]]) as usize;
            if count.checked_mul(50).and_then(|n| n.checked_add(84)) == Some(data.len()) {
                return Ok(Self::parse_binary(&data[84..], count));
            }
        }
        let text = std::str::from_utf8(data).map_err(|_| MeshError::Malformed("not UTF-8"))?;
        if !text.trim_start().starts_with("solid") {
            return Err(MeshError::Malformed("neither binary nor ASCII STL"));
        }
        let mut vertices = Vec::new();
        for line in text.lines() {
            let mut parts = line.split_whitespace();
            if parts.next() == Some("vertex") {
                let mut v = [0.0f32; 3];
                for c in &mut v {
                    *c = parts
                        .next()
                        .and_then(|s| s.parse().ok())
                        .ok_or(MeshError::Malformed("bad vertex"))?;
                }
                vertices.push(v);
            }
        }
        if vertices.len() % 3 != 0 {
            return Err(MeshError::Malformed("vertex count not a multiple of 3"));
        }
        Ok(Mesh {
            triangles: vertices
                .chunks_exact(3)
                .map(|t| [t[0], t[1], t[2]])
                .collect(),
        })
    }

    fn parse_binary(body: &[u8], count: usize) -> Self {
        let f = |b: &[u8]| f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        let triangles = body
            .chunks_exact(50)
            .take(count)
            .map(|rec| {
                let v = |i: usize| {
                    let o = 12 + i * 12;
                    [f(&rec[o..]), f(&rec[o + 4..]), f(&rec[o + 8..])]
                };
                [v(0), v(1), v(2)]
            })
            .collect();
        Mesh { triangles }
    }

    pub fn is_empty(&self) -> bool {
        self.triangles.is_empty()
    }

    /// Axis-aligned bounding box `(min, max)`; zero-sized for an empty mesh.
    pub fn bounds(&self) -> (Vec3, Vec3) {
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for v in self.triangles.iter().flatten() {
            for i in 0..3 {
                min[i] = min[i].min(v[i]);
                max[i] = max[i].max(v[i]);
            }
        }
        if self.is_empty() {
            return ([0.0; 3], [0.0; 3]);
        }
        (min, max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn tetra_ascii() -> &'static str {
        "solid t\n\
         facet normal 0 0 0\n outer loop\n  vertex 0 0 0\n  vertex 1 0 0\n  vertex 0 1 0\n endloop\nendfacet\n\
         facet normal 0 0 0\n outer loop\n  vertex 0 0 0\n  vertex 0 1 0\n  vertex 0 0 1\n endloop\nendfacet\n\
         facet normal 0 0 0\n outer loop\n  vertex 0 0 0\n  vertex 0 0 1\n  vertex 1 0 0\n endloop\nendfacet\n\
         facet normal 0 0 0\n outer loop\n  vertex 1 0 0\n  vertex 0 0 1\n  vertex 0 1 0\n endloop\nendfacet\n\
         endsolid t\n"
    }

    #[test]
    fn ascii_and_binary_agree() {
        let ascii = Mesh::parse_stl(tetra_ascii().as_bytes()).unwrap();
        assert_eq!(ascii.triangles.len(), 4);

        let mut bin = vec![0u8; 80];
        bin.extend_from_slice(&(ascii.triangles.len() as u32).to_le_bytes());
        for tri in &ascii.triangles {
            bin.extend_from_slice(&[0u8; 12]);
            for v in tri {
                for c in v {
                    bin.extend_from_slice(&c.to_le_bytes());
                }
            }
            bin.extend_from_slice(&[0u8; 2]);
        }
        assert_eq!(Mesh::parse_stl(&bin).unwrap(), ascii);
        assert_eq!(ascii.bounds(), ([0.0; 3], [1.0; 3]));
    }

    #[test]
    fn rejects_garbage() {
        assert!(Mesh::parse_stl(b"hello").is_err());
    }
}
