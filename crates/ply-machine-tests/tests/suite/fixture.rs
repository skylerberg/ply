//! The projects a test loads, and the repository paths it reads them from.

use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// A scratch directory.
pub fn scratch() -> TempDir {
    TempDir::new().expect("a scratch directory")
}

/// A temporary project whose `m.ply` is `source`.
pub fn project(source: &str) -> TempDir {
    let dir = scratch();
    write(dir.path(), "m.ply", source);
    dir
}

/// Writes `text` to `dir/name`, making the directories above it.
pub fn write(dir: &Path, name: &str, text: &str) {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("the file's directory is made");
    }
    std::fs::write(path, text).expect("the fixture is written");
}

/// The repository root, canonical so it is comparable with a path the loader resolved.
pub fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the crate lives two levels below the repository root")
}

/// What a program measures for one obligation: each binder's cardinality, and the name the
/// runtime's own texts join to. In a real run the decision is Ply's — `prove.domain`'s
/// `size`/`finite`/`name_of` over the types `prover.typed` hands over — and these audits make the
/// same decision, so what they assert is about a *measured* domain rather than a sampled one. Past
/// the bound, or a product of no points, there is no domain to walk and the obligation is sampled.
pub fn measured(
    prover: &ply_machine::engine::Prover<'_>,
    obligation: &ply_prove::Obligation,
) -> Option<ply_test::obligation::Domain> {
    let sizes: Vec<u64> = obligation
        .generated()
        .iter()
        .map(|binder| ply_prove::domain::cardinality(&binder.ty, prover.world()))
        .collect::<Option<Vec<_>>>()?;
    let points = sizes.iter().try_fold(1u64, |acc, n| acc.checked_mul(*n))?;
    if points == 0 || points > ply_prove::ENUMERATION_BOUND {
        return None;
    }
    let name = if obligation.generated().is_empty() {
        "unit".to_string()
    } else {
        obligation
            .generated()
            .iter()
            .map(|binder| binder.ty.to_string())
            .collect::<Vec<_>>()
            .join(" × ")
    };
    Some(ply_test::obligation::Domain { sizes, name })
}
