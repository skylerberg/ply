use ply_ty::DefHash;

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
