//! What a human last accepted, per definition.

use ply_ty::DefHash;
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ReviewRecord {
    pub def_hash: DefHash,
    /// Its own `requires`/`ensures` clause keys and every law that names it directly.
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
