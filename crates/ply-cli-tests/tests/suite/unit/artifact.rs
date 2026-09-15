use ply_cli::artifact::*;
use ply_hash::DefHash;
use ply_hash::body::StoredBody;
use ply_span::codes;
use std::collections::BTreeMap;
use std::path::Path;

/// Bodies that are not decodable definitions, deliberately: everything below is about the
/// *container*, and `decode` checks a body against its key and nothing more.
fn sample() -> Artifact {
    let mut bodies = BTreeMap::new();
    let mut names = Vec::new();
    let mut entry = DefHash([0; 32]);
    for (i, label) in ["m.main", "m.helper", "m.Colour"].iter().enumerate() {
        let body = StoredBody::from_bytes(vec![0u8, i as u8, 2, 3]).expect("a solo envelope");
        let hash = body.key().unwrap();
        if i == 0 {
            entry = hash;
        }
        bodies.insert(hash, body);
        names.push((label.to_string(), hash));
    }
    names.sort();
    Artifact {
        frontend: *blake3::hash(ply_store::FRONTEND_VERSION.as_bytes()).as_bytes(),
        runtime: *blake3::hash(ply_store::RUNTIME_VERSION.as_bytes()).as_bytes(),
        body_encoding: ply_store::BODY_ENCODING,
        std: ply_std::digest(),
        entry,
        bodies,
        names,
        sources: Vec::new(),
        unit: None,
    }
}

#[test]
fn an_embedded_unit_survives_encoding() {
    let mut artifact = sample();
    artifact.unit = Some(EmbeddedUnit {
        runtime: "rt-digest".to_string(),
        text: vec![1, 2, 3, 4],
    });
    let bytes = artifact.encode();
    let (decoded, _) = decode(&bytes, Path::new("x.plyx")).expect("decodes");
    assert_eq!(decoded.unit, artifact.unit);
    assert_ne!(sample().encode(), bytes, "the unit is in the digest");
}

#[test]
fn a_round_trip_preserves_every_field() {
    let artifact = sample();
    let (back, warnings) = decode(&artifact.encode(), Path::new("t.plyx")).unwrap();
    assert_eq!(back, artifact);
    assert!(warnings.is_empty());
    assert_eq!(back.entry_name(), Some("m.main"));
}

#[test]
fn sources_round_trip_and_are_believed_only_as_bytes() {
    let mut artifact = sample();
    artifact.sources = vec![
        ("a.ply".to_string(), "fn a() -> Int = 1\n".to_string()),
        ("sub/b.ply".to_string(), "fn b() -> Int = 2\n".to_string()),
    ];
    let (back, _) = decode(&artifact.encode(), Path::new("t.plyx")).unwrap();
    assert_eq!(back, artifact);
}

#[test]
fn a_flipped_bit_in_a_body_names_that_definition_and_its_offset() {
    let artifact = sample();
    let mut bytes = artifact.encode();
    // The first body's payload, past the 32-byte key and the length.
    let at = HEADER_LEN + DESCRIPTOR_LEN * 3 + 36;
    bytes[at] ^= 0xff;
    let err = decode(&bytes, Path::new("t.plyx")).unwrap_err();
    assert_eq!(err.code, codes::ARTIFACT_INVALID);
    assert!(err.message.contains("offset"), "{}", err.message);
    let first = artifact.bodies.keys().next().unwrap();
    assert!(err.message.contains(&first.short()), "{}", err.message);
}

#[test]
fn a_foreign_body_encoding_is_a_version_refusal_and_not_a_corruption_one() {
    let mut artifact = sample();
    artifact.body_encoding += 1;
    let err = decode(&artifact.encode(), Path::new("t.plyx")).unwrap_err();
    assert_eq!(err.code, codes::ARTIFACT_VERSION);
}

#[test]
fn a_foreign_stdlib_is_a_warning_and_the_artifact_still_loads() {
    let mut artifact = sample();
    artifact.std = [9; 32];
    let (back, warnings) = decode(&artifact.encode(), Path::new("t.plyx")).unwrap();
    assert_eq!(back.std, [9; 32]);
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].code, codes::STDLIB_CHANGED);
}

#[test]
fn the_digest_covers_every_byte_after_it() {
    let artifact = sample();
    let mut bytes = artifact.encode();
    let before = digest_of(&bytes).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    assert_ne!(digest_of(&bytes).unwrap(), before);
}

#[test]
fn embedding_sources_moves_the_digest() {
    let bare = sample();
    let mut with = sample();
    with.sources = vec![("m.ply".to_string(), "fn main() -> Int = 1\n".to_string())];
    assert_ne!(bare.digest(), with.digest());
    assert!(!bare.has_sources() && with.has_sources());
}

#[test]
fn a_header_shorter_than_a_header_is_a_diagnostic_rather_than_a_panic() {
    let err = decode(&[0u8; 4], Path::new("t.plyx")).unwrap_err();
    assert_eq!(err.code, codes::ARTIFACT_INVALID);
}

#[test]
fn a_wild_section_count_allocates_nothing() {
    let artifact = sample();
    let mut bytes = artifact.encode();
    bytes[OFF_SECTIONS..OFF_SECTIONS + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    let err = decode(&bytes, Path::new("t.plyx")).unwrap_err();
    assert_eq!(err.code, codes::ARTIFACT_INVALID);
}

#[test]
fn every_prefix_of_an_artifact_is_refused_rather_than_believed() {
    let artifact = sample();
    let bytes = artifact.encode();
    for cut in 0..bytes.len() {
        let err = decode(&bytes[..cut], Path::new("t.plyx"))
            .expect_err("a truncated artifact must not decode");
        assert!(
            err.code == codes::ARTIFACT_INVALID || err.code == codes::ARTIFACT_VERSION,
            "cut {cut} produced {}",
            err.code
        );
    }
}

#[test]
fn short_is_the_shape_a_ci_check_pins() {
    assert_eq!(short(&[0xab; 32]), "b3:abababababab");
    assert_eq!(short(&[0xab; 32]).len(), 15);
}
