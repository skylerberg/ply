//! Writes the corpus's own bench services out of the desk example, once.
//!
//! The w3 ladder used to rewrite `examples/desk.ply` at run time — splitting it at its tests and
//! widening ten effect rows — which meant the program it measured was one the harness built by
//! string surgery rather than one anybody had written down. The two programs it produced are the
//! corpus's now, and this is the once-off that recorded them.

use std::path::Path;

fn main() -> anyhow::Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let service = ply_corpus::w3::Service::open(&root)?;
    for variant in [
        ply_corpus::w3::Variant::Sequential,
        ply_corpus::w3::Variant::TaskPerConn,
    ] {
        let path = root.join(format!(
            "crates/ply-corpus/fixtures/desk-{}.ply",
            variant.label()
        ));
        std::fs::write(&path, service.source(variant)?)?;
        println!("wrote {}", path.display());
    }
    Ok(())
}
