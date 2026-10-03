//! The committed builder, embedded when one is committed: the runnable `ply bootstrap` wrote for the
//! compiler's own `build.main`, and the digest of what it was built from. A checkout without one
//! embeds nothing, and the launcher builds its first builder another way.

use std::path::Path;

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("bootstrap");
    println!("cargo:rerun-if-changed={}", dir.display());
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    for name in ["builder.run", "builder.digest"] {
        let bytes = std::fs::read(dir.join(name)).unwrap_or_default();
        std::fs::write(out.join(name), bytes).expect("the build script writes OUT_DIR");
    }
}
