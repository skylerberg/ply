//! What a compiled unit says about itself, embedded in its C so loading reads no source. Helpers
//! bind by name, so a unit serves while the runtime has every helper its C calls.

use super::descriptor::Descriptor;
use super::encoding::{count, decode_fields, encode_fields, line};
use super::prelude::helpers;
use anyhow::{Result, anyhow};
use ply_core::Symbol;

/// One runtime helper as a unit records it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelperShape {
    pub name: String,
    pub args: usize,
    pub answers: bool,
}

/// A digest of the runtime's whole helper table, for cache keys (stricter than serving needs).
pub fn helpers_digest() -> String {
    let mut h = blake3::Hasher::new();
    for helper in helpers() {
        h.update(format!("{} {} {}\n", helper.name, helper.args(), helper.answers()).as_bytes());
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
    /// The pure nullary functions, which the seam memoizes, each with its row of the code table.
    pub constants: Vec<(String, usize)>,
    /// How many modules the unit was emitted from.
    pub modules: usize,
    pub fields: Vec<Symbol>,
    /// Each root the unit is entered at through its `R#entered`, as its [`ENTERED`] lines say.
    pub entered: Vec<String>,
}

/// What opens a line after a unit's tables naming a root `R` the unit is entered at through the
/// definition `R#entered` it takes, which takes no words: the runtime opens a lowering for the
/// answer and over it a lifting of the arguments' list, and `R#entered` reads them, calls `R` and
/// writes its answer. The runtime enters no root such a line does not name.
pub const ENTERED: &str = "entered ";

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
                    Some(h) if h.args() == mine.args && h.answers() == mine.answers => {
                        Ok(h.address as *mut std::ffi::c_void)
                    }
                    Some(h) => Err(Unserved(format!(
                        "it was emitted against `{}` {}, and this runtime's is {}",
                        mine.name,
                        said(mine),
                        said(&HelperShape {
                            name: h.name.to_string(),
                            args: h.args(),
                            answers: h.answers(),
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
        out.push_str(&format!("constants {}\n", self.constants.len()));
        for (name, slot) in &self.constants {
            out.push_str(&format!("{name} {slot}\n"));
        }
        out.push_str(&format!("modules {}\n", self.modules));
        out.push_str(&encode_fields(&self.fields));
        for root in &self.entered {
            out.push_str(&format!("{ENTERED}{root}\n"));
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
        let n = count(line(s, &mut at)?, "constants")?;
        let mut constants = Vec::with_capacity(n);
        for _ in 0..n {
            let (name, slot) = line(s, &mut at)?.split_once(' ')?;
            constants.push((name.to_string(), slot.parse().ok()?));
        }
        let modules = count(line(s, &mut at)?, "modules")?;
        // A unit the last release wrote lists what its emitter refused, two lines each.
        let mut past = at;
        if let Some(n) = line(s, &mut past).and_then(|l| count(l, "refused")) {
            for _ in 0..2 * n {
                line(s, &mut past)?;
            }
            at = past;
        }
        let fields = decode_fields(s, &mut at)?;
        // Each root entered, a line each; a line a later unit says reads as nothing.
        let mut entered = Vec::new();
        while at < s.len() {
            if let Some(root) = line(s, &mut at)?.strip_prefix(ENTERED) {
                entered.push(root.to_string());
            }
        }
        Some(Exports {
            helpers,
            constants,
            modules,
            fields,
            entered,
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
