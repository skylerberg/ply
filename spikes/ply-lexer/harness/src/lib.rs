//! The reference dump, and the two things the comparison needs around it.
//!
//! The dump is the whole comparison: one line of text per token, in a format
//! both lexers can emit, so that "these two lexers agree" is a string equality
//! a reader can check by eye rather than a claim.
//!
//! Every character is printable ASCII and neither `"` nor `\` occurs. That is
//! not decoration: `ply run --json` renders a `String` through
//! `ply_eval::value::escape`, so a dump containing either character would come
//! back escaped and the harness would be comparing its own unescaper against
//! the lexer. Unwrapping is removing the first and last character.

use ply_span::SourceId;
use ply_syntax::lexer::{TokenKind, lex};

/// One token or diagnostic. Records are `;`-terminated.
///
/// Tokens first, in source order, then diagnostics, in the order the lexer
/// raised them.
///
/// - `S:E:i:NAME` identifier
/// - `S:E:n:DIGITS` integer
/// - `S:E:f:BITS` float, as the 16 hex digits of `f64::to_bits`
/// - `S:E:d:DIGITS:SCALE` decimal
/// - `S:E:s:HEX` string, hex of the decoded UTF-8
/// - `S:E:b:HEX` byte string, hex of the decoded bytes
/// - `S:E:k:NAME` keyword
/// - `S:E:p:NAME` punctuation
/// - `S:E:e` end of file
/// - `S:E:!:CODE` diagnostic
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

/// The one substitution the comparison makes, and the reason it is named here
/// rather than buried in the test.
///
/// Ply has no `float_of_string` and cannot build a `Float` out of digits at
/// all, so the Ply lexer emits a float's **normalised literal text** — the
/// exact string `lexer.rs` hands to `f64::from_str` — where the reference dump
/// emits the resulting bits. This converts the former into the latter with
/// Rust's parser, so what is compared is the decision the lexer made (that this
/// is a float, that it spans these bytes, that these are its digits) and not
/// the decimal-to-binary conversion, which is delegated and *not* checked.
///
/// A record that is not a float passes through unchanged. A float record whose
/// text does not parse is left as it is, so it fails the comparison loudly
/// instead of being smoothed into a passing one.
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

/// The dump as a list of records, for a diff that names the first disagreement
/// instead of printing two 300-kilobyte strings.
pub fn records(dump: &str) -> Vec<&str> {
    dump.split_terminator(';').collect()
}

/// A `b"..."` literal holding exactly these bytes, for embedding a source file
/// in a generated Ply program.
///
/// Ply is the only way in: there is no file-reading host handler, so a source
/// file reaches a Ply program as a literal or not at all.
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
