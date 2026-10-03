//! The C backend: the machine's lowered `Code`, emitted as C and handed to `cc`.
//! `PLY_C_*`, `PLY_CC*` and `PLY_HEAP_*` knobs never change a program's meaning.

mod build;
pub mod dump;
mod encoding;
pub mod exports;
mod load;
mod prelude;
pub mod stage;
pub mod sweep;
pub mod tables;
pub mod toolchain;

pub use build::{Native, load_unit};
pub use exports::{Exports, Unserved};
pub use load::{
    BUCKETS_COMPILED, BUCKETS_REUSED, Library, Parts, UNITS_MAPPED, compile_and_load, split,
};
pub use prelude::{HELPERS, PRELUDE, RUNTIME_MARK, pointer_name, runtime_header, runtime_object};
pub use toolchain::{Profile, select as select_profile};

/// The sources of the runtime the emitter runs on, as `build.rs` digests them. Not the binary: a
/// change anywhere else in `ply`, or the other build profile, answers and emits the same.
const RUNTIME: &str = env!("PLY_RUNTIME_DIGEST");

/// [`RUNTIME`], for what is kept under the runtime that answered it.
pub fn runtime_digest() -> &'static str {
    RUNTIME
}

/// The runtime's sources less how C is compiled, kept, swept and loaded and how Rust asks the
/// compiler: what an answer computed inside a loaded unit is a function of.
pub fn semantics_digest() -> &'static str {
    env!("PLY_SEMANTICS_DIGEST")
}

/// What the emitter refused, and where.
#[derive(Debug)]
pub struct Refused {
    pub function: String,
    pub construct: String,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "`{}` is outside the compiled fragment: {}",
            self.function, self.construct
        )
    }
}

impl std::error::Error for Refused {}

/// Where the emitter's answers are kept between runs, beside the objects they compile to and under
/// the same sweep.
pub fn bodies_dir() -> std::path::PathBuf {
    load::cache_dir().join("bodies")
}

/// What every unit's C opens with, up to its first bucket: the prelude and the runtime declared.
pub fn unit_head() -> String {
    format!("{PRELUDE}{}\n", runtime_header())
}

/// What a unit emitted against this runtime is a function of on the runtime's side: the sources
/// that run while the emitter emits inside a loaded unit, and the helper table its C binds by
/// position.
pub fn runtime_identity() -> String {
    let mut h = blake3::Hasher::new();
    h.update(semantics_digest().as_bytes());
    h.update(&[0]);
    h.update(exports::helpers_digest().as_bytes());
    h.finalize().to_hex().to_string()
}

/// The addresses the loaded unit binds, in [`HELPERS`]' order.
pub fn helper_addresses() -> Vec<*mut std::ffi::c_void> {
    HELPERS
        .iter()
        .map(|h| h.address as *mut std::ffi::c_void)
        .collect()
}
