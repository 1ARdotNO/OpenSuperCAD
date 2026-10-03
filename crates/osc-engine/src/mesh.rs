//! Triangle meshes loaded from OpenSCAD exports.

use std::path::Path;

pub type Vec3 = [f32; 3];

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    pub triangles: Vec<[Vec3; 3]>,
    /// Per-triangle RGBA colours (empty when the source has none).
    pub colors: Vec<[u8; 4]>,
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
            colors: Vec::new(),
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
        Mesh {
            triangles,
            colors: Vec::new(),
        }
    }

    /// Load a 3MF file (as written by OpenSCAD), keeping `color()` as
    /// per-triangle colours from `basematerials`/`colorgroup` resources.
    pub fn load_3mf(path: &Path) -> Result<Self, MeshError> {
        let file = std::fs::File::open(path)?;
        let mut archive =
            zip::ZipArchive::new(file).map_err(|_| MeshError::Malformed("not a 3MF (zip) file"))?;
        let mut xml = String::new();
        for i in 0..archive.len() {
            let mut entry = archive
                .by_index(i)
                .map_err(|_| MeshError::Malformed("unreadable 3MF entry"))?;
            if entry.name().ends_with(".model") {
                std::io::Read::read_to_string(&mut entry, &mut xml)?;
                break;
            }
        }
        if xml.is_empty() {
            return Err(MeshError::Malformed("3MF has no model part"));
        }
        Self::parse_3mf_model(&xml)
    }

    /// Parse the XML model part of a 3MF package.
    pub fn parse_3mf_model(xml: &str) -> Result<Self, MeshError> {
        use quick_xml::events::Event;
        use std::collections::HashMap;

        let mut reader = quick_xml::Reader::from_str(xml);
        // Property groups (basematerials / colorgroup) by id.
        let mut groups: HashMap<String, Vec<[u8; 4]>> = HashMap::new();
        let mut current_group: Option<(String, Vec<[u8; 4]>)> = None;
        let mut vertices: Vec<Vec3> = Vec::new();
        let mut object_default: Option<(String, usize)> = None;
        let mut mesh = Mesh::default();
        let mut any_color = false;

        loop {
            let event = reader
                .read_event()
                .map_err(|_| MeshError::Malformed("invalid 3MF XML"))?;
            let (e, is_start) = match &event {
                Event::Start(e) => (e, true),
                Event::Empty(e) => (e, false),
                Event::End(e) => {
                    let name = e.local_name();
                    match name.into_inner() {
                        "basematerials" | "colorgroup" => {
                            if let Some((id, colors)) = current_group.take() {
                                groups.insert(id, colors);
                            }
                        }
                        "object" => {
                            vertices.clear();
                            object_default = None;
                        }
                        _ => {}
                    }
                    continue;
                }
                Event::Eof => break,
                _ => continue,
            };
            let attrs: HashMap<String, String> = e
                .attributes()
                .flatten()
                .map(|a| {
                    (
                        a.key.local_name().into_inner().to_owned(),
                        a.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                            .map(|v| v.into_owned())
                            .unwrap_or_default(),
                    )
                })
                .collect();
            match e.local_name().into_inner() {
                "basematerials" | "colorgroup" if is_start => {
                    current_group = attrs.get("id").map(|id| (id.clone(), Vec::new()));
                }
                "base" | "color" => {
                    let value = attrs.get("displaycolor").or_else(|| attrs.get("color"));
                    if let (Some((_, colors)), Some(v)) = (&mut current_group, value) {
                        colors.push(parse_hex_color(v).unwrap_or([0xf9, 0xd7, 0x2c, 0xff]));
                    }
                }
                "object" => {
                    vertices.clear();
                    object_default = attrs.get("pid").map(|pid| {
                        let ix = attrs
                            .get("pindex")
                            .and_then(|p| p.parse().ok())
                            .unwrap_or(0);
                        (pid.clone(), ix)
                    });
                }
                "vertex" => {
                    let c = |k: &str| {
                        attrs
                            .get(k)
                            .and_then(|v| v.parse::<f32>().ok())
                            .unwrap_or(0.0)
                    };
                    vertices.push([c("x"), c("y"), c("z")]);
                }
                "triangle" => {
                    let ix = |k: &str| attrs.get(k).and_then(|v| v.parse::<usize>().ok());
                    let (Some(a), Some(b), Some(c)) = (ix("v1"), ix("v2"), ix("v3")) else {
                        return Err(MeshError::Malformed("triangle without vertices"));
                    };
                    let get = |i: usize| {
                        vertices
                            .get(i)
                            .copied()
                            .ok_or(MeshError::Malformed("vertex index out of range"))
                    };
                    mesh.triangles.push([get(a)?, get(b)?, get(c)?]);
                    let prop = match (attrs.get("pid"), ix("p1")) {
                        (Some(pid), Some(p1)) => Some((pid.clone(), p1)),
                        _ => object_default.clone(),
                    };
                    let color =
                        prop.and_then(|(pid, i)| groups.get(&pid).and_then(|g| g.get(i)).copied());
                    any_color |= color.is_some();
                    mesh.colors.push(color.unwrap_or([0xf9, 0xd7, 0x2c, 0xff]));
                }
                _ => {}
            }
        }
        if !any_color {
            mesh.colors.clear();
        }
        Ok(mesh)
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

/// `#RRGGBB` or `#RRGGBBAA`.
fn parse_hex_color(s: &str) -> Option<[u8; 4]> {
    let hex = s.trim().strip_prefix('#')?;
    let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
    match hex.len() {
        6 => Some([byte(0)?, byte(2)?, byte(4)?, 0xff]),
        8 => Some([byte(0)?, byte(2)?, byte(4)?, byte(6)?]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODEL: &str = r##"<?xml version="1.0" encoding="utf-8"?>
<model xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02" unit="millimeter">
  <resources>
    <basematerials id="1">
      <base name="Default" displaycolor="#F9D72CFF"/>
      <base name="Color 1" displaycolor="#FF0000FF"/>
    </basematerials>
    <object id="2" type="model" pid="1" pindex="0">
      <mesh>
        <vertices>
          <vertex x="0" y="0" z="0" /><vertex x="1" y="0" z="0" /><vertex x="0" y="1" z="0" /><vertex x="0" y="0" z="2.5" />
        </vertices>
        <triangles>
          <triangle v1="0" v2="1" v3="2" pid="1" p1="1" />
          <triangle v1="0" v2="1" v3="3" />
        </triangles>
      </mesh>
    </object>
  </resources>
  <build><item objectid="2"/></build>
</model>"##;

    #[test]
    fn parses_3mf_with_colors() {
        let m = Mesh::parse_3mf_model(MODEL).unwrap();
        assert_eq!(m.triangles.len(), 2);
        assert_eq!(m.colors, [[0xff, 0, 0, 0xff], [0xf9, 0xd7, 0x2c, 0xff]]);
        assert_eq!(m.bounds().1, [1.0, 1.0, 2.5]);
        assert!(
            Mesh::parse_3mf_model("<model><triangle v1=\"0\" v2=\"1\" v3=\"9\"/></model>").is_err()
        );
        assert_eq!(parse_hex_color("#0000ff"), Some([0, 0, 255, 255]));
    }

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
