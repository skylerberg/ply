//! The reference dump, and the two things the comparison needs around it.

use ply_span::SourceId;
use ply_syntax::lexer::{TokenKind, lex};

/// One token or diagnostic.
pub fn reference_dump(text: &str) -> String {
    let (tokens, diags) = lex(SourceId(0), text);
    let mut out = String::new();
    for t in &tokens {
        out.push_str(&format!(
            "{}:{}:{};",
            t.span.start,
            t.span.end,
            body(&t.kind)
        ));
    }
    for d in &diags {
        let span = d
            .primary_span()
            .expect("every lexer diagnostic carries a primary span");
        out.push_str(&format!("{}:{}:!:{};", span.start, span.end, d.code));
    }
    out
}

fn body(kind: &TokenKind) -> String {
    match kind {
        TokenKind::Ident(name) => format!("i:{name}"),
        TokenKind::Int(v) => format!("n:{v}"),
        TokenKind::Float(v) => format!("f:{:016x}", v.to_bits()),
        TokenKind::Decimal { mantissa, scale } => format!("d:{mantissa}:{scale}"),
        TokenKind::Str(s) => format!("s:{}", hex(s.as_bytes())),
        TokenKind::Bytes(b) => format!("b:{}", hex(b)),
        TokenKind::Kw(k) => format!("k:{}", k.as_str()),
        TokenKind::Eof => "e".to_string(),
        other => format!("p:{}", punct_name(other)),
    }
}

fn punct_name(kind: &TokenKind) -> &'static str {
    match kind {
        TokenKind::LParen => "lparen",
        TokenKind::RParen => "rparen",
        TokenKind::LBrace => "lbrace",
        TokenKind::RBrace => "rbrace",
        TokenKind::LBracket => "lbracket",
        TokenKind::RBracket => "rbracket",
        TokenKind::Comma => "comma",
        TokenKind::Semi => "semi",
        TokenKind::Colon => "colon",
        TokenKind::ColonColon => "coloncolon",
        TokenKind::Dot => "dot",
        TokenKind::DotDot => "dotdot",
        TokenKind::Underscore => "underscore",
        TokenKind::Arrow => "arrow",
        TokenKind::Eq => "eq",
        TokenKind::EqEq => "eqeq",
        TokenKind::Bang => "bang",
        TokenKind::BangEq => "bangeq",
        TokenKind::Lt => "lt",
        TokenKind::Le => "le",
        TokenKind::Gt => "gt",
        TokenKind::Ge => "ge",
        TokenKind::Plus => "plus",
        TokenKind::PlusPlus => "plusplus",
        TokenKind::Minus => "minus",
        TokenKind::Star => "star",
        TokenKind::Slash => "slash",
        TokenKind::Percent => "percent",
        TokenKind::AmpAmp => "ampamp",
        TokenKind::Pipe => "pipe",
        TokenKind::PipePipe => "pipepipe",
        // Unreachable: `body` routes every payload-carrying kind above.
        TokenKind::Ident(_)
        | TokenKind::Int(_)
        | TokenKind::Float(_)
        | TokenKind::Decimal { .. }
        | TokenKind::Str(_)
        | TokenKind::Bytes(_)
        | TokenKind::Kw(_)
        | TokenKind::Eof => unreachable!("routed by `body`"),
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// The one substitution the comparison makes, and the reason it is named here rather than buried in
/// the test.
pub fn floats_to_bits(dump: &str) -> String {
    let mut out = String::new();
    for record in dump.split_terminator(';') {
        out.push_str(&convert(record));
        out.push(';');
    }
    out
}

fn convert(record: &str) -> String {
    let parts: Vec<&str> = record.splitn(4, ':').collect();
    if parts.len() != 4 || parts[2] != "f" {
        return record.to_string();
    }
    match parts[3].parse::<f64>() {
        Ok(v) => format!("{}:{}:f:{:016x}", parts[0], parts[1], v.to_bits()),
        Err(_) => record.to_string(),
    }
}

/// The dump as a list of records, for a diff that names the first disagreement instead of printing
/// two 300-kilobyte strings.
pub fn records(dump: &str) -> Vec<&str> {
    dump.split_terminator(';').collect()
}

/// A `b"..."` literal holding exactly these bytes, for embedding a source file in a generated Ply
/// program.
pub fn byte_literal(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() + 16);
    out.push_str("b\"");
    for &b in bytes {
        match b {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            0x20..=0x7e => out.push(b as char),
            _ => out.push_str(&format!("\\x{b:02x}")),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ply_syntax::lexer::Kw;

    #[test]
    fn the_dump_is_printable_ascii_with_no_quote_and_no_backslash() {
        let dump = reference_dump("fn f() -> Int = \"a\\nb\" ++ b\"\\xff\" ++ 1.5e-3");
        for c in dump.chars() {
            assert!(
                c.is_ascii_graphic() && c != '"' && c != '\\',
                "the dump holds {c:?}, which `ply run --json` would escape:\n{dump}"
            );
        }
    }

    #[test]
    fn a_float_record_converts_to_the_bits_the_reference_dump_carries() {
        let reference = reference_dump("1.5e-3");
        let as_text = "0:6:f:1.5e-3;6:6:e;";
        assert_eq!(floats_to_bits(as_text), reference);
    }

    #[test]
    fn a_byte_literal_round_trips_every_byte_through_the_real_lexer() {
        let all: Vec<u8> = (0u8..=255).collect();
        let source = byte_literal(&all);
        let (tokens, diags) = lex(SourceId(0), &source);
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(tokens[0].kind, TokenKind::Bytes(all));
    }

    #[test]
    fn every_keyword_has_a_name_in_the_dump() {
        for kw in [
            Kw::Pub,
            Kw::Import,
            Kw::Fn,
            Kw::Type,
            Kw::Effect,
            Kw::Nondet,
            Kw::Test,
            Kw::Let,
            Kw::If,
            Kw::Else,
            Kw::Match,
            Kw::Handle,
            Kw::With,
            Kw::True,
            Kw::False,
        ] {
            let dump = reference_dump(kw.as_str());
            assert!(
                dump.starts_with(&format!("0:{}:k:{}", kw.as_str().len(), kw.as_str())),
                "{dump}"
            );
        }
    }
}
