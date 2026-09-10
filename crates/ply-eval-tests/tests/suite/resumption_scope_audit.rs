//! What a resumption sees in its **scope**, as against what it sees in the world.
//!
//! `resumption_semantics_audit` covers the world: cells thread across resumptions, which is ADR
//! 0005's rule, and every test there asserts what a cell holds. Nothing asserts the other half — that a resumption
//! re-enters with the *bindings* it captured — because a persistent `Env` gives that for free. A
//! continuation holds an immutable chain, so there is no way to get it wrong and nothing to test.
//!
//! ADR 0034's slot frames remove that guarantee. A machine-owned slot stack reuses indices across
//! activations and empties a slot at a last use, so "the scope a resumption re-enters with" becomes
//! a thing an implementation can be wrong about, silently. These are the programs that would notice.
//!
//! **Every test here passes on the chain today.** That is the point: they are written before the
//! change so that they are a check on it rather than a description of it, and each names the
//! specific way a slot machine would fail it.
