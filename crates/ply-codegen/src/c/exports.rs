//! What a compiled unit says about itself, carried in its own C so that loading one reads no
//! source and no side file: the constructor table its tags are positions in, every function it
//! holds with its arity, which of those are pure constants, how many modules it was emitted from,
//! what the fixpoint refused, and the five tables its bodies name by position.
//!
//! It is a string in the object, `ply_exports`, NUL-terminated, read back through `dlsym` once the
//! unit loads. A unit is then one file: the bootstrap bundle is its C, an artifact embeds its C,
//! and the whole-unit cache keeps an object's key and nothing beside it. This is the first piece
//! of separate compilation: a unit can be linked against with none of its sources present.

use super::cache::{count, decode_tables, encode_tables, line};
use super::load::Library;
use anyhow::{Result, anyhow};
use ply_eval::Value;
use ply_span::Symbol;

/// The symbol the table is read from.
pub const SYMBOL: &str = "ply_exports";

/// Bytes of one encoded line per C string literal, so no literal is longer than every compiler
/// takes: a pooled byte constant is one line, and one line can be a hundred kilobytes of hex.
const PIECE: usize = 2000;

#[derive(Clone)]
pub struct Exports {
    /// The table the C's tags are positions in, in tag order.
    pub ctors: Vec<(Symbol, usize)>,
    /// Every function the unit holds, with its arity, in the order the fixpoint took them.
    pub taken: Vec<(String, usize)>,
    /// The taken functions that are nullary and pure by their published row, which the seam
    /// remembers rather than runs.
    pub constants: Vec<String>,
    /// How many modules the unit was emitted from; a body stores its span against a module index.
    pub modules: usize,
    /// What the fixpoint dropped and why, so a unit loaded anywhere reports the refusals of the
    /// build that produced it.
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

    pub fn encode(&self) -> String {
        let mut out = format!("ctors {}\n", self.ctors.len());
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

    /// The table as C: one string literal per piece of a line, every byte outside plain printable
    /// ASCII written in octal, so the text is a function of the table alone whatever a name holds.
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

    /// The table a loaded unit carries.
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
