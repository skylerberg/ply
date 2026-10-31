//! Marshalling: a value emitted code lays out written as PLV1 bytes for the runtime (a lowering),
//! or bytes the runtime wrote read into one (a lifting), by emitted code calling the natives here a
//! part at a time. What the bytes cannot hold a lowering writes as a handle, which nothing reads
//! back. Marshallings stack in the context, so one begun inside another works on a frame of its own
//! and leaves the outer as it found it.
//!
//! A native that returns bytes leaves them in the context's taken bytes and returns their count;
//! emitted code makes room for them and copies them out with [`rt_taken`] before it calls another.

use super::*;
use crate::emitted::{Block, Carried, Handed, Handle, PlyType};
use crate::native::{Addr, F64, I64, TakenBytes};
use ply_core::codec::{self, Reader, Writer, fun, tag};
use ply_core::{Escapee, escape};

/// One marshalling under way.
pub(crate) enum Marshalling {
    Lowering(Lowering),
    Lifting(Lifting),
}

/// Bytes being written, and the handles they hold of what they cannot: a runtime object, or a value
/// of a type the runtime keeps, neither of which anything reads back.
pub(crate) struct Lowering {
    w: Writer,
    handles: u64,
    sharing: Option<Sharing>,
    /// The strings, bytes, lists, arrays and maps written so far, which `AGAIN` numbers.
    shareables: u64,
    /// Whether a function the program wrote was written, which no reader outside its run can hold.
    functions: bool,
    /// Whether a function holding what may not leave its run was written, which the boundary
    /// rejects as the continuation it holds.
    resumes: bool,
    /// Whether this is an entry's result, and the first handle of its run written into it, which
    /// the boundary rejects.
    results_entry: bool,
    escaping: Option<escape::Handle>,
    /// Whether this is a value a diagnostic shows, which keeps nothing beside its bytes: a runtime
    /// object written as a handle at the position [`codec::shown`] gives what it is, and a carried
    /// value written whole through its type.
    shown: bool,
}

/// What a lowering that writes a value shared as `AGAIN` knows of the values written so far.
struct Sharing {
    numbered: HashMap<Word, u64>,
    /// The value [`rt_out_again`] was asked of last, numbered as the next shareable written.
    next: Option<Word>,
}

impl Lowering {
    fn new(again: bool) -> Lowering {
        Lowering {
            w: Writer::new(),
            handles: 0,
            sharing: again.then(|| Sharing {
                numbered: HashMap::new(),
                next: None,
            }),
            shareables: 0,
            functions: false,
            resumes: false,
            results_entry: false,
            escaping: None,
            shown: false,
        }
    }

    /// A lowering of a value a diagnostic shows, a value written a second time written as `AGAIN`.
    fn shown() -> Lowering {
        Lowering {
            shown: true,
            ..Lowering::new(true)
        }
    }

    /// Numbers the shareable value whose head is about to be written.
    fn opens_shareable(&mut self) {
        let n = self.shareables;
        self.shareables += 1;
        if let Some(s) = &mut self.sharing
            && let Some(w) = s.next.take()
        {
            s.numbered.insert(w, n);
        }
    }

    fn handle(&mut self) {
        self.w.handle(self.handles);
        self.handles += 1;
    }
}

/// Bytes being read.
pub(crate) struct Lifting {
    r: Reader<Vec<u8>>,
    /// The name of the constructor read last.
    name: Option<Symbol>,
    /// The high word of the `Decimal` or width read last, until [`rt_in_high`] takes it.
    high: Option<i64>,
}

impl Lifting {
    /// `bits`' low word, its high word kept for [`rt_in_high`].
    fn keep_high(&mut self, bits: u128) -> i64 {
        self.high = Some((bits >> 64) as u64 as i64);
        bits as u64 as i64
    }
}

/// Why marshalling failed, which is Ply's defect: the bytes and the type disagree, or emitted code
/// called a native outside the marshalling it belongs to.
#[cold]
#[inline(never)]
pub(crate) fn err_marshalling(reason: &str) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("a value marshalled between compiled code and the runtime: {reason}"),
    )
    .primary(Span::DUMMY, "in compiled code")
    .note("this is Ply's fault, not the program's")
}

impl Ctx {
    fn lowering(&mut self) -> Result<&mut Lowering, i64> {
        if !matches!(self.marshallings.last(), Some(Marshalling::Lowering(_))) {
            return Err(self.fail(err_marshalling("a value written with no lowering open")));
        }
        match self.marshallings.last_mut() {
            Some(Marshalling::Lowering(l)) => Ok(l),
            _ => unreachable!("the innermost marshalling is a lowering"),
        }
    }

    /// Whether the innermost marshalling is a lowering of a value a diagnostic shows.
    fn showing(&self) -> bool {
        matches!(self.marshallings.last(), Some(Marshalling::Lowering(l)) if l.shown)
    }

    fn lifting(&mut self) -> Result<&mut Lifting, i64> {
        if !matches!(self.marshallings.last(), Some(Marshalling::Lifting(_))) {
            return Err(self.fail(err_marshalling("a value read with no lifting open")));
        }
        match self.marshallings.last_mut() {
            Some(Marshalling::Lifting(l)) => Ok(l),
            _ => unreachable!("the innermost marshalling is a lifting"),
        }
    }

    /// `read` of the innermost lifting, or 0 with the context failed where it errs.
    fn read<T: Default>(&mut self, read: impl FnOnce(&mut Lifting) -> Result<T, String>) -> T {
        let r = match self.lifting() {
            Ok(l) => read(l),
            Err(_) => return T::default(),
        };
        r.unwrap_or_else(|reason| {
            self.fail(err_marshalling(&reason));
            T::default()
        })
    }

    fn write(&mut self, write: impl FnOnce(&mut Lowering)) {
        if let Ok(l) = self.lowering() {
            write(l);
        }
    }

    /// The marshalling's bytes compiled code copies out, held until it does.
    pub(crate) fn give(&mut self, bytes: Vec<u8>) -> I64 {
        self.taken = bytes;
        I64(self.taken.len() as i64)
    }

    /// The bytes a machine-word native returns, held until compiled code copies them out.
    pub(crate) fn given(&mut self, bytes: Vec<u8>) -> TakenBytes {
        TakenBytes(self.give(bytes).0)
    }
}

/// The `n` bytes compiled code passes borrowed at `p`.
///
/// # Safety
/// `p` holds `n` bytes, or `n` is 0.
pub(crate) unsafe fn borrowed<'a>(p: Addr, n: I64) -> &'a [u8] {
    if n.0 <= 0 {
        return &[];
    }
    unsafe { std::slice::from_raw_parts(p.0, n.0 as usize) }
}

/// A value that is its tag alone: `t` one of PLV1's unit, false and true.
pub unsafe extern "C" fn rt_out_tag(ctx: *mut Ctx, t: I64) {
    let c = unsafe { &mut *ctx };
    match u8::try_from(t.0) {
        Ok(t @ (tag::UNIT | tag::FALSE | tag::TRUE)) => c.write(|l| l.w.tag(t)),
        _ => {
            c.fail(err_marshalling(&format!(
                "tag {} is not a value alone",
                t.0
            )));
        }
    }
}

pub unsafe extern "C" fn rt_out_int(ctx: *mut Ctx, v: I64) {
    unsafe { &mut *ctx }.write(|l| l.w.int(v.0));
}

pub unsafe extern "C" fn rt_out_float(ctx: *mut Ctx, bits: F64) {
    // Every NaN is written as the one `Float`'s own reader makes.
    let canonical = ply_core::float::bits(f64::from_bits(bits.0));
    unsafe { &mut *ctx }.write(|l| l.w.float(canonical));
}

/// A `Decimal` by the two words `rust_decimal`'s serialization is, little-endian.
pub unsafe extern "C" fn rt_out_decimal(ctx: *mut Ctx, lo: I64, hi: I64) {
    let mut d = [0u8; 16];
    d[..8].copy_from_slice(&lo.0.to_le_bytes());
    d[8..].copy_from_slice(&hi.0.to_le_bytes());
    unsafe { &mut *ctx }.write(|l| l.w.decimal(d));
}

/// A fixed width, `width` its place among PLV1's widths, by its bits however compiled code extends
/// them: `lo` alone for a width of at most 64 bits, `hi` above it for 128.
pub unsafe extern "C" fn rt_out_fixed(ctx: *mut Ctx, width: I64, lo: I64, hi: I64) {
    let c = unsafe { &mut *ctx };
    let Some(&ty) = usize::try_from(width.0)
        .ok()
        .and_then(|w| ply_core::INT_TYPES.get(w))
    else {
        c.fail(err_marshalling(&format!("width {} is no width", width.0)));
        return;
    };
    let high = if ty.bits() == 128 { hi.0 as u64 } else { 0 };
    let bits = (u128::from(high) << 64) | u128::from(lo.0 as u64);
    c.write(|l| l.w.fixed(ply_core::Fixed::new(ty, bits)));
}

/// A `Char` by its scalar value.
pub unsafe extern "C" fn rt_out_char(ctx: *mut Ctx, c: I64) {
    let cx = unsafe { &mut *ctx };
    match u32::try_from(c.0).ok().and_then(char::from_u32) {
        Some(ch) => cx.write(|l| l.w.char(u32::from(ch))),
        None => {
            cx.fail(err_marshalling(&format!("{} is no character", c.0)));
        }
    }
}

/// A string, the `n` bytes of UTF-8 at `p`.
pub unsafe extern "C" fn rt_out_str(ctx: *mut Ctx, p: Addr, n: I64) {
    let s = unsafe { borrowed(p, n) };
    unsafe { &mut *ctx }.write(|l| {
        l.opens_shareable();
        l.w.str(s);
    });
}

/// A bytes value, the `n` bytes at `p`.
pub unsafe extern "C" fn rt_out_bytes(ctx: *mut Ctx, p: Addr, n: I64) {
    let b = unsafe { borrowed(p, n) };
    unsafe { &mut *ctx }.write(|l| {
        l.opens_shareable();
        l.w.bytes(b);
    });
}

/// The head of a list, array or map of `n` items or of a record of `n` fields, `t` its PLV1 tag:
/// the items follow, a map's each key then its value, a record's each [`rt_out_name`] then its
/// value, in name order.
pub unsafe extern "C" fn rt_out_count(ctx: *mut Ctx, t: I64, n: I64) {
    let c = unsafe { &mut *ctx };
    let (Ok(t @ (tag::LIST | tag::ARRAY | tag::MAP | tag::RECORD)), Ok(n)) =
        (u8::try_from(t.0), u64::try_from(n.0))
    else {
        c.fail(err_marshalling(&format!("tag {} counting {}", t.0, n.0)));
        return;
    };
    c.write(|l| {
        if codec::shareable(t) {
            l.opens_shareable();
        }
        l.w.count(t, n);
    });
}

/// The head of the constructor named by the `len` bytes at `name`, given `n` arguments, which
/// follow.
pub unsafe extern "C" fn rt_out_ctor(ctx: *mut Ctx, name: Addr, len: I64, n: I64) {
    let name = unsafe { borrowed(name, len) };
    unsafe { &mut *ctx }.write(|l| l.w.ctor(name, n.0.max(0) as u64));
}

/// A record field's name, the `n` bytes at `p`, before its value.
pub unsafe extern "C" fn rt_out_name(ctx: *mut Ctx, p: Addr, n: I64) {
    let name = unsafe { borrowed(p, n) };
    unsafe { &mut *ctx }.write(|l| l.w.name(name));
}

/// A runtime object, written as a handle: one of the run in an entry's result is noted for the
/// boundary to reject, and in a value a diagnostic shows, the handle says what the object is.
pub unsafe extern "C" fn rt_out_handle(ctx: *mut Ctx, h: Handle) {
    let c = unsafe { &mut *ctx };
    let w = h.word();
    if c.showing() {
        let kind = if scoped_id(w).is_some() {
            codec::shown::SCOPED
        } else {
            codec::shown::SEALED
        };
        c.write(|l| l.w.handle(kind));
        return;
    }
    if heap::is_native(w)
        && let Some(escaping) = unsafe { heap::native_escapes(w) }
        && let Ok(l) = c.lowering()
        && l.results_entry
    {
        l.escaping.get_or_insert(escaping);
    }
    c.write(Lowering::handle);
}

/// A value the runtime keeps rather than reads, with its type, borrowed from compiled code: what is
/// in the position of an operation's own type parameter, written as a handle; in a value a
/// diagnostic shows, written in place through its type.
pub unsafe extern "C" fn rt_out_carried(ctx: *mut Ctx, w: Handed, ty: *const PlyType) {
    if unsafe { &*ctx }.showing() {
        unsafe { ((*ty).lower)(ctx, w.word()) };
        return;
    }
    unsafe { &mut *ctx }.write(Lowering::handle);
}

/// A function the program wrote, `f`, borrowed from compiled code: named by the `len` bytes at
/// `name`, or anonymous where `len` is 0; or, an anonymous one holding what may not leave its
/// thread, as a continuation's closure does, one no run but its own can resume.
pub unsafe extern "C" fn rt_out_fun(ctx: *mut Ctx, f: Handed, name: Addr, len: I64) {
    let name = unsafe { borrowed(name, len) };
    let leaves = !name.is_empty() || unsafe { Block::of(f.word()).share(ctx) };
    unsafe { &mut *ctx }.write(|l| {
        if !leaves {
            l.resumes = true;
            l.w.fun(fun::RESUMES);
            return;
        }
        l.functions = true;
        if name.is_empty() {
            l.w.fun(fun::ANON);
        } else {
            l.w.fun(fun::NAMED);
            l.w.name(name);
        }
    });
}

/// Asked before a string, bytes, list, array or map, `w` its word, which is borrowed: 1 where the
/// lowering shares values and wrote `w` before, which it has now written again, else 0, and the
/// caller writes the value.
pub unsafe extern "C" fn rt_out_again(ctx: *mut Ctx, w: Handed) -> I64 {
    let c = unsafe { &mut *ctx };
    let Ok(l) = c.lowering() else {
        return I64(0);
    };
    let Some(s) = &mut l.sharing else {
        return I64(0);
    };
    let w = w.word();
    // A static is one object for every value of its contents, a string's and a byte string's
    // alike, so only a heap object's word names one value.
    if heap::is_object(w) && unsafe { heap::count_of(heap::obj(w)) } == heap::IMMORTAL {
        s.next = None;
        return I64(0);
    }
    match s.numbered.get(&w) {
        Some(&n) => {
            l.w.again(n);
            I64(1)
        }
        None => {
            s.next = Some(w);
            I64(0)
        }
    }
}

/// A value that is its tag alone: PLV1's unit, false or true, by its tag.
pub unsafe extern "C" fn rt_in_tag(ctx: *mut Ctx) -> I64 {
    I64(unsafe { &mut *ctx }.read(|l| match l.r.byte()? {
        t @ (tag::UNIT | tag::FALSE | tag::TRUE) => Ok(i64::from(t)),
        t => Err(format!(
            "a value tagged {t} where one that is its tag alone was read"
        )),
    }))
}

pub unsafe extern "C" fn rt_in_int(ctx: *mut Ctx) -> I64 {
    I64(unsafe { &mut *ctx }.read(|l| {
        l.r.expect(tag::INT)?;
        l.r.int()
    }))
}

pub unsafe extern "C" fn rt_in_float(ctx: *mut Ctx) -> F64 {
    F64(unsafe { &mut *ctx }.read(|l| {
        l.r.expect(tag::FLOAT)?;
        Ok(u64::from_le_bytes(l.r.array()?))
    }))
}

/// A `Decimal`'s low word, the first eight bytes of `rust_decimal`'s serialization read
/// little-endian; [`rt_in_high`] returns the other eight.
pub unsafe extern "C" fn rt_in_decimal(ctx: *mut Ctx) -> I64 {
    I64(unsafe { &mut *ctx }.read(|l| {
        l.r.expect(tag::DECIMAL)?;
        let d = u128::from_le_bytes(l.r.array()?);
        Ok(l.keep_high(d))
    }))
}

/// A fixed width, which must be the one at `width` among PLV1's: its low word, its bits sign- or
/// zero-extended as the width's sign says; [`rt_in_high`] returns the word above it.
pub unsafe extern "C" fn rt_in_fixed(ctx: *mut Ctx, width: I64) -> I64 {
    I64(unsafe { &mut *ctx }.read(|l| {
        l.r.expect(tag::FIXED)?;
        let f = l.r.fixed()?;
        let at = ply_core::INT_TYPES.iter().position(|t| *t == f.ty);
        if at != usize::try_from(width.0).ok() {
            return Err(format!("a {:?} where width {} was read", f.ty, width.0));
        }
        Ok(l.keep_high(f.bits()))
    }))
}

/// The high word of the `Decimal` or width [`rt_in_decimal`] or [`rt_in_fixed`] read last, which
/// is asked once.
pub unsafe extern "C" fn rt_in_high(ctx: *mut Ctx) -> I64 {
    I64(unsafe { &mut *ctx }.read(|l| {
        l.high
            .take()
            .ok_or_else(|| "a high word asked where no wide value was read".into())
    }))
}

/// A `Char`'s scalar value.
pub unsafe extern "C" fn rt_in_char(ctx: *mut Ctx) -> I64 {
    I64(unsafe { &mut *ctx }.read(|l| {
        l.r.expect(tag::CHAR)?;
        Ok(i64::from(u32::from(l.r.char()?)))
    }))
}

/// A string's length in bytes, its UTF-8 left as the taken bytes.
pub unsafe extern "C" fn rt_in_str(ctx: *mut Ctx) -> I64 {
    let c = unsafe { &mut *ctx };
    let s: Vec<u8> = c.read(|l| {
        l.r.expect(tag::STR)?;
        Ok(l.r.str()?.as_bytes().to_vec())
    });
    c.give(s)
}

/// A bytes value's length, its bytes left as the taken bytes.
pub unsafe extern "C" fn rt_in_bytes(ctx: *mut Ctx) -> I64 {
    let c = unsafe { &mut *ctx };
    let b: Vec<u8> = c.read(|l| {
        l.r.expect(tag::BYTES)?;
        Ok(l.r.blob()?.to_vec())
    });
    c.give(b)
}

/// How many items or fields follow the head of a list, array, map, record or constructor, `t`
/// the tag it must have. A constructor's name becomes the name read last.
pub unsafe extern "C" fn rt_in_count(ctx: *mut Ctx, t: I64) -> I64 {
    I64(unsafe { &mut *ctx }.read(|l| {
        let t = u8::try_from(t.0).map_err(|_| format!("tag {}", t.0))?;
        if !matches!(
            t,
            tag::LIST | tag::ARRAY | tag::MAP | tag::RECORD | tag::CTOR
        ) {
            return Err(format!("tag {t} heads no items"));
        }
        l.r.expect(t)?;
        if t == tag::CTOR {
            l.name = Some(l.r.symbol()?);
        }
        Ok(l.r.count()? as i64)
    }))
}

/// A record field's name, which must be the `n` bytes at `p`.
pub unsafe extern "C" fn rt_in_field(ctx: *mut Ctx, p: Addr, n: I64) {
    let want = unsafe { borrowed(p, n) };
    unsafe { &mut *ctx }.read(|l| {
        let name = l.r.symbol()?;
        if name.as_bytes() != want {
            return Err(format!(
                "field `{name}` where `{}` was read",
                String::from_utf8_lossy(want)
            ));
        }
        Ok(())
    })
}

/// 1 where the constructor read last is named by the `n` bytes at `p`, else 0.
pub unsafe extern "C" fn rt_in_name_is(ctx: *mut Ctx, p: Addr, n: I64) -> I64 {
    let want = unsafe { borrowed(p, n) };
    I64(unsafe { &mut *ctx }.read(|l| match &l.name {
        Some(name) => Ok(i64::from(name.as_bytes() == want)),
        None => Err("a constructor's name asked before one was read".into()),
    }))
}

/// What a descriptor lifts by for every type but `String`, the one type the runtime lifts: it fails
/// the entry.
pub unsafe extern "C" fn rt_never_lifted(ctx: *mut Ctx) {
    unsafe { &mut *ctx }.fail(err_marshalling(
        "a value lifted through a descriptor that lifts none: the runtime lifts only a `String`",
    ));
}

/// What a descriptor lowers by where no diagnostic shows a value of its type: it fails the entry.
pub unsafe extern "C" fn rt_never_shown(ctx: *mut Ctx) {
    unsafe { &mut *ctx }.fail(err_marshalling(
        "a value shown through a descriptor that does not lower it",
    ));
}

/// Begins a lowering of a value a diagnostic shows inside whatever marshalling is under way, which
/// [`rt_lower_close`] ends: nothing is held beside its bytes.
pub unsafe extern "C" fn rt_shown_open(ctx: *mut Ctx) {
    unsafe { &mut *ctx }
        .marshallings
        .push(Marshalling::Lowering(Lowering::shown()));
}

/// Ends the innermost lowering, which holds no handle: its bytes' count, the bytes left as the
/// taken bytes.
pub unsafe extern "C" fn rt_lower_close(ctx: *mut Ctx) -> I64 {
    let c = unsafe { &mut *ctx };
    if c.lowering().is_err() {
        return I64(0);
    }
    let Some(Marshalling::Lowering(l)) = c.marshallings.pop() else {
        unreachable!("the innermost marshalling is a lowering");
    };
    if l.handles > 0 {
        c.fail(err_marshalling("a value the bytes hold only a handle of"));
        return I64(0);
    }
    c.give(l.w.finish())
}

/// Ends the innermost lifting, every byte of which must have been read.
pub unsafe extern "C" fn rt_lift_close(ctx: *mut Ctx) {
    let c = unsafe { &mut *ctx };
    if c.lifting().is_err() {
        return;
    }
    if let Err(reason) = c.close_lifting() {
        c.fail(err_marshalling(&reason));
    }
}

/// Copies the taken bytes, `n` of them, to `dst`.
pub unsafe extern "C" fn rt_taken(ctx: *mut Ctx, dst: Addr, n: I64) {
    let c = unsafe { &mut *ctx };
    if usize::try_from(n.0).ok() != Some(c.taken.len()) {
        c.fail(err_marshalling(&format!(
            "{} bytes taken, of {} given",
            n.0,
            c.taken.len()
        )));
        return;
    }
    unsafe { std::ptr::copy_nonoverlapping(c.taken.as_ptr(), dst.0, c.taken.len()) };
    c.taken.clear();
}

impl Ctx {
    /// Begins a lifting of `bytes`; one that does not open fails the context and lifts nothing, so
    /// the marshallings still nest.
    pub fn lift_from(&mut self, bytes: Vec<u8>) {
        let r = match Reader::new(bytes) {
            Ok(r) => r,
            Err(reason) => {
                self.fail(err_marshalling(&reason));
                Reader::new(codec::FORMAT.to_vec()).expect("the format alone opens")
            }
        };
        self.marshallings.push(Marshalling::Lifting(Lifting {
            r,
            name: None,
            high: None,
        }));
    }

    /// Ends the innermost lifting; an error where bytes are left.
    fn close_lifting(&mut self) -> Result<(), String> {
        let Some(Marshalling::Lifting(l)) = self.marshallings.pop() else {
            unreachable!("the innermost marshalling is a lifting");
        };
        l.r.end()
    }
}

/// The PLV1 bytes a diagnostic shows of `v`, which it borrows, lowered through its type.
pub(crate) fn shown(ctx: *mut Ctx, v: &Carried) -> Result<Vec<u8>, String> {
    let l = lowering(ctx, Lowering::shown(), |ctx| unsafe {
        (v.ty().lower)(ctx, v.word());
    })?;
    Ok(l.w.finish())
}

/// What `write` writes into `opened`, a lowering of its own.
fn lowering(
    ctx: *mut Ctx,
    opened: Lowering,
    write: impl FnOnce(*mut Ctx),
) -> Result<Lowering, String> {
    let depth = unsafe { &*ctx }.marshallings.len();
    unsafe { &mut *ctx }
        .marshallings
        .push(Marshalling::Lowering(opened));
    write(ctx);
    let c = unsafe { &mut *ctx };
    if c.marshallings.len() != depth + 1 {
        c.marshallings.truncate(depth + 1);
        return Err("a lowering left a marshalling of its own open".into());
    }
    let Some(Marshalling::Lowering(l)) = c.marshallings.pop() else {
        return Err("a lowering ended a marshalling it did not begin".into());
    };
    Ok(l)
}

/// `s` lifted through `string`, the type of `String`; `None` with the context failed where it does
/// not cross.
pub(crate) fn string_lifted(ctx: *mut Ctx, s: &str, string: &PlyType) -> Option<Handed> {
    let mut w = codec::Writer::new();
    w.str(s.as_bytes());
    lifted_from(ctx, w.finish(), string)
}

/// `bytes` lifted through `ty`; `None` with the context failed where they do not cross.
fn lifted_from(ctx: *mut Ctx, bytes: Vec<u8>, ty: &PlyType) -> Option<Handed> {
    let c = unsafe { &mut *ctx };
    let depth = c.marshallings.len();
    c.lift_from(bytes);
    let w = unsafe { (ty.lift)(ctx) };
    let c = unsafe { &mut *ctx };
    if c.marshallings.len() != depth + 1 {
        c.marshallings.truncate(depth + 1);
        c.fail(err_marshalling(
            "a lifting left a marshalling of its own open",
        ));
    }
    if let Err(reason) = c.close_lifting() {
        c.fail(err_marshalling(&reason));
    }
    (c.failed == 0).then(|| <Handed as crate::native::Returned>::of(w))
}

impl Ctx {
    /// Opens what an entry through `R#entered` runs between: a lowering for its result, and over it
    /// a lifting of `bytes`, the arguments' list.
    pub(crate) fn open_entry(&mut self, bytes: Vec<u8>) {
        self.marshallings.push(Marshalling::Lowering(Lowering {
            results_entry: true,
            ..Lowering::new(false)
        }));
        self.lift_from(bytes);
    }
}

/// What an entry's `R#entered` wrote of its root's result, kept as the bytes it wrote.
pub(crate) enum EntryBytes {
    /// The result's PLV1, untouched.
    Bytes(Vec<u8>),
    /// The result holds what the bytes cannot, which only its own run can hold.
    Holding,
    /// The result holds a function the program wrote, which only its own run can hold.
    Function,
    /// The result holds a continuation or a handle of its run, the first written, which the
    /// boundary rejects.
    Escaping(Escapee),
}

/// Ends the lowering [`Ctx::open_entry`] opened: the result kept as the bytes the entry wrote
/// where they hold all of it.
pub(crate) fn close_entry(ctx: *mut Ctx) -> Result<EntryBytes, String> {
    let l = entry_lowering(ctx)?;
    let held = l.handles > 0;
    let bytes = l.w.finish();
    if let Some(handle) = l.escaping {
        return Ok(EntryBytes::Escaping(Escapee {
            handle,
            route: Vec::new(),
        }));
    }
    if l.functions {
        return Ok(EntryBytes::Function);
    }
    if l.resumes
        && let Some(route) = continuation_in(&bytes)?
    {
        return Ok(EntryBytes::Escaping(Escapee {
            handle: escape::Handle::Continuation,
            route,
        }));
    }
    if held || l.resumes {
        return Ok(EntryBytes::Holding);
    }
    Ok(EntryBytes::Bytes(bytes))
}

/// The way to the first continuation `bytes` hold, outermost first, where they hold one.
pub fn continuation_in(bytes: &[u8]) -> Result<Option<Vec<String>>, String> {
    let mut r = Reader::new(bytes)?;
    let mut route = Vec::new();
    if !walked(&mut r, &mut route)? {
        return Ok(None);
    }
    route.reverse();
    Ok(Some(route))
}

/// Reads one value, returning whether it reaches a continuation, with the way to it in `route`,
/// innermost first.
fn walked(r: &mut Reader<&[u8]>, route: &mut Vec<String>) -> Result<bool, String> {
    let t = r.byte()?;
    let found = |route: &mut Vec<String>, step: String| {
        route.push(step);
        Ok(true)
    };
    match t {
        tag::UNIT | tag::FALSE | tag::TRUE => {}
        tag::INT => {
            r.int()?;
        }
        tag::FLOAT => {
            r.array::<8>()?;
        }
        tag::DECIMAL => {
            r.array::<16>()?;
        }
        tag::FIXED => {
            r.fixed()?;
        }
        tag::CHAR | tag::HANDLE | tag::AGAIN => {
            r.varint()?;
        }
        tag::STR | tag::BYTES => {
            r.blob()?;
        }
        tag::LIST | tag::ARRAY => {
            let noun = if t == tag::LIST { "item" } else { "element" };
            for i in 0..r.count()? {
                if walked(r, route)? {
                    return found(route, format!("{noun} {i}"));
                }
            }
        }
        tag::MAP => {
            for _ in 0..r.count()? {
                if walked(r, route)? {
                    return found(route, "a key".into());
                }
                if walked(r, route)? {
                    return found(route, "a map entry".into());
                }
            }
        }
        tag::RECORD => {
            for _ in 0..r.count()? {
                let name = r.symbol()?;
                if walked(r, route)? {
                    return found(route, format!("field `{name}`"));
                }
            }
        }
        tag::CTOR => {
            let name = r.symbol()?;
            for i in 0..r.varint()? {
                if walked(r, route)? {
                    return found(route, format!("`{name}`'s argument {}", i + 1));
                }
            }
        }
        tag::FUN => match r.byte()? {
            fun::RESUMES => return Ok(true),
            fun::NAMED => {
                r.symbol()?;
            }
            fun::ANON => {}
            fun::CONST => {
                r.varint()?;
                return walked(r, route);
            }
            fun::PROJECT => {
                r.varint()?;
                r.varint()?;
            }
            fun::TABLE => {
                r.varint()?;
                for _ in 0..r.varint()? {
                    if walked(r, route)? || walked(r, route)? {
                        return Ok(true);
                    }
                }
                return walked(r, route);
            }
            case => return Err(format!("a function of case {case}")),
        },
        t => return Err(format!("a value tagged {t}")),
    }
    Ok(false)
}

/// Ends the lowering [`Ctx::open_entry`] opened and returns it; an error where the entry ended that
/// lowering or left another marshalling open.
fn entry_lowering(ctx: *mut Ctx) -> Result<Lowering, String> {
    let c = unsafe { &mut *ctx };
    let open = c.marshallings.len();
    if open != 1 {
        c.marshallings.clear();
        return Err(match open {
            0 => "the entry ended the lowering its result was to be written into".into(),
            _ => format!(
                "the entry left {} marshallings open past its result's",
                open - 1
            ),
        });
    }
    match c.marshallings.pop() {
        Some(Marshalling::Lowering(l)) => Ok(l),
        _ => Err("the entry ended the lowering its result was to be written into".into()),
    }
}
