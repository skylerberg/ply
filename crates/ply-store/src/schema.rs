//! What the front-end cache's on-disk shape is, as a value.

use crate::frontend::{
    CachedCtor, CachedDecl, CachedDef, CachedOp, CachedTest, DeclBody, DefEntry, DefKind, FileSpan,
    Member, NameRef, SourceFingerprint,
};
use crate::{BODY_ENCODING, ContentHash, DefBody, FRONTEND_FORMAT, Outcome};
use ply_span::{Diagnostic, Edit, Span, Symbol, codes};
use ply_ty::Mode;
use ply_ty::{EffectAtom, Footprint, LabelVar, Resource, Row, RowVar, Scheme, TyVar, Type};
use std::collections::BTreeMap;

/// Every variant name the exemplars below must between them mention.
pub const COVERED: &[&str] = &[
    "Type::Var",
    "Type::Con",
    "Type::Fn",
    "Type::Record",
    "Resource::Named",
    "Resource::Var",
    "Resource::Singleton",
    "Mode::Read",
    "Mode::Write",
    "EffectAtom::mode",
    "EffectAtom::op",
    "DeclBody::Type",
    "DeclBody::Effect",
    "DefKind::Fn",
    "DefKind::Type",
    "DefKind::Effect",
    "Outcome::Pass",
    "Outcome::Fail",
];

/// One value of every stored type, between them reaching every variant of every stored enum.
pub struct Exemplars {
    pub fingerprint: SourceFingerprint,
    pub def: CachedDef,
    pub type_decl: CachedDecl,
    pub effect_decl: CachedDecl,
    pub body: DefBody,
    pub outcomes: Vec<Outcome>,
}

fn sym(s: &str) -> Symbol {
    Symbol::new(s)
}

fn h(n: u8) -> ply_ty::DefHash {
    ply_ty::DefHash([n; 32])
}

fn atom(effect: &str, resource: Resource, mode: Mode) -> EffectAtom {
    EffectAtom::new(effect, resource, mode)
}

fn footprint() -> Footprint {
    Footprint::from_atoms([
        atom("db", Resource::Named(sym("users")), Mode::Read),
        atom("clock", Resource::Singleton, Mode::Write),
        EffectAtom::operation("net", Resource::Named(sym("conn")), Mode::Write, "send"),
    ])
}

fn every_type() -> Type {
    Type::Fn {
        params: vec![
            Type::Var(TyVar(0)),
            Type::Con(sym("List"), vec![Type::Var(TyVar(1))]),
            // The only two-argument `Con`, so the codec's arity is exercised.
            Type::map(Type::string(), Type::Var(TyVar(1))),
            Type::Record(BTreeMap::from([(sym("id"), Type::int())])),
        ],
        ret: Box::new(Type::Var(TyVar(0))),
        effects: Row {
            // An atom on a label the scheme quantifies; a footprint holds one too, through the
            // same atom encoding.
            atoms: footprint()
                .0
                .into_iter()
                .chain([atom("net", Resource::Var(LabelVar(0)), Mode::Write)])
                .collect(),
            tail: Some(RowVar(0)),
        },
    }
}

pub fn exemplars() -> Exemplars {
    let scheme = Scheme {
        ty_vars: vec![TyVar(0), TyVar(1)],
        row_vars: vec![RowVar(0)],
        label_vars: vec![LabelVar(0)],
        ty: every_type(),
    };
    Exemplars {
        fingerprint: SourceFingerprint {
            content_hash: ContentHash([1u8; 32]),
            // Distinct hashes per `DefEntry`, so the pin moves if two are swapped or one dropped.
            defs: vec![
                DefEntry {
                    name: sym("user.active_users"),
                    hash: h(2),
                    span: FileSpan { start: 10, end: 42 },
                    kind: DefKind::Fn,
                    members: vec![],
                },
                DefEntry {
                    name: sym("user.User"),
                    hash: h(3),
                    span: FileSpan { start: 50, end: 80 },
                    kind: DefKind::Type,
                    members: vec![Member {
                        name: sym("user.Active"),
                        span: FileSpan { start: 60, end: 66 },
                    }],
                },
                DefEntry {
                    name: sym("user.db"),
                    hash: h(4),
                    span: FileSpan {
                        start: 90,
                        end: 120,
                    },
                    kind: DefKind::Effect,
                    members: vec![Member {
                        name: sym("user.get"),
                        span: FileSpan {
                            start: 100,
                            end: 110,
                        },
                    }],
                },
            ],
            tests: vec![CachedTest {
                name: "active_users excludes inactive".to_string(),
                hash: h(5),
                nondet: true,
                footprint: footprint(),
                span: FileSpan {
                    start: 130,
                    end: 180,
                },
            }],
        },
        def: CachedDef::new(scheme.clone(), footprint())
            .witnessed_by(vec![NameRef::new("user.User", h(3))]),
        type_decl: CachedDecl::new(DeclBody::Type {
            arity: 1,
            ctors: vec![CachedCtor {
                fields: vec![Type::Var(TyVar(0))],
                scheme: scheme.clone(),
            }],
        })
        .witnessed_by(vec![NameRef::new("user.User", h(3))]),
        effect_decl: CachedDecl::new(DeclBody::Effect {
            nondet: true,
            ops: vec![CachedOp {
                name: sym("get"),
                mode: Mode::Write,
                resource_param: true,
                params: vec![Type::int()],
                ret: Type::unit(),
            }],
        }),
        body: DefBody::new(BODY_ENCODING, vec![0x20, 0x01, 0xff]),
        outcomes: vec![
            Outcome::Pass,
            Outcome::Fail {
                message: "assertion failed: expected 0, found -5".to_string(),
                diagnostic: Some(
                    Diagnostic::error(codes::ASSERTION_FAILED, "assertion failed")
                        .primary(
                            Span::new(ply_span::SourceId(3), 88, 97),
                            "expected 0, found -5",
                        )
                        .fix(
                            "expect -5",
                            vec![Edit {
                                span: Span::new(ply_span::SourceId(3), 88, 89),
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
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"ply-store schema v1");
    hasher.update(&FRONTEND_FORMAT.to_le_bytes());
    hasher.update(&body_encoding.to_le_bytes());
    // Variant names too, so a new variant moves the digest before an exemplar reaches it.
    for name in COVERED {
        hasher.update(&(name.len() as u64).to_le_bytes());
        hasher.update(name.as_bytes());
    }
    // The result cache stores `Outcome` as JSON, so it is digested in that form.
    let outcomes = serde_json::to_vec(&e.outcomes)
        .unwrap_or_else(|e| format!("unserializable: {e}").into_bytes());
    for bytes in [
        crate::codec::encode_fingerprint(&e.fingerprint),
        crate::codec::encode_def(&e.def),
        crate::codec::encode_decl(&e.type_decl),
        crate::codec::encode_decl(&e.effect_decl),
        crate::codec::encode_body(&e.body),
        outcomes,
    ] {
        hasher.update(&(bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
    }
    ContentHash(*hasher.finalize().as_bytes())
}
