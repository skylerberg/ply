//! What a compiled unit says about itself, embedded in its C so loading reads no source. Helpers
//! bind by position, so a unit serves while the runtime's helper table starts with the unit's.

use super::cache::{count, decode_tables, encode_tables, line};
use super::load::Library;
use super::prelude::HELPERS;
use anyhow::{Result, anyhow};
use ply_eval::Value;
use ply_span::Symbol;

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
    HELPERS
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
    for helper in HELPERS {
        h.update(format!("{} {} {}\n", helper.name, helper.args, helper.answers).as_bytes());
    }
    h.finalize().to_hex().to_string()
}

/// Why a unit does not serve this runtime: its first helper the runtime lacks at that position.
#[derive(Debug)]
pub struct Unserved(pub String);

impl std::fmt::Display for Unserved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the unit does not serve this runtime: {}", self.0)
    }
}

impl std::error::Error for Unserved {}

#[derive(Clone)]
pub struct Exports {
    pub helpers: Vec<HelperShape>,
    /// The constructor table the C's tags index.
    pub ctors: Vec<(Symbol, usize)>,
    /// Every function the unit holds, with its arity.
    pub taken: Vec<(String, usize)>,
    /// The pure nullary functions, which the seam memoizes.
    pub constants: Vec<String>,
    /// How many modules the unit was emitted from; spans store a module index.
    pub modules: usize,
    /// What the fixpoint dropped and why.
    pub refusals: Vec<(String, String)>,
    pub consts: Vec<Value>,
    pub fields: Vec<Symbol>,
    pub builtins: Vec<ply_eval::Builtin>,
    pub shapes: Vec<Vec<Symbol>>,
    pub lambdas: Vec<String>,
}

impl Exports {
    pub fn names(&self) -> Vec<String> {
        self.taken.iter().map(|(n, _)| n.clone()).collect()
    }

    /// `None` when the runtime's helper table starts with this unit's.
    pub fn unserved(&self) -> Option<Unserved> {
        let runtime = runtime_helpers();
        for (i, mine) in self.helpers.iter().enumerate() {
            match runtime.get(i) {
                Some(theirs) if theirs == mine => {}
                Some(theirs) => {
                    return Some(Unserved(format!(
                        "helper {i} is `{}` taking {} and {}, and this runtime's is `{}` taking {} and {}",
                        mine.name,
                        mine.args,
                        if mine.answers {
                            "answering"
                        } else {
                            "answering nothing"
                        },
                        theirs.name,
                        theirs.args,
                        if theirs.answers {
                            "answering"
                        } else {
                            "answering nothing"
                        },
                    )));
                }
                None => {
                    return Some(Unserved(format!(
                        "helper {i} is `{}`, past the {} this runtime has",
                        mine.name,
                        runtime.len()
                    )));
                }
            }
        }
        None
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
        for (name, arity) in &self.taken {
            out.push_str(&format!("{name} {arity}\n"));
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
            let (name, arity) = line(s, &mut at)?.rsplit_once(' ')?;
            taken.push((name.to_string(), arity.parse().ok()?));
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
