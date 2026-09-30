use ply_store::schema::*;
use ply_store::{BODY_ENCODING, DefKind, FRONTEND_FORMAT, FRONTEND_VERSION, Outcome};

mod variant {
    use super::*;

    pub(super) fn def_kind(k: DefKind) -> &'static str {
        match k {
            DefKind::Fn => "DefKind::Fn",
            DefKind::Type => "DefKind::Type",
            DefKind::Effect => "DefKind::Effect",
        }
    }

    pub(super) fn outcome(o: &Outcome) -> &'static str {
        match o {
            Outcome::Pass => "Outcome::Pass",
            Outcome::Fail { .. } => "Outcome::Fail",
        }
    }
}

fn mentioned() -> Vec<&'static str> {
    let e = exemplars();
    let mut seen: Vec<&'static str> = Vec::new();
    let mut note = |name: &'static str| {
        if !seen.contains(&name) {
            seen.push(name);
        }
    };
    for d in &e.fingerprint.defs {
        note(variant::def_kind(d.kind));
    }
    for o in &e.outcomes {
        note(variant::outcome(o));
    }
    seen
}

const BUMP: &str = "the on-disk schema changed. Update the bytes pinned here and bump the constant \
                    it is keyed on: `FRONTEND_FORMAT` for a change to what an entry holds, \
                    `FRONTEND_VERSION` for a change to what the front end files";

fn le(n: u32) -> [u8; 4] {
    n.to_le_bytes()
}

/// Every exemplar as its encoder writes it, spelled out field by field: a change to an encoder
/// moves these bytes, so it cannot move the digest without a test saying so.
fn pinned() -> Vec<Vec<u8>> {
    let entry =
        |name: &[u8], hash: u8, span: (u32, u32), kind: u8, members: &[(&[u8], u32, u32)]| {
            let mut out: Vec<u8> =
                [&[0x44][..], &le(name.len() as u32), name, &[hash; 32]].concat();
            out.extend(
                [
                    &[0x43][..],
                    &le(span.0),
                    &le(span.1),
                    &[kind],
                    &le(members.len() as u32),
                ]
                .concat(),
            );
            for (member, start, end) in members {
                out.extend(
                    [
                        &[0x41][..],
                        &le(member.len() as u32),
                        member,
                        &[0x43],
                        &le(*start),
                        &le(*end),
                        &[0xee],
                    ]
                    .concat(),
                );
            }
            out.push(0xee);
            out
        };
    let mut fingerprint: Vec<u8> = [&[0x67][..], &[1; 32], &le(1), b"m", &le(3)].concat();
    fingerprint.extend(entry(b"m.f", 2, (1, 2), 0x50, &[]));
    fingerprint.extend(entry(b"m.T", 3, (3, 4), 0x51, &[(b"A", 5, 6)]));
    fingerprint.extend(entry(b"m.e", 4, (7, 8), 0x52, &[(b"op", 9, 10)]));
    fingerprint.extend(
        [
            &le(1)[..],
            &[0x45],
            &le(1),
            b"t",
            &[5; 32],
            &[1],
            &[0x43],
            &le(11),
            &le(12),
            &le(2),
            &[0xa1, 0xa2],
            &[0xee],
            &[0xee],
        ]
        .concat(),
    );
    vec![
        fingerprint,
        [&[0x60][..], &le(3), b"m.f", &le(1), &[0xd1], &[0xee]].concat(),
        [&[0x60][..], &le(3), b"m.T", &le(2), &[0xd2, 0xd3], &[0xee]].concat(),
        [
            &[0x66][..],
            &le(BODY_ENCODING),
            &le(3),
            &[0x20, 0x01, 0xff],
            &[0xee],
        ]
        .concat(),
    ]
}

#[test]
fn the_exemplars_encode_to_the_pinned_bytes() {
    let e = exemplars();
    let found = vec![
        ply_store::codec::encode_fingerprint(&e.fingerprint),
        ply_store::codec::encode_slot(&e.def),
        ply_store::codec::encode_slot(&e.decl),
        ply_store::codec::encode_body(&e.body),
    ];
    assert_eq!(found, pinned(), "{BUMP}");
    // The result cache stores `Outcome` as JSON, and the digest reads it in that form.
    assert_eq!(
        serde_json::to_value(&e.outcomes).unwrap(),
        serde_json::json!([
            { "outcome": "pass" },
            {
                "outcome": "fail",
                "message": "assertion failed: expected 0, found -5",
                "diagnostic": {
                    "severity": "error",
                    "code": "E0501",
                    "message": "assertion failed",
                    "labels": [{
                        "span": { "source": 3, "start": 88, "end": 97 },
                        "message": "expected 0, found -5",
                        "primary": true
                    }],
                    "notes": [],
                    "fixes": [{
                        "title": "expect -5",
                        "edits": [{ "span": { "source": 3, "start": 88, "end": 89 }, "text": "-5" }]
                    }]
                }
            }
        ]),
        "{BUMP}"
    );
}

/// The digest the index header carries is the pinned bytes' and nothing else's.
#[test]
fn the_stored_schema_is_the_digest_of_the_pinned_exemplars() {
    let mut encoded = pinned();
    encoded.push(serde_json::to_vec(&exemplars().outcomes).unwrap());
    assert_eq!(
        fingerprint(),
        digest_of(BODY_ENCODING, &encoded),
        "the schema digest reads something the pins do not: FRONTEND_FORMAT is {FRONTEND_FORMAT}, \
         FRONTEND_VERSION is `{FRONTEND_VERSION}`"
    );
}

/// A variant no exemplar reaches would be invisible to the pin.
#[test]
fn every_variant_is_covered() {
    let mentioned = mentioned();
    let missing: Vec<&str> = COVERED
        .iter()
        .copied()
        .filter(|name| !mentioned.contains(name))
        .collect();
    assert!(
        missing.is_empty(),
        "no exemplar reaches {missing:?}; extend `exemplars` so the pin covers them"
    );

    let unlisted: Vec<&str> = mentioned
        .iter()
        .copied()
        .filter(|name| !COVERED.contains(name))
        .collect();
    assert!(
        unlisted.is_empty(),
        "{unlisted:?} is reached but not listed in COVERED"
    );
}

/// A normalization change with no stored type reaches this digest only through `BODY_ENCODING`.
#[test]
fn the_digest_follows_the_body_encoding_generation() {
    assert_ne!(
        fingerprint_at(BODY_ENCODING),
        fingerprint_at(BODY_ENCODING - 1)
    );
    assert_eq!(fingerprint_at(BODY_ENCODING), fingerprint());
}

#[test]
fn the_digest_moves_when_a_stored_value_changes() {
    let mut encoded = pinned();
    encoded.push(serde_json::to_vec(&exemplars().outcomes).unwrap());
    let before = digest_of(BODY_ENCODING, &encoded);
    let mut e = exemplars();
    e.def.value = vec![0xd4];
    encoded[1] = ply_store::codec::encode_slot(&e.def);
    assert_ne!(before, digest_of(BODY_ENCODING, &encoded));
}
