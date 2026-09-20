//! `front.ply`'s `claims_dump` read back: each `fn` body, clause and law as `code.ply` lowers it.

use ply_span::frames::Cursor;
use ply_span::{SourceId, Span, Symbol};
use ply_ty::{BinOp, IntTy, Lit, SpecKind, UnOp};
use std::collections::{BTreeMap, BTreeSet, HashMap};

const NO_MODULE: u32 = u32::MAX;

#[derive(Clone, Debug)]
pub enum Code {
    Lit(Lit),
    Local(usize),
    Global(Symbol),
    Unary(UnOp, Box<Code>),
    Binary(BinOp, Box<Code>, Box<Code>),
    App(Box<Code>, Vec<Code>),
    If(Box<Code>, Box<Code>, Box<Code>),
    Block(Vec<Stmt>, Option<Box<Code>>),
    List(Vec<Code>),
    Record(Vec<(Symbol, Code)>),
    Field(Box<Code>, Symbol),
    Match(Box<Code>, Vec<Arm>),
    Lambda {
        params: usize,
        captures: Vec<(usize, usize)>,
        body: Box<Code>,
    },
    Region,
    Unreached,
}

#[derive(Clone, Debug)]
pub enum Stmt {
    Let(usize, Code),
    LetPat(Pat, Code),
}

#[derive(Clone, Debug)]
pub enum Pat {
    Wild,
    Var(usize),
    Lit(Lit),
    Ctor(Symbol, Vec<Pat>),
    List(Vec<Pat>, Option<Box<Pat>>),
    Nested(Vec<Pat>),
}

#[derive(Clone, Debug)]
pub struct Arm {
    pub pat: Pat,
    pub guard: Option<Code>,
    pub body: Code,
}

#[derive(Clone, Debug)]
pub struct Clause {
    pub span: Span,
    pub code: Code,
}

#[derive(Clone, Debug)]
pub struct Definition {
    pub params: usize,
    pub body: Code,
    pub spec: Vec<(SpecKind, Clause)>,
    pub refs: BTreeSet<Symbol>,
}

#[derive(Clone, Debug)]
pub struct Law {
    pub guard: Option<Clause>,
    pub body: Code,
}

#[derive(Clone, Debug, Default)]
pub struct Claims {
    pub defs: HashMap<Symbol, Definition>,
    pub laws: HashMap<Symbol, Law>,
    pub sums: BTreeMap<Symbol, usize>,
}

/// `sources[i]` is the source of the `i`th module the dump was asked over.
pub fn read_claims(dump: &str, sources: &[SourceId]) -> Result<Claims, String> {
    let mut frames = Cursor::new(dump.as_bytes(), "frame");
    let mut claims = Claims::default();
    while !frames.done() {
        let (words, payload) = frames.unit()?;
        let [kind, name] = words[..] else {
            return Err(format!(
                "frame header `{}` is not `<kind> <name> <length>`",
                words.join(" ")
            ));
        };
        let what = format!("{kind} `{name}`");
        let pairs = fields(payload, &what)?;
        match kind {
            "def" => {
                let (mut params, mut body) = (None, None);
                let (mut spec, mut refs) = (Vec::new(), BTreeSet::new());
                for (key, text) in pairs {
                    match key {
                        "params" => params = Some(number(text, &what)?),
                        "body" => body = Some(code(text, &what, &mut refs)?),
                        "requires" => {
                            spec.push((SpecKind::Requires, clause(text, sources, &what)?))
                        }
                        "ensures" => spec.push((SpecKind::Ensures, clause(text, sources, &what)?)),
                        other => return Err(format!("{what}: unknown field `{other}`")),
                    }
                }
                let definition = Definition {
                    params: params.ok_or_else(|| format!("{what} has no `params`"))?,
                    body: body.ok_or_else(|| format!("{what} has no `body`"))?,
                    spec,
                    refs,
                };
                claims.defs.insert(Symbol::new(name), definition);
            }
            "law" => {
                let (mut key, mut guard, mut body) = (None, None, None);
                for (field, text) in pairs {
                    match field {
                        "key" => key = Some(Symbol::new(text)),
                        "guard" => guard = Some(clause(text, sources, &what)?),
                        "body" => body = Some(code(text, &what, &mut BTreeSet::new())?),
                        other => return Err(format!("{what}: unknown field `{other}`")),
                    }
                }
                let law = Law {
                    guard,
                    body: body.ok_or_else(|| format!("{what} has no `body`"))?,
                };
                claims
                    .laws
                    .insert(key.ok_or_else(|| format!("{what} has no `key`"))?, law);
            }
            "sum" => {
                let mut variants = None;
                for (field, text) in pairs {
                    match field {
                        "variants" => variants = Some(number(text, &what)?),
                        other => return Err(format!("{what}: unknown field `{other}`")),
                    }
                }
                let variants = variants.ok_or_else(|| format!("{what} has no `variants`"))?;
                claims.sums.insert(Symbol::new(name), variants);
            }
            other => return Err(format!("{what}: unknown frame kind `{other}`")),
        }
    }
    Ok(claims)
}

fn fields<'a>(payload: &'a [u8], what: &str) -> Result<Vec<(&'a str, &'a str)>, String> {
    let mut cursor = Cursor::new(payload, "field");
    let mut out = Vec::new();
    while !cursor.done() {
        let (words, body) = cursor.unit()?;
        let [key] = words[..] else {
            return Err(format!(
                "{what}: field header `{}` is not `<key> <length>`",
                words.join(" ")
            ));
        };
        let text =
            std::str::from_utf8(body).map_err(|e| format!("{what}: `{key}` is not UTF-8: {e}"))?;
        out.push((key, text));
    }
    Ok(out)
}

fn number<N: std::str::FromStr>(text: &str, what: &str) -> Result<N, String> {
    text.parse()
        .map_err(|_| format!("{what}: `{text}` is not a number"))
}

fn clause(text: &str, sources: &[SourceId], what: &str) -> Result<Clause, String> {
    let (at, lowered) = text
        .split_once('\n')
        .ok_or_else(|| format!("{what}: a clause has no span line"))?;
    Ok(Clause {
        span: span(at, sources, what)?,
        code: code(lowered, what, &mut BTreeSet::new())?,
    })
}

fn span(text: &str, sources: &[SourceId], what: &str) -> Result<Span, String> {
    let words: Vec<&str> = text.split(' ').collect();
    let [module, start, end] = words[..] else {
        return Err(format!("{what}: `{text}` is not `<module> <start> <end>`"));
    };
    let number = |word: &str| {
        word.parse::<u32>()
            .map_err(|_| format!("{what}: span `{text}` holds `{word}`"))
    };
    let module = number(module)?;
    let source = if module == NO_MODULE {
        Span::DUMMY.source
    } else {
        *sources.get(module as usize).ok_or_else(|| {
            format!(
                "{what} spans module {module}, and only {} sources were handed over",
                sources.len()
            )
        })?
    };
    Ok(Span::new(source, number(start)?, number(end)?))
}

fn code(text: &str, what: &str, refs: &mut BTreeSet<Symbol>) -> Result<Code, String> {
    if text.is_empty() {
        return Ok(Code::Unreached);
    }
    let mut dump = Dump {
        text: text.as_bytes(),
        at: 0,
        what,
        refs,
    };
    let out = dump.code()?;
    if dump.at != dump.text.len() {
        return dump.fail("the end of the body");
    }
    Ok(out)
}

struct Dump<'a, 'r> {
    text: &'a [u8],
    at: usize,
    what: &'a str,
    refs: &'r mut BTreeSet<Symbol>,
}

impl<'a> Dump<'a, '_> {
    fn fail<T>(&self, expected: &str) -> Result<T, String> {
        Err(format!(
            "{}: expected {expected} at byte {} of a lowered body",
            self.what, self.at
        ))
    }

    fn peek(&self, byte: u8) -> bool {
        self.text.get(self.at) == Some(&byte)
    }

    fn eat(&mut self, byte: u8) -> bool {
        let found = self.peek(byte);
        if found {
            self.at += 1;
        }
        found
    }

    fn eat_str(&mut self, s: &str) -> bool {
        let found = self.text[self.at..].starts_with(s.as_bytes());
        if found {
            self.at += s.len();
        }
        found
    }

    fn expect(&mut self, byte: u8) -> Result<(), String> {
        if self.eat(byte) {
            Ok(())
        } else {
            self.fail(&format!("`{}`", byte as char))
        }
    }

    fn expect_str(&mut self, s: &str) -> Result<(), String> {
        if self.eat_str(s) {
            Ok(())
        } else {
            self.fail(&format!("`{s}`"))
        }
    }

    fn word(&mut self, ends: &[u8]) -> Result<&'a str, String> {
        let text = self.text;
        let start = self.at;
        while self.at < text.len() && !ends.contains(&text[self.at]) {
            self.at += 1;
        }
        if self.at == text.len() {
            return self.fail("a delimiter");
        }
        std::str::from_utf8(&text[start..self.at]).map_err(|e| format!("{}: {e}", self.what))
    }

    fn number<N: std::str::FromStr>(&mut self, ends: &[u8]) -> Result<N, String> {
        let word = self.word(ends)?;
        word.parse()
            .map_err(|_| format!("{}: `{word}` is not a number", self.what))
    }

    fn slot(&mut self, ends: &[u8]) -> Result<Option<usize>, String> {
        if self.eat(b'-') {
            return Ok(None);
        }
        self.number(ends).map(Some)
    }

    fn code(&mut self) -> Result<Code, String> {
        stacker::maybe_grow(256 * 1024, 2 * 1024 * 1024, || self.node())
    }

    fn node(&mut self) -> Result<Code, String> {
        if !(self.eat(b'b') || self.eat(b'o')) {
            return self.fail("an ownership mark");
        }
        let head = self.word(b"(")?;
        self.expect(b'(')?;
        let out = match head {
            "lit" => Code::Lit(self.lit(b")")?),
            "var" => {
                let name = self.word(b",")?;
                self.expect(b',')?;
                match self.slot(b")")? {
                    Some(slot) => Code::Local(slot),
                    None => {
                        let name = Symbol::new(name);
                        self.refs.insert(name.clone());
                        Code::Global(name)
                    }
                }
            }
            "un" => {
                let op = match self.word(b",")? {
                    "neg" => UnOp::Neg,
                    "not" => UnOp::Not,
                    "bitnot" => UnOp::BitNot,
                    _ => return self.fail("a unary operator"),
                };
                self.expect(b',')?;
                Code::Unary(op, Box::new(self.code()?))
            }
            "bin" => {
                let Some(op) = binary(self.word(b",")?) else {
                    return self.fail("a binary operator");
                };
                self.expect(b',')?;
                let lhs = self.code()?;
                self.expect(b',')?;
                Code::Binary(op, Box::new(lhs), Box::new(self.code()?))
            }
            "app" => {
                let func = self.code()?;
                let mut args = Vec::new();
                while self.eat(b',') {
                    args.push(self.code()?);
                }
                Code::App(Box::new(func), args)
            }
            "if" => {
                let cond = self.code()?;
                self.expect(b',')?;
                let then_branch = self.code()?;
                self.expect(b',')?;
                Code::If(
                    Box::new(cond),
                    Box::new(then_branch),
                    Box::new(self.code()?),
                )
            }
            "block" => {
                let mut stmts = Vec::new();
                loop {
                    if self.eat_str("let(") {
                        let pat = self.pat()?;
                        self.expect(b',')?;
                        let value = self.code()?;
                        self.expect_str("),")?;
                        stmts.push(match pat {
                            Pat::Var(slot) => Stmt::Let(slot, value),
                            pat => Stmt::LetPat(pat, value),
                        });
                    } else if self.eat_str("do(") {
                        self.code()?;
                        self.expect_str("),")?;
                    } else {
                        break;
                    }
                }
                let tail = if self.eat(b'-') {
                    None
                } else {
                    Some(Box::new(self.code()?))
                };
                Code::Block(stmts, tail)
            }
            "list" => {
                let mut items = Vec::new();
                while !self.peek(b')') {
                    items.push(self.code()?);
                    self.expect(b',')?;
                }
                Code::List(items)
            }
            "rec" => {
                let mut fields = Vec::new();
                while !self.peek(b')') {
                    let name = Symbol::new(self.word(b"=")?);
                    self.expect(b'=')?;
                    fields.push((name, self.code()?));
                    self.expect(b',')?;
                }
                Code::Record(fields)
            }
            "match" => {
                let scrutinee = self.code()?;
                let mut arms = Vec::new();
                while self.eat_str(",arm(") {
                    let pat = self.pat()?;
                    self.expect(b',')?;
                    let guard = if self.eat(b'-') {
                        None
                    } else {
                        Some(self.code()?)
                    };
                    self.expect(b',')?;
                    let body = self.code()?;
                    self.expect(b')')?;
                    arms.push(Arm { pat, guard, body });
                }
                Code::Match(Box::new(scrutinee), arms)
            }
            "perform" => {
                for _ in 0..2 {
                    self.word(b",")?;
                    self.expect(b',')?;
                }
                self.word(b",)")?;
                while self.eat(b',') {
                    self.code()?;
                }
                Code::Region
            }
            "cell" => {
                for _ in 0..3 {
                    self.word(b",")?;
                    self.expect(b',')?;
                }
                self.code()?;
                self.expect(b',')?;
                self.code()?;
                Code::Region
            }
            "region" => {
                self.code()?;
                Code::Region
            }
            "sim" => {
                self.word(b",")?;
                self.expect(b',')?;
                self.captures()?;
                self.expect(b',')?;
                self.code()?;
                Code::Region
            }
            "handle" => {
                self.code()?;
                loop {
                    self.expect(b',')?;
                    if self.eat(b'-') {
                        break;
                    }
                    let ret = self.eat_str("ret(");
                    if !ret {
                        self.expect_str("cl(")?;
                        for _ in 0..5 {
                            self.word(b",")?;
                            self.expect(b',')?;
                        }
                    }
                    self.word(b",")?;
                    self.expect(b',')?;
                    self.captures()?;
                    self.expect(b',')?;
                    self.code()?;
                    self.expect(b')')?;
                    if ret {
                        break;
                    }
                }
                Code::Region
            }
            "lam" => {
                let params = self.number(b",")?;
                self.expect(b',')?;
                self.word(b",")?;
                self.expect(b',')?;
                let captures = self.captures()?;
                self.expect(b',')?;
                Code::Lambda {
                    params,
                    captures,
                    body: Box::new(self.code()?),
                }
            }
            "fld" => {
                let base = self.code()?;
                self.expect(b',')?;
                Code::Field(Box::new(base), Symbol::new(self.word(b")")?))
            }
            "upd" => {
                let base = self.code()?;
                let mut fields = Vec::new();
                loop {
                    if self.eat_str(",c:") {
                        let name = Symbol::new(self.word(b",)")?);
                        fields.push((name.clone(), Code::Field(Box::new(base.clone()), name)));
                    } else if self.eat_str(",s:") {
                        let name = Symbol::new(self.word(b"=")?);
                        self.expect(b'=')?;
                        fields.push((name, self.code()?));
                    } else {
                        break;
                    }
                }
                Code::Record(fields)
            }
            _ => return self.fail("a node"),
        };
        self.expect(b')')?;
        Ok(out)
    }

    fn captures(&mut self) -> Result<Vec<(usize, usize)>, String> {
        self.expect(b'[')?;
        let mut out = Vec::new();
        while !self.eat(b']') {
            let outer = self.number(b">")?;
            self.expect(b'>')?;
            let inner = self.number(b"bo")?;
            if !(self.eat(b'b') || self.eat(b'o')) {
                return self.fail("an ownership mark");
            }
            self.expect(b' ')?;
            out.push((outer, inner));
        }
        Ok(out)
    }

    fn pat(&mut self) -> Result<Pat, String> {
        if self.eat(b'_') {
            return Ok(Pat::Wild);
        }
        if self.eat_str("v:") {
            self.word(b":")?;
            self.expect(b':')?;
            return match self.slot(b",)")? {
                Some(slot) => Ok(Pat::Var(slot)),
                None => self.fail("a binder's slot"),
            };
        }
        if self.eat_str("l:") {
            return Ok(Pat::Lit(self.lit(b",)")?));
        }
        if self.eat_str("c:") {
            let name = Symbol::new(self.word(b"(")?);
            self.expect(b'(')?;
            let mut args = Vec::new();
            while !self.eat(b')') {
                args.push(self.pat()?);
                self.expect(b',')?;
            }
            return Ok(Pat::Ctor(name, args));
        }
        let mut inner = Vec::new();
        if self.eat_str("r:(") {
            while !(self.eat_str("..)") || self.eat(b')')) {
                self.word(b"=")?;
                self.expect(b'=')?;
                inner.push(self.pat()?);
                self.expect(b',')?;
            }
            return Ok(Pat::Nested(inner));
        }
        if self.eat_str("s:(") {
            let mut rest = None;
            while !self.eat(b')') {
                if self.eat_str("..") {
                    rest = Some(Box::new(self.pat()?));
                } else {
                    inner.push(self.pat()?);
                    self.expect(b',')?;
                }
            }
            return Ok(Pat::List(inner, rest));
        }
        self.fail("a pattern")
    }

    fn lit(&mut self, ends: &[u8]) -> Result<Lit, String> {
        let Some(&tag) = self.text.get(self.at) else {
            return self.fail("a literal");
        };
        self.at += 1;
        Ok(match tag {
            b'i' => Lit::Int(self.number(ends)?),
            b'x' => {
                let name = self.word(b":")?;
                self.expect(b':')?;
                let bits: i64 = self.number(ends)?;
                let Some(ty) = IntTy::from_name(&name.to_ascii_uppercase()) else {
                    return self.fail("a width");
                };
                Lit::Fixed {
                    ty,
                    bits: ty.normalize(bits as u64),
                }
            }
            b'T' => Lit::Bool(true),
            b'F' => Lit::Bool(false),
            b's' => {
                let bytes = self.hex()?;
                Lit::Str(String::from_utf8(bytes).map_err(|e| format!("{}: {e}", self.what))?)
            }
            b'y' => Lit::Bytes(self.hex()?),
            // The dump carries no value, and the prover reads none: a `Float` is never proved.
            b'f' => Lit::Float(f64::NAN),
            b'd' => {
                let raw =
                    String::from_utf8(self.hex()?).map_err(|e| format!("{}: {e}", self.what))?;
                match decimal(&raw) {
                    Some(lit) => lit,
                    None => return self.fail("a `Decimal` literal"),
                }
            }
            b'u' => Lit::Unit,
            _ => {
                self.at -= 1;
                return self.fail("a literal");
            }
        })
    }

    fn hex(&mut self) -> Result<Vec<u8>, String> {
        let len: usize = self.number(b":")?;
        self.expect(b':')?;
        let Some(digits) = self.text.get(self.at..self.at + 2 * len) else {
            return self.fail("hex digits");
        };
        self.at += 2 * len;
        digits
            .chunks(2)
            .map(|pair| {
                std::str::from_utf8(pair)
                    .ok()
                    .and_then(|p| u8::from_str_radix(p, 16).ok())
                    .ok_or_else(|| format!("{}: `{pair:?}` is not a hex byte", self.what))
            })
            .collect()
    }
}

/// `1.50m` is `(150, 2)`; a pattern's `-` is part of its source, and negates it.
fn decimal(raw: &str) -> Option<Lit> {
    let (whole, fraction) = raw.split_once('.').unwrap_or((raw, ""));
    let digits: String = whole
        .chars()
        .chain(fraction.chars())
        .filter(char::is_ascii_digit)
        .collect();
    let magnitude: i128 = digits.parse().ok()?;
    Some(Lit::Decimal {
        mantissa: if raw.starts_with('-') {
            -magnitude
        } else {
            magnitude
        },
        scale: fraction.chars().filter(char::is_ascii_digit).count() as u32,
    })
}

fn binary(name: &str) -> Option<BinOp> {
    Some(match name {
        "add" => BinOp::Add,
        "sub" => BinOp::Sub,
        "mul" => BinOp::Mul,
        "div" => BinOp::Div,
        "rem" => BinOp::Rem,
        "eq" => BinOp::Eq,
        "ne" => BinOp::Ne,
        "lt" => BinOp::Lt,
        "le" => BinOp::Le,
        "gt" => BinOp::Gt,
        "ge" => BinOp::Ge,
        "and" => BinOp::And,
        "or" => BinOp::Or,
        "concat" => BinOp::Concat,
        "bitand" => BinOp::BitAnd,
        "bitor" => BinOp::BitOr,
        "bitxor" => BinOp::BitXor,
        "shl" => BinOp::Shl,
        "shr" => BinOp::Shr,
        "ushr" => BinOp::Ushr,
        _ => return None,
    })
}
