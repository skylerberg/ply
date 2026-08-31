//! Canonical encoding of the numeric literals, and the round trip through the
//! stored body.
//!
//! The invariant this file exists for: **two values a program can tell apart
//! must not share a definition hash, and a body must decode to the definition it
//! is filed under.** A cache is keyed on those bytes, so a collision here is a
//! wrong answer served forever, and a lossy decode is a body that fails its own
//! self-check on a healthy store.

use crate::body::{BodySet, StoredBody, reconstruct};
use crate::{DefHash, hash_program_with_bodies};
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

fn bodies_of(source: &str) -> BodySet {
    let (program, resolved) = compile(source);
    hash_program_with_bodies(&program, &resolved)
        .expect("hashes")
        .1
}

/// `1`, `1.0` and `1m` have three types. Sharing a hash would let a cached
/// result for one be served for another, which is the failure a
/// content-addressed cache has no way to notice.
#[test]
fn the_three_numeric_literals_are_three_definitions() {
    let int = hash_of("pub fn f() -> Int = 1");
    let float = hash_of("pub fn f() -> Float = 1.0");
    let decimal = hash_of("pub fn f() -> Decimal = 1m");
    assert_ne!(int, float);
    assert_ne!(float, decimal);
    assert_ne!(int, decimal);
}

/// The bit patterns differ, so the definitions differ. A normalizer that folded
/// them would make two textually distinct programs one definition while
/// `1.0 / -0.0` still tells them apart.
#[test]
fn positive_and_negative_zero_are_two_definitions() {
    assert_ne!(
        hash_of("pub fn f() -> Float = 0.0"),
        hash_of("pub fn f() -> Float = -0.0")
    );
}

/// Equal in value and differently written, so differently hashed. Stated rather
/// than smoothed over: the same pair is *one* map key, and both facts follow from
/// the scale being part of what the literal says.
#[test]
fn two_decimals_of_one_value_at_two_scales_are_two_definitions() {
    assert_ne!(
        hash_of("pub fn f() -> Decimal = 1.5m"),
        hash_of("pub fn f() -> Decimal = 1.50m")
    );
}

/// The encoding is over the `f64`, not over the source text, so two spellings of
/// one binary64 are one definition and two neighbouring doubles are two.
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

/// Renaming is free for a numeric definition exactly as it is for any other: the
/// literal is in the bytes and the name is not.
#[test]
fn renaming_a_definition_holding_a_numeric_literal_changes_no_hash() {
    assert_eq!(
        hash_of("pub fn price() -> Decimal = 19.99m"),
        hash_of("pub fn amount() -> Decimal = 19.99m")
    );
}

fn round_trip(source: &str) {
    let bodies = bodies_of(source);
    let mut rebuilt = reconstruct(&bodies).expect("bodies reconstruct");
    let resolved = ply_syntax::resolve(&mut rebuilt.program)
        .unwrap_or_else(|d| panic!("reconstruction did not resolve: {d:#?}"));
    let (again, _) =
        hash_program_with_bodies(&rebuilt.program, &resolved).expect("rebuilt program hashes");
    for (hash, name) in &rebuilt.names {
        let back = again
            .defs
            .get(name)
            .or_else(|| again.decls.get(name))
            .unwrap_or_else(|| panic!("`{name}` is missing from the rebuilt program"));
        assert_eq!(back, hash, "`{name}` decoded to a different definition");
    }
}

/// For a `Float` this is where the bit pattern earns its place: decoding through
/// the numeric value would merge `0.0` and `-0.0`, and the body's self-check
/// would then fail on a store that is perfectly healthy.
#[test]
fn numeric_literals_survive_the_stored_body_round_trip() {
    round_trip("pub fn f() -> Float = -0.0");
    round_trip("pub fn f() -> Float = 0.0");
    round_trip("pub fn f() -> Decimal = 1.50m");
    round_trip("pub fn f() -> Decimal = -0.000000000000000000000000001m");
    round_trip("pub fn f() -> Float = 1e300");
    round_trip(
        "pub fn total(a: Decimal, b: Decimal) -> Decimal = a + b * 2m\n\
         pub fn rate() -> Float = 1.5 / 0.0\n",
    );
}

/// A `Decimal` outside the type's range never leaves the lexer, so it never
/// enters a body — and a stream carrying one is refused rather than decoded into
/// a value the evaluator would have to invent.
#[test]
fn a_body_carrying_an_out_of_range_decimal_is_refused() {
    let bodies = bodies_of("pub fn f() -> Decimal = 1.50m");
    let (_, body) = bodies.defs().next().expect("one definition");
    let mut bytes = body.as_bytes().to_vec();

    // The scale is the last little-endian `2` in the stream: the encoder writes
    // the mantissa's sixteen bytes and then the scale's four.
    let scale = bytes
        .windows(4)
        .rposition(|w| w == 2u32.to_le_bytes())
        .expect("the scale is in the stream");
    bytes[scale..scale + 4].copy_from_slice(&99u32.to_le_bytes());

    let body = StoredBody::from_bytes(bytes).expect("still a body envelope");
    // Filed under its *own* key, so the envelope's self-check passes and the
    // range guard is what refuses it. Filing it under the original hash would
    // pass this test on the checksum alone.
    let key = body.key().expect("a solo body keys itself");
    let mut tampered = BodySet::default();
    tampered.insert(key, body);
    let diags = reconstruct(&tampered).expect_err("a scale of 99 is not a `Decimal`");
    assert!(
        diags.iter().any(|d| d.message.contains("scale 99")),
        "the refusal must name the scale: {diags:#?}"
    );
}
