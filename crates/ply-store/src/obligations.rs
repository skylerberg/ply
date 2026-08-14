//! What an obligation was discharged with, on disk.
//!
//! The store holds an **evidence**, never an outcome. There is no variant here
//! for a refutation, a vacuity or a gap, so ADR 0007 §4.3's "never cached" rows
//! are enforced by the type the cache is written in rather than by the
//! discipline of whoever writes it.
//!
//! These shapes mirror `ply-prove`'s rather than reusing them, because nothing
//! depends on `ply-prove` but `ply-cli` and a cache is not the place to invert
//! that. The mirror is exhaustive on both sides — the conversions live in
//! `ply-test`, over `match`es with no wildcard arm — so a prover that grows a
//! rule stops compiling before it can write one nothing can read back.
//!
//! The tier is **recorded**, not computed: computing it here would need
//! `MIN_PROPERTY_CASES`, and a second copy of the constant that separates a
//! coverage claim from a concrete one is the last thing this milestone should
//! own. The crate that owns the tier rule checks the record against the evidence
//! on reload and discards a disagreement, so a cached `proved` and a cached
//! `property` cannot be conflated by a file that says otherwise.

use ply_core::Type;
use ply_span::Symbol;
use serde::{Deserialize, Serialize};

/// One inference rule a certificate names. A faithful mirror of
/// `ply_prove::Rule`, whose variants are ADR 0007 §5.1's fragment plus §6's.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CachedRule {
    GroundEvaluation,
    ExhaustiveEnumeration { domain: Symbol, points: u64 },
    LinearArithmetic,
    Propositional,
    CaseSplit { ty: Symbol, arms: u32 },
    Congruence,
    Injectivity,
    Unfold { def: Symbol, depth: u32 },
    ExhaustiveInterleaving { interleavings: u32 },
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct CachedCertificate {
    pub rules: Vec<CachedRule>,
    pub steps: u32,
    pub guard_satisfiable: bool,
    pub sorts: Vec<Symbol>,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct CachedCases {
    pub generated: u32,
    pub kept: u32,
    pub rejected: u32,
    pub roots: Vec<u64>,
    pub instantiations: Vec<(Symbol, Type)>,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "evidence")]
pub enum CachedEvidence {
    Proof(CachedCertificate),
    Cases(CachedCases),
}

/// One discharged obligation, as the cache holds it.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct CachedObligation {
    /// `proved`, `property` or `example`, as the run that discharged it reported.
    ///
    /// Not the authority on anything: the reader recomputes the tier from
    /// `evidence` and discards the entry when the two disagree. It is here so
    /// that a disagreement is *detectable* — a file whose label and whose
    /// evidence tell different stories is the one shape of corruption this
    /// milestone cannot afford to read past.
    pub tier: String,
    pub evidence: CachedEvidence,
}
