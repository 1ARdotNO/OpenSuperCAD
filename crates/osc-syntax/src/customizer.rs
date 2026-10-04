//! OpenSCAD *Customizer* support.
//!
//! Follows the conventions documented for the OpenSCAD customizer:
//!
//! * Parameters are top-level assignments of a literal value that appear
//!   before the first `module` or `function` definition.
//! * `/* [Group Name] */` starts a group; `[Hidden]` hides what follows and
//!   `[Global]` parameters are shown in every group.
//! * A `// comment` on the line directly above an assignment is its
//!   description.
//! * A trailing `// …` comment on the same line is the widget annotation:
//!   `[min:max]` / `[min:step:max]` slider, `[a, b, c]` dropdown,
//!   `[10:Small, 20:Large]` labelled dropdown, `// 0.5` spin-box step for
//!   numbers or `// 8` maximum length for strings.

use std::fmt;
use std::ops::Range;

use serde::{Deserialize, Serialize};

use crate::{TokenKind, lex};

/// A literal parameter value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Bool(bool),
    Number(f64),
    String(String),
    Vector(Vec<f64>),
}

impl Value {
    /// Render the value as OpenSCAD source.
    pub fn to_scad(&self) -> String {
        match self {
            Value::Bool(b) => b.to_string(),
            Value::Number(n) => format_number(*n),
            Value::String(s) => {
                let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
                format!("\"{escaped}\"")
            }
            Value::Vector(v) => {
                let items: Vec<_> = v.iter().map(|n| format_number(*n)).collect();
                format!("[{}]", items.join(", "))
            }
        }
    }

    fn type_name(&self) -> &'static str {
        match self {
            Value::Bool(_) => "bool",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Vector(_) => "vector",
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_scad())
    }
}

fn format_number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// One option of a dropdown widget.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Choice {
    pub value: Value,
    pub label: String,
}

/// How the UI should present a parameter.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Widget {
    /// Spin box / text field / checkbox / vector editor depending on the value.
    Default,
    Slider {
        min: f64,
        max: f64,
        step: Option<f64>,
    },
    Dropdown {
        choices: Vec<Choice>,
    },
    SpinBox {
        step: f64,
    },
    Text {
        max_length: usize,
    },
}

/// A customizer parameter discovered in a source file.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Parameter {
    pub name: String,
    pub value: Value,
    pub group: Option<String>,
    pub description: Option<String>,
    pub widget: Widget,
    pub hidden: bool,
    /// 1-based line of the assignment.
    pub line: usize,
    /// Byte range of the value expression, used for in-place rewriting.
    #[serde(skip)]
    pub value_span: Range<usize>,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum Error {
    #[error("no customizer parameter named `{0}`")]
    UnknownParameter(String),
    #[error("parameter `{name}` is a {expected}, got a {got}")]
    TypeMismatch {
        name: String,
        expected: &'static str,
        got: &'static str,
    },
}

/// Extract the customizer parameters of a file.
pub fn parameters(source: &str) -> Vec<Parameter> {
    let tokens = lex(source);
    let line_of = |offset: usize| source[..offset].matches('\n').count() + 1;
    let mut params = Vec::new();
    let mut group: Option<String> = None;
    // (line the comment is on, text)
    let mut pending_description: Option<(usize, String)> = None;
    let mut i = 0;

    while i < tokens.len() {
        let tok = &tokens[i];
        let text = tok.text(source);
        match tok.kind {
            TokenKind::Whitespace => {}
            TokenKind::BlockComment => {
                let inner = text.trim_start_matches("/*").trim_end_matches("*/").trim();
                if let Some(name) = inner.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                    group = Some(name.trim().to_owned());
                }
            }
            TokenKind::LineComment => {
                let body = text.trim_start_matches('/').trim().to_owned();
                pending_description = Some((line_of(tok.span.start), body));
            }
            TokenKind::Keyword if text == "module" || text == "function" => break,
            // `use <file>` / `include <file>` have no terminating `;`.
            TokenKind::Keyword if text == "use" || text == "include" => {
                while i + 1 < tokens.len()
                    && !(tokens[i].kind == TokenKind::Operator && tokens[i].text(source) == ">")
                    && !tokens[i + 1].text(source).contains('\n')
                {
                    i += 1;
                }
                pending_description = None;
            }
            TokenKind::Identifier | TokenKind::Builtin | TokenKind::SpecialVariable => {
                let next = next_significant(&tokens, i + 1);
                let is_assign = next
                    .map(|n| tokens[n].kind == TokenKind::Operator && tokens[n].text(source) == "=")
                    .unwrap_or(false);
                let end = statement_end(&tokens, source, i);
                if is_assign && tok.kind != TokenKind::SpecialVariable {
                    let eq = next.unwrap_or(i);
                    let value_tokens: Vec<_> = tokens[eq + 1..end.min(tokens.len())]
                        .iter()
                        .filter(|t| {
                            !matches!(
                                t.kind,
                                TokenKind::Whitespace
                                    | TokenKind::LineComment
                                    | TokenKind::BlockComment
                            )
                        })
                        .collect();
                    if let (Some(first), Some(last)) = (value_tokens.first(), value_tokens.last())
                        && let Some(value) = parse_literal(&source[first.span.start..last.span.end])
                    {
                        let line = line_of(tok.span.start);
                        let annotation = trailing_comment(&tokens, source, end);
                        let widget = annotation
                            .as_deref()
                            .map(|a| parse_widget(a, &value))
                            .unwrap_or(Widget::Default);
                        let description = pending_description
                            .take()
                            .filter(|(l, _)| *l + 1 == line)
                            .map(|(_, d)| d)
                            .filter(|d| !d.is_empty());
                        let hidden = group
                            .as_deref()
                            .is_some_and(|g| g.eq_ignore_ascii_case("hidden"));
                        params.push(Parameter {
                            name: text.to_owned(),
                            value,
                            group: group.clone(),
                            description,
                            widget,
                            hidden,
                            line,
                            value_span: first.span.start..last.span.end,
                        });
                    }
                }
                // Skip past the statement (and its trailing comment, which is
                // the annotation rather than the next description).
                i = end;
                if let Some(c) = trailing_comment_index(&tokens, source, end) {
                    i = c;
                }
                pending_description = None;
            }
            _ => {
                i = statement_end(&tokens, source, i);
                pending_description = None;
            }
        }
        i += 1;
    }
    params
}

/// Rewrite the value of parameter `name` in `source`, keeping everything else
/// (comments, annotations, formatting) intact.
pub fn set_parameter(source: &str, name: &str, value: &Value) -> Result<String, Error> {
    let param = parameters(source)
        .into_iter()
        .find(|p| p.name == name)
        .ok_or_else(|| Error::UnknownParameter(name.to_owned()))?;
    let compatible = matches!(
        (&param.value, value),
        (Value::Bool(_), Value::Bool(_))
            | (Value::Number(_), Value::Number(_))
            | (Value::String(_), Value::String(_))
            | (Value::Vector(_), Value::Vector(_))
    );
    if !compatible {
        return Err(Error::TypeMismatch {
            name: name.to_owned(),
            expected: param.value.type_name(),
            got: value.type_name(),
        });
    }
    let mut out = String::with_capacity(source.len());
    out.push_str(&source[..param.value_span.start]);
    out.push_str(&value.to_scad());
    out.push_str(&source[param.value_span.end..]);
    Ok(out)
}

fn next_significant(tokens: &[crate::Token], mut i: usize) -> Option<usize> {
    while i < tokens.len() {
        if !matches!(
            tokens[i].kind,
            TokenKind::Whitespace | TokenKind::LineComment | TokenKind::BlockComment
        ) {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Index of the token that ends the statement starting at `i`: the `;` at
/// nesting depth zero, or the matching `}` of a block.
fn statement_end(tokens: &[crate::Token], source: &str, mut i: usize) -> usize {
    let mut depth = 0i32;
    while i < tokens.len() {
        if tokens[i].kind == TokenKind::Punctuation {
            match tokens[i].text(source) {
                "(" | "[" | "{" => depth += 1,
                ")" | "]" => depth -= 1,
                "}" => {
                    depth -= 1;
                    if depth <= 0 {
                        return i;
                    }
                }
                ";" if depth <= 0 => return i,
                _ => {}
            }
        }
        i += 1;
    }
    tokens.len()
}

fn trailing_comment_index(tokens: &[crate::Token], source: &str, end: usize) -> Option<usize> {
    let mut j = end + 1;
    while j < tokens.len() {
        match tokens[j].kind {
            TokenKind::Whitespace if !tokens[j].text(source).contains('\n') => j += 1,
            TokenKind::LineComment => return Some(j),
            _ => return None,
        }
    }
    None
}

fn trailing_comment(tokens: &[crate::Token], source: &str, end: usize) -> Option<String> {
    trailing_comment_index(tokens, source, end).map(|j| {
        tokens[j]
            .text(source)
            .trim_start_matches('/')
            .trim()
            .to_owned()
    })
}

fn parse_number(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok().filter(|n| n.is_finite())
}

fn parse_literal(expr: &str) -> Option<Value> {
    let expr = expr.trim();
    match expr {
        "true" => return Some(Value::Bool(true)),
        "false" => return Some(Value::Bool(false)),
        _ => {}
    }
    if let Some(n) = parse_number(expr) {
        return Some(Value::Number(n));
    }
    if expr.len() >= 2 && expr.starts_with('"') && expr.ends_with('"') {
        let inner = &expr[1..expr.len() - 1];
        let mut out = String::new();
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.next() {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some(other) => out.push(other),
                    None => {}
                }
            } else if c == '"' {
                return None; // concatenation or expression, not a literal
            } else {
                out.push(c);
            }
        }
        return Some(Value::String(out));
    }
    if let Some(inner) = expr.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        if inner.trim().is_empty() {
            return Some(Value::Vector(Vec::new()));
        }
        return inner
            .split(',')
            .map(parse_number)
            .collect::<Option<Vec<_>>>()
            .map(Value::Vector);
    }
    None
}

fn parse_widget(annotation: &str, value: &Value) -> Widget {
    let annotation = annotation.trim();
    if let Some(inner) = annotation
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
    {
        // Slider: [max], [min:max] or [min:step:max] — all-numeric, colon separated.
        if !inner.contains(',') && matches!(value, Value::Number(_) | Value::Vector(_)) {
            let parts: Option<Vec<f64>> = inner.split(':').map(parse_number).collect();
            match parts.as_deref() {
                Some([max]) => {
                    return Widget::Slider {
                        min: 0.0,
                        max: *max,
                        step: None,
                    };
                }
                Some([min, max]) => {
                    return Widget::Slider {
                        min: *min,
                        max: *max,
                        step: None,
                    };
                }
                Some([min, step, max]) => {
                    return Widget::Slider {
                        min: *min,
                        max: *max,
                        step: Some(*step),
                    };
                }
                _ => {}
            }
        }
        // Dropdown: comma separated, each entry optionally `value:label`.
        let choices = inner
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|entry| {
                let (raw, label) = match entry.split_once(':') {
                    Some((v, l)) => (v.trim(), l.trim().to_owned()),
                    None => (entry, entry.to_owned()),
                };
                let value = match value {
                    Value::Number(_) => parse_number(raw)
                        .map(Value::Number)
                        .unwrap_or_else(|| Value::String(raw.to_owned())),
                    _ => Value::String(raw.trim_matches('"').to_owned()),
                };
                Choice { value, label }
            })
            .collect::<Vec<_>>();
        if !choices.is_empty() {
            return Widget::Dropdown { choices };
        }
        return Widget::Default;
    }
    match (value, parse_number(annotation)) {
        (Value::Number(_), Some(step)) if step > 0.0 => Widget::SpinBox { step },
        (Value::String(_), Some(len)) if len >= 0.0 => Widget::Text {
            max_length: len as usize,
        },
        _ => Widget::Default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = r#"// Overall width of the box
width = 40; // [10:100]
height = 20.5; // [5:0.5:50]
/* [Style] */
// Corner style
style = "round"; // [round:Rounded, sharp:Sharp]
wall = 2; // [1, 2, 3]
label = "Hi"; // 8
fillet = 0.4; // 0.1
lid = true;
size = [10, 20, 30];
derived = width * 2;
/* [Hidden] */
$fn = 64;
eps = 0.01;

module box() { inner = 3; cube([width, height, 1]); }
after = 5;
"#;

    #[test]
    fn extracts_parameters() {
        let p = parameters(SRC);
        let names: Vec<_> = p.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "width", "height", "style", "wall", "label", "fillet", "lid", "size", "eps"
            ]
        );

        assert_eq!(
            p[0].description.as_deref(),
            Some("Overall width of the box")
        );
        assert_eq!(p[0].group, None);
        assert_eq!(
            p[0].widget,
            Widget::Slider {
                min: 10.0,
                max: 100.0,
                step: None
            }
        );
        assert_eq!(
            p[1].description, None,
            "annotation must not become a description"
        );
        assert_eq!(
            p[1].widget,
            Widget::Slider {
                min: 5.0,
                max: 50.0,
                step: Some(0.5)
            }
        );
        assert_eq!(p[2].group.as_deref(), Some("Style"));
        assert_eq!(p[2].description.as_deref(), Some("Corner style"));
        match &p[2].widget {
            Widget::Dropdown { choices } => {
                assert_eq!(choices[1].value, Value::String("sharp".into()));
                assert_eq!(choices[1].label, "Sharp");
            }
            other => panic!("unexpected widget {other:?}"),
        }
        match &p[3].widget {
            Widget::Dropdown { choices } => assert_eq!(choices[2].value, Value::Number(3.0)),
            other => panic!("unexpected widget {other:?}"),
        }
        assert_eq!(p[4].widget, Widget::Text { max_length: 8 });
        assert_eq!(p[5].widget, Widget::SpinBox { step: 0.1 });
        assert_eq!(p[6].value, Value::Bool(true));
        assert_eq!(p[7].value, Value::Vector(vec![10.0, 20.0, 30.0]));
        assert!(p[8].hidden);
        assert_eq!(p[8].line, 15);
    }

    #[test]
    fn use_and_include_lines_do_not_hide_parameters() {
        let src = "use <parts.scad>\ninclude <lib/x.scad>\n\nsize = 20; // [5:50]\npeg(size);\n";
        let p = parameters(src);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].name, "size");
        assert_eq!(p[0].line, 4);
    }

    #[test]
    fn rewrites_values_in_place() {
        let out = set_parameter(SRC, "width", &Value::Number(55.0)).unwrap();
        assert!(out.contains("width = 55; // [10:100]"));
        let out = set_parameter(&out, "style", &Value::String("sh\"arp".into())).unwrap();
        assert!(out.contains(r#"style = "sh\"arp"; // [round"#));
        assert_eq!(
            parameters(&out)
                .iter()
                .find(|p| p.name == "style")
                .unwrap()
                .value,
            Value::String("sh\"arp".into())
        );
        let out = set_parameter(&out, "size", &Value::Vector(vec![1.5, 2.0])).unwrap();
        assert!(out.contains("size = [1.5, 2];"));
        // The rest of the file is untouched.
        assert_eq!(out.lines().count(), SRC.lines().count());
    }

    #[test]
    fn rejects_unknown_and_mistyped() {
        assert_eq!(
            set_parameter(SRC, "nope", &Value::Bool(true)),
            Err(Error::UnknownParameter("nope".into()))
        );
        assert!(matches!(
            set_parameter(SRC, "lid", &Value::Number(1.0)),
            Err(Error::TypeMismatch { .. })
        ));
    }

    #[test]
    fn value_json_roundtrip() {
        let v: Value = serde_json::from_str("[1, 2.5]").unwrap();
        assert_eq!(v, Value::Vector(vec![1.0, 2.5]));
        let v: Value = serde_json::from_str("true").unwrap();
        assert_eq!(v, Value::Bool(true));
        let v: Value = serde_json::from_str("\"x\"").unwrap();
        assert_eq!(v.to_scad(), "\"x\"");
    }
}
