//! What a human last accepted, per definition.
//!
//! The baseline `ply review --changed` compares against is what a *person*
//! accepted, not what a machine last ran: a run that proved everything green
//! reviewed nothing.
//!
//! Keyed by program-wide name, which is the same trade [`crate::PassRecord`]
//! makes and for the same reason — the point of a baseline is to survive an edit
//! that moves the hash, so the key has to be the thing the edit did not move.
//! Renaming a definition therefore loses its baseline and it is reported as
//! newly unreviewed, which costs one re-read and never a false "unchanged".

use ply_hash::DefHash;
use serde::{Deserialize, Serialize};

/// The definition and the claims about it, as they stood when they were
/// accepted.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ReviewRecord {
    /// The implementation. Moving it means the body, or something in its
    /// closure, changed.
    pub def_hash: DefHash,
    /// Every claim *about* this definition: its own `requires` / `ensures`
    /// clause keys, and the hash of every law that names it directly. A law is
    /// part of the specification of the definitions it speaks about, so a law
    /// edit has to read as a spec change on each of them — otherwise the one
    /// row of ADR 0007 §9.2's table that says "read the spec diff, and nothing
    /// else" would be reached with an unread diff.
    ///
    /// Sorted, so that reordering the sources cannot read as a change.
    pub specs: Vec<DefHash>,
}

impl ReviewRecord {
    pub fn new(def_hash: DefHash, specs: impl IntoIterator<Item = DefHash>) -> ReviewRecord {
        let mut specs: Vec<DefHash> = specs.into_iter().collect();
        specs.sort_unstable();
        specs.dedup();
        ReviewRecord { def_hash, specs }
    }
}
