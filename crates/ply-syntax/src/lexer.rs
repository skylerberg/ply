//! Hand-written lexer.

use ply_span::{Diagnostic, SourceId, Span, Symbol, codes};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kw {
    Pub,
    Import,
    Fn,
    Type,
    Effect,
    Nondet,
    Test,
    Let,
    If,
    Else,
    Match,
    Handle,
    With,
    True,
    False,
}

impl Kw {
    pub fn as_str(self) -> &'static str {
        match self {
            Kw::Pub => "pub",
            Kw::Import => "import",
            Kw::Fn => "fn",
            Kw::Type => "type",
            Kw::Effect => "effect",
            Kw::Nondet => "nondet",
            Kw::Test => "test",
            Kw::Let => "let",
            Kw::If => "if",
            Kw::Else => "else",
            Kw::Match => "match",
            Kw::Handle => "handle",
            Kw::With => "with",
            Kw::True => "true",
            Kw::False => "false",
        }
    }

    pub fn from_text(s: &str) -> Option<Kw> {
        Some(match s {
            "pub" => Kw::Pub,
            "import" => Kw::Import,
            "fn" => Kw::Fn,
            "type" => Kw::Type,
            "effect" => Kw::Effect,
            "nondet" => Kw::Nondet,
            "test" => Kw::Test,
            "let" => Kw::Let,
            "if" => Kw::If,
            "else" => Kw::Else,
            "match" => Kw::Match,
            "handle" => Kw::Handle,
            "with" => Kw::With,
            "true" => Kw::True,
            "false" => Kw::False,
            _ => return None,
        })
    }
}

/// `Eq` is deliberately absent: [`TokenKind::Float`] carries an `f64`, whose `==` is not reflexive.
#[derive(Clone, PartialEq, Debug)]
pub enum TokenKind {
    Ident(Symbol),
    Int(i64),
    /// `1.5`, `1e9`.
    Float(f64),
    /// `1.50m`.
    Decimal {
        mantissa: i128,
        scale: u32,
    },
    Str(String),
    /// `b"GET "`.
    Bytes(Vec<u8>),
    Kw(Kw),

    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,

    Comma,
    Semi,
    Colon,
    ColonColon,
    Dot,
    DotDot,
    Underscore,

    Arrow,
    Eq,
    EqEq,
    Bang,
    BangEq,
    Lt,
    Le,
    Gt,
    Ge,

    Plus,
    PlusPlus,
    Minus,
    Star,
    Slash,
    Percent,

    AmpAmp,
    Pipe,
    PipePipe,

    /// `e?` — the postfix try operator (GUIDE §6.10).
    Question,

    Eof,
}

impl TokenKind {
    pub fn describe(&self) -> String {
        match self {
            TokenKind::Ident(n) => format!("identifier `{n}`"),
            TokenKind::Int(v) => format!("integer `{v}`"),
            TokenKind::Float(v) => format!("float `{v}`"),
            TokenKind::Decimal { mantissa, scale } => {
                format!("decimal `{}`", render_decimal(*mantissa, *scale))
            }
            TokenKind::Str(_) => "string literal".to_string(),
            TokenKind::Bytes(_) => "byte-string literal".to_string(),
            TokenKind::Kw(k) => format!("keyword `{}`", k.as_str()),
            TokenKind::Eof => "end of file".to_string(),
            other => format!("`{}`", other.punct_text()),
        }
    }

    fn punct_text(&self) -> &'static str {
        match self {
            TokenKind::LParen => "(",
            TokenKind::RParen => ")",
            TokenKind::LBrace => "{",
            TokenKind::RBrace => "}",
            TokenKind::LBracket => "[",
            TokenKind::RBracket => "]",
            TokenKind::Comma => ",",
            TokenKind::Semi => ";",
            TokenKind::Colon => ":",
            TokenKind::ColonColon => "::",
            TokenKind::Dot => ".",
            TokenKind::DotDot => "..",
            TokenKind::Underscore => "_",
            TokenKind::Arrow => "->",
            TokenKind::Eq => "=",
            TokenKind::EqEq => "==",
            TokenKind::Bang => "!",
            TokenKind::BangEq => "!=",
            TokenKind::Lt => "<",
            TokenKind::Le => "<=",
            TokenKind::Gt => ">",
            TokenKind::Ge => ">=",
            TokenKind::Plus => "+",
            TokenKind::PlusPlus => "++",
            TokenKind::Minus => "-",
            TokenKind::Star => "*",
            TokenKind::Slash => "/",
            TokenKind::Percent => "%",
            TokenKind::AmpAmp => "&&",
            TokenKind::Pipe => "|",
            TokenKind::PipePipe => "||",
            TokenKind::Question => "?",
            TokenKind::Ident(_)
            | TokenKind::Int(_)
            | TokenKind::Float(_)
            | TokenKind::Decimal { .. }
            | TokenKind::Str(_)
            | TokenKind::Bytes(_)
            | TokenKind::Kw(_)
            | TokenKind::Eof => "",
        }
    }
}

/// The digits a `(mantissa, scale)` pair stands for, with the scale's trailing zeros kept: `(150,
/// 2)` is `1.50`.
pub fn render_decimal(mantissa: i128, scale: u32) -> String {
    let sign = if mantissa < 0 { "-" } else { "" };
    let digits = mantissa.unsigned_abs().to_string();
    if scale == 0 {
        return format!("{sign}{digits}");
    }
    let scale = scale as usize;
    let padded = if digits.len() <= scale {
        format!("{}{}", "0".repeat(scale - digits.len() + 1), digits)
    } else {
        digits
    };
    let point = padded.len() - scale;
    format!("{sign}{}.{}", &padded[..point], &padded[point..])
}

#[derive(Clone, Debug)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

pub fn lex(source: SourceId, text: &str) -> (Vec<Token>, Vec<Diagnostic>) {
    let mut lexer = Lexer {
        text,
        source,
        pos: 0,
        diags: Vec::new(),
    };
    let tokens = lexer.run();
    (tokens, lexer.diags)
}

struct Lexer<'a> {
    text: &'a str,
    source: SourceId,
    pos: usize,
    diags: Vec<Diagnostic>,
}

/// Public because module names are derived from file and directory names, which never pass through
/// the lexer.
pub fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(is_ident_start) && chars.all(is_ident_continue)
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_ident_continue(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl<'a> Lexer<'a> {
    fn peek(&self) -> Option<char> {
        self.text[self.pos..].chars().next()
    }

    fn peek2(&self) -> Option<char> {
        let mut it = self.text[self.pos..].chars();
        it.next();
        it.next()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.pos += c.len_utf8();
            true
        } else {
            false
        }
    }

    fn span_from(&self, start: usize) -> Span {
        Span::new(self.source, start as u32, self.pos as u32)
    }

    fn run(&mut self) -> Vec<Token> {
        let mut out = Vec::new();
        loop {
            self.skip_trivia();
            let start = self.pos;
            let Some(c) = self.peek() else { break };
            let kind = if c.is_ascii_digit() {
                self.number()
            } else if c == 'b' && self.peek2() == Some('"') {
                self.bump();
                self.bytes()
            } else if is_ident_start(c) {
                self.ident()
            } else if c == '"' {
                self.string()
            } else {
                match self.punct() {
                    Some(k) => k,
                    None => continue,
                }
            };
            out.push(Token {
                kind,
                span: self.span_from(start),
            });
        }
        let end = self.text.len() as u32;
        out.push(Token {
            kind: TokenKind::Eof,
            span: Span::new(self.source, end, end),
        });
        out
    }

    fn skip_trivia(&mut self) {
        loop {
            match self.peek() {
                Some(c) if c.is_whitespace() => {
                    self.bump();
                }
                Some('/') if self.peek2() == Some('/') => {
                    while let Some(c) = self.peek() {
                        if c == '\n' {
                            break;
                        }
                        self.bump();
                    }
                }
                _ => return,
            }
        }
    }

    fn ident(&mut self) -> TokenKind {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if is_ident_continue(c) {
                self.bump();
            } else {
                break;
            }
        }
        let text = &self.text[start..self.pos];
        if text == "_" {
            return TokenKind::Underscore;
        }
        match Kw::from_text(text) {
            Some(k) => TokenKind::Kw(k),
            None => TokenKind::Ident(Symbol::new(text)),
        }
    }

    fn digits(&mut self, out: &mut String) {
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                out.push(c);
                self.bump();
            } else if c == '_' {
                self.bump();
            } else {
                break;
            }
        }
    }

    /// `1`, `1.5`, `1e9`, `1.50m`.
    fn number(&mut self) -> TokenKind {
        let start = self.pos;
        let mut whole = String::new();
        self.digits(&mut whole);

        let mut fraction = String::new();
        // `..` is the range separator and `.` alone cannot open a field name, so a fraction is
        // exactly a dot with a digit behind it.
        let has_fraction =
            self.peek() == Some('.') && self.peek2().is_some_and(|c| c.is_ascii_digit());
        if has_fraction {
            self.bump();
            self.digits(&mut fraction);
        }

        let exponent = self.exponent();

        if self.peek() == Some('m') && !self.text[self.pos + 1..].starts_with(is_ident_continue) {
            self.bump();
            return self.decimal(start, &whole, &fraction, exponent.is_some());
        }

        if self.peek().is_some_and(is_ident_start) {
            let suffix_start = self.pos;
            while let Some(c) = self.peek() {
                if is_ident_continue(c) {
                    self.bump();
                } else {
                    break;
                }
            }
            let suffix = self.text[suffix_start..self.pos].to_string();
            self.error(
                codes::UNEXPECTED_TOKEN,
                format!("invalid suffix `{suffix}` on a numeric literal"),
                self.span_from(start),
                "the only suffix is `m`, for a `Decimal`; separate a name with a space",
            );
        }

        if !has_fraction && exponent.is_none() {
            return match whole.parse::<i64>() {
                Ok(v) => TokenKind::Int(v),
                Err(_) => {
                    self.error(
                        codes::UNEXPECTED_TOKEN,
                        format!("integer literal `{whole}` does not fit in `Int`"),
                        self.span_from(start),
                        "`Int` is a 64-bit signed integer; use a smaller value",
                    );
                    TokenKind::Int(0)
                }
            };
        }

        let mut text = whole;
        if has_fraction {
            text.push('.');
            text.push_str(&fraction);
        }
        if let Some(e) = &exponent {
            text.push('e');
            text.push_str(e);
        }
        // Rust's parser is correctly rounded and saturates to an infinity rather than failing,
        // which is what IEEE says decimal-to-binary conversion does.
        match text.parse::<f64>() {
            Ok(v) => TokenKind::Float(v),
            Err(_) => {
                self.error(
                    codes::UNEXPECTED_TOKEN,
                    format!("`{text}` is not a floating-point literal"),
                    self.span_from(start),
                    "write digits, an optional `.` fraction, and an optional `e` exponent",
                );
                TokenKind::Float(0.0)
            }
        }
    }

    /// The `e` of an exponent and its digits, consumed only when digits actually follow: `1e9` is a
    /// float and `1 else` is two tokens.
    fn exponent(&mut self) -> Option<String> {
        if !matches!(self.peek(), Some('e' | 'E')) {
            return None;
        }
        let rest = &self.text[self.pos + 1..];
        let after_sign = rest.strip_prefix(['+', '-']).unwrap_or(rest);
        if !after_sign.starts_with(|c: char| c.is_ascii_digit()) {
            return None;
        }
        self.bump();
        let mut out = String::new();
        if let Some(sign) = self.peek().filter(|c| *c == '+' || *c == '-') {
            out.push(sign);
            self.bump();
        }
        self.digits(&mut out);
        Some(out)
    }

    /// `rust_decimal`'s domain, checked here so that every `Lit::Decimal` in the AST is one the
    /// evaluator can build.
    fn decimal(
        &mut self,
        start: usize,
        whole: &str,
        fraction: &str,
        had_exponent: bool,
    ) -> TokenKind {
        if had_exponent {
            self.error(
                codes::UNEXPECTED_TOKEN,
                "a `Decimal` literal has no exponent",
                self.span_from(start),
                "write the digits out, or drop the `m` for a `Float`",
            );
            return TokenKind::Decimal {
                mantissa: 0,
                scale: 0,
            };
        }
        const MAX_SCALE: u32 = 28;
        const MAX_MANTISSA: i128 = (1i128 << 96) - 1;

        let scale = fraction.len();
        if scale > MAX_SCALE as usize {
            self.error(
                codes::UNEXPECTED_TOKEN,
                format!("a `Decimal` literal has at most {MAX_SCALE} decimal places, not {scale}"),
                self.span_from(start),
                "round the literal, or use `Float`",
            );
            return TokenKind::Decimal {
                mantissa: 0,
                scale: 0,
            };
        }
        let mut digits = String::with_capacity(whole.len() + fraction.len());
        digits.push_str(whole);
        digits.push_str(fraction);
        let mantissa = digits.parse::<i128>().ok().filter(|m| *m <= MAX_MANTISSA);
        match mantissa {
            Some(mantissa) => TokenKind::Decimal {
                mantissa,
                scale: scale as u32,
            },
            None => {
                self.error(
                    codes::UNEXPECTED_TOKEN,
                    "a `Decimal` literal has at most 96 bits of mantissa",
                    self.span_from(start),
                    "the largest is 79228162514264337593543950335",
                );
                TokenKind::Decimal {
                    mantissa: 0,
                    scale: 0,
                }
            }
        }
    }

    fn string(&mut self) -> TokenKind {
        let open = self.pos;
        self.bump();
        let mut out = String::new();
        loop {
            match self.peek() {
                None | Some('\n') => {
                    self.error(
                        codes::UNTERMINATED_STRING,
                        "unterminated string literal",
                        self.span_from(open),
                        "add a closing `\"`; a string may not span a line break",
                    );
                    return TokenKind::Str(out);
                }
                Some('"') => {
                    self.bump();
                    return TokenKind::Str(out);
                }
                Some('\\') => {
                    let esc_start = self.pos;
                    self.bump();
                    match self.bump() {
                        Some('n') => out.push('\n'),
                        Some('t') => out.push('\t'),
                        Some('r') => out.push('\r'),
                        Some('0') => out.push('\0'),
                        Some('\\') => out.push('\\'),
                        Some('"') => out.push('"'),
                        Some(other) => {
                            self.error(
                                codes::UNEXPECTED_TOKEN,
                                format!("unknown escape sequence `\\{other}`"),
                                self.span_from(esc_start),
                                "valid escapes are \\n \\t \\r \\0 \\\\ and \\\"",
                            );
                            out.push(other);
                        }
                        None => {
                            self.error(
                                codes::UNTERMINATED_STRING,
                                "unterminated string literal",
                                self.span_from(open),
                                "add a closing `\"`",
                            );
                            return TokenKind::Str(out);
                        }
                    }
                }
                Some(c) => {
                    self.bump();
                    out.push(c);
                }
            }
        }
    }

    /// `b"..."`, entered with the `b` already consumed.
    fn bytes(&mut self) -> TokenKind {
        let open = self.pos - 1;
        self.bump();
        let mut out: Vec<u8> = Vec::new();
        loop {
            match self.peek() {
                None | Some('\n') => {
                    self.error(
                        codes::UNTERMINATED_STRING,
                        "unterminated byte-string literal",
                        self.span_from(open),
                        "add a closing `\"`; a literal may not span a line break",
                    );
                    return TokenKind::Bytes(out);
                }
                Some('"') => {
                    self.bump();
                    return TokenKind::Bytes(out);
                }
                Some('\\') => {
                    let esc_start = self.pos;
                    self.bump();
                    match self.bump() {
                        Some('n') => out.push(b'\n'),
                        Some('t') => out.push(b'\t'),
                        Some('r') => out.push(b'\r'),
                        Some('0') => out.push(0),
                        Some('\\') => out.push(b'\\'),
                        Some('"') => out.push(b'"'),
                        Some('x') => out.push(self.hex_byte(esc_start)),
                        Some(other) => {
                            self.error(
                                codes::UNEXPECTED_TOKEN,
                                format!("unknown escape sequence `\\{other}`"),
                                self.span_from(esc_start),
                                "valid escapes are \\n \\t \\r \\0 \\\\ \\\" and \\xNN",
                            );
                        }
                        None => {
                            self.error(
                                codes::UNTERMINATED_STRING,
                                "unterminated byte-string literal",
                                self.span_from(open),
                                "add a closing `\"`",
                            );
                            return TokenKind::Bytes(out);
                        }
                    }
                }
                Some(c) if c.is_ascii() => {
                    self.bump();
                    out.push(c as u8);
                }
                Some(c) => {
                    let start = self.pos;
                    self.bump();
                    // The file was read as UTF-8 or it did not parse at all, so these are the bytes
                    // the author is looking at.
                    let encoded: String = c
                        .to_string()
                        .bytes()
                        .map(|b| format!("\\x{b:02x}"))
                        .collect();
                    self.error(
                        codes::UNEXPECTED_TOKEN,
                        format!(
                            "`{c}` is not an ASCII character, so it has no place in `b\"...\"`"
                        ),
                        self.span_from(start),
                        format!("write `{encoded}` instead"),
                    );
                }
            }
        }
    }

    /// The two hex digits of a `\xNN`, with the backslash and `x` consumed.
    fn hex_byte(&mut self, esc_start: usize) -> u8 {
        let start = self.pos;
        for _ in 0..2 {
            match self.peek() {
                Some(c) if c.is_ascii_hexdigit() => {
                    self.bump();
                }
                _ => break,
            }
        }
        let digits = &self.text[start..self.pos];
        match u8::from_str_radix(digits, 16) {
            Ok(b) if digits.len() == 2 => b,
            _ => {
                self.error(
                    codes::UNEXPECTED_TOKEN,
                    "`\\x` needs exactly two hex digits",
                    self.span_from(esc_start),
                    "write two hex digits, as in `\\x0d`",
                );
                0
            }
        }
    }

    /// `None` means the character was not punctuation: an error was reported and the character
    /// consumed, so the caller should just carry on.
    fn punct(&mut self) -> Option<TokenKind> {
        let start = self.pos;
        let c = self.bump()?;
        let kind = match c {
            '(' => TokenKind::LParen,
            ')' => TokenKind::RParen,
            '{' => TokenKind::LBrace,
            '}' => TokenKind::RBrace,
            '[' => TokenKind::LBracket,
            ']' => TokenKind::RBracket,
            ',' => TokenKind::Comma,
            ';' => TokenKind::Semi,
            ':' => {
                if self.eat(':') {
                    TokenKind::ColonColon
                } else {
                    TokenKind::Colon
                }
            }
            '.' => {
                if self.eat('.') {
                    TokenKind::DotDot
                } else {
                    TokenKind::Dot
                }
            }
            '=' => {
                if self.eat('=') {
                    TokenKind::EqEq
                } else {
                    TokenKind::Eq
                }
            }
            '!' => {
                if self.eat('=') {
                    TokenKind::BangEq
                } else {
                    TokenKind::Bang
                }
            }
            '<' => {
                if self.eat('=') {
                    TokenKind::Le
                } else {
                    TokenKind::Lt
                }
            }
            '>' => {
                if self.eat('=') {
                    TokenKind::Ge
                } else {
                    TokenKind::Gt
                }
            }
            '+' => {
                if self.eat('+') {
                    TokenKind::PlusPlus
                } else {
                    TokenKind::Plus
                }
            }
            '-' => {
                if self.eat('>') {
                    TokenKind::Arrow
                } else {
                    TokenKind::Minus
                }
            }
            '*' => TokenKind::Star,
            '/' => TokenKind::Slash,
            '%' => TokenKind::Percent,
            '?' => TokenKind::Question,
            '&' => {
                if self.eat('&') {
                    TokenKind::AmpAmp
                } else {
                    self.error(
                        codes::UNEXPECTED_TOKEN,
                        "unexpected character `&`",
                        self.span_from(start),
                        "Ply has no bitwise `&`; write `&&` for logical and",
                    );
                    return None;
                }
            }
            '|' => {
                if self.eat('|') {
                    TokenKind::PipePipe
                } else {
                    TokenKind::Pipe
                }
            }
            other => {
                self.error(
                    codes::UNEXPECTED_TOKEN,
                    format!("unexpected character `{other}`"),
                    self.span_from(start),
                    "this character has no meaning in Ply source",
                );
                return None;
            }
        };
        Some(kind)
    }

    fn error(
        &mut self,
        code: &'static str,
        message: impl Into<String>,
        span: Span,
        label: impl Into<String>,
    ) {
        self.diags
            .push(Diagnostic::error(code, message).primary(span, label));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            kinds("-> - ++ + == = != ! <= < >= > && || | .. ."),
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
                TokenKind::PipePipe,
                TokenKind::Pipe,
                TokenKind::DotDot,
                TokenKind::Dot,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn a_lone_ampersand_is_reported_and_skipped() {
        let (toks, diags) = lex(SourceId(0), "a & b");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].notes.is_empty());
        assert_eq!(toks.len(), 3);
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
}
