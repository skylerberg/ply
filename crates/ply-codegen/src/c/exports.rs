//! What a compiled unit says about itself, embedded in its C so loading reads no source. Helpers
//! bind by name, so a unit serves while the runtime has every helper its C calls.

use super::encoding::{count, decode_tables, encode_tables, line};
use super::load::Library;
use super::prelude::{helpers, pointer_name};
use super::tables::Defined;
use anyhow::{Result, anyhow};
use ply_eval::{Symbol, Value};

/// The symbol the table is read from.
pub const SYMBOL: &str = "ply_exports";

/// Bytes per C string literal, so no literal exceeds what compilers accept.
const PIECE: usize = 2000;

/// One runtime helper as a unit records it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelperShape {
    pub name: String,
    pub args: usize,
    pub answers: bool,
}

pub fn runtime_helpers() -> Vec<HelperShape> {
    helpers()
        .iter()
        .map(|h| HelperShape {
            name: h.name.to_string(),
            args: h.args,
            answers: h.answers,
        })
        .collect()
}

/// A digest of the runtime's whole helper table, for cache keys (stricter than serving needs).
pub fn helpers_digest() -> String {
    let mut h = blake3::Hasher::new();
    for helper in helpers() {
        h.update(format!("{} {} {}\n", helper.name, helper.args, helper.answers).as_bytes());
    }
    h.finalize().to_hex().to_string()
}

/// Why a unit does not serve this runtime: a helper its C calls that the runtime does not have as
/// the unit knows it.
#[derive(Debug)]
pub struct Unserved(pub String);

impl std::fmt::Display for Unserved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the unit does not serve this runtime: {}", self.0)
    }
}

impl std::error::Error for Unserved {}

/// One function the unit holds: its Ply name, its arity, and the C the emitter wrote for it, so a
/// reader of a unit binds and calls what that unit defines rather than a spelling of its own.
#[derive(Clone)]
pub struct Taken {
    pub name: String,
    pub arity: usize,
    pub symbol: String,
    pub entry: String,
}

#[derive(Clone)]
pub struct Exports {
    pub helpers: Vec<HelperShape>,
    /// The constructor table the C's tags index.
    pub ctors: Vec<(Symbol, usize)>,
    /// Every function the unit holds.
    pub taken: Vec<Taken>,
    /// The pure nullary functions, which the seam memoizes.
    pub constants: Vec<String>,
    /// How many modules the unit was emitted from.
    pub modules: usize,
    /// What the fixpoint dropped and why.
    pub refusals: Vec<(String, String)>,
    pub consts: Vec<Value>,
    pub fields: Vec<Symbol>,
    pub builtins: Vec<ply_eval::Builtin>,
    pub shapes: Vec<Vec<Symbol>>,
    pub lambdas: Vec<String>,
    /// Each bucket's table by the bucket's id: the unit's position of every key its C names.
    pub buckets: Vec<(u8, Vec<u32>)>,
    /// Each constructor whose type's module states a `key` or a `show`, with the function of
    /// each: `-` where it states none, `!` for one the unit does not take.
    pub instances: Vec<(Symbol, String, String)>,
}

/// Whether `text` calls `helper` through its pointer, as the emitter writes a call: the name
/// whole, then `(`.
fn calls(text: &str, helper: &str) -> bool {
    let call = format!("{}(", pointer_name(helper));
    text.match_indices(&call).any(|(at, _)| {
        !text[..at]
            .bytes()
            .next_back()
            .is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_')
    })
}

impl Exports {
    pub fn names(&self) -> Vec<String> {
        self.taken.iter().map(|t| t.name.clone()).collect()
    }

    pub fn taken_by_name(&self, name: &str) -> Option<&Taken> {
        self.taken.iter().find(|t| t.name == name)
    }

    /// The address each helper the unit records binds to, in the unit's order. One this runtime
    /// no longer has binds to nothing, which serves a unit whose C never calls it: `text` is
    /// that C.
    pub fn bound(&self, text: &str) -> Result<Vec<*mut std::ffi::c_void>, Unserved> {
        let said = |h: &HelperShape| {
            format!(
                "taking {} and {}",
                h.args,
                if h.answers {
                    "answering"
                } else {
                    "answering nothing"
                }
            )
        };
        self.helpers
            .iter()
            .map(
                |mine| match helpers().iter().find(|h| h.name == mine.name) {
                    Some(h) if h.args == mine.args && h.answers == mine.answers => {
                        Ok(h.address as *mut std::ffi::c_void)
                    }
                    Some(h) => Err(Unserved(format!(
                        "it was emitted against `{}` {}, and this runtime's is {}",
                        mine.name,
                        said(mine),
                        said(&HelperShape {
                            name: h.name.to_string(),
                            args: h.args,
                            answers: h.answers,
                        }),
                    ))),
                    None if calls(text, &mine.name) => Err(Unserved(format!(
                        "it calls `{}`, which this runtime does not have",
                        mine.name
                    ))),
                    None => Ok(std::ptr::null_mut()),
                },
            )
            .collect()
    }

    pub fn encode(&self) -> String {
        let mut out = format!("helpers {}\n", self.helpers.len());
        for h in &self.helpers {
            out.push_str(&format!("{} {} {}\n", h.name, h.args, u8::from(h.answers)));
        }
        out.push_str(&format!("ctors {}\n", self.ctors.len()));
        for (name, arity) in &self.ctors {
            out.push_str(&format!("{name} {arity}\n"));
        }
        out.push_str(&format!("taken {}\n", self.taken.len()));
        for t in &self.taken {
            out.push_str(&format!(
                "{} {} {} {}\n",
                t.name, t.arity, t.symbol, t.entry
            ));
        }
        out.push_str(&format!("constants {}\n", self.constants.len()));
        for name in &self.constants {
            out.push_str(&format!("{name}\n"));
        }
        out.push_str(&format!("modules {}\n", self.modules));
        out.push_str(&format!("refused {}\n", self.refusals.len()));
        for (function, construct) in &self.refusals {
            out.push_str(&format!("{function}\n{construct}\n"));
        }
        out.push_str(&encode_tables(
            &self.consts,
            &self.builtins,
            &self.fields,
            &self.shapes,
            &self.lambdas,
        ));
        out.push_str(&format!("buckets {}\n", self.buckets.len()));
        for (id, places) in &self.buckets {
            out.push_str(&id.to_string());
            for place in places {
                out.push_str(&format!(" {place}"));
            }
            out.push('\n');
        }
        // Written only by a unit one of whose types states a `key` or a `show`.
        if !self.instances.is_empty() {
            out.push_str(&format!("instances {}\n", self.instances.len()));
            for (ctor, key, show) in &self.instances {
                out.push_str(&format!("{ctor} {key} {show}\n"));
            }
        }
        out
    }

    pub fn decode(s: &str) -> Option<Exports> {
        let mut at = 0usize;
        let n = count(line(s, &mut at)?, "helpers")?;
        let mut helpers = Vec::with_capacity(n);
        for _ in 0..n {
            let mut parts = line(s, &mut at)?.split(' ');
            let name = parts.next()?.to_string();
            let args = parts.next()?.parse().ok()?;
            let answers = match parts.next()? {
                "1" => true,
                "0" => false,
                _ => return None,
            };
            helpers.push(HelperShape {
                name,
                args,
                answers,
            });
        }
        let n = count(line(s, &mut at)?, "ctors")?;
        let mut ctors = Vec::with_capacity(n);
        for _ in 0..n {
            let (name, arity) = line(s, &mut at)?.rsplit_once(' ')?;
            ctors.push((Symbol::new(name), arity.parse().ok()?));
        }
        let n = count(line(s, &mut at)?, "taken")?;
        let mut taken = Vec::with_capacity(n);
        for _ in 0..n {
            let mut parts = line(s, &mut at)?.split(' ');
            let name = parts.next()?.to_string();
            let arity = parts.next()?.parse().ok()?;
            let published = Defined {
                symbol: parts.next()?.to_string(),
                entry: parts.next()?.to_string(),
            };
            taken.push(Taken {
                name,
                arity,
                symbol: published.symbol,
                entry: published.entry,
            });
        }
        let n = count(line(s, &mut at)?, "constants")?;
        let mut constants = Vec::with_capacity(n);
        for _ in 0..n {
            constants.push(line(s, &mut at)?.to_string());
        }
        let modules = count(line(s, &mut at)?, "modules")?;
        let n = count(line(s, &mut at)?, "refused")?;
        let mut refusals = Vec::with_capacity(n);
        for _ in 0..n {
            let function = line(s, &mut at)?.to_string();
            let construct = line(s, &mut at)?.to_string();
            refusals.push((function, construct));
        }
        let t = decode_tables(s, &mut at)?;
        // A unit whose C writes the unit's positions itself has no bucket tables to fill.
        let n = if at == s.len() {
            0
        } else {
            count(line(s, &mut at)?, "buckets")?
        };
        let mut buckets = Vec::with_capacity(n);
        for _ in 0..n {
            let mut parts = line(s, &mut at)?.split(' ');
            let id = parts.next()?.parse().ok()?;
            let places = parts
                .map(|p| p.parse().ok())
                .collect::<Option<Vec<u32>>>()?;
            buckets.push((id, places));
        }
        // A unit none of whose types states a `key` or a `show` has no such table.
        let n = if at == s.len() {
            0
        } else {
            count(line(s, &mut at)?, "instances")?
        };
        let mut instances = Vec::with_capacity(n);
        for _ in 0..n {
            let mut parts = line(s, &mut at)?.split(' ');
            let ctor = Symbol::new(parts.next()?);
            instances.push((ctor, parts.next()?.to_string(), parts.next()?.to_string()));
        }
        Some(Exports {
            helpers,
            ctors,
            taken,
            constants,
            modules,
            refusals,
            consts: t.consts,
            fields: t.fields,
            builtins: t.builtins,
            shapes: t.shapes,
            lambdas: t.lambdas,
            buckets,
            instances,
        })
    }

    /// The table as C string literals, non-printable bytes in octal.
    pub fn embed(&self) -> String {
        let encoded = self.encode();
        let mut out = String::with_capacity(encoded.len() + encoded.len() / 8);
        out.push_str("\n/* --- what this unit says about itself, read by the loader --- */\n");
        out.push_str(&format!("const char {SYMBOL}[] =\n"));
        for l in encoded.lines() {
            let bytes = l.as_bytes();
            let mut pieces: Vec<&[u8]> = bytes.chunks(PIECE).collect();
            if pieces.is_empty() {
                pieces.push(&[]);
            }
            let last = pieces.len() - 1;
            for (i, piece) in pieces.iter().enumerate() {
                out.push('"');
                for &b in *piece {
                    match b {
                        b'"' => out.push_str("\\\""),
                        b'\\' => out.push_str("\\\\"),
                        0x20..=0x7e if b != b'?' => out.push(b as char),
                        _ => out.push_str(&format!("\\{b:03o}")),
                    }
                }
                if i == last {
                    out.push_str("\\n");
                }
                out.push_str("\"\n");
            }
        }
        out.push_str(";\n");
        out
    }

    /// The table [`Exports::embed`] wrote into a unit's C, read from the text without compiling it.
    pub fn from_text(c: &str) -> Option<Exports> {
        let head = format!("const char {SYMBOL}[] =\n");
        let start = c.rfind(&head)? + head.len();
        let mut encoded: Vec<u8> = Vec::new();
        for line in c[start..].lines() {
            if line == ";" {
                return Exports::decode(&String::from_utf8(encoded).ok()?);
            }
            let piece = line.strip_prefix('"')?.strip_suffix('"')?.as_bytes();
            let mut i = 0;
            while i < piece.len() {
                if piece[i] != b'\\' {
                    encoded.push(piece[i]);
                    i += 1;
                    continue;
                }
                match *piece.get(i + 1)? {
                    b'n' => encoded.push(b'\n'),
                    b @ (b'"' | b'\\') => encoded.push(b),
                    _ => {
                        let octal = std::str::from_utf8(piece.get(i + 1..i + 4)?).ok()?;
                        encoded.push(u8::from_str_radix(octal, 8).ok()?);
                        i += 4;
                        continue;
                    }
                }
                i += 2;
            }
        }
        None
    }

    pub fn read(lib: &Library) -> Result<Exports> {
        let Some(p) = lib.symbol(SYMBOL) else {
            return Err(anyhow!(
                "the unit at {} carries no `{SYMBOL}`",
                lib.path().display()
            ));
        };
        let text = unsafe { std::ffi::CStr::from_ptr(p as *const std::ffi::c_char) }
            .to_str()
            .map_err(|_| anyhow!("the unit's `{SYMBOL}` is not UTF-8"))?;
        Exports::decode(text)
            .ok_or_else(|| anyhow!("the unit's `{SYMBOL}` does not decode as a unit's table"))
    }
}
