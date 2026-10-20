//! Turns source text into tokens.

use crate::diag::{Diagnostic, Diagnostics, Span};

/// The largest integer literal: Quill integers are 63-bit (one bit is used to tag them at run time).
pub const INT_MAX: i64 = (1i64 << 62) - 1;

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Int(i64),
    Str(String),
    /// A lower-case identifier: variables, functions, type variables.
    Ident(String),
    /// An upper-case identifier: types and constructors.
    UIdent(String),
    // Keywords
    Fn,
    Let,
    Var,
    If,
    Else,
    While,
    Match,
    Type,
    Return,
    True,
    False,
    // Punctuation
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Semi,
    Colon,
    Arrow,
    FatArrow,
    Eq,
    EqEq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    PlusPlus,
    Bang,
    AndAnd,
    OrOr,
    Pipe,
    Underscore,
    Eof,
}

impl Tok {
    pub fn describe(&self) -> String {
        match self {
            Tok::Int(n) => format!("integer `{n}`"),
            Tok::Str(_) => "string literal".into(),
            Tok::Ident(s) | Tok::UIdent(s) => format!("`{s}`"),
            Tok::Eof => "end of file".into(),
            t => format!("`{}`", t.text()),
        }
    }

    pub fn text(&self) -> &'static str {
        match self {
            Tok::Fn => "fn",
            Tok::Let => "let",
            Tok::Var => "var",
            Tok::If => "if",
            Tok::Else => "else",
            Tok::While => "while",
            Tok::Match => "match",
            Tok::Type => "type",
            Tok::Return => "return",
            Tok::True => "true",
            Tok::False => "false",
            Tok::LParen => "(",
            Tok::RParen => ")",
            Tok::LBrace => "{",
            Tok::RBrace => "}",
            Tok::LBracket => "[",
            Tok::RBracket => "]",
            Tok::Comma => ",",
            Tok::Semi => ";",
            Tok::Colon => ":",
            Tok::Arrow => "->",
            Tok::FatArrow => "=>",
            Tok::Eq => "=",
            Tok::EqEq => "==",
            Tok::Ne => "!=",
            Tok::Lt => "<",
            Tok::Le => "<=",
            Tok::Gt => ">",
            Tok::Ge => ">=",
            Tok::Plus => "+",
            Tok::Minus => "-",
            Tok::Star => "*",
            Tok::Slash => "/",
            Tok::Percent => "%",
            Tok::PlusPlus => "++",
            Tok::Bang => "!",
            Tok::AndAnd => "&&",
            Tok::OrOr => "||",
            Tok::Pipe => "|",
            Tok::Underscore => "_",
            _ => "",
        }
    }
}

pub const KEYWORDS: &[&str] = &["fn", "let", "var", "if", "else", "while", "match", "type", "return", "true", "false"];

#[derive(Clone, Debug)]
pub struct Token {
    pub tok: Tok,
    pub span: Span,
}

pub fn lex(src: &str, diags: &mut Diagnostics) -> Vec<Token> {
    let bytes = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        let start = i;
        // Whitespace and comments
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if c == b'/' && bytes.get(i + 1) == Some(&b'/') {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == b'/' && bytes.get(i + 1) == Some(&b'*') {
            let mut depth = 0;
            loop {
                if i + 1 >= bytes.len() {
                    diags.error(Span::new(start, start + 2), "unterminated block comment");
                    i = bytes.len();
                    break;
                }
                if bytes[i] == b'/' && bytes[i + 1] == b'*' {
                    depth += 1;
                    i += 2;
                } else if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            continue;
        }
        if c.is_ascii_digit() {
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
                i += 1;
            }
            let text: String = src[start..i].chars().filter(|&c| c != '_').collect();
            // 2^62 itself is allowed here: it is valid after a minus sign (the parser checks).
            let value = match text.parse::<i64>() {
                Ok(v) if v <= INT_MAX + 1 => v,
                _ => {
                    diags.push(
                        Diagnostic::error(Span::new(start, i), "integer literal is too large")
                            .with_note(format!("Quill integers are 63-bit: the largest is {INT_MAX}")),
                    );
                    0
                }
            };
            out.push(Token { tok: Tok::Int(value), span: Span::new(start, i) });
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let word = &src[start..i];
            let tok = match word {
                "fn" => Tok::Fn,
                "let" => Tok::Let,
                "var" => Tok::Var,
                "if" => Tok::If,
                "else" => Tok::Else,
                "while" => Tok::While,
                "match" => Tok::Match,
                "type" => Tok::Type,
                "return" => Tok::Return,
                "true" => Tok::True,
                "false" => Tok::False,
                "_" => Tok::Underscore,
                w if w.as_bytes()[0].is_ascii_uppercase() => Tok::UIdent(w.to_string()),
                w => Tok::Ident(w.to_string()),
            };
            out.push(Token { tok, span: Span::new(start, i) });
            continue;
        }
        if c == b'"' {
            i += 1;
            let mut s = String::new();
            let mut closed = false;
            while i < bytes.len() {
                let ch = src[i..].chars().next().unwrap();
                if ch == '"' {
                    i += 1;
                    closed = true;
                    break;
                }
                if ch == '\n' {
                    break;
                }
                if ch == '\\' {
                    let esc_start = i;
                    i += 1;
                    let e = src[i..].chars().next();
                    match e {
                        Some('n') => s.push('\n'),
                        Some('t') => s.push('\t'),
                        Some('r') => s.push('\r'),
                        Some('0') => s.push('\0'),
                        Some('\\') => s.push('\\'),
                        Some('"') => s.push('"'),
                        Some(other) => {
                            diags.error(Span::new(esc_start, i + other.len_utf8()), format!("unknown escape `\\{other}`"));
                        }
                        None => break,
                    }
                    if let Some(e) = e {
                        i += e.len_utf8();
                    }
                    continue;
                }
                s.push(ch);
                i += ch.len_utf8();
            }
            if !closed {
                diags.error(Span::new(start, i), "unterminated string literal");
            }
            out.push(Token { tok: Tok::Str(s), span: Span::new(start, i) });
            continue;
        }
        let two = if i + 1 < bytes.len() { &src[i..i + 2] } else { "" };
        let tok2 = match two {
            "->" => Some(Tok::Arrow),
            "=>" => Some(Tok::FatArrow),
            "==" => Some(Tok::EqEq),
            "!=" => Some(Tok::Ne),
            "<=" => Some(Tok::Le),
            ">=" => Some(Tok::Ge),
            "++" => Some(Tok::PlusPlus),
            "&&" => Some(Tok::AndAnd),
            "||" => Some(Tok::OrOr),
            _ => None,
        };
        if let Some(t) = tok2 {
            out.push(Token { tok: t, span: Span::new(i, i + 2) });
            i += 2;
            continue;
        }
        let tok1 = match c {
            b'(' => Some(Tok::LParen),
            b')' => Some(Tok::RParen),
            b'{' => Some(Tok::LBrace),
            b'}' => Some(Tok::RBrace),
            b'[' => Some(Tok::LBracket),
            b']' => Some(Tok::RBracket),
            b',' => Some(Tok::Comma),
            b';' => Some(Tok::Semi),
            b':' => Some(Tok::Colon),
            b'=' => Some(Tok::Eq),
            b'<' => Some(Tok::Lt),
            b'>' => Some(Tok::Gt),
            b'+' => Some(Tok::Plus),
            b'-' => Some(Tok::Minus),
            b'*' => Some(Tok::Star),
            b'/' => Some(Tok::Slash),
            b'%' => Some(Tok::Percent),
            b'!' => Some(Tok::Bang),
            b'|' => Some(Tok::Pipe),
            _ => None,
        };
        match tok1 {
            Some(t) => {
                out.push(Token { tok: t, span: Span::new(i, i + 1) });
                i += 1;
            }
            None => {
                let ch = src[i..].chars().next().unwrap();
                diags.error(Span::new(i, i + ch.len_utf8()), format!("unexpected character `{ch}`"));
                i += ch.len_utf8();
            }
        }
    }
    out.push(Token { tok: Tok::Eof, span: Span::new(src.len(), src.len()) });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<Tok> {
        let mut d = Diagnostics::default();
        let t = lex(s, &mut d).into_iter().map(|t| t.tok).collect();
        assert!(!d.has_errors(), "{:?}", d.list);
        t
    }

    #[test]
    fn lexes_operators_keywords_and_literals() {
        assert_eq!(
            toks("fn f(x) -> Int { x ++ \"a\\n\" } // c"),
            vec![
                Tok::Fn,
                Tok::Ident("f".into()),
                Tok::LParen,
                Tok::Ident("x".into()),
                Tok::RParen,
                Tok::Arrow,
                Tok::UIdent("Int".into()),
                Tok::LBrace,
                Tok::Ident("x".into()),
                Tok::PlusPlus,
                Tok::Str("a\n".into()),
                Tok::RBrace,
                Tok::Eof
            ]
        );
        assert_eq!(toks("1_000 /* nested /* c */ */ _"), vec![Tok::Int(1000), Tok::Underscore, Tok::Eof]);
    }

    #[test]
    fn rejects_huge_integers() {
        let mut d = Diagnostics::default();
        lex("4611686018427387905", &mut d);
        assert!(d.has_errors());
    }
}
