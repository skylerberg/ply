//! What content addressing publishes: the definition hash, the keys derived from it, and the
//! table a hashed program answers. The hashing itself — normalizing a definition — is `ply-hash`'s;
//! this is the vocabulary the store, the test runner and the prover key on, kept where they can
//! read it without the hasher.

use crate::DefConstraint;
use crate::decl::SpecKind;
use crate::ty::{EffectAtom, Footprint, Mode, Resource, Row, RowVar, Scheme, TyVar, Type};
use indexmap::IndexMap;
use ply_span::Symbol;
use rustc_hash::FxHashMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeSet;
use std::fmt;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct DefHash(pub [u8; 32]);

impl DefHash {
    pub fn to_hex(&self) -> String {
        let mut s = String::with_capacity(64);
        for b in self.0 {
            s.push(char::from_digit((b >> 4) as u32, 16).unwrap_or('0'));
            s.push(char::from_digit((b & 0xf) as u32, 16).unwrap_or('0'));
        }
        s
    }

    pub fn short(&self) -> String {
        self.to_hex()[..12].to_string()
    }

    pub fn from_hex(s: &str) -> Option<DefHash> {
        if s.len() != 64 {
            return None;
        }
        let bytes = s.as_bytes();
        let mut out = [0u8; 32];
        for (i, byte) in out.iter_mut().enumerate() {
            let hi = (bytes[2 * i] as char).to_digit(16)?;
            let lo = (bytes[2 * i + 1] as char).to_digit(16)?;
            *byte = ((hi << 4) | lo) as u8;
        }
        Some(DefHash(out))
    }

    pub fn of(bytes: &[u8]) -> DefHash {
        DefHash(*blake3::hash(bytes).as_bytes())
    }
}

impl fmt::Display for DefHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.short())
    }
}

/// Hex rather than 32 numbers: the on-disk cache is meant to be readable by hand, and a hash has to
/// work as a JSON object key.
impl Serialize for DefHash {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for DefHash {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        DefHash::from_hex(&s)
            .ok_or_else(|| serde::de::Error::custom(format!("malformed definition hash `{s}`")))
    }
}

/// Every map is keyed by the program-wide name — `store.orders.place`, and `<module>.<label>` for a
/// test.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HashOutput {
    pub defs: IndexMap<Symbol, DefHash>,
    /// `defs`, with each reference written as the referent's name rather than
    /// its hash, so this moves with a definition's own text and not with a
    /// callee's.
    ///
    /// Never an identity: two definitions calling different functions of the
    /// same shape share one.
    pub own: IndexMap<Symbol, DefHash>,
    /// `type` and `effect` declarations, which `defs` deliberately omits — only a `fn` is a
    /// definition a test can be selected on.
    pub decls: IndexMap<Symbol, DefHash>,
    pub tests: Vec<DefHash>,
    /// Parallel to `CheckOutput::laws`.
    pub laws: Vec<DefHash>,
    /// Definition program-wide name -> one hash per `requires` / `ensures` clause, in source order.
    pub specs: IndexMap<Symbol, Vec<DefHash>>,
    /// The same clauses as `specs`, identified as **sentences** rather than as obligations:
    /// references by name, and no owner hash in the stream.
    pub spec_texts: IndexMap<Symbol, Vec<DefHash>>,
    /// Parallel to `laws`, and a sentence identity for the same reason [`HashOutput::spec_texts`]
    /// is one.
    pub law_texts: Vec<DefHash>,
    /// Direct references, definition name -> names it mentions.
    pub deps: IndexMap<Symbol, Vec<Symbol>>,
    /// Transitive closure, including the definition itself.
    pub closure: IndexMap<Symbol, BTreeSet<Symbol>>,
}

/// Domain tag, so a spec's key cannot collide with a definition's own hash, which is `blake3` over
/// normalized bytes carrying no tag.
const SPEC_DOMAIN: &[u8] = b"ply.spec.1";

/// Domain tag for a claim's *sentence* identity, kept apart from [`SPEC_DOMAIN`] so that a review
/// baseline can never be mistaken for an obligation key.
const SPEC_TEXT_DOMAIN: &[u8] = b"ply.spec.text.1";

/// Domain tag for [`HashOutput::own`]. A definition's own-form key is one hash
/// of one definition's bytes and so is its `DefHash`; tagging is what stops the
/// two being interchangeable at a call site that takes `DefHash` and means
/// identity.
const OWN_DOMAIN: &[u8] = b"ply.own.1";

/// Domain tag, so an interface key can never be mistaken for the [`DefHash`] it
/// is carried beside. The same device as [`SPEC_DOMAIN`].
const INTERFACE_DOMAIN: &[u8] = b"ply.interface.1";

/// The identity of a claim as written, for [`HashOutput::spec_texts`] and
/// [`HashOutput::law_texts`].
pub fn spec_text_hash(kind: Option<SpecKind>, index: u32, normalized: &[u8]) -> DefHash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(SPEC_TEXT_DOMAIN);
    hasher.update(&[kind.map_or(0, |k| k.tag())]);
    hasher.update(&index.to_le_bytes());
    hasher.update(normalized);
    DefHash(*hasher.finalize().as_bytes())
}

/// The key for [`HashOutput::own`], over a definition's by-name encoding.
pub fn own_hash(normalized: &[u8]) -> DefHash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(OWN_DOMAIN);
    hasher.update(normalized);
    DefHash(*hasher.finalize().as_bytes())
}

/// The key an obligation attached to a definition is discharged under.
pub fn spec_hash(owner: DefHash, kind: SpecKind, index: u32, normalized: &[u8]) -> DefHash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(SPEC_DOMAIN);
    hasher.update(&owner.0);
    hasher.update(&[kind.tag()]);
    hasher.update(&index.to_le_bytes());
    hasher.update(normalized);
    DefHash(*hasher.finalize().as_bytes())
}

/// How an access mode is written in a hashed stream.
pub fn mode_byte(mode: Mode) -> u8 {
    match mode {
        Mode::Read => 0,
        Mode::Write => 1,
    }
}

mod tag {
    pub const VAR: u8 = 1;
    pub const CON: u8 = 2;
    pub const FN: u8 = 3;
    pub const RECORD: u8 = 4;
    pub const ATOM: u8 = 5;
    pub const RESOURCE_NAMED: u8 = 6;
    pub const RESOURCE_SINGLETON: u8 = 7;
    pub const ROW_TAIL: u8 = 8;
    pub const ROW_CLOSED: u8 = 9;
    pub const CONSTRAINT_VAR: u8 = 10;
    /// A `where derivable(D, a)` whose `a` is not a quantifier of the scheme it
    /// came with. Written rather than dropped: two constraint sets that differ
    /// only in such a clause must not collapse onto one key, since a collapse is
    /// a recheck that never happens.
    pub const CONSTRAINT_UNBOUND: u8 = 11;
}

/// Everything a caller is checked against: the type it calls at, the effects it
/// inherits, and the constraints its arguments must satisfy.
///
/// Quantified variables are renumbered here rather than by the caller, because
/// `generalize` hands out whatever numbers the run's counter reached: hash a
/// scheme raw and every interface reads as changed, so the cutoff never fires.
/// Pass the scheme as published — a `DefConstraint::param` indexes its
/// `ty_vars`, so canonicalizing first names the wrong quantifier.
pub fn interface_hash(
    scheme: &Scheme,
    footprint: &Footprint,
    constraints: &[DefConstraint],
) -> DefHash {
    let mut enc = Interface::default();
    enc.scheme(scheme);
    enc.footprint(footprint);
    enc.constraints(scheme, constraints);
    let mut hasher = blake3::Hasher::new();
    hasher.update(INTERFACE_DOMAIN);
    hasher.update(&enc.out);
    DefHash(*hasher.finalize().as_bytes())
}

/// A type is as deep as the program wrote it, and the parser's nesting limit is the only bound.
fn grow<R>(f: impl FnOnce() -> R) -> R {
    const RED_ZONE: usize = 256 * 1024;
    const NEW_SEGMENT: usize = 2 * 1024 * 1024;
    stacker::maybe_grow(RED_ZONE, NEW_SEGMENT, f)
}

#[derive(Default)]
struct Interface {
    out: Vec<u8>,
    tys: FxHashMap<TyVar, u32>,
    rows: FxHashMap<RowVar, u32>,
}

impl Interface {
    fn tag(&mut self, t: u8) {
        self.out.push(t);
    }

    fn u32v(&mut self, v: u32) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }

    fn strv(&mut self, s: &str) {
        self.u32v(s.len() as u32);
        self.out.extend_from_slice(s.as_bytes());
    }

    fn ty_var(&mut self, v: TyVar) -> u32 {
        let next = self.tys.len() as u32;
        *self.tys.entry(v).or_insert(next)
    }

    fn row_var(&mut self, v: RowVar) -> u32 {
        let next = self.rows.len() as u32;
        *self.rows.entry(v).or_insert(next)
    }

    fn scheme(&mut self, scheme: &Scheme) {
        // The type first, so a variable's number is where it is *used* and a
        // quantifier list written in another order cannot change it. The lists
        // are then sorted, which is what makes those two orders one key.
        self.ty(&scheme.ty);
        let mut tys: Vec<u32> = scheme.ty_vars.iter().map(|v| self.ty_var(*v)).collect();
        let mut rows: Vec<u32> = scheme.row_vars.iter().map(|v| self.row_var(*v)).collect();
        tys.sort_unstable();
        tys.dedup();
        rows.sort_unstable();
        rows.dedup();
        self.u32v(tys.len() as u32);
        for v in tys {
            self.u32v(v);
        }
        self.u32v(rows.len() as u32);
        for v in rows {
            self.u32v(v);
        }
    }

    fn ty(&mut self, ty: &Type) {
        grow(|| self.ty_inner(ty));
    }

    fn ty_inner(&mut self, ty: &Type) {
        match ty {
            Type::Var(v) => {
                self.tag(tag::VAR);
                let n = self.ty_var(*v);
                self.u32v(n);
            }
            Type::Con(name, args) => {
                self.tag(tag::CON);
                self.strv(name);
                self.u32v(args.len() as u32);
                for a in args {
                    self.ty(a);
                }
            }
            Type::Fn {
                params,
                ret,
                effects,
            } => {
                self.tag(tag::FN);
                self.u32v(params.len() as u32);
                for p in params {
                    self.ty(p);
                }
                self.ty(ret);
                self.row(effects);
            }
            // A `BTreeMap` iterates in key order, so the traversal — and with it
            // the numbering above — does not depend on how the record was built.
            Type::Record(fields) => {
                self.tag(tag::RECORD);
                self.u32v(fields.len() as u32);
                for (name, t) in fields {
                    self.strv(name);
                    self.ty(t);
                }
            }
        }
    }

    fn row(&mut self, row: &Row) {
        self.u32v(row.atoms.len() as u32);
        for atom in &row.atoms {
            self.atom(atom);
        }
        match row.tail {
            None => self.tag(tag::ROW_CLOSED),
            Some(t) => {
                self.tag(tag::ROW_TAIL);
                let n = self.row_var(t);
                self.u32v(n);
            }
        }
    }

    fn atom(&mut self, atom: &EffectAtom) {
        self.tag(tag::ATOM);
        self.strv(&atom.effect);
        match &atom.resource {
            Resource::Named(r) => {
                self.tag(tag::RESOURCE_NAMED);
                self.strv(r);
            }
            Resource::Singleton => self.tag(tag::RESOURCE_SINGLETON),
        }
        self.out.push(mode_byte(atom.mode));
    }

    fn footprint(&mut self, footprint: &Footprint) {
        self.u32v(footprint.0.len() as u32);
        for atom in footprint.atoms() {
            self.atom(atom);
        }
    }

    /// A constraint names a quantifier by its position in `Scheme::ty_vars`,
    /// whose order is an artefact of how the run collected it. So the key is
    /// written in the canonical *variable* that position names: a scheme that
    /// lists its quantifiers the other way round, with its constraints following
    /// them, is one interface rather than two.
    fn constraints(&mut self, scheme: &Scheme, constraints: &[DefConstraint]) {
        let mut written: Vec<(u8, u32, u8)> = constraints
            .iter()
            .map(|c| match scheme.ty_vars.get(c.param) {
                Some(&v) => (tag::CONSTRAINT_VAR, self.ty_var(v), c.deriver.tag()),
                None => (tag::CONSTRAINT_UNBOUND, c.param as u32, c.deriver.tag()),
            })
            .collect();
        written.sort_unstable();
        written.dedup();
        self.u32v(written.len() as u32);
        for (lane, var, deriver) in written {
            self.tag(lane);
            self.u32v(var);
            self.tag(deriver);
        }
    }
}
