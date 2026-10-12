//! What a compiled unit says about itself, embedded in its C so loading reads no source. Helpers
//! bind by name, so a unit serves while the runtime has every helper its C calls.

use super::descriptor::Descriptor;
use super::encoding::{count, decode_pools, encode_pools, line, section};
use super::prelude::helpers;
use crate::heap::RecordLayout;
use anyhow::{Result, anyhow};
use ply_eval::{Symbol, Value};

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

/// What a unit's loader reads of it once. What it binds, the code table and each definition's
/// entry, the unit lays out in its [`Descriptor`] instead.
#[derive(Clone)]
pub struct Exports {
    pub helpers: Vec<HelperShape>,
    /// The constructor table the C's tags index: each one's name, arity, and arguments as a shape
    /// spells them, `a000` first, none where it holds every argument as its word.
    pub ctors: Vec<(Symbol, usize, Vec<Symbol>)>,
    /// The pure nullary functions, which the seam memoizes, each with its row of the code table.
    pub constants: Vec<(String, usize)>,
    /// How many modules the unit was emitted from.
    pub modules: usize,
    /// What the fixpoint dropped and why.
    pub refusals: Vec<(String, String)>,
    pub consts: Vec<Value>,
    pub fields: Vec<Symbol>,
    pub builtins: Vec<ply_eval::Builtin>,
    pub shapes: Vec<Vec<Symbol>>,
    /// Each constructor whose type's module states a `key` or a `show`, with the function of
    /// each: `-` where it states none, `!` for one the unit does not take.
    pub instances: Vec<(Symbol, String, String)>,
    /// Each constructor whose type's module states a `numeric`, with its functions in the
    /// order of [`ply_eval::instances`]' roles, spelled as the others are. Each has a row above.
    pub numerics: Vec<(Symbol, [String; 5])>,
    /// How the unit's C reads a record or an array, as its [`IN_PLACE`], [`RAW_WIDTHS`] and
    /// [`INLINE_ARRAYS`] lines say.
    pub layout: RecordLayout,
}

/// A line after a unit's tables, where its C holds a record of a record type in place. A unit
/// without it was emitted by a `ply` that holds every record by its word, as a bootstrap's first
/// builder is.
pub const IN_PLACE: &str = "records held in place";

/// A line after a unit's tables, where its C holds a 64-bit width as its bits wherever a record
/// holds one; a unit without it holds the width's word there.
pub const RAW_WIDTHS: &str = "widths held raw";

/// A line after a unit's tables, where its C reads an array's records, `Float`s and 64-bit widths
/// in place; a unit without it reads each element's word.
pub const INLINE_ARRAYS: &str = "arrays hold their elements in place";

/// A line after a unit's tables, where its C reads a constructor's arguments as its declaration
/// types them, as the unit's constructor table spells them; a unit without it reads their words.
pub const CTOR_ARGS: &str = "constructors hold their arguments by type";

impl Exports {
    /// The address of each helper the unit binds, in its order; a unit recording one this runtime
    /// lacks, or has under another shape, is refused.
    pub fn bound(&self) -> Result<Vec<*mut std::ffi::c_void>, Unserved> {
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
                    None => Err(Unserved(format!(
                        "it calls `{}`, which this runtime does not have",
                        mine.name
                    ))),
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
        for (name, arity, args) in &self.ctors {
            out.push_str(&format!("{name} {arity}"));
            for a in args {
                out.push_str(&format!(" {a}"));
            }
            out.push('\n');
        }
        out.push_str(&format!("constants {}\n", self.constants.len()));
        for (name, slot) in &self.constants {
            out.push_str(&format!("{name} {slot}\n"));
        }
        out.push_str(&format!("modules {}\n", self.modules));
        out.push_str(&format!("refused {}\n", self.refusals.len()));
        for (function, construct) in &self.refusals {
            out.push_str(&format!("{function}\n{construct}\n"));
        }
        out.push_str(&encode_pools(
            &self.consts,
            &self.builtins,
            &self.fields,
            &self.shapes,
        ));
        // Written only by a unit one of whose types states a `key` or a `show`.
        if !self.instances.is_empty() {
            out.push_str(&format!("instances {}\n", self.instances.len()));
            for (ctor, key, show) in &self.instances {
                out.push_str(&format!("{ctor} {key} {show}"));
                if let Some((_, fs)) = self.numerics.iter().find(|(c, _)| c == ctor) {
                    out.push_str(&format!(" {}", fs.join(" ")));
                }
                out.push('\n');
            }
        }
        if self.layout.in_place {
            out.push_str(&format!("{IN_PLACE}\n"));
        }
        if self.layout.raw_widths {
            out.push_str(&format!("{RAW_WIDTHS}\n"));
        }
        if self.layout.inline_arrays {
            out.push_str(&format!("{INLINE_ARRAYS}\n"));
        }
        if self.layout.ctor_args {
            out.push_str(&format!("{CTOR_ARGS}\n"));
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
            let mut parts = line(s, &mut at)?.split(' ');
            let name = Symbol::new(parts.next()?);
            let arity = parts.next()?.parse().ok()?;
            ctors.push((name, arity, parts.map(Symbol::new).collect()));
        }
        let n = count(line(s, &mut at)?, "constants")?;
        let mut constants = Vec::with_capacity(n);
        for _ in 0..n {
            let (name, slot) = line(s, &mut at)?.split_once(' ')?;
            constants.push((name.to_string(), slot.parse().ok()?));
        }
        let modules = count(line(s, &mut at)?, "modules")?;
        let n = count(line(s, &mut at)?, "refused")?;
        let mut refusals = Vec::with_capacity(n);
        for _ in 0..n {
            let function = line(s, &mut at)?.to_string();
            let construct = line(s, &mut at)?.to_string();
            refusals.push((function, construct));
        }
        let t = decode_pools(s, &mut at)?;
        // A unit none of whose types states a `key` or a `show` has no such table.
        let n = section(s, &mut at, "instances")?;
        let mut instances = Vec::with_capacity(n);
        let mut numerics = Vec::new();
        for _ in 0..n {
            let mut parts = line(s, &mut at)?.split(' ');
            let ctor = Symbol::new(parts.next()?);
            instances.push((
                ctor.clone(),
                parts.next()?.to_string(),
                parts.next()?.to_string(),
            ));
            let rest: Vec<String> = parts.map(str::to_string).collect();
            if !rest.is_empty() {
                numerics.push((ctor, <[String; 5]>::try_from(rest).ok()?));
            }
        }
        // What layout its C reads, a line each, in any order; one this runtime does not know reads
        // as nothing.
        let mut layout = RecordLayout {
            in_place: false,
            raw_widths: false,
            inline_arrays: false,
            ctor_args: false,
        };
        while at < s.len() {
            match line(s, &mut at)? {
                IN_PLACE => layout.in_place = true,
                RAW_WIDTHS => layout.raw_widths = true,
                INLINE_ARRAYS => layout.inline_arrays = true,
                CTOR_ARGS => layout.ctor_args = true,
                _ => {}
            }
        }
        Some(Exports {
            helpers,
            ctors,
            constants,
            modules,
            refusals,
            consts: t.consts,
            fields: t.fields,
            builtins: t.builtins,
            shapes: t.shapes,
            instances,
            numerics,
            layout,
        })
    }

    /// What `unit` says about itself.
    pub fn read(unit: &Descriptor) -> Result<Exports> {
        let text = unit
            .exports()
            .ok_or_else(|| anyhow!("what the unit says about itself is not UTF-8"))?;
        Exports::decode(text).ok_or_else(|| {
            anyhow!("what the unit says about itself does not read as a unit's table")
        })
    }
}
