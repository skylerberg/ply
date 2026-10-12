//! What a compiled unit says about itself, embedded in its C so loading reads no source. Helpers
//! bind by name, so a unit serves while the runtime has every helper its C calls.

use super::encoding::{count, decode_tables, encode_tables, line};
use super::load::Library;
use super::prelude::helpers;
use super::tables::Defined;
use anyhow::{Result, anyhow};
use ply_eval::{Symbol, Value};

/// The symbol the table is read from.
pub const SYMBOL: &str = "ply_exports";

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
    /// Each constructor whose type's module states a `numeric`, with its functions in the
    /// order of [`ply_eval::instances`]' roles, spelled as the others are. Each has a row above.
    pub numerics: Vec<(Symbol, [String; 5])>,
    /// Whether the unit's C holds a record of a record type in place, as [`IN_PLACE`] says.
    pub in_place: bool,
}

/// The last line of a unit whose C holds a record of a record type in place. A unit without it was
/// emitted by a `ply` that holds every record by its word, as a bootstrap's first builder is.
pub const IN_PLACE: &str = "records held in place";

impl Exports {
    pub fn names(&self) -> Vec<String> {
        self.taken.iter().map(|t| t.name.clone()).collect()
    }

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
                out.push_str(&format!("{ctor} {key} {show}"));
                if let Some((_, fs)) = self.numerics.iter().find(|(c, _)| c == ctor) {
                    out.push_str(&format!(" {}", fs.join(" ")));
                }
                out.push('\n');
            }
        }
        if self.in_place {
            out.push_str(&format!("{IN_PLACE}\n"));
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
        let n = if s[at..].starts_with("instances ") {
            count(line(s, &mut at)?, "instances")?
        } else {
            0
        };
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
        let in_place = at < s.len() && line(s, &mut at)? == IN_PLACE;
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
            numerics,
            in_place,
        })
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
