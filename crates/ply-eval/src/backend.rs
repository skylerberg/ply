//! Backends a shipping command can attach.

use crate::compiled::Compiled;
use crate::value::Value;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
pub struct Counters {
    offered: AtomicU64,
    bytes_in: AtomicU64,
    bytes_out: AtomicU64,
    str_in: AtomicU64,
    str_out: AtomicU64,
    containers_out: AtomicU64,
    converted_in: AtomicU64,
    converted_out: AtomicU64,
}

impl Counters {
    pub fn note_offer(&self, args: &[Value]) {
        self.offered.fetch_add(1, Ordering::Relaxed);
        if args.iter().any(|a| matches!(a, Value::Bytes(_))) {
            self.bytes_in.fetch_add(1, Ordering::Relaxed);
        }
        if args.iter().any(|a| matches!(a, Value::Str(_))) {
            self.str_in.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// An answered [`Value::Bytes`].
    pub fn note_bytes_out(&self) {
        self.bytes_out.fetch_add(1, Ordering::Relaxed);
    }

    /// An answered [`Value::Str`].
    pub fn note_str_out(&self) {
        self.str_out.fetch_add(1, Ordering::Relaxed);
    }

    pub fn offers(&self) -> Offers {
        Offers {
            offered: self.offered.load(Ordering::Relaxed),
            bytes_in: self.bytes_in.load(Ordering::Relaxed),
            bytes_out: self.bytes_out.load(Ordering::Relaxed),
            str_in: self.str_in.load(Ordering::Relaxed),
            str_out: self.str_out.load(Ordering::Relaxed),
            containers_out: self.containers_out.load(Ordering::Relaxed),
            converted_in: self.converted_in.load(Ordering::Relaxed),
            converted_out: self.converted_out.load(Ordering::Relaxed),
        }
    }

    pub fn note_converted(&self, inward: u64, outward: u64) {
        self.converted_in.fetch_add(inward, Ordering::Relaxed);
        self.converted_out.fetch_add(outward, Ordering::Relaxed);
    }

    pub fn note_container_out(&self) {
        self.containers_out.fetch_add(1, Ordering::Relaxed);
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Offers {
    pub offered: u64,
    /// Offers carrying at least one [`Value::Bytes`] argument.
    pub bytes_in: u64,
    /// Entered calls that answered a [`Value::Bytes`].
    pub bytes_out: u64,
    /// Offers carrying at least one [`Value::Str`] argument.
    pub str_in: u64,
    /// Entered calls that answered a [`Value::Str`].
    pub str_out: u64,
    /// Entered calls that answered a `List`, `Map`, `Record` or `Ctor`.
    pub containers_out: u64,
    /// Non-immediate objects built from entry arguments; `converted_out` counts those read back.
    pub converted_in: u64,
    pub converted_out: u64,
}

/// One per run, shared by every worker; the only way a shipping command installs a backend.
pub trait Provider: Send + Sync {
    fn attach(&'static self, spec: &Spec) -> Rc<dyn Compiled>;

    /// What `--backend` calls this, for a report a user reads.
    fn name(&self) -> &'static str;

    /// How many definitions this provider has a body for.
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn offers(&self) -> Offers;

    /// What this provider spent compiling, or `None` for one that compiles nothing.
    fn compilation(&self) -> Option<Compilation> {
        None
    }

    /// Workers this provider could not build a backend for.
    fn unbuilt(&self) -> u64 {
        0
    }

    /// Moves where failures are reported to `front`'s layout of the program this was built from;
    /// `false`, moving nothing, when a definition's own text in `sources` changed, since a site is
    /// an offset into it.
    fn relocate(&self, front: &ply_ty::Front, sources: &ply_span::SourceMap) -> bool;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Compilation {
    pub analysis_nanos: u64,
    /// Nanoseconds inside the code generator, summed over every backend built.
    pub codegen_nanos: u64,
    pub units: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    /// `ply_codegen::c`: the program emitted as C and compiled by `cc`.
    #[default]
    C,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::C => "c",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Spec {
    pub kind: Kind,
}

/// Parses a `--backend` argument.
pub fn parse(spec: &str) -> Result<Spec, String> {
    match spec {
        "c" => Ok(Spec { kind: Kind::C }),
        _ => Err(format!("unknown backend `{spec}`; the only backend is `c`")),
    }
}
