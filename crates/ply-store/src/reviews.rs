//! What a human last accepted, per definition.

use ply_hash::DefHash;
use serde::{Deserialize, Serialize};

/// The definition and the claims about it, as they stood when they were accepted.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ReviewRecord {
    /// The implementation.
    pub def_hash: DefHash,
    /// Every claim *about* this definition: its own `requires` / `ensures` clause keys, and the
    /// hash of every law that names it directly.
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
