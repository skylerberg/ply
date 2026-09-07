//! The front end written in Ply, entered from Rust as a compiled artefact.
//!
//! This is the bootstrap's spine and it is the part that had never been run. `spikes/ply-parser`
//! is a front end for Ply written in Ply -- twelve modules, fourteen hundred definitions, and six
//! differentials against the Rust one -- but everything that has ever driven it drove it as a
//! *program*: `ply run` over a generated probe, one process per input, the whole front end
//! interpreted. Nothing has loaded it as an object and called into it.
//!
//! What that leaves open is not whether the Ply front end is correct. `spikes/ply-parser/run.sh`
//! settles that, in CI, and it is the first link of the chain this file completes:
//!
//!   1. the Ply front end, **interpreted**, answers what `crates/ply-syntax` answers -- the spike's
//!      own differential, over every `.ply` file in the tree;
//!   2. the Ply front end, **compiled and entered from Rust**, answers what it answers interpreted
//!      -- this file;
//!
//! so the artefact answers what the Rust front end answers, and the remaining work to replace the
//! Rust front end is the value bridge rather than the plumbing.
//!
//! A binary of its own because it builds fourteen hundred definitions; the unit cache
//! (`c/cache.rs`) makes that milliseconds after the first run, and the `development` profile makes
//! the first run a quarter of a second rather than thirty-eight.

use ply_codegen::Source;
use ply_eval::{Machine, Value};
use ply_span::{Span, Symbol};
use ply_syntax::ast::{ModuleName, Program};
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate sits two levels under the repository root")
        .to_path_buf()
}

struct Loaded {
    program: &'static Program,
    resolved: &'static ply_syntax::resolve::Resolved,
    check: &'static ply_core::CheckOutput,
    hashes: ply_hash::HashOutput,
}

/// The self-hosted front end and the standard library it imports, as one program.
fn front_end() -> &'static Loaded {
    let dir = repo().join("spikes/ply-parser");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "ply"))
        .collect();
    files.sort();
    assert!(files.len() > 8, "the front end changed shape: {files:?}");

    let mut sources = ply_span::SourceMap::new();
    let mut inputs = Vec::new();
    for (module, text) in ply_std::sources() {
        let module = ModuleName::from_dotted(module);
        let id = sources.add(ply_std::pseudo_path(&module), text.to_string());
        inputs.push((id, module, text));
    }
    for path in &files {
        let stem = path.file_stem().and_then(|s| s.to_str()).expect("a stem");
        let text: &'static str = Box::leak(
            std::fs::read_to_string(path)
                .expect("the front end is readable")
                .into_boxed_str(),
        );
        let id = sources.add(path.clone(), text.to_string());
        inputs.push((id, ModuleName::from_dotted(stem), text));
    }
    let mut ast = ply_syntax::parse_program(inputs).expect("the front end parses");
    assert!(ply_derive::expand_program(&mut ast).is_empty());
    let resolved = ply_syntax::resolve::resolve(&mut ast).expect("the front end resolves");
    let check = ply_core::check_program(&ast, &resolved).expect("the front end checks");
    let program: &'static Program = Box::leak(Box::new(ast));
    let resolved = Box::leak(Box::new(resolved));
    let check = Box::leak(Box::new(check));
    let hashes = ply_hash::hash_program(program, resolved, check).expect("the front end hashes");
    Box::leak(Box::new(Loaded {
        program,
        resolved,
        check,
        hashes,
    }))
}

/// The artefact: every definition of the front end, emitted as C, compiled and loaded.
///
/// Keyed on each definition's content hash, which is what lets the emitted bodies and the whole
/// unit be kept between runs -- the same keys `ply test --backend c` uses, so a developer who has
/// run the front end once has already paid for this.
fn artefact(loaded: &'static Loaded) -> ply_codegen::c::Native {
    let keys: std::collections::HashMap<String, String> = loaded
        .hashes
        .defs
        .iter()
        .map(|(name, h)| (name.to_string(), h.to_hex()))
        .collect();
    let source: &'static Source = Box::leak(Box::new(Source::keyed(
        loaded.program,
        loaded.resolved,
        loaded.check,
        keys,
    )));
    let names: Vec<String> = source.functions();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let (native, _refused) = ply_codegen::c::build(source, &refs, ply_codegen::Opts::default())
        .expect("the front end compiles");
    native
}

/// A few programs the front end has to have an opinion about, including one that does not parse.
const INPUTS: &[&str] = &[
    "fn f(x: Int) -> Int = x + 1\n",
    "type T = { a: Int, b: Bytes }\nfn g(t: T) -> Int = t.a\n",
    "fn h(xs: List<Int>) -> Int = fold(xs, 0, |a: Int, b: Int| a + b)\n",
    "effect e { read r[x]() -> Int }\nfn k() -> Int / {e.read[q]} = e.r[q]()\n",
    "fn broken(x: Int) -> Int = x +\n",
    "",
];

#[test]
fn the_compiled_front_end_answers_what_the_interpreted_one_answers() {
    let loaded = front_end();
    let native = artefact(loaded);

    let entry = "items.dump";
    assert!(
        native.entry(entry).is_some(),
        "the artefact has no `{entry}`, so nothing below is a comparison"
    );
    let mut machine = Machine::new(loaded.program, loaded.resolved, loaded.check);
    let mut through_artefact = Vec::new();
    let mut interpreted = Vec::new();

    for input in INPUTS {
        let arg = Value::bytes(Vec::from(input.as_bytes()));

        let mut ctx = native.context();
        ctx.fuel = 10_000_000;
        let word = ctx.heap.to_word(&native.tables().layouts, &arg);
        let f = native.entry(entry).expect("checked above");
        let answer = unsafe { f(&mut ctx, [word].as_ptr()) };
        assert_eq!(ctx.failed, 0, "the artefact raised on {input:?}");
        let mut walked = ply_codegen::heap::Walked::default();
        let compiled = ply_codegen::heap::Heap::to_value_counted(
            &native.tables().layouts,
            answer,
            &mut walked,
        );

        let reference = machine
            .call(entry, vec![arg], Span::DUMMY)
            .unwrap_or_else(|d| panic!("the interpreter raised on {input:?}: {}", d.message));

        through_artefact.push(compiled.render());
        interpreted.push(reference.render());
    }

    assert_eq!(
        through_artefact, interpreted,
        "the front end compiled and the front end interpreted disagree"
    );
    // Not vacuous, twice over: an empty answer for every input, or the same answer for every
    // input, would both pass the comparison above.
    assert!(
        through_artefact.iter().all(|d| d.len() > 2),
        "every dump was empty, so the comparison above compared nothing: {through_artefact:?}"
    );
    let distinct: std::collections::HashSet<&String> = through_artefact.iter().collect();
    assert_eq!(
        distinct.len(),
        INPUTS.len(),
        "two of {} inputs dumped the same, so the front end is not reading them",
        INPUTS.len()
    );
    let _ = Symbol::new(entry);
}

/// The artefact hands back the *tree*, not only a rendering of it.
///
/// `items.dump` answers a `String`, which is the shape the spike's differentials compare and the
/// shape that needs no bridge. Replacing `crates/ply-syntax` needs the other one: `items.parse`
/// answers an `RModule`, a record of records and constructors, and the question this settles is
/// whether that crosses at all -- whether the seam converts a whole parse tree into a `Value` a
/// bridge could walk, or declines it the way it declines a handle.
///
/// It crosses. What is left between here and a Ply front end that replaces the Rust one is a
/// converter from that `Value` to `ply_syntax::ast::Program` -- fifteen enums and about
/// ninety-five variants, so on the order of the 1,437 lines the spike's own dumper takes to walk
/// the same tree the other way.
#[test]
fn the_artefact_hands_back_a_tree_and_not_only_a_rendering() {
    let loaded = front_end();
    let native = artefact(loaded);
    let entry = "items.parse";
    let Some(f) = native.entry(entry) else {
        panic!("the artefact has no `{entry}`");
    };

    let mut ctx = native.context();
    ctx.fuel = 10_000_000;
    let arg = Value::bytes(Vec::from(
        "fn f(x: Int) -> Int = x + 1
type T = { a: Int }
"
        .as_bytes(),
    ));
    let word = ctx.heap.to_word(&native.tables().layouts, &arg);
    let answer = unsafe { f(&mut ctx, [word].as_ptr()) };
    assert_eq!(ctx.failed, 0, "`{entry}` raised");

    let mut walked = ply_codegen::heap::Walked::default();
    let tree =
        ply_codegen::heap::Heap::to_value_counted(&native.tables().layouts, answer, &mut walked);
    assert!(
        !walked.handle,
        "the parse tree carries a handle, so it cannot leave the entry that made it"
    );
    let Value::Record(fields) = &tree else {
        panic!("`{entry}` answered {} and not a record", tree.type_name());
    };
    assert!(
        fields.iter().any(|(n, _)| n.as_str() == "node"),
        "the answer has no `node`, so it is not the `RModule` this bridge would start from: {:?}",
        fields
            .iter()
            .map(|(n, _)| n.to_string())
            .collect::<Vec<_>>()
    );
    // The same tree the interpreter builds, which is what makes it a bridge's input rather than
    // merely a value.
    let mut machine = Machine::new(loaded.program, loaded.resolved, loaded.check);
    let reference = machine
        .call(entry, vec![arg], Span::DUMMY)
        .expect("the interpreter parses");
    assert_eq!(
        tree.render(),
        reference.render(),
        "the compiled parse tree and the interpreted one differ"
    );
}
