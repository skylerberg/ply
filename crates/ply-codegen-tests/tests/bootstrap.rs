//! The emitter written in Ply is built from a bootstrap bundle, the C it emitted for its own
//! sources, and the bundle serves as long as it is a fixpoint: the emitter built from it emits,
//! for those sources, C that builds an emitter that emits the same C. ADR 0045's second stage.
//!
//! `PLY_C_BOOTSTRAP_REFRESH=1` rewrites `spikes/ply-parser/bootstrap` with the fixpoint's own
//! emission, which is how the bundle is refreshed after a change the old one cannot build; with
//! no bundle at all, the refresh builds the first emitter with the reference.

use ply_codegen::Source;
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

/// The emitter's own program, the standard library alongside, keyed as the CLI keys it.
fn emitter_source() -> (&'static Source, String) {
    let dir = repo().join("spikes/ply-parser");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("the emitter's directory is readable")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "ply"))
        .collect();
    files.sort();
    let modules: Vec<(String, String)> = files
        .iter()
        .map(|p| {
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            (
                name,
                std::fs::read_to_string(p).expect("the emitter is readable"),
            )
        })
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
    for path in &files {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .expect("a module name");
        let text: &'static str = Box::leak(
            std::fs::read_to_string(path)
                .expect("the emitter is readable")
                .into_boxed_str(),
        );
        texts.insert(stem.to_string(), text.to_string());
        let id = sources.add(path.clone(), text.to_string());
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
    let (native, _) = match from {
        Some(dir) => ply_codegen::c::bundle::build(source, dir).map_err(|e| format!("{e:#}"))?,
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

#[test]
fn the_bootstrap_bundle_is_a_fixpoint_of_the_emitter_it_builds() {
    let (source, identity) = emitter_source();
    let bundle = repo().join("spikes/ply-parser/bootstrap");
    let refresh = std::env::var("PLY_C_BOOTSTRAP_REFRESH").is_ok();
    let have = ply_codegen::c::bundle::exists(&bundle);
    assert!(
        have || refresh,
        "no bootstrap bundle at {}; run this test with PLY_C_BOOTSTRAP_REFRESH=1 to write one from the reference",
        bundle.display()
    );
    let scratch = std::env::temp_dir().join(format!("ply-bootstrap-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    producer::install(
        std::sync::Arc::new(move || {
            let from = FROM.lock().unwrap().clone();
            build_from(source, from.as_deref())
        }),
        identity.clone(),
    );
    producer::set_whole(true);

    // The emitter built from the bundle, or from the reference when there is none, emits itself.
    // An old bundle's emitter may refuse what the current sources emit; only the emitter built
    // from a current emission has to refuse nothing of itself.
    let first = have.then(|| bundle.clone());
    let (c1, r1, _) = emit_with(source, first.as_deref(), &scratch);
    let stage = scratch.join("stage1");
    ply_codegen::c::bundle::write(&stage, &c1, &r1, &identity).unwrap();

    // The emitter built from that emission emits itself again.
    let (c2, r2, refused) = emit_with(source, Some(&stage), &scratch);
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
    let differ = |label: &str, a: &str, b: &str| {
        let pa = scratch.join(format!("{label}-a.c"));
        let pb = scratch.join(format!("{label}-b.c"));
        std::fs::write(&pa, a).unwrap();
        std::fs::write(&pb, b).unwrap();
        panic!(
            "the emitter built from one emission and the emitter built from its own emit different C: diff {} {}",
            pa.display(),
            pb.display()
        );
    };
    if refresh {
        // A refresh writes the current emitter's own emission, once a third emitter built from it
        // has emitted the same thing: the bundle written is a fixpoint on the day it is written.
        let stage2 = scratch.join("stage2");
        ply_codegen::c::bundle::write(&stage2, &c2, &r2, &identity).unwrap();
        let (c3, r3, _) = emit_with(source, Some(&stage2), &scratch);
        if !same(&c2, &r2, &c3, &r3) {
            differ("refresh", &c2, &c3);
        }
        ply_codegen::c::bundle::write(&bundle, &c2, &r2, &identity).unwrap();
        eprintln!("bootstrap bundle written to {}", bundle.display());
    } else if !same(&c1, &r1, &c2, &r2) {
        differ("fixpoint", &c1, &c2);
    }
    if let Some(recorded) = ply_codegen::c::bundle::sources_digest(&bundle)
        && recorded != identity
    {
        eprintln!(
            "the bundle was emitted from sources {recorded} and these are {identity}; it still serves, and PLY_C_BOOTSTRAP_REFRESH=1 would bring it up to date"
        );
    }
    let _ = std::fs::remove_dir_all(&scratch);
}
