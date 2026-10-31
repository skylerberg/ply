//! Where what a run caches between runs, apart from compiled objects, is stored: the builder and
//! the programs it built for this binary, the rows that seed their next builds, and the front-end
//! results `ply run` records, each a stage under one root that `sweep` keeps to its budget.

use std::path::PathBuf;

/// The stage `identity` names. A stage is a product of what it was built from alone, so it lives
/// apart from the C cache rather than under it: a run with a cache of its own still finds the
/// stage an earlier one wrote.
pub fn stage_dir(identity: &str) -> PathBuf {
    stage_root().join(identity)
}

/// The directory every stage is stored under, which `sweep` keeps to its budget: the one
/// `PLY_C_STAGE` names, or the user's own.
pub fn stage_root() -> PathBuf {
    super::cache_dir_for("stage", "PLY_C_STAGE")
}
