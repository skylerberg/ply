use ply_store::schema::*;
use ply_store::{BODY_ENCODING, ContentHash, DeclBody, DefKind, FRONTEND_VERSION, Outcome};
use ply_ty::Mode;
use ply_ty::{Footprint, Resource, Type};

mod variant {
    use super::*;

    pub(super) fn ty(t: &Type) -> &'static str {
        match t {
            Type::Var(_) => "Type::Var",
            Type::Con(..) => "Type::Con",
            Type::Fn { .. } => "Type::Fn",
            Type::Record(_) => "Type::Record",
        }
    }

    pub(super) fn resource(r: &Resource) -> &'static str {
        match r {
            Resource::Named(_) => "Resource::Named",
            Resource::Singleton => "Resource::Singleton",
        }
    }

    pub(super) fn mode(m: Mode) -> &'static str {
        match m {
            Mode::Read => "Mode::Read",
            Mode::Write => "Mode::Write",
        }
    }

    pub(super) fn decl_body(b: &DeclBody) -> &'static str {
        match b {
            DeclBody::Type { .. } => "DeclBody::Type",
            DeclBody::Effect { .. } => "DeclBody::Effect",
        }
    }

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

/// The digest of the shapes this build stores.
const PINNED: &str = "ec625d46135fe36307cb13dfe7c563fabe50497cae7713feccd5a365eea30315";

#[test]
fn the_stored_schema_is_pinned() {
    assert_eq!(
        fingerprint().to_hex(),
        PINNED,
        "the on-disk schema changed. Update PINNED to the digest above and \
         bump FRONTEND_VERSION (currently `{FRONTEND_VERSION}`)"
    );
}

/// A variant no exemplar reaches contributes nothing to the digest, so a change to it would be
/// invisible to the pin.
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

/// `Bytes` added a normalization tag and no stored type, so `BODY_ENCODING` is the whole path
/// by which it reaches this digest.
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
