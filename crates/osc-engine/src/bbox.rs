//! Bounding box of the final geometry, estimated from OpenSCAD's `.csg` tree.
//!
//! OpenSCAD's preview `--viewall` frames the terms of its normalised CSG
//! tree, which keeps the operands of `intersection()` and of nested
//! `difference()`s, so a 200 mm helper cube shrinks the model to a few
//! pixels. Evaluating the tree ourselves gives a conservative box of what
//! remains: it may be larger than the true result, never smaller.

use std::iter::Peekable;
use std::str::Chars;

/// An axis-aligned box in model coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoundingBox {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

impl BoundingBox {
    fn from_points(points: impl IntoIterator<Item = [f64; 3]>) -> Option<Self> {
        points.into_iter().fold(None, |acc: Option<Self>, p| {
            let b = Self { min: p, max: p };
            Some(acc.map_or(b, |a| a.union(&b)))
        })
    }

    pub fn center(&self) -> [f64; 3] {
        std::array::from_fn(|i| (self.min[i] + self.max[i]) / 2.0)
    }

    /// Half the diagonal, which OpenSCAD's "view all" fits into the view.
    pub fn radius(&self) -> f64 {
        (0..3)
            .map(|i| (self.max[i] - self.min[i]).powi(2))
            .sum::<f64>()
            .sqrt()
            / 2.0
    }

    fn union(&self, o: &Self) -> Self {
        Self {
            min: std::array::from_fn(|i| self.min[i].min(o.min[i])),
            max: std::array::from_fn(|i| self.max[i].max(o.max[i])),
        }
    }

    fn intersect(&self, o: &Self) -> Option<Self> {
        let b = Self {
            min: std::array::from_fn(|i| self.min[i].max(o.min[i])),
            max: std::array::from_fn(|i| self.max[i].min(o.max[i])),
        };
        (0..3).all(|i| b.min[i] <= b.max[i]).then_some(b)
    }

    fn corners(&self) -> impl Iterator<Item = [f64; 3]> + '_ {
        (0..8).map(move |k| {
            std::array::from_fn(|i| {
                if k >> i & 1 == 0 {
                    self.min[i]
                } else {
                    self.max[i]
                }
            })
        })
    }

    fn transform(&self, m: &[[f64; 4]; 3]) -> Self {
        let pts = self.corners().map(|p| {
            std::array::from_fn(|r| m[r][0] * p[0] + m[r][1] * p[1] + m[r][2] * p[2] + m[r][3])
        });
        Self::from_points(pts).unwrap_or(*self)
    }
}

/// The bounding box of the geometry a `.csg` file describes, as OpenSCAD's
/// preview draws it (`#` and `%` objects included). `None` when the file
/// is empty or uses a node whose extent cannot be known without evaluating
/// it (`import`, `text`, `surface`, `resize`, …).
pub fn from_csg(src: &str) -> Option<BoundingBox> {
    let mut p = Parser::new(src);
    let nodes = p.nodes().ok()?;
    p.at_end().then_some(())?;
    let b = union(&nodes).ok()??;
    let finite = b.min.iter().chain(&b.max).all(|v| v.is_finite());
    (finite && b.radius() > 0.0).then_some(b)
}

#[derive(Clone, Debug, PartialEq)]
enum Val {
    Num(f64),
    Vec(Vec<Val>),
    Other,
}

impl Val {
    fn num(&self) -> Option<f64> {
        match self {
            Val::Num(n) => Some(*n),
            _ => None,
        }
    }

    fn nums(&self) -> Option<Vec<f64>> {
        match self {
            Val::Vec(v) => v.iter().map(Val::num).collect(),
            _ => None,
        }
    }

    fn truthy(&self) -> bool {
        matches!(self, Val::Num(n) if *n != 0.0)
    }
}

#[derive(Debug)]
struct Node {
    name: String,
    /// `#` (highlight) or `%` (background): drawn in preview, but not part
    /// of the CSG operation.
    shown_apart: bool,
    background: bool,
    args: Vec<(Option<String>, Val)>,
    children: Vec<Node>,
}

impl Node {
    fn arg(&self, name: &str) -> Option<&Val> {
        self.args
            .iter()
            .find(|(n, _)| n.as_deref() == Some(name))
            .map(|(_, v)| v)
    }

    fn num(&self, name: &str) -> Result<f64, Unknown> {
        self.arg(name).and_then(Val::num).ok_or(Unknown)
    }

    fn flag(&self, name: &str) -> bool {
        self.arg(name).is_some_and(Val::truthy)
    }
}

/// A node or argument whose extent cannot be determined.
#[derive(Debug)]
struct Unknown;

type Extent = Result<Option<BoundingBox>, Unknown>;

fn union(nodes: &[Node]) -> Extent {
    let mut acc: Option<BoundingBox> = None;
    for n in nodes {
        if let Some(b) = extent(n)? {
            acc = Some(acc.map_or(b, |a| a.union(&b)));
        }
    }
    Ok(acc)
}

fn union_of(a: Option<BoundingBox>, b: Option<BoundingBox>) -> Option<BoundingBox> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.union(&b)),
        (a, b) => a.or(b),
    }
}

fn square(min: [f64; 2], max: [f64; 2]) -> Option<BoundingBox> {
    Some(BoundingBox {
        min: [min[0], min[1], 0.0],
        max: [max[0], max[1], 0.0],
    })
}

/// `size` as OpenSCAD accepts it: a number or a vector of `dims` numbers.
fn size(node: &Node, dims: usize) -> Result<Vec<f64>, Unknown> {
    match node.arg("size") {
        Some(Val::Num(n)) => Ok(vec![*n; dims]),
        Some(v) => v.nums().filter(|s| s.len() == dims).ok_or(Unknown),
        None => Ok(vec![1.0; dims]),
    }
}

fn points(node: &Node) -> Result<Vec<[f64; 3]>, Unknown> {
    let Some(Val::Vec(pts)) = node.arg("points") else {
        return Err(Unknown);
    };
    pts.iter()
        .map(|p| match p.nums().as_deref() {
            Some([x, y]) => Ok([*x, *y, 0.0]),
            Some([x, y, z]) => Ok([*x, *y, *z]),
            _ => Err(Unknown),
        })
        .collect()
}

fn extent(node: &Node) -> Extent {
    let children = &node.children;
    let b = match node.name.as_str() {
        "group" | "union" | "color" | "render" | "hull" => union(children)?,
        "multmatrix" => {
            let m = node.args.first().map(|(_, v)| v).ok_or(Unknown)?;
            let Val::Vec(rows) = m else {
                return Err(Unknown);
            };
            let mut mat = [[0.0; 4]; 3];
            for (r, row) in mat.iter_mut().enumerate() {
                let vals = rows.get(r).and_then(Val::nums).ok_or(Unknown)?;
                *row = vals.try_into().map_err(|_| Unknown)?;
            }
            union(children)?.map(|b| b.transform(&mat))
        }
        "difference" => {
            // Only the first operand survives; `#`/`%` operands are still drawn.
            let mut csg = children.iter().filter(|c| !c.background);
            let first = csg.next().map(extent).transpose()?.flatten();
            let mut apart = children.iter().filter(|c| c.shown_apart);
            apart.try_fold(first, |acc, c| Ok(union_of(acc, extent(c)?)))?
        }
        "intersection" => {
            let mut acc: Option<Option<BoundingBox>> = None;
            for c in children.iter().filter(|c| !c.background) {
                let b = extent(c)?;
                acc = Some(match acc {
                    None => b,
                    Some(a) => a.zip(b).and_then(|(a, b)| a.intersect(&b)),
                });
            }
            let mut apart = children.iter().filter(|c| c.shown_apart);
            apart.try_fold(acc.flatten(), |acc, c| Ok(union_of(acc, extent(c)?)))?
        }
        "minkowski" => {
            let mut acc: Option<BoundingBox> = None;
            for c in children {
                let Some(b) = extent(c)? else { continue };
                acc = Some(match acc {
                    None => b,
                    Some(a) => BoundingBox {
                        min: std::array::from_fn(|i| a.min[i] + b.min[i]),
                        max: std::array::from_fn(|i| a.max[i] + b.max[i]),
                    },
                });
            }
            acc
        }
        "cube" => {
            let s = size(node, 3)?;
            let b = BoundingBox {
                min: [0.0; 3],
                max: [s[0], s[1], s[2]],
            };
            if node.flag("center") {
                b.transform(&translation(s.iter().map(|v| -v / 2.0)))
            } else {
                b
            }
            .into()
        }
        "sphere" => {
            let r = node.num("r")?.abs();
            Some(BoundingBox {
                min: [-r; 3],
                max: [r; 3],
            })
        }
        "cylinder" => {
            let h = node.num("h")?;
            let r = node.num("r1")?.abs().max(node.num("r2")?.abs());
            let z0 = if node.flag("center") { -h / 2.0 } else { 0.0 };
            Some(BoundingBox {
                min: [-r, -r, z0],
                max: [r, r, z0 + h],
            })
        }
        "polyhedron" | "polygon" => BoundingBox::from_points(points(node)?),
        "square" => {
            let s = size(node, 2)?;
            if node.flag("center") {
                square([-s[0] / 2.0, -s[1] / 2.0], [s[0] / 2.0, s[1] / 2.0])
            } else {
                square([0.0, 0.0], [s[0], s[1]])
            }
        }
        "circle" => {
            let r = node.num("r")?.abs();
            square([-r, -r], [r, r])
        }
        "linear_extrude" => {
            let h = node.num("height")?;
            let Some(b) = union(children)? else {
                return Ok(None);
            };
            let (sx, sy) = match node.arg("scale") {
                None => (1.0, 1.0),
                Some(Val::Num(s)) => (*s, *s),
                Some(v) => match v.nums().as_deref() {
                    Some([sx, sy]) => (*sx, *sy),
                    _ => return Err(Unknown),
                },
            };
            let top = b.transform(&[
                [sx, 0.0, 0.0, 0.0],
                [0.0, sy, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
            ]);
            let mut xy = b.union(&top);
            if node.arg("twist").and_then(Val::num).unwrap_or(0.0) != 0.0 {
                let r = xy.corners().map(|p| p[0].hypot(p[1])).fold(0.0, f64::max);
                xy = BoundingBox {
                    min: [-r, -r, 0.0],
                    max: [r, r, 0.0],
                };
            }
            let z0 = if node.flag("center") { -h / 2.0 } else { 0.0 };
            Some(BoundingBox {
                min: [xy.min[0], xy.min[1], z0],
                max: [xy.max[0], xy.max[1], z0 + h],
            })
        }
        "rotate_extrude" => union(children)?.map(|b| {
            let r = b.min[0].abs().max(b.max[0].abs());
            BoundingBox {
                min: [-r, -r, b.min[1]],
                max: [r, r, b.max[1]],
            }
        }),
        "projection" => {
            union(children)?.and_then(|b| square([b.min[0], b.min[1]], [b.max[0], b.max[1]]))
        }
        "offset" => {
            let grow = ["r", "delta"]
                .iter()
                .filter_map(|k| node.arg(k).and_then(Val::num))
                .fold(0.0, f64::max);
            union(children)?.and_then(|b| {
                square(
                    [b.min[0] - grow, b.min[1] - grow],
                    [b.max[0] + grow, b.max[1] + grow],
                )
            })
        }
        _ => return Err(Unknown),
    };
    Ok(b)
}

fn translation(t: impl Iterator<Item = f64>) -> [[f64; 4]; 3] {
    let t: Vec<f64> = t.collect();
    [
        [1.0, 0.0, 0.0, t[0]],
        [0.0, 1.0, 0.0, t[1]],
        [0.0, 0.0, 1.0, t[2]],
    ]
}

struct Parser<'a> {
    chars: Peekable<Chars<'a>>,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            chars: src.chars().peekable(),
        }
    }

    fn skip_ws(&mut self) {
        while self.chars.next_if(|c| c.is_whitespace()).is_some() {}
    }

    fn at_end(&mut self) -> bool {
        self.skip_ws();
        self.chars.peek().is_none()
    }

    fn eat(&mut self, c: char) -> bool {
        self.skip_ws();
        self.chars.next_if_eq(&c).is_some()
    }

    fn expect(&mut self, c: char) -> Result<(), Unknown> {
        self.eat(c).then_some(()).ok_or(Unknown)
    }

    fn word(&mut self) -> String {
        self.skip_ws();
        let mut s = String::new();
        while let Some(c) = self
            .chars
            .next_if(|c| c.is_alphanumeric() || matches!(c, '_' | '$' | '.' | '+' | '-'))
        {
            s.push(c);
        }
        s
    }

    /// Statements until `}` or the end of input.
    fn nodes(&mut self) -> Result<Vec<Node>, Unknown> {
        let mut nodes = Vec::new();
        loop {
            self.skip_ws();
            match self.chars.peek() {
                None | Some('}') => return Ok(nodes),
                _ => {}
            }
            if self.eat(';') {
                continue;
            }
            let (mut shown_apart, mut background) = (false, false);
            loop {
                if self.eat('#') {
                    shown_apart = true;
                } else if self.eat('%') {
                    shown_apart = true;
                    background = true;
                } else if !self.eat('!') {
                    break;
                }
            }
            let name = self.word();
            if name.is_empty() {
                return Err(Unknown);
            }
            self.expect('(')?;
            let mut args = Vec::new();
            if !self.eat(')') {
                loop {
                    args.push(self.arg()?);
                    if self.eat(')') {
                        break;
                    }
                    self.expect(',')?;
                }
            }
            let children = if self.eat('{') {
                let c = self.nodes()?;
                self.expect('}')?;
                c
            } else {
                self.expect(';')?;
                Vec::new()
            };
            nodes.push(Node {
                name,
                shown_apart,
                background,
                args,
                children,
            });
        }
    }

    fn arg(&mut self) -> Result<(Option<String>, Val), Unknown> {
        self.skip_ws();
        let starts_name = self
            .chars
            .peek()
            .is_some_and(|c| c.is_alphabetic() || matches!(c, '_' | '$'));
        if starts_name {
            let word = self.word();
            if self.eat('=') {
                return Ok((Some(word), self.value()?));
            }
            return Ok((None, word_value(&word)));
        }
        Ok((None, self.value()?))
    }

    fn value(&mut self) -> Result<Val, Unknown> {
        self.skip_ws();
        match self.chars.peek() {
            Some('[') => {
                self.chars.next();
                let mut items = Vec::new();
                if !self.eat(']') {
                    loop {
                        items.push(self.value()?);
                        if self.eat(']') {
                            break;
                        }
                        self.expect(',')?;
                    }
                }
                Ok(Val::Vec(items))
            }
            Some('"') => {
                self.chars.next();
                while let Some(c) = self.chars.next() {
                    match c {
                        '"' => return Ok(Val::Other),
                        '\\' => {
                            self.chars.next();
                        }
                        _ => {}
                    }
                }
                Err(Unknown)
            }
            _ => {
                let word = self.word();
                if word.is_empty() {
                    return Err(Unknown);
                }
                Ok(word_value(&word))
            }
        }
    }
}

fn word_value(word: &str) -> Val {
    match word {
        "true" => Val::Num(1.0),
        "false" => Val::Num(0.0),
        _ => word.parse().map(Val::Num).unwrap_or(Val::Other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bb(src: &str) -> Option<([f64; 3], [f64; 3])> {
        from_csg(src).map(|b| {
            let r = |v: [f64; 3]| v.map(|x| (x * 1e6).round() / 1e6);
            (r(b.min), r(b.max))
        })
    }

    #[test]
    fn difference_keeps_the_first_operand() {
        let src = "difference() {\n\tcube(size = [20, 10, 5], center = false);\n\tmultmatrix([[1, 0, 0, -100], [0, 1, 0, -100], [0, 0, 1, 3], [0, 0, 0, 1]]) {\n\t\tcube(size = [200, 200, 200], center = false);\n\t}\n}\n";
        assert_eq!(bb(src), Some(([0.0; 3], [20.0, 10.0, 5.0])));
    }

    #[test]
    fn highlighted_and_background_objects_are_drawn() {
        let src = "difference() {\n\tcube(size = 10, center = true);\n#\tsphere($fn = 0, r = 8);\n}\n%cube(size = [1, 1, 1], center = false);\n";
        assert_eq!(bb(src), Some(([-8.0; 3], [8.0; 3])));
        let src = "difference() {\n%\tsphere(r = 1);\n\tcube(size = [2, 2, 2], center = false);\n\tcube(size = [9, 9, 9], center = false);\n}\n";
        assert_eq!(bb(src), Some(([-1.0; 3], [2.0; 3])));
    }

    #[test]
    fn intersection_and_minkowski() {
        let src = "intersection() {\n\tsphere($fn = 16, $fa = 12, $fs = 2, r = 5);\n\tcylinder($fn = 16, h = 20, r1 = 3, r2 = 1, center = true);\n}\n";
        assert_eq!(bb(src), Some(([-3.0, -3.0, -5.0], [3.0, 3.0, 5.0])));
        let src = "intersection() { cube(size = 1); multmatrix([[1, 0, 0, 5], [0, 1, 0, 0], [0, 0, 1, 0], [0, 0, 0, 1]]) { cube(size = 1); } }\ncube(size = [2, 3, 4]);";
        assert_eq!(bb(src), Some(([0.0; 3], [2.0, 3.0, 4.0])));
        let src = "minkowski(convexity = 0) {\n\tcube(size = [1, 1, 1], center = false);\n\tsphere(r = 0.5);\n}\n";
        assert_eq!(bb(src), Some(([-0.5; 3], [1.5; 3])));
    }

    #[test]
    fn rotations_bound_the_rotated_box() {
        // rotate([0, 0, 90]) of a 10 x 2 x 1 cube.
        let src = "multmatrix([[0, -1, 0, 0], [1, 0, 0, 0], [0, 0, 1, 0], [0, 0, 0, 1]]) {\n\tcube(size = [10, 2, 1], center = false);\n}\n";
        assert_eq!(bb(src), Some(([-2.0, 0.0, 0.0], [0.0, 10.0, 1.0])));
    }

    #[test]
    fn extrusions_and_2d() {
        let src = "linear_extrude(height = 4, center = true, convexity = 1, twist = 0, scale = [2, 1], $fn = 16) {\n\tsquare(size = [2, 3], center = false);\n}\n";
        assert_eq!(bb(src), Some(([0.0, 0.0, -2.0], [4.0, 3.0, 2.0])));
        let src = "rotate_extrude(angle = 360, convexity = 2) {\n\tmultmatrix([[1, 0, 0, 5], [0, 1, 0, 0], [0, 0, 1, 0], [0, 0, 0, 1]]) {\n\t\tcircle(r = 1);\n\t}\n}\n";
        assert_eq!(bb(src), Some(([-6.0, -6.0, -1.0], [6.0, 6.0, 1.0])));
        let src = "offset(r = 1, $fn = 16) {\n\tpolygon(points = [[0, 0], [1, 0], [0, 1]], paths = undef, convexity = 1);\n}\n";
        assert_eq!(bb(src), Some(([-1.0, -1.0, 0.0], [2.0, 2.0, 0.0])));
        let src = "polyhedron(points = [[0, 0, 0], [1, 0, 0], [0, 1, 0], [0, 0, -1]], faces = [[0, 1, 2], [0, 1, 3], [0, 2, 3], [1, 2, 3]], convexity = 1);";
        assert_eq!(bb(src), Some(([0.0, 0.0, -1.0], [1.0, 1.0, 0.0])));
    }

    #[test]
    fn unknown_or_empty_geometry_is_none() {
        assert_eq!(
            bb("cube(size = 1);\nimport(file = \"a.stl\", layer = \"\");\n"),
            None
        );
        assert_eq!(
            bb("text(text = \"hi\", size = 10, font = \"\", $fn = 0);\n"),
            None
        );
        assert_eq!(
            bb("resize(newsize = [2,2,2], auto = [0,0,0]) { cube(size = 1); }"),
            None
        );
        assert_eq!(bb(""), None);
        assert_eq!(bb("group();\n"), None);
        assert_eq!(bb("cube(size = [1, 1, 1]"), None);
        // Unknown nodes do not matter where they are subtracted away.
        let src = "difference() { cube(size = 1); text(text = \"x\"); }";
        assert_eq!(bb(src), Some(([0.0; 3], [1.0; 3])));
    }
}
