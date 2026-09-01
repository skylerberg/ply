//! The encoding of one stored entry's payload.

use crate::DefBody;
use crate::binary::{Decoded, Reader, Writer};
use crate::frontend::{
    CachedCtor, CachedDecl, CachedDef, CachedOp, CachedTest, DeclBody, DefEntry, DefKind, FileSpan,
    ImportEdge, Member, NameRef, SourceFingerprint,
};
use ply_core::{EffectAtom, Footprint, Resource, Row, RowVar, Scheme, TyVar, Type};
use ply_syntax::ast::Mode;
use std::collections::{BTreeMap, BTreeSet};

/// Discriminants.
mod tag {
    pub(super) const TYPE_VAR: u8 = 0x10;
    pub(super) const TYPE_CON: u8 = 0x11;
    pub(super) const TYPE_FN: u8 = 0x12;
    pub(super) const TYPE_RECORD: u8 = 0x13;

    pub(super) const RESOURCE_NAMED: u8 = 0x20;
    pub(super) const RESOURCE_SINGLETON: u8 = 0x21;

    pub(super) const MODE_READ: u8 = 0x28;
    pub(super) const MODE_WRITE: u8 = 0x29;

    pub(super) const ATOM: u8 = 0x30;
    pub(super) const ROW: u8 = 0x31;
    pub(super) const FOOTPRINT: u8 = 0x32;
    pub(super) const SCHEME: u8 = 0x33;

    pub(super) const NAME_REF: u8 = 0x40;
    pub(super) const MEMBER: u8 = 0x41;
    pub(super) const IMPORT_EDGE: u8 = 0x42;
    pub(super) const FILE_SPAN: u8 = 0x43;
    pub(super) const DEF_ENTRY: u8 = 0x44;
    pub(super) const CACHED_TEST: u8 = 0x45;

    pub(super) const DEF_KIND_FN: u8 = 0x50;
    pub(super) const DEF_KIND_TYPE: u8 = 0x51;
    pub(super) const DEF_KIND_EFFECT: u8 = 0x52;

    pub(super) const CACHED_DEF: u8 = 0x60;
    pub(super) const CACHED_DECL: u8 = 0x61;
    pub(super) const DECL_TYPE: u8 = 0x62;
    pub(super) const DECL_EFFECT: u8 = 0x63;
    pub(super) const CACHED_CTOR: u8 = 0x64;
    pub(super) const CACHED_OP: u8 = 0x65;
    pub(super) const DEF_BODY: u8 = 0x66;
    pub(super) const FINGERPRINT: u8 = 0x67;

    /// Closes every composite.
    pub(super) const END: u8 = 0xee;
}

/// Neither side of this codec may bound nesting the other does not: an encoder that writes what the
/// decoder refuses makes a healthy cache report itself corrupt on every run, with no remedy.
pub(crate) fn grow<R>(f: impl FnOnce() -> R) -> R {
    const RED_ZONE: usize = 256 * 1024;
    const NEW_SEGMENT: usize = 2 * 1024 * 1024;
    stacker::maybe_grow(RED_ZONE, NEW_SEGMENT, f)
}

fn put_type(w: &mut Writer, ty: &Type) {
    grow(|| put_type_inner(w, ty))
}

fn put_type_inner(w: &mut Writer, ty: &Type) {
    match ty {
        Type::Var(TyVar(v)) => {
            w.tag(tag::TYPE_VAR);
            w.u32(*v);
        }
        Type::Con(name, args) => {
            w.tag(tag::TYPE_CON);
            w.symbol(name);
            w.count(args.len());
            for arg in args {
                put_type(w, arg);
            }
        }
        Type::Fn {
            params,
            ret,
            effects,
        } => {
            w.tag(tag::TYPE_FN);
            w.count(params.len());
            for param in params {
                put_type(w, param);
            }
            put_type(w, ret);
            put_row(w, effects);
        }
        Type::Record(fields) => {
            w.tag(tag::TYPE_RECORD);
            w.count(fields.len());
            for (name, ty) in fields {
                w.symbol(name);
                put_type(w, ty);
            }
        }
    }
    w.tag(tag::END);
}

fn get_type(r: &mut Reader) -> Decoded<Type> {
    grow(|| get_type_inner(r))
}

fn get_type_inner(r: &mut Reader) -> Decoded<Type> {
    const WHAT: &str = "malformed type";
    let ty = match r.byte(WHAT)? {
        tag::TYPE_VAR => Type::Var(TyVar(r.u32(WHAT)?)),
        tag::TYPE_CON => {
            let name = r.symbol(WHAT)?;
            let count = r.count(WHAT)?;
            let mut args = Vec::with_capacity(count);
            for _ in 0..count {
                args.push(get_type(r)?);
            }
            Type::Con(name, args)
        }
        tag::TYPE_FN => {
            let count = r.count(WHAT)?;
            let mut params = Vec::with_capacity(count);
            for _ in 0..count {
                params.push(get_type(r)?);
            }
            let ret = Box::new(get_type(r)?);
            let effects = get_row(r)?;
            Type::Fn {
                params,
                ret,
                effects,
            }
        }
        tag::TYPE_RECORD => {
            let count = r.count(WHAT)?;
            let mut fields = BTreeMap::new();
            for _ in 0..count {
                let name = r.symbol(WHAT)?;
                fields.insert(name, get_type(r)?);
            }
            Type::Record(fields)
        }
        _ => return Err(crate::binary::DecodeError { what: WHAT, at: 0 }),
    };
    r.tag(tag::END, WHAT)?;
    Ok(ty)
}

fn put_mode(w: &mut Writer, mode: Mode) {
    w.tag(match mode {
        Mode::Read => tag::MODE_READ,
        Mode::Write => tag::MODE_WRITE,
    });
}

fn get_mode(r: &mut Reader) -> Decoded<Mode> {
    const WHAT: &str = "malformed effect mode";
    match r.byte(WHAT)? {
        tag::MODE_READ => Ok(Mode::Read),
        tag::MODE_WRITE => Ok(Mode::Write),
        _ => Err(crate::binary::DecodeError { what: WHAT, at: 0 }),
    }
}

fn put_atom(w: &mut Writer, atom: &EffectAtom) {
    w.tag(tag::ATOM);
    w.symbol(&atom.effect);
    match &atom.resource {
        Resource::Named(name) => {
            w.tag(tag::RESOURCE_NAMED);
            w.symbol(name);
        }
        Resource::Singleton => w.tag(tag::RESOURCE_SINGLETON),
    }
    put_mode(w, atom.mode);
    w.tag(tag::END);
}

fn get_atom(r: &mut Reader) -> Decoded<EffectAtom> {
    const WHAT: &str = "malformed effect atom";
    r.tag(tag::ATOM, WHAT)?;
    let effect = r.symbol(WHAT)?;
    let resource = match r.byte(WHAT)? {
        tag::RESOURCE_NAMED => Resource::Named(r.symbol(WHAT)?),
        tag::RESOURCE_SINGLETON => Resource::Singleton,
        _ => return Err(crate::binary::DecodeError { what: WHAT, at: 0 }),
    };
    let mode = get_mode(r)?;
    r.tag(tag::END, WHAT)?;
    Ok(EffectAtom {
        effect,
        resource,
        mode,
    })
}

fn put_atoms(w: &mut Writer, atoms: &BTreeSet<EffectAtom>) {
    w.count(atoms.len());
    for atom in atoms {
        put_atom(w, atom);
    }
}

fn get_atoms(r: &mut Reader) -> Decoded<BTreeSet<EffectAtom>> {
    let count = r.count("malformed atom set")?;
    let mut atoms = BTreeSet::new();
    for _ in 0..count {
        atoms.insert(get_atom(r)?);
    }
    Ok(atoms)
}

fn put_row(w: &mut Writer, row: &Row) {
    w.tag(tag::ROW);
    put_atoms(w, &row.atoms);
    match row.tail {
        Some(RowVar(v)) => {
            w.bool(true);
            w.u32(v);
        }
        None => w.bool(false),
    }
    w.tag(tag::END);
}

fn get_row(r: &mut Reader) -> Decoded<Row> {
    const WHAT: &str = "malformed effect row";
    r.tag(tag::ROW, WHAT)?;
    let atoms = get_atoms(r)?;
    let tail = r.bool(WHAT)?.then(|| r.u32(WHAT).map(RowVar)).transpose()?;
    r.tag(tag::END, WHAT)?;
    Ok(Row { atoms, tail })
}

fn put_footprint(w: &mut Writer, footprint: &Footprint) {
    w.tag(tag::FOOTPRINT);
    put_atoms(w, &footprint.0);
    w.tag(tag::END);
}

fn get_footprint(r: &mut Reader) -> Decoded<Footprint> {
    const WHAT: &str = "malformed footprint";
    r.tag(tag::FOOTPRINT, WHAT)?;
    let atoms = get_atoms(r)?;
    r.tag(tag::END, WHAT)?;
    Ok(Footprint(atoms))
}

fn put_scheme(w: &mut Writer, scheme: &Scheme) {
    w.tag(tag::SCHEME);
    w.count(scheme.ty_vars.len());
    for TyVar(v) in &scheme.ty_vars {
        w.u32(*v);
    }
    w.count(scheme.row_vars.len());
    for RowVar(v) in &scheme.row_vars {
        w.u32(*v);
    }
    put_type(w, &scheme.ty);
    w.tag(tag::END);
}

fn get_scheme(r: &mut Reader) -> Decoded<Scheme> {
    const WHAT: &str = "malformed type scheme";
    r.tag(tag::SCHEME, WHAT)?;
    let count = r.count(WHAT)?;
    let mut ty_vars = Vec::with_capacity(count);
    for _ in 0..count {
        ty_vars.push(TyVar(r.u32(WHAT)?));
    }
    let count = r.count(WHAT)?;
    let mut row_vars = Vec::with_capacity(count);
    for _ in 0..count {
        row_vars.push(RowVar(r.u32(WHAT)?));
    }
    let ty = get_type(r)?;
    r.tag(tag::END, WHAT)?;
    Ok(Scheme {
        ty_vars,
        row_vars,
        ty,
    })
}

fn put_names(w: &mut Writer, names: &[NameRef]) {
    w.count(names.len());
    for name in names {
        w.tag(tag::NAME_REF);
        w.symbol(&name.name);
        w.def_hash(name.hash);
        w.tag(tag::END);
    }
}

fn get_names(r: &mut Reader) -> Decoded<Vec<NameRef>> {
    const WHAT: &str = "malformed resolution witness";
    let count = r.count(WHAT)?;
    let mut names = Vec::with_capacity(count);
    for _ in 0..count {
        r.tag(tag::NAME_REF, WHAT)?;
        let name = r.symbol(WHAT)?;
        let hash = r.def_hash(WHAT)?;
        r.tag(tag::END, WHAT)?;
        names.push(NameRef { name, hash });
    }
    Ok(names)
}

fn put_span(w: &mut Writer, span: FileSpan) {
    w.tag(tag::FILE_SPAN);
    w.u32(span.start);
    w.u32(span.end);
}

fn get_span(r: &mut Reader) -> Decoded<FileSpan> {
    const WHAT: &str = "malformed span";
    r.tag(tag::FILE_SPAN, WHAT)?;
    Ok(FileSpan {
        start: r.u32(WHAT)?,
        end: r.u32(WHAT)?,
    })
}

fn put_symbols(w: &mut Writer, symbols: &[ply_span::Symbol]) {
    w.count(symbols.len());
    for symbol in symbols {
        w.symbol(symbol);
    }
}

fn get_symbols(r: &mut Reader, what: &'static str) -> Decoded<Vec<ply_span::Symbol>> {
    let count = r.count(what)?;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        out.push(r.symbol(what)?);
    }
    Ok(out)
}

/// The witness comes first so that [`peek_names`] can identify which definition an entry belongs to
/// without decoding a scheme it is about to discard.
pub(crate) fn encode_def(def: &CachedDef) -> Vec<u8> {
    let mut w = Writer::new();
    w.tag(tag::CACHED_DEF);
    put_names(&mut w, &def.names);
    put_scheme(&mut w, &def.scheme);
    put_footprint(&mut w, &def.footprint);
    put_footprint(&mut w, &def.performed);
    w.count(def.row_aliases.len());
    for alias in &def.row_aliases {
        w.symbol(alias);
    }
    w.tag(tag::END);
    w.finish()
}

pub(crate) fn decode_def(bytes: &[u8]) -> Decoded<CachedDef> {
    const WHAT: &str = "malformed cached definition";
    let mut r = Reader::new(bytes);
    r.tag(tag::CACHED_DEF, WHAT)?;
    let names = get_names(&mut r)?;
    let scheme = get_scheme(&mut r)?;
    let footprint = get_footprint(&mut r)?;
    let performed = get_footprint(&mut r)?;
    let count = r.count(WHAT)?;
    let mut row_aliases = Vec::with_capacity(count.min(64));
    for _ in 0..count {
        row_aliases.push(r.symbol(WHAT)?);
    }
    r.tag(tag::END, WHAT)?;
    r.end(WHAT)?;
    Ok(CachedDef {
        scheme,
        footprint,
        performed,
        row_aliases,
        names,
    })
}

pub(crate) fn encode_decl(decl: &CachedDecl) -> Vec<u8> {
    let mut w = Writer::new();
    w.tag(tag::CACHED_DECL);
    put_names(&mut w, &decl.names);
    match &decl.body {
        DeclBody::Type { arity, ctors } => {
            w.tag(tag::DECL_TYPE);
            w.count(*arity);
            w.count(ctors.len());
            for ctor in ctors {
                w.tag(tag::CACHED_CTOR);
                w.count(ctor.fields.len());
                for field in &ctor.fields {
                    put_type(&mut w, field);
                }
                put_scheme(&mut w, &ctor.scheme);
                w.tag(tag::END);
            }
        }
        DeclBody::Effect { nondet, ops } => {
            w.tag(tag::DECL_EFFECT);
            w.bool(*nondet);
            w.count(ops.len());
            for op in ops {
                w.tag(tag::CACHED_OP);
                w.symbol(&op.name);
                put_mode(&mut w, op.mode);
                w.bool(op.resource_param);
                w.count(op.params.len());
                for param in &op.params {
                    put_type(&mut w, param);
                }
                put_type(&mut w, &op.ret);
                w.tag(tag::END);
            }
        }
    }
    w.tag(tag::END);
    w.tag(tag::END);
    w.finish()
}

pub(crate) fn decode_decl(bytes: &[u8]) -> Decoded<CachedDecl> {
    const WHAT: &str = "malformed cached declaration";
    let mut r = Reader::new(bytes);
    r.tag(tag::CACHED_DECL, WHAT)?;
    let names = get_names(&mut r)?;
    let body = match r.byte(WHAT)? {
        tag::DECL_TYPE => {
            let arity = r.count(WHAT)?;
            let count = r.count(WHAT)?;
            let mut ctors = Vec::with_capacity(count);
            for _ in 0..count {
                r.tag(tag::CACHED_CTOR, WHAT)?;
                let fields = r.count(WHAT)?;
                let mut out = Vec::with_capacity(fields);
                for _ in 0..fields {
                    out.push(get_type(&mut r)?);
                }
                let scheme = get_scheme(&mut r)?;
                r.tag(tag::END, WHAT)?;
                ctors.push(CachedCtor {
                    fields: out,
                    scheme,
                });
            }
            DeclBody::Type { arity, ctors }
        }
        tag::DECL_EFFECT => {
            let nondet = r.bool(WHAT)?;
            let count = r.count(WHAT)?;
            let mut ops = Vec::with_capacity(count);
            for _ in 0..count {
                r.tag(tag::CACHED_OP, WHAT)?;
                let name = r.symbol(WHAT)?;
                let mode = get_mode(&mut r)?;
                let resource_param = r.bool(WHAT)?;
                let count = r.count(WHAT)?;
                let mut params = Vec::with_capacity(count);
                for _ in 0..count {
                    params.push(get_type(&mut r)?);
                }
                let ret = get_type(&mut r)?;
                r.tag(tag::END, WHAT)?;
                ops.push(CachedOp {
                    name,
                    mode,
                    resource_param,
                    params,
                    ret,
                });
            }
            DeclBody::Effect { nondet, ops }
        }
        _ => return Err(crate::binary::DecodeError { what: WHAT, at: 0 }),
    };
    r.tag(tag::END, WHAT)?;
    r.tag(tag::END, WHAT)?;
    r.end(WHAT)?;
    Ok(CachedDecl { body, names })
}

pub(crate) fn encode_body(body: &DefBody) -> Vec<u8> {
    let mut w = Writer::new();
    w.tag(tag::DEF_BODY);
    w.u32(body.encoding());
    w.bytes(body.as_bytes());
    w.tag(tag::END);
    w.finish()
}

pub(crate) fn decode_body(bytes: &[u8]) -> Decoded<DefBody> {
    const WHAT: &str = "malformed definition body";
    let mut r = Reader::new(bytes);
    r.tag(tag::DEF_BODY, WHAT)?;
    let encoding = r.u32(WHAT)?;
    let payload = r.bytes(WHAT)?.to_vec();
    r.tag(tag::END, WHAT)?;
    r.end(WHAT)?;
    Ok(DefBody::new(encoding, payload))
}

fn put_kind(w: &mut Writer, kind: DefKind) {
    w.tag(match kind {
        DefKind::Fn => tag::DEF_KIND_FN,
        DefKind::Type => tag::DEF_KIND_TYPE,
        DefKind::Effect => tag::DEF_KIND_EFFECT,
    });
}

fn get_kind(r: &mut Reader) -> Decoded<DefKind> {
    const WHAT: &str = "malformed definition kind";
    match r.byte(WHAT)? {
        tag::DEF_KIND_FN => Ok(DefKind::Fn),
        tag::DEF_KIND_TYPE => Ok(DefKind::Type),
        tag::DEF_KIND_EFFECT => Ok(DefKind::Effect),
        _ => Err(crate::binary::DecodeError { what: WHAT, at: 0 }),
    }
}

pub(crate) fn encode_fingerprint(f: &SourceFingerprint) -> Vec<u8> {
    let mut w = Writer::new();
    w.tag(tag::FINGERPRINT);
    w.content_hash(f.content_hash);

    w.count(f.imports.len());
    for import in &f.imports {
        w.tag(tag::IMPORT_EDGE);
        w.symbol(&import.module);
        w.content_hash(import.exports);
        w.tag(tag::END);
    }

    put_names(&mut w, &f.deps);

    w.count(f.defs.len());
    for def in &f.defs {
        w.tag(tag::DEF_ENTRY);
        w.symbol(&def.name);
        w.def_hash(def.hash);
        w.def_hash(def.own);
        w.def_hash(def.iface);
        put_span(&mut w, def.span);
        put_kind(&mut w, def.kind);
        w.count(def.members.len());
        for member in &def.members {
            w.tag(tag::MEMBER);
            w.symbol(&member.name);
            put_span(&mut w, member.span);
            w.tag(tag::END);
        }
        put_symbols(&mut w, &def.deps);
        w.tag(tag::END);
    }

    w.count(f.tests.len());
    for test in &f.tests {
        w.tag(tag::CACHED_TEST);
        w.text(&test.name);
        w.def_hash(test.hash);
        w.bool(test.nondet);
        put_footprint(&mut w, &test.footprint);
        put_span(&mut w, test.span);
        put_span(&mut w, test.name_span);
        put_symbols(&mut w, &test.deps);
        w.tag(tag::END);
    }

    w.tag(tag::END);
    w.finish()
}

pub(crate) fn decode_fingerprint(bytes: &[u8]) -> Decoded<SourceFingerprint> {
    const WHAT: &str = "malformed source fingerprint";
    let mut r = Reader::new(bytes);
    r.tag(tag::FINGERPRINT, WHAT)?;
    let content_hash = r.content_hash(WHAT)?;

    let count = r.count(WHAT)?;
    let mut imports = Vec::with_capacity(count);
    for _ in 0..count {
        r.tag(tag::IMPORT_EDGE, WHAT)?;
        let module = r.symbol(WHAT)?;
        let exports = r.content_hash(WHAT)?;
        r.tag(tag::END, WHAT)?;
        imports.push(ImportEdge { module, exports });
    }

    let deps = get_names(&mut r)?;

    let count = r.count(WHAT)?;
    let mut defs = Vec::with_capacity(count);
    for _ in 0..count {
        r.tag(tag::DEF_ENTRY, WHAT)?;
        let name = r.symbol(WHAT)?;
        let hash = r.def_hash(WHAT)?;
        let own = r.def_hash(WHAT)?;
        let iface = r.def_hash(WHAT)?;
        let span = get_span(&mut r)?;
        let kind = get_kind(&mut r)?;
        let count = r.count(WHAT)?;
        let mut members = Vec::with_capacity(count);
        for _ in 0..count {
            r.tag(tag::MEMBER, WHAT)?;
            let name = r.symbol(WHAT)?;
            let span = get_span(&mut r)?;
            r.tag(tag::END, WHAT)?;
            members.push(Member { name, span });
        }
        let deps = get_symbols(&mut r, WHAT)?;
        r.tag(tag::END, WHAT)?;
        defs.push(DefEntry {
            name,
            hash,
            own,
            iface,
            span,
            kind,
            members,
            deps,
        });
    }

    let count = r.count(WHAT)?;
    let mut tests = Vec::with_capacity(count);
    for _ in 0..count {
        r.tag(tag::CACHED_TEST, WHAT)?;
        let name = r.text(WHAT)?.to_string();
        let hash = r.def_hash(WHAT)?;
        let nondet = r.bool(WHAT)?;
        let footprint = get_footprint(&mut r)?;
        let span = get_span(&mut r)?;
        let name_span = get_span(&mut r)?;
        let deps = get_symbols(&mut r, WHAT)?;
        r.tag(tag::END, WHAT)?;
        tests.push(CachedTest {
            name,
            hash,
            nondet,
            footprint,
            span,
            name_span,
            deps,
        });
    }

    r.tag(tag::END, WHAT)?;
    r.end(WHAT)?;
    Ok(SourceFingerprint {
        content_hash,
        imports,
        deps,
        defs,
        tests,
    })
}

/// The witness of a `CachedDef` or `CachedDecl` payload, without the scheme behind it.
pub(crate) fn peek_names(kind: u8, bytes: &[u8]) -> Decoded<Vec<NameRef>> {
    const WHAT: &str = "malformed cached interface";
    let mut r = Reader::new(bytes);
    r.tag(kind, WHAT)?;
    get_names(&mut r)
}

pub(crate) const DEF_TAG: u8 = tag::CACHED_DEF;
pub(crate) const DECL_TAG: u8 = tag::CACHED_DECL;
