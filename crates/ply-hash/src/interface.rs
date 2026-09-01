//! What a *caller* can observe about a definition, hashed.
//!
//! A `DefHash` covers a definition's whole body and every body it reaches, which
//! is right for an identity and far too much for a recheck decision: it moves
//! for edits a caller cannot see. This is the complement — the published scheme,
//! the published footprint and the published constraints, and nothing else.

use ply_core::{DefConstraint, EffectAtom, Footprint, Resource, Row, RowVar, Scheme, TyVar, Type};
use rustc_hash::FxHashMap;

use crate::DefHash;
use crate::normalize::{grow, mode_byte};

/// Domain tag, so an interface key can never be mistaken for the [`DefHash`] it
/// is carried beside. The same device as `crate::SPEC_DOMAIN`.
const INTERFACE_DOMAIN: &[u8] = b"ply.interface.1";

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
/// Quantified variables are renumbered by first occurrence in a traversal of the
/// scheme's own type — the canonical form `ply-store` puts a scheme in before
/// storing it, restated here because that crate depends on this one. It cannot
/// be left to the caller: `ply_core::env::generalize` quantifies over whatever
/// numbers the run's counter handed out, so one definition generalizes to
/// alpha-equivalent schemes with different numbers depending on what subset of
/// the program was checked. Hash those raw and every interface reads as changed
/// on every run, and the cutoff this key exists for never fires.
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
