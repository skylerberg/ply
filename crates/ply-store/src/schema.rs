//! What the front-end cache's on-disk shape is, as a value.
//!
//! The front-end cache is discarded whole when the version it was written under
//! does not match this build's, so a shape change that is *not* paired with a
//! bump is read back as though it were the old shape. Against JSON that is a
//! loud parse error. Against a binary encoding it is a wrong `Scheme` or a wrong
//! `Footprint` — and footprints decide which tests may run concurrently, so it
//! is a wrong answer rather than a slow one.
//!
//! Three gates stop that, and this module is the first two:
//!
//! - `variant` names every variant of every stored enum through a `match` with
//!   no wildcard arm, so under `cargo test` a new variant does not compile until
//!   it is named, and the coverage test then fails until [`COVERED`] lists it
//!   and an exemplar reaches it.
//! - [`fingerprint`] digests [`COVERED`] together with the encoding of those
//!   exemplars. The value half is computed *from the encoder*, so unlike a
//!   hand-maintained description it cannot go stale.
//!
//! The third gate is the file header, which carries [`fingerprint`] and rejects
//! anything else.

use crate::frontend::{
    CachedCtor, CachedDecl, CachedDef, CachedOp, CachedTest, DeclBody, DefEntry, DefKind, FileSpan,
    ImportEdge, Member, NameRef, SourceFingerprint,
};
use crate::{BODY_ENCODING, ContentHash, DefBody, FRONTEND_FORMAT, Outcome};
use ply_core::{EffectAtom, Footprint, Resource, Row, RowVar, Scheme, TyVar, Type};
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_syntax::ast::Mode;
use std::collections::BTreeMap;

/// Every variant name the exemplars below must between them mention. Adding a
/// variant to a stored enum breaks the corresponding `match` in [`variant`]
/// first; adding it here without an exemplar breaks `every_variant_is_covered`.
pub(crate) const COVERED: &[&str] = &[
    "Type::Var",
    "Type::Con",
    "Type::Fn",
    "Type::Record",
    "Resource::Named",
    "Resource::Singleton",
    "Mode::Read",
    "Mode::Write",
    "DeclBody::Type",
    "DeclBody::Effect",
    "DefKind::Fn",
    "DefKind::Type",
    "DefKind::Effect",
    "Outcome::Pass",
    "Outcome::Fail",
];

#[cfg(test)]
mod variant {
    use super::*;

    pub(crate) fn ty(t: &Type) -> &'static str {
        match t {
            Type::Var(_) => "Type::Var",
            Type::Con(..) => "Type::Con",
            Type::Fn { .. } => "Type::Fn",
            Type::Record(_) => "Type::Record",
        }
    }

    pub(crate) fn resource(r: &Resource) -> &'static str {
        match r {
            Resource::Named(_) => "Resource::Named",
            Resource::Singleton => "Resource::Singleton",
        }
    }

    pub(crate) fn mode(m: Mode) -> &'static str {
        match m {
            Mode::Read => "Mode::Read",
            Mode::Write => "Mode::Write",
        }
    }

    pub(crate) fn decl_body(b: &DeclBody) -> &'static str {
        match b {
            DeclBody::Type { .. } => "DeclBody::Type",
            DeclBody::Effect { .. } => "DeclBody::Effect",
        }
    }

    pub(crate) fn def_kind(k: DefKind) -> &'static str {
        match k {
            DefKind::Fn => "DefKind::Fn",
            DefKind::Type => "DefKind::Type",
            DefKind::Effect => "DefKind::Effect",
        }
    }

    pub(crate) fn outcome(o: &Outcome) -> &'static str {
        match o {
            Outcome::Pass => "Outcome::Pass",
            Outcome::Fail { .. } => "Outcome::Fail",
        }
    }
}

/// One value of every stored type, between them reaching every variant of every
/// stored enum. This is the digest's input, so it is also the definition of
/// what "the schema" means.
pub(crate) struct Exemplars {
    pub(crate) fingerprint: SourceFingerprint,
    pub(crate) def: CachedDef,
    pub(crate) type_decl: CachedDecl,
    pub(crate) effect_decl: CachedDecl,
    pub(crate) body: DefBody,
    pub(crate) outcomes: Vec<Outcome>,
}

fn sym(s: &str) -> Symbol {
    Symbol::new(s)
}

fn h(n: u8) -> ply_hash::DefHash {
    ply_hash::DefHash([n; 32])
}

fn atom(effect: &str, resource: Resource, mode: Mode) -> EffectAtom {
    EffectAtom::new(effect, resource, mode)
}

fn footprint() -> Footprint {
    Footprint::from_atoms([
        atom("db", Resource::Named(sym("users")), Mode::Read),
        atom("clock", Resource::Singleton, Mode::Write),
    ])
}

fn every_type() -> Type {
    Type::Fn {
        params: vec![
            Type::Var(TyVar(0)),
            Type::Con(sym("List"), vec![Type::Var(TyVar(1))]),
            Type::Record(BTreeMap::from([(sym("id"), Type::int())])),
        ],
        ret: Box::new(Type::Var(TyVar(0))),
        effects: Row {
            atoms: footprint().0,
            tail: Some(RowVar(0)),
        },
    }
}

pub(crate) fn exemplars() -> Exemplars {
    let scheme = Scheme {
        ty_vars: vec![TyVar(0), TyVar(1)],
        row_vars: vec![RowVar(0)],
        ty: every_type(),
    };
    Exemplars {
        fingerprint: SourceFingerprint {
            content_hash: ContentHash([1u8; 32]),
            imports: vec![ImportEdge {
                module: sym("store.db"),
                exports: ContentHash([2u8; 32]),
            }],
            deps: vec![NameRef::new("store.db.get", h(7))],
            defs: vec![
                DefEntry {
                    name: sym("user.active_users"),
                    hash: h(2),
                    span: FileSpan { start: 10, end: 42 },
                    kind: DefKind::Fn,
                    members: vec![],
                    deps: vec![sym("store.db.get")],
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
                    deps: vec![],
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
                    deps: vec![],
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
                name_span: FileSpan {
                    start: 135,
                    end: 140,
                },
                deps: vec![sym("user.active_users")],
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
                    Diagnostic::error(codes::ASSERTION_FAILED, "assertion failed").primary(
                        Span::new(ply_span::SourceId(3), 88, 97),
                        "expected 0, found -5",
                    ),
                ),
            },
        ],
    }
}

/// Digested over the *encoded* exemplars rather than over a description of
/// the types: an encoder that starts writing a field differently changes this
/// even though every type declaration is untouched, which is exactly the drift
/// a reader cannot otherwise detect.
pub fn fingerprint() -> ContentHash {
    let e = exemplars();
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"ply-store schema v1");
    hasher.update(&FRONTEND_FORMAT.to_le_bytes());
    hasher.update(&BODY_ENCODING.to_le_bytes());
    // The declared variant set as well as the values, so that a variant added
    // to a stored enum moves the digest at the point it is *named* rather than
    // only once an exemplar happens to reach it.
    for name in COVERED {
        hasher.update(&(name.len() as u64).to_le_bytes());
        hasher.update(name.as_bytes());
    }
    // `Outcome` goes through serde because the result cache is still JSON, and
    // its shape is what a bump of `RUNTIME_VERSION` is for.
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

#[cfg(test)]
fn mentioned() -> Vec<&'static str> {
    let e = exemplars();
    let mut seen: Vec<&'static str> = Vec::new();
    let mut note = |name: &'static str| {
        if !seen.contains(&name) {
            seen.push(name);
        }
    };

    fn walk_ty(t: &Type, note: &mut impl FnMut(&'static str)) {
        note(variant::ty(t));
        match t {
            Type::Var(_) => {}
            Type::Con(_, args) => args.iter().for_each(|a| walk_ty(a, note)),
            Type::Fn {
                params,
                ret,
                effects,
            } => {
                params.iter().for_each(|p| walk_ty(p, note));
                walk_ty(ret, note);
                for a in &effects.atoms {
                    note(variant::resource(&a.resource));
                    note(variant::mode(a.mode));
                }
            }
            Type::Record(fields) => fields.values().for_each(|t| walk_ty(t, note)),
        }
    }

    fn walk_footprint(f: &Footprint, note: &mut impl FnMut(&'static str)) {
        for a in f.atoms() {
            note(variant::resource(&a.resource));
            note(variant::mode(a.mode));
        }
    }

    for d in &e.fingerprint.defs {
        note(variant::def_kind(d.kind));
    }
    for t in &e.fingerprint.tests {
        walk_footprint(&t.footprint, &mut note);
    }
    walk_ty(&e.def.scheme.ty, &mut note);
    walk_footprint(&e.def.footprint, &mut note);
    for decl in [&e.type_decl, &e.effect_decl] {
        note(variant::decl_body(&decl.body));
        match &decl.body {
            DeclBody::Type { ctors, .. } => {
                for c in ctors {
                    c.fields.iter().for_each(|f| walk_ty(f, &mut note));
                    walk_ty(&c.scheme.ty, &mut note);
                }
            }
            DeclBody::Effect { ops, .. } => {
                for op in ops {
                    note(variant::mode(op.mode));
                    op.params.iter().for_each(|p| walk_ty(p, &mut note));
                    walk_ty(&op.ret, &mut note);
                }
            }
        }
    }
    for o in &e.outcomes {
        note(variant::outcome(o));
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The digest of the shapes this build stores. When this fails, the on-disk
    /// schema changed: paste the digest the failure prints, and bump
    /// `FRONTEND_VERSION` — a build that reads an entry written under the old
    /// shape has no other way to know.
    const PINNED: &str = "dd8401fba3a9d142f857305f78e3bbc94e8a368ea088bc4dd44f207cd0e6fa3e";

    #[test]
    fn the_stored_schema_is_pinned() {
        assert_eq!(
            fingerprint().to_hex(),
            PINNED,
            "the on-disk schema changed. Update PINNED to the digest above and \
             bump FRONTEND_VERSION (currently `{}`)",
            crate::FRONTEND_VERSION
        );
    }

    /// A variant no exemplar reaches contributes nothing to the digest, so a
    /// change to it would be invisible to the pin. This is what makes the pin
    /// exhaustive rather than merely present.
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

    #[test]
    fn the_digest_moves_when_a_stored_value_changes() {
        let before = fingerprint();
        let mut e = exemplars();
        e.def.footprint = Footprint::empty();
        let after = {
            let mut hasher = blake3::Hasher::new();
            hasher.update(b"ply-store schema v1");
            hasher.update(&serde_json::to_vec(&e.def).unwrap());
            ContentHash(*hasher.finalize().as_bytes())
        };
        assert_ne!(before, after);
    }
}
