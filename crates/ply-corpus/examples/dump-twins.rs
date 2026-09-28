//! Writes the corpus's own bench services out of the desk example, once.
//!
//! The request-path ladders used to build the program they measured by string surgery on
//! `examples/desk.ply` — splitting it at its tests, expanding effect sets, widening rows with
//! `task.write`, and swapping the entry point for `run_memory` or `run_tls`. Those programs are the
//! corpus's now. This is the once-off that recorded them, and it produces them the only way that
//! cannot drift: by running the three harnesses' own project steps, into a directory this reads
//! back.

use ply_corpus::w3::{Service, Transport, Variant};
use ply_corpus::w4;
use ply_corpus::w5;
use std::path::Path;

fn main() -> anyhow::Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out = root.join("crates/ply-corpus/fixtures");
    std::fs::create_dir_all(&out)?;
    let service = Service::open(&root)?;
    let scratch = tempfile::tempdir()?;

    // w3's own projects: one per variant and transport, which is what its ladders run.
    for variant in [Variant::Sequential, Variant::TaskPerConn] {
        for transport in [Transport::Http, Transport::Https] {
            let dir = scratch.path().join(format!("w3-{}-{}", variant.label(), transport.label()));
            std::fs::create_dir_all(&dir)?;
            service.project(&dir, variant, transport, 0, 1)?;
            let name = format!("desk-{}-{}.ply", variant.label(), transport.label());
            copy(&dir.join("desk.ply"), &out.join(&name))?;
        }
    }

    // w4's: the same service with the store swapped.
    let sequential = service.source(Variant::Sequential)?;
    for store in [w4::Store::Postgres, w4::Store::Twin] {
        let dir = scratch.path().join(format!("w4-{}", store.label()));
        std::fs::create_dir_all(&dir)?;
        w4::project(&dir, &sequential, store)?;
        let name = format!("desk-w4-{}.ply", store.label());
        copy(&dir.join("desk.ply"), &out.join(&name))?;
    }

    // w5's: the same, with the TLS stack besides.
    for stack in [w5::Stack::Postgres, w5::Stack::PostgresTls, w5::Stack::Twin] {
        let dir = scratch.path().join(format!("w5-{}", stack.label().replace(", ", "-")));
        std::fs::create_dir_all(&dir)?;
        w5::project(&dir, &sequential, stack)?;
        let name = format!("desk-w5-{}.ply", stack.label().replace(", ", "-"));
        copy(&dir.join("desk.ply"), &out.join(&name))?;
    }
    Ok(())
}

fn copy(from: &Path, to: &Path) -> anyhow::Result<()> {
    std::fs::copy(from, to)?;
    println!("wrote {}", to.display());
    Ok(())
}
