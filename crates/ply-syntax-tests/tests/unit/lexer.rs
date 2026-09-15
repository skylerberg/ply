use ply_span::{Diagnostic, SourceId, Span, Symbol, codes};
use ply_syntax::lexer::{Kw, TokenKind, lex};

fn kinds(text: &str) -> Vec<TokenKind> {
    let (toks, diags) = lex(SourceId(0), text);
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
    toks.into_iter().map(|t| t.kind).collect()
}

fn diags_of(text: &str) -> Vec<Diagnostic> {
    lex(SourceId(0), text).1
}

#[test]
fn keywords_are_distinct_from_identifiers() {
    assert_eq!(
        kinds("fn fnord effect effects"),
        vec![
            TokenKind::Kw(Kw::Fn),
            TokenKind::Ident(Symbol::new("fnord")),
            TokenKind::Kw(Kw::Effect),
            TokenKind::Ident(Symbol::new("effects")),
            TokenKind::Eof,
        ]
    );
}

#[test]
fn read_and_write_stay_identifiers() {
    assert_eq!(
        kinds("read write return with_cell with_region"),
        vec![
            TokenKind::Ident(Symbol::new("read")),
            TokenKind::Ident(Symbol::new("write")),
            TokenKind::Ident(Symbol::new("return")),
            TokenKind::Ident(Symbol::new("with_cell")),
            TokenKind::Ident(Symbol::new("with_region")),
            TokenKind::Eof,
        ]
    );
}

#[test]
fn underscore_alone_is_a_wildcard_but_prefixes_an_identifier() {
    assert_eq!(
        kinds("_ _x"),
        vec![
            TokenKind::Underscore,
            TokenKind::Ident(Symbol::new("_x")),
            TokenKind::Eof
        ]
    );
}

#[test]
fn integers_allow_underscore_separators() {
    assert_eq!(
        kinds("1_000_000"),
        vec![TokenKind::Int(1_000_000), TokenKind::Eof]
    );
}

#[test]
fn integer_overflow_is_a_diagnostic_not_a_panic() {
    let d = diags_of("99999999999999999999");
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].code, codes::UNEXPECTED_TOKEN);
    assert!(d[0].message.contains("does not fit"));
}

#[test]
fn integer_suffix_is_rejected() {
    let d = diags_of("12abc");
    assert_eq!(d.len(), 1);
    assert!(d[0].message.contains("invalid suffix"));
    assert_eq!(d[0].primary_span().unwrap(), Span::new(SourceId(0), 0, 5));
}

#[test]
fn string_escapes_decode() {
    assert_eq!(
        kinds(r#""a\nb\t\\\"\r\0""#),
        vec![TokenKind::Str("a\nb\t\\\"\r\0".to_string()), TokenKind::Eof]
    );
}

#[test]
fn unterminated_string_reports_from_the_opening_quote() {
    let d = diags_of("let s = \"oops\nlet t = 1");
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].code, codes::UNTERMINATED_STRING);
    let span = d[0].primary_span().unwrap();
    assert_eq!(span.start, 8);
    assert_eq!(span.end, 13);
}

#[test]
fn unknown_escape_keeps_lexing() {
    let (toks, diags) = lex(SourceId(0), r#""a\qb""#);
    assert_eq!(diags.len(), 1);
    assert_eq!(toks[0].kind, TokenKind::Str("aqb".to_string()));
}

#[test]
fn a_byte_literal_takes_the_string_escapes_plus_hex() {
    assert_eq!(
        kinds(r#"b"GET \r\n\x00\xff\"\\""#),
        vec![
            TokenKind::Bytes(b"GET \r\n\x00\xff\"\\".to_vec()),
            TokenKind::Eof
        ]
    );
    assert_eq!(
        kinds(r#"b"""#),
        vec![TokenKind::Bytes(Vec::new()), TokenKind::Eof]
    );
}

/// The `b` prefix binds only when the quote is the very next character, so an ordinary
/// identifier called `b` keeps working.
#[test]
fn b_is_a_prefix_only_when_the_quote_follows_immediately() {
    assert_eq!(
        kinds("b \"x\""),
        vec![
            TokenKind::Ident(Symbol::new("b")),
            TokenKind::Str("x".to_string()),
            TokenKind::Eof
        ]
    );
    assert_eq!(
        kinds("bytes b"),
        vec![
            TokenKind::Ident(Symbol::new("bytes")),
            TokenKind::Ident(Symbol::new("b")),
            TokenKind::Eof
        ]
    );
}

/// The bytes of a literal may not depend on how the file was saved, so the diagnostic hands
/// back the exact escapes the author should have written.
#[test]
fn a_non_ascii_character_in_a_byte_literal_is_refused_with_its_escapes() {
    let (toks, diags) = lex(SourceId(0), "b\"é\"");
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].code, codes::UNEXPECTED_TOKEN);
    assert!(diags[0].message.contains("not an ASCII character"));
    assert!(
        diags[0].labels[0].message.contains("\\xc3\\xa9"),
        "{:?}",
        diags[0].labels
    );
    assert_eq!(toks[0].kind, TokenKind::Bytes(Vec::new()));
}

#[test]
fn a_short_hex_escape_is_reported_without_swallowing_the_literal() {
    let (toks, diags) = lex(SourceId(0), r#"b"\xg1" 7"#);
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].code, codes::UNEXPECTED_TOKEN);
    assert_eq!(toks[0].kind, TokenKind::Bytes(b"\0g1".to_vec()));
    assert_eq!(toks[1].kind, TokenKind::Int(7));
}

#[test]
fn an_unterminated_byte_literal_reports_from_its_opening_quote() {
    let d = diags_of("let s = b\"oops\nlet t = 1");
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].code, codes::UNTERMINATED_STRING);
    let span = d[0].primary_span().unwrap();
    assert_eq!(span.start, 8);
}

#[test]
fn comments_run_to_end_of_line() {
    assert_eq!(
        kinds("1 // two\n3"),
        vec![TokenKind::Int(1), TokenKind::Int(3), TokenKind::Eof]
    );
}

#[test]
fn operators_use_maximal_munch() {
    assert_eq!(
        kinds("-> - ++ + == = != ! <= < >= > && & || | ^ ~ .. ."),
        vec![
            TokenKind::Arrow,
            TokenKind::Minus,
            TokenKind::PlusPlus,
            TokenKind::Plus,
            TokenKind::EqEq,
            TokenKind::Eq,
            TokenKind::BangEq,
            TokenKind::Bang,
            TokenKind::Le,
            TokenKind::Lt,
            TokenKind::Ge,
            TokenKind::Gt,
            TokenKind::AmpAmp,
            TokenKind::Amp,
            TokenKind::PipePipe,
            TokenKind::Pipe,
            TokenKind::Caret,
            TokenKind::Tilde,
            TokenKind::DotDot,
            TokenKind::Dot,
            TokenKind::Eof,
        ]
    );
}

/// The one munch the lexer deliberately does *not* do. `Map<Int, List<Int>>`
/// closes with two `>` that must stay two tokens, so a shift is assembled by
/// the expression parser out of adjacent ones (the operator decision) and never here.
#[test]
fn angle_brackets_never_munch_into_a_shift() {
    assert_eq!(
        kinds(">> >>>"),
        vec![
            TokenKind::Gt,
            TokenKind::Gt,
            TokenKind::Gt,
            TokenKind::Gt,
            TokenKind::Gt,
            TokenKind::Eof,
        ]
    );
    assert_eq!(
        kinds("<<"),
        vec![TokenKind::Lt, TokenKind::Lt, TokenKind::Eof]
    );
    // `>>=` is `>` then `>=`, not two `>`, so it cannot become a shift.
    assert_eq!(
        kinds(">>="),
        vec![TokenKind::Gt, TokenKind::Ge, TokenKind::Eof]
    );
}

/// Was `a_lone_ampersand_is_reported_and_skipped`. the operator decision makes the
/// character real, so the diagnostic that said Ply has no bitwise `&` is
/// gone and this is the assertion that it is gone.
#[test]
fn a_lone_ampersand_is_a_token_of_its_own() {
    assert_eq!(
        kinds("a & b"),
        vec![
            TokenKind::Ident(Symbol::new("a")),
            TokenKind::Amp,
            TokenKind::Ident(Symbol::new("b")),
            TokenKind::Eof,
        ]
    );
}

#[test]
fn unknown_character_is_reported_once_and_lexing_continues() {
    let (toks, diags) = lex(SourceId(0), "a $ b");
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].code, codes::UNEXPECTED_TOKEN);
    assert_eq!(
        toks.iter().map(|t| t.kind.clone()).collect::<Vec<_>>(),
        vec![
            TokenKind::Ident(Symbol::new("a")),
            TokenKind::Ident(Symbol::new("b")),
            TokenKind::Eof
        ]
    );
}

#[test]
fn spans_are_byte_ranges_over_multibyte_source() {
    let (toks, _) = lex(SourceId(0), "\"é\" x");
    assert_eq!(toks[0].span, Span::new(SourceId(0), 0, 4));
    assert_eq!(toks[1].span, Span::new(SourceId(0), 5, 6));
}

#[test]
fn eof_span_is_at_the_end_of_input() {
    let (toks, _) = lex(SourceId(0), "abc");
    let eof = toks.last().unwrap();
    assert_eq!(eof.kind, TokenKind::Eof);
    assert_eq!(eof.span, Span::new(SourceId(0), 3, 3));
}
