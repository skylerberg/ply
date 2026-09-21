use ply_cli::artifact::*;
use ply_span::codes;
use ply_store::body::StoredBody;
use ply_ty::DefHash;
use std::collections::BTreeMap;
use std::path::Path;

/// Not decodable definitions, deliberately: `decode` checks a body against its key and nothing more.
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
        closure: Vec::new(),
        unit: None,
    }
}

fn written(artifact: &Artifact) -> Vec<u8> {
    artifact.encode().expect("the container should be written")
}

#[test]
fn an_embedded_unit_survives_encoding() {
    let mut artifact = sample();
    artifact.unit = Some(EmbeddedUnit {
        text: vec![1, 2, 3, 4],
    });
    let bytes = written(&artifact);
    let (decoded, _) = decode(&bytes, Path::new("x.plyx")).expect("decodes");
    assert_eq!(decoded.unit, artifact.unit);
    assert_ne!(written(&sample()), bytes, "the unit is in the digest");
}

#[test]
fn a_round_trip_preserves_every_field() {
    let artifact = sample();
    let (back, warnings) = decode(&written(&artifact), Path::new("t.plyx")).unwrap();
    assert_eq!(back, artifact);
    assert!(warnings.is_empty());
    assert_eq!(back.entry_name(), Some("m.main"));
}

#[test]
fn a_closure_round_trips_and_is_believed_only_as_bytes() {
    let mut artifact = sample();
    artifact.closure = vec![
        ("a.ply".to_string(), "fn a() -> Int = 1\n".to_string()),
        ("sub/b.ply".to_string(), "fn b() -> Int = 2\n".to_string()),
    ];
    let (back, _) = decode(&written(&artifact), Path::new("t.plyx")).unwrap();
    assert_eq!(back, artifact);
}

#[test]
fn a_foreign_body_encoding_is_a_version_refusal_and_not_a_corruption_one() {
    let mut artifact = sample();
    artifact.body_encoding += 1;
    let err = decode(&written(&artifact), Path::new("t.plyx")).unwrap_err();
    assert_eq!(err.code, codes::ARTIFACT_VERSION);
}

#[test]
fn a_foreign_stdlib_is_a_warning_and_the_artifact_still_loads() {
    let mut artifact = sample();
    artifact.std = [9; 32];
    let (back, warnings) = decode(&written(&artifact), Path::new("t.plyx")).unwrap();
    assert_eq!(back.std, [9; 32]);
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].code, codes::STDLIB_CHANGED);
}

#[test]
fn the_digest_covers_every_byte_after_it() {
    let mut bytes = written(&sample());
    let before = digest_of(&bytes).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    assert_ne!(digest_of(&bytes).unwrap(), before);
}

#[test]
fn the_closure_is_in_the_digest() {
    let mut with = sample();
    with.closure = vec![("m.ply".to_string(), "fn main() -> Int = 1\n".to_string())];
    assert_ne!(sample().digest(), with.digest());
}

#[test]
fn the_entry_point_is_in_the_digest() {
    let artifact = sample();
    let mut elsewhere = sample();
    elsewhere.entry = *artifact
        .bodies
        .keys()
        .find(|hash| **hash != artifact.entry)
        .expect("a second definition");
    assert_ne!(artifact.digest(), elsewhere.digest());
}

#[test]
fn a_format_the_container_does_not_write_is_a_version_refusal() {
    let mut bytes = written(&sample());
    // The format number, where the header states it.
    bytes[8..12].copy_from_slice(&3u32.to_le_bytes());
    let err = decode(&bytes, Path::new("t.plyx")).unwrap_err();
    assert_eq!(err.code, codes::ARTIFACT_VERSION);
}

#[test]
fn a_header_shorter_than_a_header_is_a_diagnostic_rather_than_a_panic() {
    let err = decode(&[0u8; 4], Path::new("t.plyx")).unwrap_err();
    assert_eq!(err.code, codes::ARTIFACT_INVALID);
}

#[test]
fn short_is_the_shape_a_ci_check_pins() {
    assert_eq!(short(&[0xab; 32]), "b3:abababababab");
    assert_eq!(short(&[0xab; 32]).len(), 15);
}
