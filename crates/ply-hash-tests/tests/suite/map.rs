//! `Map` in a definition body, and what it does to that definition's hash.

use ply_hash::{DefHash, HashOutput, hash_program_ast};
use ply_span::{SourceId, Symbol};
use ply_syntax::ast::{ModuleName, Program};

fn program_of(files: &[(&str, &str)]) -> Program {
    let inputs = files
        .iter()
        .enumerate()
        .map(|(i, (name, source))| (SourceId(i as u32), ModuleName::from_dotted(name), *source));
    match ply_syntax::parse_program(inputs) {
        Ok(program) => program,
        Err(diags) => panic!("program did not parse: {diags:#?}"),
    }
}

fn hashes(source: &str) -> HashOutput {
    let mut program = program_of(&[("m", source)]);
    let resolved = match ply_syntax::resolve(&mut program) {
        Ok(r) => r,
        Err(diags) => panic!("program did not resolve: {diags:#?}"),
    };
    hash_program_ast(&program, &resolved).expect("program should hash")
}

fn hash_of(source: &str, name: &str) -> DefHash {
    hashes(source).defs[&Symbol::new(format!("m.{name}"))]
}

const BUILD: &str = "\
fn build() -> Map<String, Int> =
  map_insert(map_insert(map_new(), \"a\", 1), \"b\", 2)
";

/// Renaming a local changes nothing, exactly as it does for every other body.
#[test]
fn a_map_body_hashes_like_any_other_body() {
    let one = hash_of(
        "fn build() -> Map<String, Int> = { let m = map_new(); map_insert(m, \"a\", 1) }\n",
        "build",
    );
    let other = hash_of(
        "fn build() -> Map<String, Int> = { let entries = map_new(); map_insert(entries, \"a\", 1) }\n",
        "build",
    );
    assert_eq!(one, other, "a local's name reached the hash");
}

/// Two definitions that build the same *value* by different insertion orders are two computations,
/// so they hash differently — and that is right.
#[test]
fn two_ways_of_building_one_map_are_two_definitions() {
    let forward = hash_of(BUILD, "build");
    let backward = hash_of(
        "fn build() -> Map<String, Int> =\n  map_insert(map_insert(map_new(), \"b\", 2), \"a\", 1)\n",
        "build",
    );
    assert_ne!(forward, backward);
}

/// The declared `Map<k, v>` is part of the published signature, so swapping the key and value types
/// moves the hash even though the body is untouched.
#[test]
fn the_declared_key_and_value_types_are_in_the_hash() {
    let a = hash_of("fn empty() -> Map<String, Int> = map_new()\n", "empty");
    let b = hash_of("fn empty() -> Map<Int, String> = map_new()\n", "empty");
    assert_ne!(a, b);
}

/// A pin, and therefore a claim across runs, processes and builds: a definition whose body is a map
/// is normalized to these bytes and to no others.
#[test]
fn a_map_body_normalizes_to_a_pinned_hash() {
    assert_eq!(
        hash_of(BUILD, "build").to_hex(),
        "e0f5b0bfddb15952a4147a798953ad50fe2d71faf63f52c16518b3f571421c61",
        "the normalized form of a map-building definition moved. If normalization changed \
         on purpose, paste the digest above and bump `FRONTEND_VERSION`"
    );
}
