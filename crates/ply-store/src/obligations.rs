//! What an obligation was discharged with, on disk.

use ply_core::Type;
use ply_span::Symbol;
use serde::{Deserialize, Serialize};

/// One inference rule a certificate names.
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
    pub tier: String,
    pub evidence: CachedEvidence,
}
