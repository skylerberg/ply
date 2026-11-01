use super::descriptor::Descriptor;
use super::encoding::{count, decode_fields, encode_fields, line};
use super::prelude::helpers;
use anyhow::{Result, anyhow};
use ply_core::Symbol;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelperShape {
    pub name: String,
    pub args: usize,
    pub results: bool,
}

pub fn helpers_digest() -> String {
    let mut h = blake3::Hasher::new();
    for helper in helpers() {
        h.update(format!("{} {} {}\n", helper.name, helper.args(), helper.results()).as_bytes());
    }
    h.finalize().to_hex().to_string()
}

#[derive(Debug)]
pub struct RuntimeMismatch(pub String);

impl std::fmt::Display for RuntimeMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the unit does not serve this runtime: {}", self.0)
    }
}

impl std::error::Error for RuntimeMismatch {}

#[derive(Clone)]
pub struct Exports {
    pub helpers: Vec<HelperShape>,
    pub constants: Vec<(String, usize)>,
    pub modules: usize,
    pub fields: Vec<Symbol>,
    pub entered: Vec<String>,
}

pub const ENTERED: &str = "entered ";

impl Exports {
    pub fn bound(&self) -> Result<Vec<*mut std::ffi::c_void>, RuntimeMismatch> {
        let said = |h: &HelperShape| {
            format!(
                "taking {} and {}",
                h.args,
                if h.results {
                    "returning"
                } else {
                    "returning nothing"
                }
            )
        };
        self.helpers
            .iter()
            .map(
                |mine| match helpers().iter().find(|h| h.name == mine.name) {
                    Some(h) if h.args() == mine.args && h.results() == mine.results => {
                        Ok(h.address as *mut std::ffi::c_void)
                    }
                    Some(h) => Err(RuntimeMismatch(format!(
                        "it was emitted against `{}` {}, and this runtime's is {}",
                        mine.name,
                        said(mine),
                        said(&HelperShape {
                            name: h.name.to_string(),
                            args: h.args(),
                            results: h.results(),
                        }),
                    ))),
                    None => Err(RuntimeMismatch(format!(
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
            out.push_str(&format!("{} {} {}\n", h.name, h.args, u8::from(h.results)));
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
            let results = match parts.next()? {
                "1" => true,
                "0" => false,
                _ => return None,
            };
            helpers.push(HelperShape {
                name,
                args,
                results,
            });
        }
        let n = count(line(s, &mut at)?, "constants")?;
        let mut constants = Vec::with_capacity(n);
        for _ in 0..n {
            let (name, slot) = line(s, &mut at)?.split_once(' ')?;
            constants.push((name.to_string(), slot.parse().ok()?));
        }
        let modules = count(line(s, &mut at)?, "modules")?;
        let mut past = at;
        if let Some(n) = line(s, &mut past).and_then(|l| count(l, "refused")) {
            for _ in 0..2 * n {
                line(s, &mut past)?;
            }
            at = past;
        }
        let fields = decode_fields(s, &mut at)?;
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

    pub fn read(unit: &Descriptor) -> Result<Exports> {
        let text = unit
            .exports()
            .ok_or_else(|| anyhow!("what the unit says about itself is not UTF-8"))?;
        Exports::decode(text).ok_or_else(|| {
            anyhow!("what the unit says about itself does not read as a unit's table")
        })
    }
}
