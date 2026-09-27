use indexmap::IndexMap;
use ply_span::Symbol;
use ply_ty::{DefHash, HashOutput};

/// A small table touching every list the digest frames: `defs`, `decls`, `specs`, `tests`, `laws`.
fn sample_hashes() -> HashOutput {
    let h = |v: u8| DefHash([v; 32]);
    let mut defs = IndexMap::new();
    defs.insert(Symbol::new("a"), h(1));
    let mut decls = IndexMap::new();
    decls.insert(Symbol::new("b"), h(2));
    let mut specs = IndexMap::new();
    specs.insert(Symbol::new("a"), vec![h(3), h(4)]);
    HashOutput {
        defs,
        own: IndexMap::new(),
        decls,
        tests: vec![h(5)],
        laws: vec![h(6)],
        specs,
        spec_texts: IndexMap::new(),
        law_texts: Vec::new(),
        deps: IndexMap::new(),
        closure: IndexMap::new(),
    }
}

#[test]
fn hex_round_trips_and_short_is_a_prefix() {
    let h = DefHash::of(b"fn f() = 1");
    assert_eq!(h.to_hex().len(), 64);
    assert_eq!(h.short().len(), 12);
    assert!(h.to_hex().starts_with(&h.short()));
    assert_eq!(DefHash::from_hex(&h.to_hex()), Some(h));
    assert_eq!(h.to_string(), h.short());
    assert_eq!(DefHash::from_hex("nonsense"), None);
    assert_eq!(DefHash::from_hex(&"z".repeat(64)), None);
}

#[test]
fn a_hash_serializes_as_a_hex_string() {
    let h = DefHash::of(b"fn f() = 1");
    let json = serde_json::to_string(&h).unwrap();
    assert_eq!(json, format!("\"{}\"", h.to_hex()));
    assert_eq!(serde_json::from_str::<DefHash>(&json).unwrap(), h);
    assert!(serde_json::from_str::<DefHash>("\"short\"").is_err());
}

/// The digest is the compiler's own (`hash.ply`'s `digest`) as much as it is this one: both are
/// pinned to the same value, so a change to either framing breaks the test on that side.
#[test]
fn the_digest_of_the_published_tables_is_pinned() {
    assert_eq!(
        sample_hashes().digest().to_hex(),
        "ac9e5970a91b7f3d7f83d63ec7ce2f66789c1204021e9ab1357f24a9878f07a8"
    );
    // The framing is length-prefixed, so one more hash moves it.
    let mut grown = sample_hashes();
    grown.tests.push(DefHash([7; 32]));
    assert_ne!(grown.digest(), sample_hashes().digest());
}
