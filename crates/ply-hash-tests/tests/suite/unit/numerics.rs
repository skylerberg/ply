use ply_hash::{DefHash, hash_program_with_bodies};
use ply_span::SourceId;
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::resolve::Resolved;

fn compile(source: &str) -> (Program, Resolved) {
    let mut program =
        ply_syntax::parse_program([(SourceId(0), ModuleName::from_dotted("m"), source)])
            .unwrap_or_else(|d| panic!("did not parse: {d:#?}"));
    let resolved =
        ply_syntax::resolve(&mut program).unwrap_or_else(|d| panic!("did not resolve: {d:#?}"));
    (program, resolved)
}

fn hash_of(source: &str) -> DefHash {
    let (program, resolved) = compile(source);
    let (out, _) = hash_program_with_bodies(&program, &resolved).expect("hashes");
    let mut hashes: Vec<DefHash> = out.defs.values().copied().collect();
    assert_eq!(hashes.len(), 1, "expected exactly one definition");
    hashes.pop().unwrap()
}

#[test]
fn the_three_numeric_literals_are_three_definitions() {
    let int = hash_of("pub fn f() -> Int = 1");
    let float = hash_of("pub fn f() -> Float = 1.0");
    let decimal = hash_of("pub fn f() -> Decimal = 1m");
    assert_ne!(int, float);
    assert_ne!(float, decimal);
    assert_ne!(int, decimal);
}

#[test]
fn positive_and_negative_zero_are_two_definitions() {
    assert_ne!(
        hash_of("pub fn f() -> Float = 0.0"),
        hash_of("pub fn f() -> Float = -0.0")
    );
}

#[test]
fn two_decimals_of_one_value_at_two_scales_are_two_definitions() {
    assert_ne!(
        hash_of("pub fn f() -> Decimal = 1.5m"),
        hash_of("pub fn f() -> Decimal = 1.50m")
    );
}

#[test]
fn a_float_hashes_by_bit_pattern_rather_than_by_spelling() {
    assert_eq!(
        hash_of("pub fn f() -> Float = 1.0"),
        hash_of("pub fn f() -> Float = 1e0")
    );
    assert_ne!(
        hash_of("pub fn f() -> Float = 1.0"),
        hash_of("pub fn f() -> Float = 1.0000000000000002")
    );
}

#[test]
fn renaming_a_definition_holding_a_numeric_literal_changes_no_hash() {
    assert_eq!(
        hash_of("pub fn price() -> Decimal = 19.99m"),
        hash_of("pub fn amount() -> Decimal = 19.99m")
    );
}
