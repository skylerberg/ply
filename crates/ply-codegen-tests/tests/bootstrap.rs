//! The emitter written in Ply is built from a bootstrap bundle, the C it emitted for its own
//! sources, and the bundle serves as long as it is a fixpoint: the emitter built from it emits,
//! for those sources, the bundle's own C. ADR 0045's second stage.
//!
//! The bundle has to be the one emitted from the sources in the tree: an older one still runs,
//! since the unit carries the constructor table it was emitted against, and is what the refresh
//! builds the new one with, but what it emits is the old emitter's C and the caches would key it
//! as the new one's. `PLY_C_BOOTSTRAP_REFRESH=1` rewrites `crates/ply-compiler/bootstrap` with the
//! fixpoint's own emission; CI does the same when this test goes red and hands the result back as
//! the `bootstrap-bundle` artifact. The emitter is only ever built from a bundle, so one this
//! runtime does not serve is recovered by checking out an older one from git history, then
//! refreshing.

use ply_codegen::Source;
use ply_codegen::c::Produced;
use ply_codegen::c::producer::{self, PlyProducer};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("the crate sits two levels under the repository root")
        .to_path_buf()
}

/// Where the emitter's sources live in the tree, for the `SourceMap` a diagnostic would point at.
fn ply_compiler_dir() -> String {
    repo().join("crates/ply-compiler").display().to_string()
}

/// The emitter's own program, the standard library alongside, keyed as the CLI keys it.
fn emitter_source() -> (&'static Source, String) {
    // The embedded compiler, in the order `ply_compiler::MODULES` is in, so the identity this
    // writes into the bundle is the one the producer computes when it reads it back.
    let modules: Vec<(String, String)> = ply_std::sources()
        .chain(ply_compiler::sources())
        .map(|(m, t)| (m.to_string(), t.to_string()))
        .collect();
    let identity = producer::digest_of(&modules);
    let mut sources = ply_span::SourceMap::new();
    let mut inputs = Vec::new();
    let mut texts: HashMap<String, String> = HashMap::new();
    for (module, text) in ply_std::sources() {
        texts.insert(module.to_string(), text.to_string());
        let module = ply_syntax::ast::ModuleName::from_dotted(module);
        let id = sources.add(ply_std::pseudo_path(&module), text.to_string());
        inputs.push((id, module, text));
    }
    for (stem, text) in ply_compiler::sources() {
        texts.insert(stem.to_string(), text.to_string());
        let id = sources.add(
            PathBuf::from(format!("{}/ply/{stem}.ply", ply_compiler_dir())),
            text.to_string(),
        );
        inputs.push((id, ply_syntax::ast::ModuleName::from_dotted(stem), text));
    }
    let mut ast = ply_syntax::parse_program(inputs).expect("the emitter parses");
    let expanded = ply_derive::expand_program(&mut ast);
    assert!(expanded.is_empty(), "{expanded:?}");
    let resolved = ply_syntax::resolve::resolve(&mut ast).expect("the emitter resolves");
    // Nothing installs a recipe here since the fixpoint test began handing its emitter over, so
    // the committed bundle answers for these sources; the emitter each round tests is handed
    // over inside `emit_with`, and `with_current` prefers a handover to anything installed.
    let ids: Vec<_> = (0..modules.len())
        .map(|i| ply_span::SourceId(i as u32))
        .collect();
    let check = producer::checked_front(&modules, &ids)
        .expect("the emitter checks")
        .check;
    let program: &'static ply_syntax::ast::Program = Box::leak(Box::new(ast));
    let resolved = Box::leak(Box::new(resolved));
    let check = Box::leak(Box::new(check));
    let hashes = ply_hash::hash_program(program, resolved, check).expect("the emitter hashes");
    let keys: HashMap<String, String> = hashes
        .defs
        .iter()
        .map(|(name, h)| (name.to_string(), h.to_hex()))
        .collect();
    let source: &'static Source = Box::leak(Box::new(
        Source::keyed(program, resolved, check, keys).with_texts(texts),
    ));
    (source, identity)
}

fn build_from(dir: &Path) -> Result<PlyProducer, String> {
    let bundle = ply_codegen::c::bundle::from_dir(dir)
        .ok_or_else(|| format!("no bootstrap bundle at {}", dir.display()))?;
    let (native, _) = ply_codegen::c::bundle::build(&bundle).map_err(|e| {
        if e.downcast_ref::<ply_codegen::c::Unserved>().is_some() {
            format!(
                "{e:#}; this runtime cannot build the emitter from the bundle at {}: check out an \
                 older bundle it serves from git history, then refresh it with \
                 PLY_C_BOOTSTRAP_REFRESH=1",
                dir.display()
            )
        } else {
            format!("{e:#}")
        }
    })?;
    PlyProducer::new(native).map_err(|e| format!("{e:#}"))
}

/// Emits the emitter's own unit with the producer built from `from`, into a cache of its own so
/// nothing an earlier emission wrote is read back.
fn emit_with(source: &'static Source, from: &Path, scratch: &Path, identity: &str) -> Produced {
    producer::reset_thread();
    let cache = scratch.join(format!("cache-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&cache);
    unsafe { std::env::set_var("PLY_C_CACHE", &cache) };
    let names: Vec<String> = source.functions();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    // This round's emitter is handed over rather than installed. A `OnceLock` would keep the
    // first round's emitter for the second, and the difference between those two emissions is
    // the whole of what the fixpoint compares.
    let _held = producer::hand_over(
        build_from(from).expect("the emitter builds"),
        identity.to_string(),
    );
    ply_codegen::c::produce(source, &refs).expect("the emitter's unit emits")
}

// The emitter emitting its own sources is one entry; with the tier releasing within an entry
// (ADR 0046) it runs in a runner's memory. The refresh is the same test.
#[test]
fn the_bootstrap_bundle_is_a_fixpoint_of_the_emitter_it_builds() {
    let (source, identity) = emitter_source();
    let bundle = PathBuf::from(ply_compiler::bootstrap::DIR);
    let refresh = std::env::var("PLY_C_BOOTSTRAP_REFRESH").is_ok();
    assert!(
        ply_codegen::c::bundle::exists(&bundle),
        "no bootstrap bundle at {}; check one out from git history, then refresh it with PLY_C_BOOTSTRAP_REFRESH=1",
        bundle.display()
    );
    if !refresh {
        let current = ply_codegen::c::bundle::from_dir(&bundle).expect("the bundle serves");
        assert_eq!(
            current.sources_digest(),
            Some(identity.as_str()),
            "the bundle at {} was emitted from other sources than these; refresh it: PLY_C_BOOTSTRAP_REFRESH=1 cargo nextest run -p ply-codegen-tests --test bootstrap, or take CI's `bootstrap-bundle` artifact",
            bundle.display()
        );
    }
    let scratch = std::env::temp_dir().join(format!("ply-bootstrap-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    let p1 = emit_with(source, &bundle, &scratch, &identity);
    assert!(
        p1.refused.is_empty(),
        "the emitter refuses part of itself: {:?}",
        p1.refused
    );
    let differ = |label: &str, a: &str, b: &str, what: &str| {
        let pa = scratch.join(format!("{label}-a.c"));
        let pb = scratch.join(format!("{label}-b.c"));
        std::fs::write(&pa, a).unwrap();
        std::fs::write(&pb, b).unwrap();
        panic!("{what}: diff {} {}", pa.display(), pb.display());
    };
    // The C is the whole comparison: the unit's table -- what it took, what it refused, its
    // arities and constants -- is its last declaration, so a bundle whose table differs from what
    // these sources derive differs here.
    if refresh {
        // A refresh writes an emission an emitter built from it emits again: a fixpoint on the
        // day it is written. The first emission is the old bundle's emitter over the new sources,
        // so a change to what the emitter emits shows only in the second, and the rounds go on
        // from each emission until two agree.
        let mut last = p1;
        let mut written = false;
        for round in 1..=3 {
            let stage = scratch.join(format!("stage{round}"));
            ply_codegen::c::bundle::write(&stage, &last.text, &identity).unwrap();
            let next = emit_with(source, &stage, &scratch, &identity);
            assert!(
                next.refused.is_empty(),
                "the emitter built from its own emission refuses part of itself: {:?}",
                next.refused
            );
            if last.text == next.text {
                ply_codegen::c::bundle::write(&bundle, &next.text, &identity).unwrap();
                eprintln!("bootstrap bundle written to {}", bundle.display());
                written = true;
                break;
            }
            if round == 3 {
                differ(
                    "refresh",
                    &last.text,
                    &next.text,
                    "the emitter built from one emission and the emitter built from its own emit different C after three rounds",
                );
            }
            last = next;
        }
        assert!(written);
    } else {
        // The bundle was emitted from these sources, so the emitter built from it emitting its own
        // C -- the bundle's, byte for byte -- is the fixpoint stated directly, in one emission
        // rather than the two it took to compare an emission with the emission of that emission.
        let current = ply_codegen::c::bundle::from_dir(&bundle).expect("the bundle serves");
        let text = ply_codegen::c::bundle::text_of(&current).expect("the bundle's C unpacks");
        if p1.text != text {
            differ(
                "fixpoint",
                &text,
                &p1.text,
                "the emitter built from the bundle emits other C for these sources than the bundle holds",
            );
        }
    }
    let _ = std::fs::remove_dir_all(&scratch);
}
