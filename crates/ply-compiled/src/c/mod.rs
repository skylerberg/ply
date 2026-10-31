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

pub use build::{Native, load_from_path, load_linked};
pub use described::described;
pub use exports::{Exports, RuntimeMismatch};
pub use load::{Library, cache_dir};
pub(crate) use prelude::runtime_native;
pub use prelude::{MachineNative, builtin_helper_name, helpers, runtime_helpers, runtime_natives};

pub fn semantics_digest() -> &'static str {
    env!("PLY_SEMANTICS_DIGEST")
}

pub fn bodies_dir() -> std::path::PathBuf {
    load::cache_dir().join("bodies")
}

pub fn cache_dirs() -> Vec<std::path::PathBuf> {
    vec![load::cache_dir(), stage::stage_root()]
}

pub(crate) fn cache_dir_for(kind: &str, named: &str) -> std::path::PathBuf {
    let var = |name: &str| std::env::var_os(name).map(std::path::PathBuf::from);
    cache_root_for(
        kind,
        var(named),
        var("HOME"),
        var("XDG_CACHE_HOME"),
        cfg!(target_os = "macos"),
        std::env::temp_dir(),
    )
}

pub fn cache_root_for(
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

pub fn runtime_identity() -> String {
    let mut h = blake3::Hasher::new();
    h.update(semantics_digest().as_bytes());
    h.update(&[0]);
    h.update(exports::helpers_digest().as_bytes());
    h.finalize().to_hex().to_string()
}
