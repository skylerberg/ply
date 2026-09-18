//! Hand-written lexer.

use crate::ast::IntTy;
use ply_span::{Diagnostic, SourceId, Span, Symbol, codes};
pub use ply_ty::is_ident;
use ply_ty::{is_ident_continue, is_ident_start};

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

/// No `Eq`: `Float` carries an `f64`.
#[derive(Clone, PartialEq, Debug)]
pub enum TokenKind {
    Ident(Symbol),
    Int(i64),
    /// `255u8`, `0x6A09_E667u32`.
    Fixed {
        ty: IntTy,
        bits: u64,
    },
    Float(f64),
    /// `1.50m`.
    Decimal {
        mantissa: i128,
        scale: u32,
    },
    Str(String),
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

    Amp,
    AmpAmp,
    Caret,
    Pipe,
    PipePipe,
    Tilde,

    /// Postfix try, `e?`.
    Question,

    Eof,
}

impl TokenKind {
    pub fn describe(&self) -> String {
        match self {
            TokenKind::Ident(n) => format!("identifier `{n}`"),
            TokenKind::Int(v) => format!("integer `{v}`"),
            TokenKind::Fixed { ty, bits } => format!("`{}` literal `{}`", ty, ty.value(*bits)),
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
            TokenKind::Amp => "&",
            TokenKind::AmpAmp => "&&",
            TokenKind::Caret => "^",
            TokenKind::Pipe => "|",
            TokenKind::PipePipe => "||",
            TokenKind::Tilde => "~",
            TokenKind::Question => "?",
            TokenKind::Ident(_)
            | TokenKind::Int(_)
            | TokenKind::Fixed { .. }
            | TokenKind::Float(_)
            | TokenKind::Decimal { .. }
            | TokenKind::Str(_)
            | TokenKind::Bytes(_)
            | TokenKind::Kw(_)
            | TokenKind::Eof => "",
        }
    }
}

/// Keeps the scale's trailing zeros: `(150, 2)` is `1.50`.
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

    /// `None` is no suffix; `Some(Err(()))` is a refused suffix, already reported.
    fn width_suffix(&mut self, lit_start: usize) -> Option<Result<IntTy, ()>> {
        if !self.peek().is_some_and(is_ident_start) {
            return None;
        }
        let start = self.pos;
        while let Some(c) = self.peek() {
            if is_ident_continue(c) {
                self.bump();
            } else {
                break;
            }
        }
        let suffix = &self.text[start..self.pos];
        match IntTy::from_name(&suffix.to_ascii_uppercase()) {
            Some(ty) => Some(Ok(ty)),
            None => {
                let message = format!("invalid suffix `{suffix}` on a numeric literal");
                self.error(
                    codes::UNEXPECTED_TOKEN,
                    message,
                    self.span_from(lit_start),
                    "the suffixes are `m` for a `Decimal` and a width — `u8`, `i32` and the six \
                     others; separate a name with a space",
                );
                Some(Err(()))
            }
        }
    }

    /// Bounded as a `u64` bit pattern, so `0xFFFF_FFFF_FFFF_FFFF` is `-1`.
    fn hex(&mut self, start: usize) -> TokenKind {
        self.bump();
        self.bump();
        let mut digits = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_hexdigit() {
                digits.push(c);
                self.bump();
            } else if c == '_' {
                self.bump();
            } else {
                break;
            }
        }
        if digits.is_empty() {
            self.error(
                codes::UNEXPECTED_TOKEN,
                "hex literal with no digits",
                self.span_from(start),
                "write at least one digit after `0x`, as in `0xFF`",
            );
            return TokenKind::Int(0);
        }
        let suffix = self.width_suffix(start);
        match u64::from_str_radix(&digits, 16) {
            Ok(v) => match suffix {
                // A bit pattern: bounded by the width, not the type's range.
                Some(Ok(ty)) => {
                    if ty.bits() < 64 && v > (u64::MAX >> (64 - ty.bits())) {
                        self.error(
                            codes::LITERAL_OUT_OF_RANGE,
                            format!("`0x{digits}` does not fit in `{ty}`"),
                            self.span_from(start),
                            format!("a `{ty}` is {} bits", ty.bits()),
                        );
                        return TokenKind::Fixed { ty, bits: 0 };
                    }
                    TokenKind::Fixed {
                        ty,
                        bits: ty.normalize(v),
                    }
                }
                // Keep the digits so a bad suffix is one diagnostic, not a second from a `0`.
                Some(Err(())) | None => TokenKind::Int(v as i64),
            },
            Err(_) => {
                self.error(
                    codes::UNEXPECTED_TOKEN,
                    format!("hex literal `0x{digits}` does not fit in `Int`"),
                    self.span_from(start),
                    "`Int` is 64 bits, so a hex literal has at most 16 digits",
                );
                TokenKind::Int(0)
            }
        }
    }

    fn number(&mut self) -> TokenKind {
        let start = self.pos;
        if self.peek() == Some('0') && matches!(self.peek2(), Some('x') | Some('X')) {
            return self.hex(start);
        }
        let mut whole = String::new();
        self.digits(&mut whole);

        let mut fraction = String::new();
        // A dot with a digit behind it, so `..` stays the range separator.
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

        let width = if !has_fraction && exponent.is_none() {
            self.width_suffix(start)
        } else {
            None
        };
        if let Some(Ok(ty)) = width {
            return match whole.parse::<i128>() {
                Ok(v) if ty.holds(v) => TokenKind::Fixed {
                    ty,
                    bits: ty.normalize(v as u64),
                },
                _ => {
                    self.error(
                        codes::LITERAL_OUT_OF_RANGE,
                        format!("`{whole}` is not a value of `{ty}`"),
                        self.span_from(start),
                        format!("`{ty}` holds {} to {}", ty.min(), ty.max()),
                    );
                    TokenKind::Fixed { ty, bits: 0 }
                }
            };
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
                "the suffixes are `m` for a `Decimal` and a width — `u8`, `i32` and the six \
                 others; separate a name with a space",
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

    /// Consumed only when digits follow, so `1 else` is two tokens.
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

    /// Checks `rust_decimal`'s domain so every `Lit::Decimal` is buildable by the evaluator.
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

    /// The two hex digits of a `\xNN`, after the `\x`.
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

    /// `None`: not punctuation; already reported and consumed.
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
            '^' => TokenKind::Caret,
            '~' => TokenKind::Tilde,
            '&' => {
                if self.eat('&') {
                    TokenKind::AmpAmp
                } else {
                    TokenKind::Amp
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
