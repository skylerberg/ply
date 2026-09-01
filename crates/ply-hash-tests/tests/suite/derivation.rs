//! What a `derive` does to a hash.

use ply_hash::{DefHash, HashOutput, hash_program_ast};
use ply_span::{SourceId, Symbol};
use ply_syntax::ast::{ModuleName, Program};

fn program_of(files: &[(&str, &str)]) -> Program {
    let inputs = files
        .iter()
        .enumerate()
        .map(|(i, (name, source))| (SourceId(i as u32), ModuleName::from_dotted(name), *source));
    let mut program = match ply_syntax::parse_program(inputs) {
        Ok(program) => program,
        Err(diags) => panic!("program did not parse: {diags:#?}"),
    };
    let diags = ply_derive::expand_program(&mut program);
    assert!(diags.is_empty(), "expansion failed: {diags:#?}");
    program
}

/// Enough of `std.json` for `import std.json` to resolve.
const JSON_STUB: &str = "// std.json, as much of it as a hash needs\n";

fn hashes(source: &str) -> HashOutput {
    let mut program = program_of(&[("std.json", JSON_STUB), ("m", source)]);
    let resolved = match ply_syntax::resolve(&mut program) {
        Ok(r) => r,
        Err(diags) => panic!("program did not resolve: {diags:#?}"),
    };
    hash_program_ast(&program, &resolved).expect("program should hash")
}

fn hash_of(source: &str, name: &str) -> DefHash {
    let out = hashes(source);
    let key = Symbol::new(format!("m.{name}"));
    *out.defs
        .get(&key)
        .unwrap_or_else(|| panic!("no definition `{key}` in {:?}", out.defs.keys()))
}

/// A `derive eq` for a one-field record, written out.
const HANDWRITTEN: &str = "type Order = {id: Int}\n\
                           fn by_hand() -> {eq: (Order, Order) -> Bool} = \
                           {eq: |da: Order, db: Order| da == db}";

#[test]
fn a_generated_definition_and_a_hand_written_one_are_the_same_definition() {
    let generated = hash_of("type Order = {id: Int}\nderive eq for Order", "order_eq");
    let written = hash_of(HANDWRITTEN, "by_hand");
    assert_eq!(
        generated, written,
        "provenance is erased by normalization: a hand-written definition \
         byte-identical to a generated one is the same computation"
    );
}

#[test]
fn renaming_the_derived_type_moves_no_hash() {
    let before = hash_of(
        "import std.json\ntype Order = {id: Int, sku: String}\nderive json for Order",
        "order_json",
    );
    let after = hash_of(
        "import std.json\ntype Purchase = {id: Int, sku: String}\nderive json for Purchase",
        "purchase_json",
    );
    assert_eq!(
        before, after,
        "the generated definition's *name* changed and its body did not: \
         field names are what the encoding contains, and neither moved"
    );
}

#[test]
fn renaming_a_variant_moves_the_hash() {
    let before = hash_of(
        "import std.json\ntype Status = Placed | Shipped(Int)\nderive json for Status",
        "status_json",
    );
    let after = hash_of(
        "import std.json\ntype Status = Created | Shipped(Int)\nderive json for Status",
        "status_json",
    );
    assert_ne!(
        before, after,
        "the JSON tag changed, which is an observable protocol change"
    );
}

#[test]
fn reordering_two_fields_moves_the_hash() {
    let before = hash_of(
        "import std.json\ntype Order = {id: Int, sku: String}\nderive json for Order",
        "order_json",
    );
    let after = hash_of(
        "import std.json\ntype Order = {sku: String, id: Int}\nderive json for Order",
        "order_json",
    );
    assert_ne!(before, after, "JSON object order is observable");
}

#[test]
fn adding_a_field_moves_the_hash_and_removing_it_moves_it_back() {
    let one = hash_of(
        "import std.json\ntype Order = {id: Int}\nderive json for Order",
        "order_json",
    );
    let two = hash_of(
        "import std.json\ntype Order = {id: Int, sku: String}\nderive json for Order",
        "order_json",
    );
    let back = hash_of(
        "import std.json\ntype Order = {id: Int}\nderive json for Order",
        "order_json",
    );
    assert_ne!(one, two);
    assert_eq!(one, back);
}

#[test]
fn a_change_to_a_composed_types_codec_moves_the_composing_one() {
    let before = hash_of(
        "import std.json\ntype Line = {sku: String}\ntype Order = {lines: List<Line>}\n\
         derive json for Line\nderive json for Order",
        "order_json",
    );
    let after = hash_of(
        "import std.json\ntype Line = {sku: String, qty: Int}\ntype Order = {lines: List<Line>}\n\
         derive json for Line\nderive json for Order",
        "order_json",
    );
    assert_ne!(
        before, after,
        "derivation composes through named types by *hash*, which is what makes \
         a change to `Line` re-select exactly the tests that reach an `Order`"
    );
}

#[test]
fn moving_a_derivation_to_another_module_moves_no_hash() {
    let here = {
        let source = "type Order = {id: Int}\nderive eq for Order";
        let mut program = program_of(&[("m", source)]);
        let resolved = ply_syntax::resolve(&mut program).expect("resolves");
        hash_program_ast(&program, &resolved).expect("hashes").defs[&Symbol::new("m.order_eq")]
    };
    let there = {
        let mut program = program_of(&[
            ("m", "fn unrelated() -> Int = 1"),
            ("other", "type Order = {id: Int}\nderive eq for Order"),
        ]);
        let resolved = ply_syntax::resolve(&mut program).expect("resolves");
        hash_program_ast(&program, &resolved).expect("hashes").defs[&Symbol::new("other.order_eq")]
    };
    assert_eq!(
        here, there,
        "a module is metadata over hashes, not part of one"
    );
}

#[test]
fn generating_the_same_program_twice_produces_the_same_hashes() {
    let source = "import std.json\n\
                  type Order = {id: Int, sku: String}\n\
                  type Status = Placed | Shipped(Int)\n\
                  derive json for Order\nderive eq for Order\nderive json for Status";
    let first = hashes(source);
    for _ in 0..16 {
        let again = hashes(source);
        assert_eq!(first.defs, again.defs, "derivation is not deterministic");
    }
}
