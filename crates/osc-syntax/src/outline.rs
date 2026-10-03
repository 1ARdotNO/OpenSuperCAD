use crate::{TokenKind, lex};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Module,
    Function,
}

/// A top-level or nested `module`/`function` definition.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Symbol {
    pub kind: SymbolKind,
    pub name: String,
    /// 1-based line of the definition.
    pub line: usize,
    /// Brace nesting depth at which the symbol is defined (0 = top level).
    pub depth: usize,
}

/// Collect every module and function definition in the source.
pub fn outline(source: &str) -> Vec<Symbol> {
    let tokens: Vec<_> = lex(source)
        .into_iter()
        .filter(|t| {
            !matches!(
                t.kind,
                TokenKind::Whitespace | TokenKind::LineComment | TokenKind::BlockComment
            )
        })
        .collect();
    let mut symbols = Vec::new();
    let mut depth = 0usize;
    for (idx, tok) in tokens.iter().enumerate() {
        let text = tok.text(source);
        match (tok.kind, text) {
            (TokenKind::Punctuation, "{") => depth += 1,
            (TokenKind::Punctuation, "}") => depth = depth.saturating_sub(1),
            (TokenKind::Keyword, "module" | "function") => {
                if let Some(name) = tokens
                    .get(idx + 1)
                    .filter(|n| matches!(n.kind, TokenKind::Identifier | TokenKind::Builtin))
                {
                    symbols.push(Symbol {
                        kind: if text == "module" {
                            SymbolKind::Module
                        } else {
                            SymbolKind::Function
                        },
                        name: name.text(source).to_owned(),
                        line: source[..tok.span.start].matches('\n').count() + 1,
                        depth,
                    });
                }
            }
            _ => {}
        }
    }
    symbols
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_modules_and_functions() {
        let src =
            "// module fake()\nmodule box(s) {\n  module inner() {}\n}\nfunction f(x) = x * 2;\n";
        let syms = outline(src);
        assert_eq!(syms.len(), 3);
        assert_eq!(syms[0].name, "box");
        assert_eq!(syms[0].line, 2);
        assert_eq!(syms[1].name, "inner");
        assert_eq!(syms[1].depth, 1);
        assert_eq!(syms[2].kind, SymbolKind::Function);
        assert_eq!(syms[2].line, 5);
    }
}
