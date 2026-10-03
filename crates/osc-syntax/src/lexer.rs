use std::ops::Range;

/// The category of a lexed token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Whitespace,
    LineComment,
    BlockComment,
    Keyword,
    Builtin,
    Identifier,
    /// `$fn`, `$fa`, `$t`, `$children`, …
    SpecialVariable,
    Number,
    String,
    Operator,
    Punctuation,
    /// A byte sequence the lexer could not classify (kept so the token stream
    /// stays lossless).
    Unknown,
}

/// A token with its byte range in the source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Range<usize>,
}

impl Token {
    pub fn text<'a>(&self, source: &'a str) -> &'a str {
        &source[self.span.clone()]
    }
}

/// Lex OpenSCAD source into a lossless token stream: concatenating the token
/// texts reproduces the input exactly.
pub fn lex(source: &str) -> Vec<Token> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        let c = bytes[i];
        let kind = match c {
            b if b.is_ascii_whitespace() => {
                while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                TokenKind::Whitespace
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                TokenKind::LineComment
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i < bytes.len() && !(bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/')) {
                    i += 1;
                }
                i = (i + 2).min(bytes.len());
                TokenKind::BlockComment
            }
            b'"' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
                i = (i + 1).min(bytes.len());
                TokenKind::String
            }
            b'0'..=b'9' => {
                i = scan_number(bytes, i);
                TokenKind::Number
            }
            b'.' if bytes.get(i + 1).is_some_and(u8::is_ascii_digit) => {
                i = scan_number(bytes, i);
                TokenKind::Number
            }
            b'$' | b'_' | b'a'..=b'z' | b'A'..=b'Z' => {
                i += 1;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                let word = &source[start..i];
                if word.starts_with('$') {
                    TokenKind::SpecialVariable
                } else if crate::KEYWORDS.contains(&word) {
                    TokenKind::Keyword
                } else if crate::BUILTINS.contains(&word) {
                    TokenKind::Builtin
                } else {
                    TokenKind::Identifier
                }
            }
            b'(' | b')' | b'[' | b']' | b'{' | b'}' | b',' | b';' | b':' | b'.' => {
                i += 1;
                TokenKind::Punctuation
            }
            b'=' | b'!' | b'<' | b'>' => {
                i += 1;
                if bytes.get(i) == Some(&b'=') {
                    i += 1;
                }
                TokenKind::Operator
            }
            b'&' | b'|' => {
                i += 1;
                if bytes.get(i) == Some(&c) {
                    i += 1;
                }
                TokenKind::Operator
            }
            b'+' | b'-' | b'*' | b'/' | b'%' | b'^' | b'?' | b'#' => {
                i += 1;
                TokenKind::Operator
            }
            _ => {
                // Advance by a whole UTF-8 character.
                i += source[i..].chars().next().map_or(1, char::len_utf8);
                TokenKind::Unknown
            }
        };
        tokens.push(Token {
            kind,
            span: start..i,
        });
    }
    tokens
}

fn scan_number(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
        i += 1;
    }
    if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
        let mut j = i + 1;
        if j < bytes.len() && (bytes[j] == b'+' || bytes[j] == b'-') {
            j += 1;
        }
        if j < bytes.len() && bytes[j].is_ascii_digit() {
            i = j;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
        }
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<(TokenKind, &str)> {
        lex(src)
            .into_iter()
            .filter(|t| t.kind != TokenKind::Whitespace)
            .map(|t| (t.kind, &src[t.span]))
            .collect()
    }

    #[test]
    fn lossless() {
        let src = "module a(r=1.5e3){ /* c */ cube([1,2,3]); } // end\n$fn=64; s=\"x\\\"y\"; é";
        let joined: String = lex(src).iter().map(|t| t.text(src)).collect();
        assert_eq!(joined, src);
    }

    #[test]
    fn classifies() {
        assert_eq!(
            kinds("module foo() cube($fn = 2.5e-1);"),
            vec![
                (TokenKind::Keyword, "module"),
                (TokenKind::Identifier, "foo"),
                (TokenKind::Punctuation, "("),
                (TokenKind::Punctuation, ")"),
                (TokenKind::Builtin, "cube"),
                (TokenKind::Punctuation, "("),
                (TokenKind::SpecialVariable, "$fn"),
                (TokenKind::Operator, "="),
                (TokenKind::Number, "2.5e-1"),
                (TokenKind::Punctuation, ")"),
                (TokenKind::Punctuation, ";"),
            ]
        );
    }

    #[test]
    fn unterminated_constructs_do_not_panic() {
        for src in ["\"abc", "/* abc", "1e", "&", "\"\\"] {
            let joined: String = lex(src).iter().map(|t| t.text(src)).collect();
            assert_eq!(joined, src);
        }
    }
}
