//! What the front-end cache's on-disk shape is, as a value.

use crate::frontend::{DefEntry, DefKind, FileSpan, Member, Slot, SourceFingerprint, TestEntry};
use crate::{BODY_ENCODING, ContentHash, DefBody, FRONTEND_FORMAT, Outcome};
use ply_eval::{Diagnostic, Edit, Span, Symbol, codes};

/// Every variant name the exemplars below must between them mention.
pub const COVERED: &[&str] = &[
    "DefKind::Fn",
    "DefKind::Type",
    "DefKind::Effect",
    "Outcome::Pass",
    "Outcome::Fail",
];

/// One value of every stored type, between them reaching every variant of every stored enum. A
/// slot's value and a test's row are the front end's own bytes, which this crate never reads.
pub struct Exemplars {
    pub fingerprint: SourceFingerprint,
    pub def: Slot,
    pub decl: Slot,
    pub body: DefBody,
    pub outcomes: Vec<Outcome>,
}

fn h(n: u8) -> ply_eval::DefHash {
    ply_eval::DefHash([n; 32])
}

fn span(start: u32, end: u32) -> FileSpan {
    FileSpan { start, end }
}

pub fn exemplars() -> Exemplars {
    Exemplars {
        fingerprint: SourceFingerprint {
            content_hash: ContentHash([1u8; 32]),
            module: "m".to_string(),
            // Distinct hashes per entry, so the pin moves if two are swapped or one dropped.
            defs: vec![
                DefEntry {
                    name: Symbol::new("m.f"),
                    hash: h(2),
                    span: span(1, 2),
                    kind: DefKind::Fn,
                    members: vec![],
                },
                DefEntry {
                    name: Symbol::new("m.T"),
                    hash: h(3),
                    span: span(3, 4),
                    kind: DefKind::Type,
                    members: vec![Member {
                        name: Symbol::new("A"),
                        span: span(5, 6),
                    }],
                },
                DefEntry {
                    name: Symbol::new("m.e"),
                    hash: h(4),
                    span: span(7, 8),
                    kind: DefKind::Effect,
                    members: vec![Member {
                        name: Symbol::new("op"),
                        span: span(9, 10),
                    }],
                },
            ],
            tests: vec![TestEntry {
                name: "t".to_string(),
                hash: h(5),
                nondet: true,
                span: span(11, 12),
                row: vec![0xa1, 0xa2],
            }],
        },
        def: Slot {
            name: Symbol::new("m.f"),
            value: vec![0xd1],
        },
        decl: Slot {
            name: Symbol::new("m.T"),
            value: vec![0xd2, 0xd3],
        },
        body: DefBody::new(BODY_ENCODING, vec![0x20, 0x01, 0xff]),
        outcomes: vec![
            Outcome::Pass,
            Outcome::Fail {
                message: "assertion failed: expected 0, found -5".to_string(),
                diagnostic: Some(
                    Diagnostic::error(codes::ASSERTION_FAILED, "assertion failed")
                        .primary(
                            Span::new(ply_eval::SourceId(3), 88, 97),
                            "expected 0, found -5",
                        )
                        .fix(
                            "expect -5",
                            vec![Edit {
                                span: Span::new(ply_eval::SourceId(3), 88, 89),
                                text: "-5".to_string(),
                            }],
                        ),
                ),
            },
        ],
    }
}

/// Digests the *encoded* exemplars, so an encoder change moves it even when no type changes.
pub fn fingerprint() -> ContentHash {
    fingerprint_at(BODY_ENCODING)
}

pub fn fingerprint_at(body_encoding: u32) -> ContentHash {
    let e = exemplars();
    // The result cache stores `Outcome` as JSON, so it is digested in that form.
    let outcomes = serde_json::to_vec(&e.outcomes)
        .unwrap_or_else(|e| format!("unserializable: {e}").into_bytes());
    digest_of(
        body_encoding,
        &[
            crate::codec::encode_fingerprint(&e.fingerprint),
            crate::codec::encode_slot(&e.def),
            crate::codec::encode_slot(&e.decl),
            crate::codec::encode_body(&e.body),
            outcomes,
        ],
    )
}

/// The digest over encoded exemplars, in the order [`fingerprint_at`] lists them.
pub fn digest_of(body_encoding: u32, encoded: &[Vec<u8>]) -> ContentHash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"ply-store schema v2");
    hasher.update(&FRONTEND_FORMAT.to_le_bytes());
    hasher.update(&body_encoding.to_le_bytes());
    // Variant names too, so a new variant moves the digest before an exemplar reaches it.
    for name in COVERED {
        hasher.update(&(name.len() as u64).to_le_bytes());
        hasher.update(name.as_bytes());
    }
    for bytes in encoded {
        hasher.update(&(bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    }
    ContentHash(*hasher.finalize().as_bytes())
}
