//! The encoding of one stored entry's payload.

use crate::DefBody;
use crate::binary::{Decoded, Reader, Writer};
use crate::frontend::{DefEntry, DefKind, FileSpan, Member, Slot, SourceFingerprint, TestEntry};

mod tag {
    pub(super) const MEMBER: u8 = 0x41;
    pub(super) const FILE_SPAN: u8 = 0x43;
    pub(super) const DEF_ENTRY: u8 = 0x44;
    pub(super) const TEST_ENTRY: u8 = 0x45;

    pub(super) const DEF_KIND_FN: u8 = 0x50;
    pub(super) const DEF_KIND_TYPE: u8 = 0x51;
    pub(super) const DEF_KIND_EFFECT: u8 = 0x52;

    pub(super) const SLOT: u8 = 0x60;
    pub(super) const DEF_BODY: u8 = 0x66;
    pub(super) const FINGERPRINT: u8 = 0x67;

    /// Closes every composite.
    pub(super) const END: u8 = 0xee;
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

/// The name comes first so [`peek_slot_name`] can find a slot without copying its value.
pub fn encode_slot(slot: &Slot) -> Vec<u8> {
    let mut w = Writer::new();
    w.tag(tag::SLOT);
    w.symbol(&slot.name);
    w.bytes(&slot.value);
    w.tag(tag::END);
    w.finish()
}

pub(crate) fn decode_slot(bytes: &[u8]) -> Decoded<Slot> {
    const WHAT: &str = "malformed slot";
    let mut r = Reader::new(bytes);
    r.tag(tag::SLOT, WHAT)?;
    let name = r.symbol(WHAT)?;
    let value = r.bytes(WHAT)?.to_vec();
    r.tag(tag::END, WHAT)?;
    r.end(WHAT)?;
    Ok(Slot { name, value })
}

pub(crate) fn peek_slot_name(bytes: &[u8]) -> Decoded<ply_eval::Symbol> {
    const WHAT: &str = "malformed slot";
    let mut r = Reader::new(bytes);
    r.tag(tag::SLOT, WHAT)?;
    r.symbol(WHAT)
}

pub fn encode_body(body: &DefBody) -> Vec<u8> {
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

pub fn encode_fingerprint(f: &SourceFingerprint) -> Vec<u8> {
    let mut w = Writer::new();
    w.tag(tag::FINGERPRINT);
    w.content_hash(f.content_hash);
    w.text(&f.module);

    w.count(f.defs.len());
    for def in &f.defs {
        w.tag(tag::DEF_ENTRY);
        w.symbol(&def.name);
        w.def_hash(def.hash);
        put_span(&mut w, def.span);
        put_kind(&mut w, def.kind);
        w.count(def.members.len());
        for member in &def.members {
            w.tag(tag::MEMBER);
            w.symbol(&member.name);
            put_span(&mut w, member.span);
            w.tag(tag::END);
        }
        w.tag(tag::END);
    }

    w.count(f.tests.len());
    for test in &f.tests {
        w.tag(tag::TEST_ENTRY);
        w.text(&test.name);
        w.def_hash(test.hash);
        w.bool(test.nondet);
        put_span(&mut w, test.span);
        w.bytes(&test.row);
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
    let module = r.text(WHAT)?.to_string();

    let count = r.count(WHAT)?;
    let mut defs = Vec::with_capacity(count);
    for _ in 0..count {
        r.tag(tag::DEF_ENTRY, WHAT)?;
        let name = r.symbol(WHAT)?;
        let hash = r.def_hash(WHAT)?;
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
        r.tag(tag::END, WHAT)?;
        defs.push(DefEntry {
            name,
            hash,
            span,
            kind,
            members,
        });
    }

    let count = r.count(WHAT)?;
    let mut tests = Vec::with_capacity(count);
    for _ in 0..count {
        r.tag(tag::TEST_ENTRY, WHAT)?;
        let name = r.text(WHAT)?.to_string();
        let hash = r.def_hash(WHAT)?;
        let nondet = r.bool(WHAT)?;
        let span = get_span(&mut r)?;
        let row = r.bytes(WHAT)?.to_vec();
        r.tag(tag::END, WHAT)?;
        tests.push(TestEntry {
            name,
            hash,
            nondet,
            span,
            row,
        });
    }

    r.tag(tag::END, WHAT)?;
    r.end(WHAT)?;
    Ok(SourceFingerprint {
        content_hash,
        module,
        defs,
        tests,
    })
}
