//! Crossings: a value emitted code lays out written as PLV1 bytes the runtime reads (a lowering),
//! or bytes the runtime wrote read into one (a lifting), by emitted code calling the natives here a
//! part at a time. What the bytes cannot hold crosses beside them as a handle. Crossings stack in
//! the context, so one begun inside another works on a frame of its own and leaves the outer as it
//! found it.
//!
//! A native that answers bytes leaves them in the context's taken bytes and answers their count;
//! emitted code makes room for them and copies them out with [`rt_taken`] before it calls another.

use super::*;
use crate::emitted::{Block, Carried, Handed, Handle, PlyType};
use crate::native::{Addr, F64, I64, TakenBytes};
use ply_core::codec::{self, Reader, Writer, fun, tag};
use ply_core::{Escapee, escape};

/// One crossing under way.
pub(crate) enum Crossing {
    Lowering(Lowering),
    Lifting(Lifting),
}

/// Bytes being written, and what crosses beside them, each lent for as long as the crossing lasts.
pub(crate) struct Lowering {
    w: Writer,
    beside: Vec<Beside>,
    sharing: Option<Sharing>,
    /// The strings, bytes, lists, arrays and maps written so far, which `AGAIN` numbers.
    shareables: u64,
    /// Whether a function the program wrote was written, which no reader outside its run can hold.
    functions: bool,
    /// Whether a function holding what may not leave its run was written, which the boundary
    /// refuses as the continuation it holds.
    resumes: bool,
    /// Whether this is an entry's answer, and the first handle of its run written into it, which
    /// the boundary refuses.
    answers_entry: bool,
    escaping: Option<escape::Handle>,
}

/// What crosses beside a lowering's bytes, held by the lowering itself, since compiled code may let
/// go of what it wrote before the lowering is read: a runtime object's value, or a value compiled
/// code lent with its type, held once more until the lowering ends.
pub(crate) enum Beside {
    Runtime(Value),
    Carried(Carried),
}

/// Lets go of what a lowering's `beside` holds.
pub(crate) fn released(ctx: *mut Ctx, beside: Vec<Beside>) {
    for b in beside {
        if let Beside::Carried(v) = b {
            v.release(ctx);
        }
    }
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
            beside: Vec::new(),
            sharing: again.then(|| Sharing {
                numbered: HashMap::new(),
                next: None,
            }),
            shareables: 0,
            functions: false,
            resumes: false,
            answers_entry: false,
            escaping: None,
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

    fn beside(&mut self, b: Beside) {
        self.w.handle(self.beside.len() as u64);
        self.beside.push(b);
    }
}

/// What crosses beside a lifting's bytes, holding a count until compiled code takes it: a runtime
/// object, or a value the runtime kept with its type.
pub enum Lifted {
    Runtime(Word),
    Carried(Carried),
}

impl Lifted {
    fn release(self, ctx: *mut Ctx) {
        match self {
            Lifted::Runtime(w) => heap::release_object(ctx, w),
            Lifted::Carried(v) => v.release(ctx),
        }
    }
}

/// Bytes being read, and what crosses beside them, each holding a count until compiled code takes
/// it.
pub(crate) struct Lifting {
    r: Reader<Vec<u8>>,
    beside: Vec<Option<Lifted>>,
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

    /// What the position the next `HANDLE` names holds, taken where it is a carried value as
    /// `carried` says, and left where it is not.
    fn take(&mut self, carried: bool) -> Result<Lifted, String> {
        self.r.expect(tag::HANDLE)?;
        let at = self.r.varint()?;
        let slot = usize::try_from(at)
            .ok()
            .and_then(|at| self.beside.get_mut(at))
            .ok_or_else(|| format!("handle {at}, which is not kept"))?;
        match slot {
            None => Err(format!("handle {at}, which was taken")),
            Some(Lifted::Carried(_)) if !carried => Err(format!(
                "handle {at}, a value kept with its type, read as a runtime object"
            )),
            Some(Lifted::Runtime(_)) if carried => Err(format!(
                "handle {at}, a runtime object, read as a value kept with its type"
            )),
            Some(_) => Ok(slot.take().expect("the slot holds what it was matched as")),
        }
    }
}

/// Why a crossing failed, which is Ply's defect: the bytes and the type disagree, or emitted code
/// called a native outside the crossing it belongs to.
#[cold]
#[inline(never)]
pub(crate) fn err_crossing(why: &str) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("a value crossing between compiled code and the runtime: {why}"),
    )
    .primary(Span::DUMMY, "in compiled code")
    .note("this is Ply's fault, not the program's")
}

impl Ctx {
    fn lowering(&mut self) -> Result<&mut Lowering, i64> {
        if !matches!(self.crossings.last(), Some(Crossing::Lowering(_))) {
            return Err(self.fail(err_crossing("a value written with no lowering open")));
        }
        match self.crossings.last_mut() {
            Some(Crossing::Lowering(l)) => Ok(l),
            _ => unreachable!("the innermost crossing is a lowering"),
        }
    }

    /// Ends the innermost lowering, answering it whole; `None` with the context failed where the
    /// innermost crossing is no lowering.
    fn end_lowering(&mut self) -> Option<Lowering> {
        self.lowering().ok()?;
        match self.crossings.pop() {
            Some(Crossing::Lowering(l)) => Some(l),
            _ => unreachable!("the innermost crossing is a lowering"),
        }
    }

    fn lifting(&mut self) -> Result<&mut Lifting, i64> {
        if !matches!(self.crossings.last(), Some(Crossing::Lifting(_))) {
            return Err(self.fail(err_crossing("a value read with no lifting open")));
        }
        match self.crossings.last_mut() {
            Some(Crossing::Lifting(l)) => Ok(l),
            _ => unreachable!("the innermost crossing is a lifting"),
        }
    }

    /// `read` of the innermost lifting, or 0 with the context failed where it errs.
    fn read<T: Default>(&mut self, read: impl FnOnce(&mut Lifting) -> Result<T, String>) -> T {
        let r = match self.lifting() {
            Ok(l) => read(l),
            Err(_) => return T::default(),
        };
        r.unwrap_or_else(|why| {
            self.fail(err_crossing(&why));
            T::default()
        })
    }

    fn write(&mut self, write: impl FnOnce(&mut Lowering)) {
        if let Ok(l) = self.lowering() {
            write(l);
        }
    }

    /// The crossing's bytes compiled code copies out, held until it does.
    pub(crate) fn give(&mut self, bytes: Vec<u8>) -> I64 {
        self.taken = bytes;
        I64(self.taken.len() as i64)
    }

    /// The bytes a machine-word native answers, held until compiled code copies them out.
    pub(crate) fn given(&mut self, bytes: Vec<u8>) -> TakenBytes {
        TakenBytes(self.give(bytes).0)
    }
}

/// The `n` bytes compiled code lends at `p`.
///
/// # Safety
/// `p` holds `n` bytes, or `n` is 0.
pub(crate) unsafe fn lent<'a>(p: Addr, n: I64) -> &'a [u8] {
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
            c.fail(err_crossing(&format!("tag {} is not a value alone", t.0)));
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
        c.fail(err_crossing(&format!("width {} is no width", width.0)));
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
            cx.fail(err_crossing(&format!("{} is no character", c.0)));
        }
    }
}

/// A string, the `n` bytes of UTF-8 at `p`.
pub unsafe extern "C" fn rt_out_str(ctx: *mut Ctx, p: Addr, n: I64) {
    let s = unsafe { lent(p, n) };
    unsafe { &mut *ctx }.write(|l| {
        l.opens_shareable();
        l.w.str(s);
    });
}

/// A bytes value, the `n` bytes at `p`.
pub unsafe extern "C" fn rt_out_bytes(ctx: *mut Ctx, p: Addr, n: I64) {
    let b = unsafe { lent(p, n) };
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
        c.fail(err_crossing(&format!("tag {} counting {}", t.0, n.0)));
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
    let name = unsafe { lent(name, len) };
    unsafe { &mut *ctx }.write(|l| l.w.ctor(name, n.0.max(0) as u64));
}

/// A record field's name, the `n` bytes at `p`, before its value.
pub unsafe extern "C" fn rt_out_name(ctx: *mut Ctx, p: Addr, n: I64) {
    let name = unsafe { lent(p, n) };
    unsafe { &mut *ctx }.write(|l| l.w.name(name));
}

/// A runtime object, its value read as it is written. A handle of the run in an entry's answer is
/// left for the boundary to refuse, held as nothing.
pub unsafe extern "C" fn rt_out_handle(ctx: *mut Ctx, h: Handle) {
    let c = unsafe { &mut *ctx };
    let w = h.word();
    if heap::is_native(w)
        && let Some(escaping) = unsafe { heap::native_escapes(w) }
        && let Ok(l) = c.lowering()
        && l.answers_entry
    {
        l.escaping.get_or_insert(escaping);
        l.beside(Beside::Runtime(Value::Unit));
        return;
    }
    match runtime_value(ctx, w) {
        Ok(v) => c.write(|l| l.beside(Beside::Runtime(v))),
        Err(why) => {
            c.fail(err_crossing(&why));
        }
    }
}

/// A value the runtime keeps rather than reads, with its type, which compiled code lends: what is
/// in the position of an operation's own type parameter, held until the lowering ends.
pub unsafe extern "C" fn rt_out_carried(ctx: *mut Ctx, w: Handed, ty: *const PlyType) {
    let held = unsafe { Carried::kept_of(ctx, w, ty) };
    let c = unsafe { &mut *ctx };
    match c.lowering() {
        Ok(l) => l.beside(Beside::Carried(held)),
        Err(_) => held.release(ctx),
    }
}

/// A function the program wrote, `f`, which compiled code lends: named by the `len` bytes at
/// `name`, or anonymous where `len` is 0; or, an anonymous one holding what may not leave its
/// thread, as a continuation's closure does, one no run but its own can resume.
pub unsafe extern "C" fn rt_out_fun(ctx: *mut Ctx, f: Handed, name: Addr, len: I64) {
    let name = unsafe { lent(name, len) };
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

/// Asked before a string, bytes, list, array or map, `w` its word, which is lent: 1 where the
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
/// little-endian; [`rt_in_high`] answers the other eight.
pub unsafe extern "C" fn rt_in_decimal(ctx: *mut Ctx) -> I64 {
    I64(unsafe { &mut *ctx }.read(|l| {
        l.r.expect(tag::DECIMAL)?;
        let d = u128::from_le_bytes(l.r.array()?);
        Ok(l.keep_high(d))
    }))
}

/// A fixed width, which must be the one at `width` among PLV1's: its low word, its bits sign- or
/// zero-extended as the width's sign says; [`rt_in_high`] answers the word above it.
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
    let want = unsafe { lent(p, n) };
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
    let want = unsafe { lent(p, n) };
    I64(unsafe { &mut *ctx }.read(|l| match &l.name {
        Some(name) => Ok(i64::from(name.as_bytes() == want)),
        None => Err("a constructor's name asked before one was read".into()),
    }))
}

/// A runtime object, given with the count the lifting held of it.
pub unsafe extern "C" fn rt_in_handle(ctx: *mut Ctx) -> Handle {
    Handle::of(unsafe { &mut *ctx }.read(|l| match l.take(false)? {
        Lifted::Runtime(w) => Ok(w),
        Lifted::Carried(_) => unreachable!("a runtime object was taken"),
    }))
}

/// A value the runtime kept with its type, such as a task's answer or a value a channel held, given
/// with the count the lifting held of it.
pub unsafe extern "C" fn rt_in_carried(ctx: *mut Ctx) -> Handed {
    Handed::of(unsafe { &mut *ctx }.read(|l| match l.take(true)? {
        Lifted::Carried(v) => Ok(v.handed().word()),
        Lifted::Runtime(_) => unreachable!("a carried value was taken"),
    }))
}

/// What a descriptor lifts by where a value of its type can hold a function, which crosses out of
/// compiled code and never back: it fails the entry.
pub unsafe extern "C" fn rt_never_lifted(ctx: *mut Ctx) {
    unsafe { &mut *ctx }.fail(err_crossing(
        "a value whose type can hold a function lifted from the runtime: a function crosses out \
         of compiled code and never back",
    ));
}

/// Begins a lowering inside whatever crossing is under way; where `again` is not 0, a value written
/// a second time is written as `AGAIN`.
pub unsafe extern "C" fn rt_lower_open(ctx: *mut Ctx, again: I64) {
    unsafe { &mut *ctx }
        .crossings
        .push(Crossing::Lowering(Lowering::new(again.0 != 0)));
}

/// Ends the innermost lowering, which holds no handle: its bytes' count, the bytes left as the
/// taken bytes.
pub unsafe extern "C" fn rt_lower_close(ctx: *mut Ctx) -> I64 {
    let c = unsafe { &mut *ctx };
    if c.lowering().is_err() {
        return I64(0);
    }
    let Some(Crossing::Lowering(l)) = c.crossings.pop() else {
        unreachable!("the innermost crossing is a lowering");
    };
    if !l.beside.is_empty() {
        c.fail(err_crossing(
            "a value the bytes cannot hold, which no handles were kept beside",
        ));
        released(ctx, l.beside);
        return I64(0);
    }
    c.give(l.w.finish())
}

/// Begins a lifting of the `n` bytes at `p`, which hold one value and no handle.
pub unsafe extern "C" fn rt_lift_open(ctx: *mut Ctx, p: Addr, n: I64) {
    let c = unsafe { &mut *ctx };
    let bytes = unsafe { lent(p, n) }.to_vec();
    c.lift_from(bytes, Vec::new());
}

/// Ends the innermost lifting, every byte of which must have been read.
pub unsafe extern "C" fn rt_lift_close(ctx: *mut Ctx) {
    let c = unsafe { &mut *ctx };
    if c.lifting().is_err() {
        return;
    }
    if let Err(why) = c.close_lifting() {
        c.fail(err_crossing(&why));
    }
}

/// Copies the taken bytes, `n` of them, to `dst`.
pub unsafe extern "C" fn rt_taken(ctx: *mut Ctx, dst: Addr, n: I64) {
    let c = unsafe { &mut *ctx };
    if usize::try_from(n.0).ok() != Some(c.taken.len()) {
        c.fail(err_crossing(&format!(
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
    /// Begins a lifting of `bytes`, `beside` the handles they name, whose counts it takes; one that
    /// does not open fails the context, lets `beside` go and lifts nothing, so the crossings still
    /// nest.
    pub fn lift_from(&mut self, bytes: Vec<u8>, mut beside: Vec<Lifted>) {
        let r = match Reader::new(bytes) {
            Ok(r) => r,
            Err(why) => {
                let ctx: *mut Ctx = self;
                beside.drain(..).for_each(|b| b.release(ctx));
                self.fail(err_crossing(&why));
                Reader::new(codec::FORMAT.to_vec()).expect("the format alone opens")
            }
        };
        self.crossings.push(Crossing::Lifting(Lifting {
            r,
            beside: beside.into_iter().map(Some).collect(),
            name: None,
            high: None,
        }));
    }

    /// Begins a lifting of `bytes`, each of `held` made a runtime object for the handle it is.
    fn lift_holding(&mut self, bytes: Vec<u8>, held: Vec<Value>) {
        let beside = held
            .into_iter()
            .map(|v| Lifted::Runtime(self.heap.bridge(v)))
            .collect();
        self.lift_from(bytes, beside);
    }

    /// Ends the innermost lifting, letting go of what compiled code did not take of its handles;
    /// an error where bytes are left.
    fn close_lifting(&mut self) -> Result<(), String> {
        let Some(Crossing::Lifting(l)) = self.crossings.pop() else {
            unreachable!("the innermost crossing is a lifting");
        };
        let ctx: *mut Ctx = self;
        l.beside.into_iter().flatten().for_each(|b| b.release(ctx));
        l.r.end()
    }
}

/// What a value of a runtime object reads as: a bridge's value, a native object's, or a hold,
/// whose held value is laid out by a type the crossing does not carry, so it reads as holding none.
/// Nothing else the runtime lays out is read here.
pub(crate) fn runtime_value(ctx: *mut Ctx, w: Word) -> Result<Value, String> {
    if let Some(id) = hold_id(w) {
        let kind = ClosureKind::Hold { held: None, id };
        return Ok(Value::Closure(Arc::new(Closure { name: None, kind })));
    }
    if heap::is_native(w) {
        return unsafe { heap::native_value(ctx, w) };
    }
    if !heap::is_bridge(w) {
        return Err("a block that is no runtime handle crossed as one".to_string());
    }
    Ok(unsafe { bridged(obj(w)) }.clone())
}

/// What `write` writes into a lowering of its own, read back as a value, each handle as the value
/// it stands for: a runtime object's own, a carried value lowered through its type.
pub(crate) fn lowered_value(ctx: *mut Ctx, write: impl FnOnce(*mut Ctx)) -> Result<Value, String> {
    let l = lowering(ctx, write)?;
    read_back(ctx, l)
}

/// What `write` writes into a lowering of its own.
fn lowering(ctx: *mut Ctx, write: impl FnOnce(*mut Ctx)) -> Result<Lowering, String> {
    let depth = unsafe { &*ctx }.crossings.len();
    unsafe { &mut *ctx }
        .crossings
        .push(Crossing::Lowering(Lowering::new(false)));
    write(ctx);
    let c = unsafe { &mut *ctx };
    if c.crossings.len() != depth + 1 {
        c.crossings.truncate(depth + 1);
        return Err("a lowering left a crossing of its own open".into());
    }
    let Some(Crossing::Lowering(l)) = c.crossings.pop() else {
        return Err("a lowering ended a crossing it did not begin".into());
    };
    Ok(l)
}

/// A lowering's bytes read as a value, each handle as the value it stands for: a runtime object's
/// own, a lent value lowered through its type.
fn read_back(ctx: *mut Ctx, l: Lowering) -> Result<Value, String> {
    let bytes = l.w.finish();
    let beside = l.beside;
    let read = codec::decode_with(&bytes, &mut |at| match usize::try_from(at)
        .ok()
        .and_then(|at| beside.get(at))
    {
        Some(Beside::Runtime(v)) => Ok(v.clone()),
        Some(Beside::Carried(v)) => v.lower(ctx),
        None => Err(format!("handle {at}, which was not kept")),
    });
    released(ctx, beside);
    read
}

/// The items of the list of arguments `v` is.
fn arguments(v: Value) -> Result<Vec<Value>, String> {
    match &v {
        Value::List(items) => Ok(items.iter().cloned().collect()),
        other => Err(format!(
            "{} where the arguments' list was read",
            other.type_name()
        )),
    }
}

/// The values `args` stand for, each lowered through its type.
pub(crate) fn lowered_values(
    ctx: *mut Ctx,
    args: &[Word],
    types: &[&'static PlyType],
) -> Result<Vec<Value>, String> {
    arguments(lowered_value(ctx, |ctx| {
        if let Ok(l) = unsafe { &mut *ctx }.lowering() {
            l.w.count(tag::LIST, args.len() as u64);
        }
        for (w, ty) in args.iter().zip(types) {
            unsafe { (ty.lower)(ctx, *w) };
        }
    })?)
}

/// `v`'s bytes, and the values they cannot hold in the order their handles name them.
pub(crate) fn encoded(v: &Value) -> (Vec<u8>, Vec<Value>) {
    let mut held = Vec::new();
    let bytes = codec::encode_with(v, &mut |x| {
        held.push(x.clone());
        Ok(held.len() as u64 - 1)
    })
    .expect("a value is written whole but for what is kept beside it");
    (bytes, held)
}

/// `v` lifted through `ty`, each value the bytes cannot hold made a runtime object; `None` with the
/// context failed where it does not cross.
pub(crate) fn lifted(ctx: *mut Ctx, v: &Value, ty: &PlyType) -> Option<Handed> {
    let (bytes, held) = encoded(v);
    let c = unsafe { &mut *ctx };
    let beside = held
        .into_iter()
        .map(|v| Lifted::Runtime(c.heap.bridge(v)))
        .collect();
    lifted_from(ctx, bytes, beside, ty)
}

/// `s` lifted through `string`, the type of `String`; `None` with the context failed where it does
/// not cross.
pub(crate) fn string_lifted(ctx: *mut Ctx, s: &str, string: &PlyType) -> Option<Handed> {
    let mut w = codec::Writer::new();
    w.str(s.as_bytes());
    lifted_from(ctx, w.finish(), Vec::new(), string)
}

/// `bytes` lifted through `ty`, `beside` the handles they name, whose counts it takes; `None` with
/// the context failed where they do not cross.
pub(crate) fn lifted_from(
    ctx: *mut Ctx,
    bytes: Vec<u8>,
    beside: Vec<Lifted>,
    ty: &PlyType,
) -> Option<Handed> {
    let c = unsafe { &mut *ctx };
    let depth = c.crossings.len();
    c.lift_from(bytes, beside);
    let w = unsafe { (ty.lift)(ctx) };
    let c = unsafe { &mut *ctx };
    if c.crossings.len() != depth + 1 {
        c.crossings.truncate(depth + 1);
        c.fail(err_crossing("a lifting left a crossing of its own open"));
    }
    if let Err(why) = c.close_lifting() {
        c.fail(err_crossing(&why));
    }
    (c.failed == 0).then(|| <Handed as crate::native::Answered>::of(w))
}

/// Calls the unit's builtin at `index` in its pool over the arguments the innermost lowering holds,
/// a list of them in order, and ends that lowering: what a builder the previous release compiled
/// still calls this way, `panic` and `to_int`, each read from its arguments' bytes. Where `to_int`
/// answers, a lifting of its answer is open for compiled code to read and end; `panic` raises.
pub unsafe extern "C" fn rt_builtin_encoded(ctx: *mut Ctx, index: I64) {
    let c = unsafe { &mut *ctx };
    let Some(l) = c.end_lowering() else {
        return;
    };
    let Some(&b) = usize::try_from(index.0)
        .ok()
        .and_then(|i| c.tables.builtins.get(i))
    else {
        c.fail(err_crossing(&format!(
            "builtin {} of a pool of {}",
            index.0,
            c.tables.builtins.len()
        )));
        return;
    };
    let bytes = l.w.finish();
    released(ctx, l.beside);
    let c = unsafe { &mut *ctx };
    if let Err(why) = encoded_called(c, b, &bytes) {
        c.fail(err_crossing(&why));
    }
}

/// `b` over the arguments' list `bytes` hold, as [`rt_builtin_encoded`] calls it.
fn encoded_called(c: &mut Ctx, b: Builtin, bytes: &[u8]) -> Result<(), String> {
    let mut r = Reader::new(bytes)?;
    r.expect(tag::LIST)?;
    match (b, r.count()?) {
        (Builtin::Panic, 1) => {
            r.expect(tag::STR)?;
            let message = r.str()?.to_string();
            let d = Diagnostic::error(codes::RUNTIME_ERROR, format!("panic: {message}"))
                .primary(c.site(), "`panic` called here");
            c.pend(d, message);
            Ok(())
        }
        (Builtin::ToInt, 2) => {
            r.expect(tag::INT)?;
            r.int()?;
            let int = match r.byte()? {
                tag::INT => Some(r.int()?),
                tag::FIXED => r.fixed()?.to_i128().and_then(|v| i64::try_from(v).ok()),
                t => return Err(format!("`to_int` of a value tagged {t}")),
            };
            let mut w = codec::Writer::new();
            match int {
                Some(n) => {
                    w.ctor(b"Some", 1);
                    w.int(n);
                }
                None => w.ctor(b"None", 0),
            }
            c.lift_from(w.finish(), Vec::new());
            Ok(())
        }
        (b, _) => Err(format!(
            "`{}` over its arguments' encoding, which only `panic` and `to_int` are",
            b.name()
        )),
    }
}

impl Ctx {
    /// Opens what an entry through `R#entered` runs between: a lowering for its answer, and over it
    /// a lifting of `bytes`, the arguments' list.
    pub(crate) fn open_entry(&mut self, bytes: Vec<u8>) {
        self.crossings.push(Crossing::Lowering(Lowering {
            answers_entry: true,
            ..Lowering::new(false)
        }));
        self.lift_holding(bytes, Vec::new());
    }
}

/// What an entry's `R#entered` wrote of its root's answer, kept as the bytes it wrote.
pub(crate) enum EntryBytes {
    /// The answer's PLV1, untouched.
    Bytes(Vec<u8>),
    /// The answer holds what the bytes cannot, which only its own run can hold.
    Holding,
    /// The answer holds a function the program wrote, which only its own run can hold.
    Function,
    /// The answer holds a continuation or a handle of its run, the first written, which the
    /// boundary refuses.
    Escaping(Escapee),
}

/// Ends the lowering [`Ctx::open_entry`] opened: the answer kept as the bytes the entry wrote
/// where they hold all of it.
pub(crate) fn close_entry(ctx: *mut Ctx) -> Result<EntryBytes, String> {
    let l = entry_lowering(ctx)?;
    let held = !l.beside.is_empty();
    let bytes = l.w.finish();
    released(ctx, l.beside);
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

/// Reads one value, answering whether it reaches a continuation, with the way to it in `route`,
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

/// Ends the lowering [`Ctx::open_entry`] opened and answers it; an error where the entry ended that
/// lowering or left another crossing open.
fn entry_lowering(ctx: *mut Ctx) -> Result<Lowering, String> {
    let c = unsafe { &mut *ctx };
    let open = c.crossings.len();
    if open != 1 {
        c.crossings.clear();
        return Err(match open {
            0 => "the entry ended the lowering its answer was to be written into".into(),
            _ => format!(
                "the entry left {} crossings open past its answer's",
                open - 1
            ),
        });
    }
    match c.crossings.pop() {
        Some(Crossing::Lowering(l)) => Ok(l),
        _ => Err("the entry ended the lowering its answer was to be written into".into()),
    }
}
