//! The emitter written in Ply is built from a bootstrap bundle, the C it emitted for its own
//! sources, and the bundle serves as long as it is a fixpoint: the emitter built from it emits,
//! for those sources, the bundle's own C. ADR 0045's second stage.
//!
//! The bundle has to be the one emitted from the sources in the tree: an older one still runs,
//! since its constructor table travels with it, and is what the refresh builds the new one with,
//! but what it emits is the old emitter's C and the caches would key it as the new one's.
//! `PLY_C_BOOTSTRAP_REFRESH=1` rewrites `crates/ply-compiler/bootstrap` with the fixpoint's own
//! emission; CI does the same when this test goes red and hands the result back as the
//! `bootstrap-bundle` artifact. With no bundle at all, the refresh builds the first emitter with
//! the reference, which under tier-only can no longer emit it whole.

use ply_codegen::Source;
use ply_codegen::c::bundle::Load;
use ply_codegen::c::producer::{self, PlyProducer};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

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
    let check = ply_core::check_program(&ast, &resolved).expect("the emitter checks");
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

/// Where the recipe builds the emitter from; changed between the fixpoint's two emissions.
static FROM: Mutex<Option<PathBuf>> = Mutex::new(None);

fn build_from(source: &'static Source, from: Option<&Path>) -> Result<PlyProducer, String> {
    let (native, _) = match from.and_then(ply_codegen::c::bundle::from_dir) {
        Some(bundle) => {
            ply_codegen::c::bundle::build(&bundle, || Ok(source)).map_err(|e| format!("{e:#}"))?
        }
        None => {
            let names: Vec<String> = source.functions();
            let refs: Vec<&str> = names.iter().map(String::as_str).collect();
            ply_codegen::c::build(source, &refs).map_err(|e| format!("{e:#}"))?
        }
    };
    PlyProducer::new(native).map_err(|e| format!("{e:#}"))
}

/// Emits the emitter's own unit with the producer built from `from`, into a cache of its own so
/// nothing an earlier emission wrote is read back.
fn emit_with(
    source: &'static Source,
    from: Option<&Path>,
    scratch: &Path,
) -> (
    String,
    ply_codegen::c::cache::UnitCache,
    Vec<ply_codegen::c::Refused>,
) {
    *FROM.lock().unwrap() = from.map(Path::to_path_buf);
    producer::reset_thread();
    let cache = scratch.join(format!("cache-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&cache);
    unsafe { std::env::set_var("PLY_C_CACHE", &cache) };
    let names: Vec<String> = source.functions();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let (text, record, refused) =
        ply_codegen::c::emit_unit_record(source, &refs).expect("the emitter's unit emits");
    (text, record, refused)
}

// The emitter emitting its own sources is one entry; with the tier releasing within an entry
// (ADR 0046) it runs in a runner's memory. The refresh is the same test.
#[test]
fn the_bootstrap_bundle_is_a_fixpoint_of_the_emitter_it_builds() {
    // The emitter emitting its sources allocates two hundred million objects; a debug heap that
    // never reuses a dead block, so that a read of one is caught, would need a runner it cannot
    // have. This test is not that check's oracle -- the audits are -- so it reuses.
    ply_codegen::heap::reuse_by_default(true);
    let (source, identity) = emitter_source();
    let bundle = PathBuf::from(ply_compiler::bootstrap::DIR);
    let refresh = std::env::var("PLY_C_BOOTSTRAP_REFRESH").is_ok();
    let have = ply_codegen::c::bundle::exists(&bundle);
    assert!(
        have || refresh,
        "no bootstrap bundle serves at {}{}; run this test with PLY_C_BOOTSTRAP_REFRESH=1 to write one from the reference",
        bundle.display(),
        if ply_codegen::c::bundle::stale_runtime(&bundle) {
            " (it was emitted against another runtime's helper table)"
        } else {
            ""
        }
    );
    if have && !refresh {
        let current = ply_codegen::c::bundle::from_dir(&bundle).expect("the bundle serves");
        assert_eq!(
            current.sources_digest(),
            Some(identity.as_str()),
            "the bundle at {} was emitted from other sources than these; refresh it: PLY_C_BOOTSTRAP_REFRESH=1 cargo nextest run -p ply-codegen-tests --test bootstrap, or take CI's `bootstrap-bundle` artifact",
            bundle.display()
        );
    }
    let ctors = source.ctors();
    let scratch = std::env::temp_dir().join(format!("ply-bootstrap-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    producer::install(
        std::sync::Arc::new(move || {
            let from = FROM.lock().unwrap().clone();
            build_from(source, from.as_deref())
        }),
        identity.clone(),
    );

    // The emitter built from the bundle, or from the reference when there is none, emits itself.
    let first = have.then(|| bundle.clone());
    let (c1, r1, refused) = emit_with(source, first.as_deref(), &scratch);
    assert!(
        refused.is_empty(),
        "the emitter refuses part of itself: {refused:?}"
    );
    let same = |a: &str,
                ra: &ply_codegen::c::cache::UnitCache,
                b: &str,
                rb: &ply_codegen::c::cache::UnitCache| {
        a == b && ra.taken == rb.taken && ra.refusals == rb.refusals
    };
    let differ = |label: &str, a: &str, b: &str, what: &str| {
        let pa = scratch.join(format!("{label}-a.c"));
        let pb = scratch.join(format!("{label}-b.c"));
        std::fs::write(&pa, a).unwrap();
        std::fs::write(&pb, b).unwrap();
        panic!("{what}: diff {} {}", pa.display(), pb.display());
    };
    if refresh {
        // A refresh writes the current emitter's own emission, once an emitter built from it has
        // emitted the same thing again: the bundle written is a fixpoint on the day it is written.
        let stage = scratch.join("stage1");
        let load = Load::of(source, &r1.taken);
        ply_codegen::c::bundle::write(&stage, &c1, &r1, &ctors, &load, &identity).unwrap();
        let (c2, r2, _) = emit_with(source, Some(&stage), &scratch);
        if !same(&c1, &r1, &c2, &r2) {
            differ(
                "refresh",
                &c1,
                &c2,
                "the emitter built from one emission and the emitter built from its own emit different C",
            );
        }
        ply_codegen::c::bundle::write(&bundle, &c2, &r2, &ctors, &load, &identity).unwrap();
        eprintln!("bootstrap bundle written to {}", bundle.display());
    } else {
        // The bundle was emitted from these sources, so the emitter built from it emitting its own
        // C -- the bundle's, byte for byte -- is the fixpoint stated directly, in one emission
        // rather than the two it took to compare an emission with the emission of that emission.
        let current = ply_codegen::c::bundle::from_dir(&bundle).expect("the bundle serves");
        let text = ply_codegen::c::bundle::text_of(&current).expect("the bundle's C unpacks");
        let record = ply_codegen::c::bundle::record(&bundle).expect("the bundle's record decodes");
        if !same(&c1, &r1, &text, &record) {
            differ(
                "fixpoint",
                &text,
                &c1,
                "the emitter built from the bundle emits other C for these sources than the bundle holds",
            );
        }
        // The load record is the fixpoint's too: what the bundle says loading needs is what these
        // sources say, or the bundle enters its functions at the wrong arities.
        assert_eq!(
            current.load(),
            Some(&Load::of(source, &record.taken)),
            "the bundle at {} does not carry the load record these sources derive; refresh it: PLY_C_BOOTSTRAP_REFRESH=1 cargo nextest run -p ply-codegen-tests --test bootstrap, or take CI's `bootstrap-bundle` artifact",
            bundle.display()
        );
    }
    let _ = std::fs::remove_dir_all(&scratch);
}
