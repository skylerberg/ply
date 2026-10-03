//! Where what a run keeps between runs, apart from the compiled objects, is kept: the builder and
//! the programs it built for this binary, the rows that seed their next builds, and the front-end
//! answers `ply run` files, each a stage under one root that `sweep` keeps to its budget.

use std::path::PathBuf;

/// The stage `identity` names. A stage is a product of what it was built from alone, so it lives
/// beside the unit cache rather than under it: a run with a cache of its own still finds the stage
/// an earlier one wrote. `PLY_C_STAGE` names another root.
pub fn stage_dir(identity: &str) -> PathBuf {
    stage_root().join(identity)
}

/// The directory every stage is kept under, which `sweep` keeps to its budget.
pub fn stage_root() -> PathBuf {
    std::env::var("PLY_C_STAGE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("ply-c-stage"))
}
