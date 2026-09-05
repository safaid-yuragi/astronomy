//! Lexer for the `.arn` textual format (§31, §5).
//!
//! The format is line-oriented and explicit; the lexer produces a flat
//! token stream (markers, handles, identifiers, numbers, strings and a few
//! punctuation characters). Newlines are not tokens — the grammar is fully
//! determined by the directive structure — but every token records its
//! line for diagnostics.

use crate::error::ParseError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TokenKind {
    /// `::ASTRONOMY::NAME` — the name is stored without the prefix.
    Marker(String),
    /// `%name` or `%123`.
    ValueRef(String),
    /// `@name`.
    FuncRef(String),
    /// A bare identifier (`i64`, `entry`, `linkage`, `c0`, ...).
    Ident(String),
    /// An integer literal.
    Int(i128),
    /// A float literal, kept as raw text for exact `f32`/`f64` parsing.
    FloatText(String),
    /// A string literal (decoded bytes).
    Str(Vec<u8>),
    /// One of `( ) < > , =`.
    Punct(char),
    /// `->`
    Arrow,
    /// `...`
    Ellipsis,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Token {
    pub kind: TokenKind,
    pub line: u32,
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn err_unexpected(line: u32, found: &str) -> ParseError {
    ParseError::UnexpectedToken {
        line,
        expected: "a valid ARN token".to_string(),
        found: found.to_string(),
    }
}

pub(crate) fn lex(src: &str) -> Result<Vec<Token>, ParseError> {
    let bytes = src.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;
    let mut line = 1u32;
    let mut out: Vec<Token> = Vec::new();

    while i < len {
        let b = bytes[i];
        match b {
            b' ' | b'\t' | b'\r' => {
                i += 1;
            }
            b'\n' => {
                line += 1;
                i += 1;
            }
            b'#' => {
                while i < len && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b':' => {
                if i + 1 < len && bytes[i + 1] == b':' {
                    i += 2;
                    if !src[i..].starts_with("ASTRONOMY") {
                        return Err(ParseError::UnexpectedToken {
                            line,
                            expected: "`::ASTRONOMY::`".to_string(),
                            found: "another `::` sequence".to_string(),
                        });
                    }
                    i += "ASTRONOMY".len();
                    if !(i + 1 < len && bytes[i] == b':' && bytes[i + 1] == b':') {
                        return Err(ParseError::UnexpectedToken {
                            line,
                            expected: "`::` after `::ASTRONOMY`".to_string(),
                            found: "end of marker".to_string(),
                        });
                    }
                    i += 2;
                    let start = i;
                    while i < len && is_ident_char(bytes[i]) {
                        i += 1;
                    }
                    if start == i {
                        return Err(ParseError::UnexpectedToken {
                            line,
                            expected: "a directive name after `::ASTRONOMY::`".to_string(),
                            found: "end of line".to_string(),
                        });
                    }
                    out.push(Token {
                        kind: TokenKind::Marker(src[start..i].to_string()),
                        line,
                    });
                } else {
                    return Err(err_unexpected(line, "`:`"));
                }
            }
            b'%' => {
                i += 1;
                let (name, ni) = read_ident_or_digits(src, i);
                if name.is_empty() {
                    return Err(ParseError::UnexpectedToken {
                        line,
                        expected: "a value handle after `%`".to_string(),
                        found: "nothing".to_string(),
                    });
                }
                i = ni;
                out.push(Token {
                    kind: TokenKind::ValueRef(name),
                    line,
                });
            }
            b'@' => {
                i += 1;
                let (name, ni) = read_ident_or_digits(src, i);
                if name.is_empty() {
                    return Err(ParseError::UnexpectedToken {
                        line,
                        expected: "a function name after `@`".to_string(),
                        found: "nothing".to_string(),
                    });
                }
                i = ni;
                out.push(Token {
                    kind: TokenKind::FuncRef(name),
                    line,
                });
            }
            b'"' => {
                i += 1;
                let (data, ni) = lex_string(src, i, line)?;
                i = ni;
                // Consume the closing quote.
                if i >= len || bytes[i] != b'"' {
                    return Err(ParseError::InvalidString {
                        line,
                        reason: "unterminated string literal".to_string(),
                    });
                }
                i += 1;
                out.push(Token {
                    kind: TokenKind::Str(data),
                    line,
                });
            }
            b'-' => {
                if i + 1 < len && bytes[i + 1] == b'>' {
                    i += 2;
                    out.push(Token {
                        kind: TokenKind::Arrow,
                        line,
                    });
                } else if i + 1 < len && bytes[i + 1].is_ascii_digit() {
                    let (tok, ni) = lex_number(src, i, line)?;
                    i = ni;
                    out.push(tok);
                } else if src[i..].starts_with("-inf") {
                    i += 4;
                    out.push(Token {
                        kind: TokenKind::FloatText("-inf".to_string()),
                        line,
                    });
                } else {
                    return Err(err_unexpected(line, "`-`"));
                }
            }
            b'.' => {
                if i + 2 < len && bytes[i + 1] == b'.' && bytes[i + 2] == b'.' {
                    i += 3;
                    out.push(Token {
                        kind: TokenKind::Ellipsis,
                        line,
                    });
                } else {
                    return Err(err_unexpected(line, "`.`"));
                }
            }
            b'0'..=b'9' => {
                let (tok, ni) = lex_number(src, i, line)?;
                i = ni;
                out.push(tok);
            }
            b'(' | b')' | b'<' | b'>' | b',' | b'=' => {
                i += 1;
                out.push(Token {
                    kind: TokenKind::Punct(b as char),
                    line,
                });
            }
            _ if is_ident_start(b) => {
                let start = i;
                while i < len && is_ident_char(bytes[i]) {
                    i += 1;
                }
                out.push(Token {
                    kind: TokenKind::Ident(src[start..i].to_string()),
                    line,
                });
            }
            _ => {
                let ch = src[i..].chars().next().unwrap_or('?');
                return Err(err_unexpected(line, &format!("`{ch}`")));
            }
        }
    }
    Ok(out)
}

fn read_ident_or_digits(src: &str, mut i: usize) -> (String, usize) {
    let bytes = src.as_bytes();
    let start = i;
    if i < bytes.len() && bytes[i].is_ascii_digit() {
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
    } else if i < bytes.len() && is_ident_start(bytes[i]) {
        while i < bytes.len() && is_ident_char(bytes[i]) {
            i += 1;
        }
    }
    (src[start..i].to_string(), i)
}

fn lex_number(src: &str, start: usize, line: u32) -> Result<(Token, usize), ParseError> {
    let bytes = src.as_bytes();
    let len = bytes.len();
    let mut i = start;
    let negative = bytes[i] == b'-';
    if negative {
        i += 1;
    }
    let digits_start = i;
    while i < len && bytes[i].is_ascii_digit() {
        i += 1;
    }
    let digits = &src[digits_start..i];

    let mut is_float = false;
    // Fraction: '.' followed by a digit.
    if i + 1 < len && bytes[i] == b'.' && bytes[i + 1].is_ascii_digit() {
        is_float = true;
        i += 1;
        while i < len && bytes[i].is_ascii_digit() {
            i += 1;
        }
    }
    // Exponent: 'e'/'E' [+-] digit+.
    if i < len && (bytes[i] == b'e' || bytes[i] == b'E') {
        let mut j = i + 1;
        if j < len && (bytes[j] == b'+' || bytes[j] == b'-') {
            j += 1;
        }
        if j < len && bytes[j].is_ascii_digit() {
            is_float = true;
            i = j;
            while i < len && bytes[i].is_ascii_digit() {
                i += 1;
            }
        }
    }
    if i < len && is_ident_char(bytes[i]) {
        return Err(ParseError::InvalidNumber {
            line,
            reason: format!("unexpected character `{}` in number", src[i..].chars().next().unwrap_or('?')),
        });
    }

    if is_float {
        let raw = &src[start..i];
        Ok((
            Token {
                kind: TokenKind::FloatText(raw.to_string()),
                line,
            },
            i,
        ))
    } else {
        let value: i128 = format!("{}{}", if negative { "-" } else { "" }, digits)
            .parse()
            .map_err(|_| ParseError::InvalidNumber {
                line,
                reason: "integer literal out of range".to_string(),
            })?;
        Ok((
            Token {
                kind: TokenKind::Int(value),
                line,
            },
            i,
        ))
    }
}

/// Decodes string literal escapes starting after the opening quote.
/// Returns the decoded bytes and the index just after the last escape.
fn lex_string(src: &str, mut i: usize, line: u32) -> Result<(Vec<u8>, usize), ParseError> {
    let bytes = src.as_bytes();
    let len = bytes.len();
    let mut data = Vec::new();
    while i < len && bytes[i] != b'"' {
        if bytes[i] == b'\\' {
            i += 1;
            if i >= len {
                return Err(ParseError::InvalidString {
                    line,
                    reason: "unterminated escape".to_string(),
                });
            }
            match bytes[i] {
                b'n' => {
                    data.push(b'\n');
                    i += 1;
                }
                b't' => {
                    data.push(b'\t');
                    i += 1;
                }
                b'r' => {
                    data.push(b'\r');
                    i += 1;
                }
                b'0' => {
                    data.push(0);
                    i += 1;
                }
                b'\\' => {
                    data.push(b'\\');
                    i += 1;
                }
                b'"' => {
                    data.push(b'"');
                    i += 1;
                }
                b'\'' => {
                    data.push(b'\'');
                    i += 1;
                }
                b'x' => {
                    if i + 2 >= len
                        || !bytes[i + 1].is_ascii_hexdigit()
                        || !bytes[i + 2].is_ascii_hexdigit()
                    {
                        return Err(ParseError::InvalidString {
                            line,
                            reason: "`\\x` requires two hex digits".to_string(),
                        });
                    }
                    let hex = &src[i + 1..i + 3];
                    data.push(u8::from_str_radix(hex, 16).unwrap());
                    i += 3;
                }
                b'u' => {
                    // \u{...} — codepoint encoded as UTF-8 bytes.
                    if i + 1 >= len || bytes[i + 1] != b'{' {
                        return Err(ParseError::InvalidString {
                            line,
                            reason: "`\\u` requires `{...}`".to_string(),
                        });
                    }
                    let close = src[i + 2..].find('}').map(|p| i + 2 + p);
                    let close = match close {
                        Some(c) if c > i + 2 => c,
                        _ => {
                            return Err(ParseError::InvalidString {
                                line,
                                reason: "`\\u{...}` requires at least one hex digit".to_string(),
                            })
                        }
                    };
                    let hex = &src[i + 2..close];
                    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
                        return Err(ParseError::InvalidString {
                            line,
                            reason: "`\\u{...}` requires hex digits only".to_string(),
                        });
                    }
                    let cp = u32::from_str_radix(hex, 16)
                        .ok()
                        .and_then(char::from_u32)
                        .ok_or_else(|| ParseError::InvalidString {
                            line,
                            reason: "invalid unicode scalar value".to_string(),
                        })?;
                    let mut buf = [0u8; 4];
                    data.extend_from_slice(cp.encode_utf8(&mut buf).as_bytes());
                    i = close + 1;
                }
                other => {
                    return Err(ParseError::InvalidString {
                        line,
                        reason: format!("unknown escape `\\{}`", other as char),
                    })
                }
            }
        } else {
            data.push(bytes[i]);
            i += 1;
        }
    }
    Ok((data, i))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<TokenKind> {
        lex(src).unwrap().into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn markers() {
        assert_eq!(
            kinds("::ASTRONOMY::MODULE_START"),
            vec![TokenKind::Marker("MODULE_START".into())]
        );
    }

    #[test]
    fn handles_and_idents() {
        assert_eq!(
            kinds("%x @printf c0"),
            vec![
                TokenKind::ValueRef("x".into()),
                TokenKind::FuncRef("printf".into()),
                TokenKind::Ident("c0".into()),
            ]
        );
    }

    #[test]
    fn numbers() {
        assert_eq!(kinds("42"), vec![TokenKind::Int(42)]);
        assert_eq!(kinds("-7"), vec![TokenKind::Int(-7)]);
        assert_eq!(
            kinds("3.5"),
            vec![TokenKind::FloatText("3.5".into())]
        );
        assert_eq!(
            kinds("1e3"),
            vec![TokenKind::FloatText("1e3".into())]
        );
        assert_eq!(
            kinds("-2.5e-3"),
            vec![TokenKind::FloatText("-2.5e-3".into())]
        );
    }

    #[test]
    fn strings() {
        assert_eq!(
            kinds(r#""a\nb""#),
            vec![TokenKind::Str(b"a\nb".to_vec())]
        );
        assert_eq!(
            kinds(r#""\x41B""#),
            vec![TokenKind::Str(b"AB".to_vec())]
        );
        assert_eq!(
            kinds(r#""\u{3042}""#),
            vec![TokenKind::Str("あ".as_bytes().to_vec())]
        );
    }

    #[test]
    fn punctuation() {
        assert_eq!(
            kinds("-> ... < > ( ) , ="),
            vec![
                TokenKind::Arrow,
                TokenKind::Ellipsis,
                TokenKind::Punct('<'),
                TokenKind::Punct('>'),
                TokenKind::Punct('('),
                TokenKind::Punct(')'),
                TokenKind::Punct(','),
                TokenKind::Punct('='),
            ]
        );
    }

    #[test]
    fn comments_and_lines() {
        let toks = lex("::ASTRONOMY::CONSTANT_INT c0 i64 1 # trailing\n%1 = ::ASTRONOMY::ADD i64 i64 %0, i64 %0").unwrap();
        assert_eq!(toks[4].line, 2);
        assert_eq!(toks[5].line, 2);
    }

    #[test]
    fn rejects_garbage() {
        assert!(lex("::NOT_ASTRONOMY").is_err());
        assert!(lex("%").is_err());
        assert!(lex("\"unterminated").is_err());
        assert!(lex("123abc").is_err());
        assert!(lex("&").is_err());
    }
}
