//! An `effect set` is an abbreviation, and abbreviations do not change meaning.

use ply_hash::{DefHash, HashOutput, hash_ast};
use ply_span::{SourceId, Symbol};

fn hashes(source: &str) -> HashOutput {
    let module = match ply_syntax::parse(SourceId(0), source) {
        Ok(m) => m,
        Err(diags) => panic!("source did not parse: {diags:#?}\n---\n{source}"),
    };
    hash_ast(&module).expect("module should hash")
}

fn def(source: &str, name: &str) -> DefHash {
    let out = hashes(source);
    *out.defs
        .get(&Symbol::new(name))
        .unwrap_or_else(|| panic!("no definition named `{name}` in\n{source}"))
}

const EFFECTS: &str = "\
effect db {
  read  all[t]() -> Int
  write save[t](n: Int) -> Int
}
effect log {
  write line(n: Int) -> Int
}
effect clock {
  read now() -> Int
}
";

fn with_effects(rest: &str) -> String {
    format!("{EFFECTS}{rest}")
}

/// The headline property.
#[test]
fn a_named_row_and_a_written_one_are_one_definition() {
    let named = with_effects(
        "effect set Web = {db.read[users], log.write}\n\
         fn f() -> Int / {Web} = db.all[users]()\n",
    );
    let written = with_effects("fn f() -> Int / {db.read[users], log.write} = db.all[users]()\n");
    assert_eq!(def(&named, "f"), def(&written, "f"));
}

/// The same, with the alias mixed in among atoms written out beside it.
#[test]
fn a_set_beside_written_atoms_hashes_as_the_union() {
    let named = with_effects(
        "effect set Web = {db.read[users], log.write}\n\
         fn f() -> Int / {Web, clock.read} = db.all[users]()\n",
    );
    let written =
        with_effects("fn f() -> Int / {clock.read, db.read[users], log.write} = db.all[users]()\n");
    assert_eq!(def(&named, "f"), def(&written, "f"));
}

#[test]
fn a_nested_set_hashes_as_its_transitive_expansion() {
    let nested = with_effects(
        "effect set Inner = {db.read[users]}\n\
         effect set Web = {Inner, log.write}\n\
         fn f() -> Int / {Web} = db.all[users]()\n",
    );
    let written = with_effects("fn f() -> Int / {db.read[users], log.write} = db.all[users]()\n");
    assert_eq!(def(&nested, "f"), def(&written, "f"));
}

#[test]
fn renaming_a_set_moves_no_hash() {
    let before = with_effects(
        "effect set Web = {db.read[users], log.write}\n\
         fn f() -> Int / {Web} = db.all[users]()\n",
    );
    let after = with_effects(
        "effect set Surface = {db.read[users], log.write}\n\
         fn f() -> Int / {Surface} = db.all[users]()\n",
    );
    assert_eq!(def(&before, "f"), def(&after, "f"));
}

#[test]
fn reordering_a_sets_members_moves_no_hash() {
    let before = with_effects(
        "effect set Web = {db.read[users], log.write, clock.read}\n\
         fn f() -> Int / {Web} = db.all[users]()\n",
    );
    let after = with_effects(
        "effect set Web = {clock.read, log.write, db.read[users]}\n\
         fn f() -> Int / {Web} = db.all[users]()\n",
    );
    assert_eq!(def(&before, "f"), def(&after, "f"));
}

#[test]
fn writing_a_member_twice_moves_no_hash() {
    let before = with_effects(
        "effect set Web = {db.read[users], log.write}\n\
         fn f() -> Int / {Web} = db.all[users]()\n",
    );
    let after = with_effects(
        "effect set Web = {db.read[users], log.write, db.read[users]}\n\
         fn f() -> Int / {Web} = db.all[users]()\n",
    );
    assert_eq!(def(&before, "f"), def(&after, "f"));
}

#[test]
fn regrouping_two_sets_that_denote_the_same_atoms_moves_no_hash() {
    let before = with_effects(
        "effect set A = {db.read[users]}\n\
         effect set B = {log.write}\n\
         fn f() -> Int / {A, B} = db.all[users]()\n",
    );
    let after = with_effects(
        "effect set A = {db.read[users], log.write}\n\
         effect set B = {}\n\
         fn f() -> Int / {A, B} = db.all[users]()\n",
    );
    assert_eq!(def(&before, "f"), def(&after, "f"));
}

#[test]
fn declaring_a_set_nothing_uses_moves_no_hash() {
    let before = with_effects("fn f() -> Int / {db.read[users]} = db.all[users]()\n");
    let after = with_effects(
        "effect set Unused = {log.write, clock.read}\n\
         fn f() -> Int / {db.read[users]} = db.all[users]()\n",
    );
    assert_eq!(def(&before, "f"), def(&after, "f"));
}

/// Not a concession.
#[test]
fn changing_which_atoms_a_set_contains_moves_the_annotated_definitions_hash() {
    let before = with_effects(
        "effect set Web = {db.read[users]}\n\
         fn f() -> Int / {Web} = db.all[users]()\n",
    );
    let after = with_effects(
        "effect set Web = {db.read[users], log.write}\n\
         fn f() -> Int / {Web} = db.all[users]()\n",
    );
    assert_ne!(def(&before, "f"), def(&after, "f"));
}

#[test]
fn changing_a_set_moves_no_hash_of_a_definition_that_does_not_name_it() {
    let before = with_effects(
        "effect set Web = {db.read[users]}\n\
         fn f() -> Int / {Web} = db.all[users]()\n\
         fn g() -> Int / {clock.read} = clock.now()\n",
    );
    let after = with_effects(
        "effect set Web = {db.read[users], log.write}\n\
         fn f() -> Int / {Web} = db.all[users]()\n\
         fn g() -> Int / {clock.read} = clock.now()\n",
    );
    assert_eq!(def(&before, "g"), def(&after, "g"));
    assert_ne!(def(&before, "f"), def(&after, "f"));
}

/// `BODY_ENCODING` does not move for an alias, because an alias expands to atoms the row encoder
/// already writes.
#[test]
fn a_row_with_no_effect_set_normalizes_to_its_w2_hash() {
    let source = with_effects("fn f() -> Int / {db.read[users], log.write} = db.all[users]()\n");
    assert_eq!(
        def(&source, "f").to_hex(),
        "9f5af4f49c9a0a292b7978b42a3676e84b2af9a36ffffd136f6e54bf2586d1d4",
        "the normalized form of an ordinary annotated row moved. An `effect set` \
         must not change what a program that has none hashes to"
    );
}

/// The property the sort in `normalize::row` claims, tested without an `effect set` in sight —
/// because an alias splices a set's atoms in beside hand-written ones and can produce any order at
/// all, so a row whose meaning depends on how it was typed would make the headline property above
/// hold only by coincidence.
#[test]
fn reordering_a_written_row_moves_no_hash() {
    let before = with_effects("fn f() -> Int / {db.read[users], log.write} = db.all[users]()\n");
    let after = with_effects("fn f() -> Int / {log.write, db.read[users]} = db.all[users]()\n");
    assert_eq!(def(&before, "f"), def(&after, "f"));
}

/// The alias and the explicit row are written in *opposite* orders, which is the case the property
/// is actually about.
#[test]
fn a_set_matches_an_explicit_row_written_in_the_other_order() {
    let named = with_effects(
        "effect set Web = {log.write, db.read[users]}\n\
         fn f() -> Int / {Web} = db.all[users]()\n",
    );
    let written = with_effects("fn f() -> Int / {db.read[users], log.write} = db.all[users]()\n");
    assert_eq!(def(&named, "f"), def(&written, "f"));
}
