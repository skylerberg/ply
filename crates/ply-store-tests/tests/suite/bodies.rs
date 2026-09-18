//! Definition bodies through the store.

use ply_hash::body::BodySet;
use ply_hash::{DefHash, HashOutput, hash_program_with_bodies};
use ply_span::{SourceId, Symbol};
use ply_store::{BODY_ENCODING, DefBody, Store};
use ply_syntax::ast::ModuleName;
use std::path::{Path, PathBuf};

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(name: &str) -> TempRoot {
        let dir = std::env::temp_dir().join(format!(
            "ply-bodies-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temp root");
        TempRoot(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn open(&self) -> Store {
        Store::open(&self.0).expect("the store should open")
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const SOURCE: &str = r#"
effect db { read get[r](key: Int) -> Int }

type Colour = | Red | Blue(Int)

fn shade(c: Colour) -> Int = match c { Red -> 0, Blue(n) -> n }

fn lookup(key: Int) -> Int / {db.read[users]} = db.get[users](key) + shade(Red)
"#;

fn compile(source: &str) -> (HashOutput, BodySet) {
    let mut program =
        ply_syntax::parse_program([(SourceId(0), ModuleName::from_dotted("m"), source)])
            .expect("it should parse");
    let resolved = ply_syntax::resolve(&mut program).expect("it should resolve");
    hash_program_with_bodies(&program, &resolved).expect("it should hash")
}

fn every_hash(hashes: &HashOutput) -> Vec<DefHash> {
    hashes
        .defs
        .values()
        .chain(hashes.decls.values())
        .copied()
        .collect()
}

/// A later process rebuilds the program without the source or any names.
#[test]
fn a_stored_definition_set_rebuilds_into_a_program_that_checks() {
    let root = TempRoot::new("rebuild");
    let (hashes, bodies) = compile(SOURCE);

    let mut store = root.open();
    for (hash, body) in bodies.defs() {
        store.put_body(hash, DefBody::of(body.clone()));
    }
    store.flush().expect("the cache should flush");
    assert_eq!(store.bodies_len(), every_hash(&hashes).len());

    let reopened = root.open();
    let mut rebuilt = reopened
        .reconstruct(every_hash(&hashes))
        .expect("the stored bodies should rebuild");

    ply_syntax::resolve(&mut rebuilt.program).expect("it should resolve");
    let printed = ply_syntax::print::program(&rebuilt.program);
    let ids: Vec<SourceId> = (0..printed.len()).map(|i| SourceId(i as u32)).collect();
    let check = ply_codegen::c::producer::checked_front(&printed, &ids)
        .unwrap_or_else(|e| panic!("it should check: {e:#}"))
        .check;

    let lookup = hashes.defs[&Symbol::new("m.lookup")];
    let name = rebuilt.name_of(lookup).expect("a name for lookup");
    let info = &check.defs[name];
    assert_eq!(info.footprint.atoms().count(), 1);
    assert_eq!(
        info.footprint.atoms().next().unwrap().resource,
        ply_ty::Resource::Named(Symbol::new("users"))
    );
}

#[test]
fn a_body_verifies_against_its_key_and_no_other() {
    let (hashes, bodies) = compile(SOURCE);
    let shade = hashes.defs[&Symbol::new("m.shade")];
    let lookup = hashes.defs[&Symbol::new("m.lookup")];

    let body = DefBody::of(bodies.get(shade).expect("a body for shade").clone());
    assert_eq!(body.key(), Some(shade));
    assert!(body.verifies_as(shade));
    assert!(!body.verifies_as(lookup));
}

#[test]
fn bytes_that_are_not_a_body_have_no_key() {
    let junk = DefBody::new(BODY_ENCODING, b"not a body".to_vec());
    assert_eq!(junk.key(), None);
    assert!(!junk.verifies_as(DefHash([0; 32])));
    assert_eq!(junk.stored(), None);
}

#[test]
fn a_body_written_under_another_encoding_is_not_decoded() {
    let (hashes, bodies) = compile(SOURCE);
    let shade = hashes.defs[&Symbol::new("m.shade")];
    let bytes = bodies.get(shade).unwrap().clone().into_bytes();

    let foreign = DefBody::new(BODY_ENCODING + 1, bytes);
    assert_eq!(foreign.stored(), None);
    assert_eq!(foreign.key(), None);
}

#[test]
fn a_definition_with_no_stored_body_is_named_rather_than_skipped() {
    let root = TempRoot::new("missing");
    let (hashes, bodies) = compile(SOURCE);
    let shade = hashes.defs[&Symbol::new("m.shade")];
    let lookup = hashes.defs[&Symbol::new("m.lookup")];

    let mut store = root.open();
    store.put_body(shade, DefBody::of(bodies.get(shade).unwrap().clone()));

    let (set, missing) = store.body_set([shade, lookup]);
    assert_eq!(set.len(), 1);
    assert_eq!(missing, vec![lookup]);

    let diags = store
        .reconstruct([shade, lookup])
        .expect_err("an incomplete set must not rebuild");
    assert!(
        diags
            .iter()
            .any(|d| d.code == ply_span::codes::CACHE_UNREADABLE)
    );
}

#[test]
fn a_misfiled_body_is_not_handed_back() {
    let root = TempRoot::new("misfiled");
    let (hashes, bodies) = compile(SOURCE);
    let shade = hashes.defs[&Symbol::new("m.shade")];
    let lookup = hashes.defs[&Symbol::new("m.lookup")];

    let mut store = root.open();
    store.put_body(lookup, DefBody::of(bodies.get(shade).unwrap().clone()));

    let (set, missing) = store.body_set([lookup]);
    assert!(set.is_empty());
    assert_eq!(missing, vec![lookup]);
}

#[test]
fn bodies_survive_a_round_trip_through_the_cache_file() {
    let root = TempRoot::new("persist");
    let (_, bodies) = compile(SOURCE);

    let mut store = root.open();
    for (hash, body) in bodies.defs() {
        store.put_body(hash, DefBody::of(body.clone()));
    }
    store.flush().expect("the cache should flush");

    let reopened = root.open();
    for (hash, body) in bodies.defs() {
        let back = reopened.body(hash).expect("a body after reopening");
        assert_eq!(back.as_bytes(), body.as_bytes());
        assert!(back.verifies_as(hash));
    }
    assert!(root.path().join(".ply-cache").exists());
}
