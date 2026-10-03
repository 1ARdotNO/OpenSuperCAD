//! OpenSCAD language support for OpenSuperCAD.
//!
//! This crate is deliberately dependency-light: it provides a lossless lexer
//! ([`lex`]), a document outline ([`outline`]) and full support for the
//! OpenSCAD *Customizer* conventions ([`customizer`]) so that both the UI and
//! the agent-facing MCP tools can read and tweak design parameters without
//! invoking OpenSCAD itself.

pub mod customizer;
mod lexer;
mod outline;

pub use lexer::{Token, TokenKind, lex};
pub use outline::{Symbol, SymbolKind, outline};

/// Reserved words of the OpenSCAD language.
pub const KEYWORDS: &[&str] = &[
    "module",
    "function",
    "if",
    "else",
    "for",
    "let",
    "each",
    "include",
    "use",
    "assert",
    "echo",
    "intersection_for",
    "true",
    "false",
    "undef",
];

/// Built-in modules and functions, used for highlighting and completion.
pub const BUILTINS: &[&str] = &[
    // 3D
    "cube",
    "sphere",
    "cylinder",
    "polyhedron",
    "import",
    "surface",
    // 2D
    "square",
    "circle",
    "polygon",
    "text",
    "projection",
    // transforms
    "translate",
    "rotate",
    "scale",
    "resize",
    "mirror",
    "multmatrix",
    "color",
    "offset",
    "hull",
    "minkowski",
    "linear_extrude",
    "rotate_extrude",
    "render",
    "children",
    // booleans
    "union",
    "difference",
    "intersection",
    // functions
    "abs",
    "sign",
    "sin",
    "cos",
    "tan",
    "asin",
    "acos",
    "atan",
    "atan2",
    "floor",
    "round",
    "ceil",
    "ln",
    "log",
    "pow",
    "sqrt",
    "exp",
    "len",
    "min",
    "max",
    "norm",
    "cross",
    "concat",
    "lookup",
    "str",
    "chr",
    "ord",
    "search",
    "version",
    "version_num",
    "parent_module",
    "rands",
    "is_undef",
    "is_bool",
    "is_num",
    "is_string",
    "is_list",
    "is_function",
];
