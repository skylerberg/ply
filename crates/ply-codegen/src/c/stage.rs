//! Where what a run keeps between runs, apart from the compiled objects, is kept: the builder and
//! the programs it built for this binary, the rows that seed their next builds, and the front-end
//! answers `ply run` files, each a stage under one root that `sweep` keeps to its budget.

use std::path::PathBuf;

/// The stage `identity` names. A stage is a product of what it was built from alone, so it lives
/// apart from the C cache rather than under it: a run with a cache of its own still finds the
/// stage an earlier one wrote.
pub fn stage_dir(identity: &str) -> PathBuf {
    stage_root().join(identity)
}

/// The directory every stage is kept under, which `sweep` keeps to its budget: the one
/// `PLY_C_STAGE` names, or the user's own, which a restart of the machine leaves as it was.
pub fn stage_root() -> PathBuf {
    let var = |name: &str| std::env::var_os(name).map(PathBuf::from);
    root_of(
        var("PLY_C_STAGE"),
        var("HOME"),
        var("XDG_CACHE_HOME"),
        cfg!(target_os = "macos"),
        std::env::temp_dir(),
    )
}

/// [`stage_root`] of what the environment holds: `named` where it is set, else `ply/stage` under
/// the user's caches, which are `Library/Caches` under `home` on macOS and `xdg`, when it is an
/// absolute path, or `.cache` under `home` elsewhere; under `temp` for a user with no home.
pub fn root_of(
    named: Option<PathBuf>,
    home: Option<PathBuf>,
    xdg: Option<PathBuf>,
    macos: bool,
    temp: PathBuf,
) -> PathBuf {
    if let Some(named) = named {
        return named;
    }
    let home = home.filter(|h| h.is_absolute());
    let caches = if macos {
        home.map(|h| h.join("Library/Caches"))
    } else {
        xdg.filter(|x| x.is_absolute())
            .or(home.map(|h| h.join(".cache")))
    };
    caches.unwrap_or(temp).join("ply").join("stage")
}
