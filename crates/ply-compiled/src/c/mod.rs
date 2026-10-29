//! The C backend: the machine's lowered `Code`, emitted as C and handed to `cc`.
//! `PLY_C_*`, `PLY_CC*` and `PLY_HEAP_*` knobs never change a program's meaning.

mod build;
pub mod described;
pub mod descriptor;
mod encoding;
pub mod exports;
pub mod linked;
mod load;
mod prelude;
pub mod stage;
pub mod sweep;
pub mod tables;

pub use build::{Native, load_linked, load_opened};
pub use described::described;
pub use exports::{Exports, Unserved};
pub use load::{Library, cache_dir};
pub(crate) use prelude::machine_native;
pub use prelude::{MachineNative, builtin_helper_name, helpers, machine_natives, own_helpers};

/// The runtime's sources, as `build.rs` digests them, less how C is compiled, kept, swept and
/// loaded and how Rust asks the compiler: what an answer computed inside a loaded unit is a
/// function of. Not the binary: a change anywhere else in `ply`, or the other build profile,
/// answers the same.
pub fn semantics_digest() -> &'static str {
    env!("PLY_SEMANTICS_DIGEST")
}

/// Where the emitter's answers are kept between runs, beside the objects they compile to and under
/// the same sweep.
pub fn bodies_dir() -> std::path::PathBuf {
    load::cache_dir().join("bodies")
}

/// Every directory a run keeps for the next one in: the compiled objects and the stages. What is
/// there is a product of its key, so no answer depends on finding it.
pub fn kept_dirs() -> Vec<std::path::PathBuf> {
    vec![load::cache_dir(), stage::stage_root()]
}

/// The directory a run keeps `kind` of what it keeps in: the one the variable `named` names, else
/// `ply/<kind>` under the user's caches, which a restart of the machine leaves as they were.
pub(crate) fn kept_dir(kind: &str, named: &str) -> std::path::PathBuf {
    let var = |name: &str| std::env::var_os(name).map(std::path::PathBuf::from);
    kept_root(
        kind,
        var(named),
        var("HOME"),
        var("XDG_CACHE_HOME"),
        cfg!(target_os = "macos"),
        std::env::temp_dir(),
    )
}

/// [`kept_dir`] of what the environment holds: `named` where it is set, else `ply/<kind>` under
/// the user's caches, which are `Library/Caches` under `home` on macOS and `xdg`, when it is an
/// absolute path, or `.cache` under `home` elsewhere; under `temp` for a user with no home.
pub fn kept_root(
    kind: &str,
    named: Option<std::path::PathBuf>,
    home: Option<std::path::PathBuf>,
    xdg: Option<std::path::PathBuf>,
    macos: bool,
    temp: std::path::PathBuf,
) -> std::path::PathBuf {
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
    caches.unwrap_or(temp).join("ply").join(kind)
}

/// What a unit emitted against this runtime is a function of on the runtime's side: the sources
/// that run while the emitter emits inside a loaded unit, and the helpers its C binds.
pub fn runtime_identity() -> String {
    let mut h = blake3::Hasher::new();
    h.update(semantics_digest().as_bytes());
    h.update(&[0]);
    h.update(exports::helpers_digest().as_bytes());
    h.finalize().to_hex().to_string()
}
