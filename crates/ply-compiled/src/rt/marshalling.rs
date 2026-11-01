use super::*;
use crate::emitted::{Block, Carried, Handed, Handle, PlyType};
use crate::native::{Address, F64, I64, TakenBytes};
use ply_core::codec::{self, Reader, Writer, fun, tag};
use ply_core::{EscapedHandle, escape};

pub(crate) enum Marshalling {
    Lowering(Lowering),
    Decoding(Decoding),
}

pub(crate) struct Lowering {
    w: Writer,
    handles: u64,
    sharing: Option<SharedValueIndex>,
    heap_value_count: u64,
    functions: bool,
    resumes: bool,
    writes_entry_result: bool,
    escaping: Option<escape::Handle>,
    shown: bool,
}

struct SharedValueIndex {
    numbered: HashMap<Word, u64>,
    next: Option<Word>,
}

impl Lowering {
    fn new(again: bool) -> Lowering {
        Lowering {
            w: Writer::new(),
            handles: 0,
            sharing: again.then(|| SharedValueIndex {
                numbered: HashMap::new(),
                next: None,
            }),
            heap_value_count: 0,
            functions: false,
            resumes: false,
            writes_entry_result: false,
            escaping: None,
            shown: false,
        }
    }

    fn shown() -> Lowering {
        Lowering {
            shown: true,
            ..Lowering::new(true)
        }
    }

    fn begin_shared_value(&mut self) {
        let n = self.heap_value_count;
        self.heap_value_count += 1;
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

pub(crate) struct Decoding {
    r: Reader<Vec<u8>>,
    name: Option<Symbol>,
    high: Option<i64>,
}

impl Decoding {
    fn store_high_return_low(&mut self, bits: u128) -> i64 {
        self.high = Some((bits >> 64) as u64 as i64);
        bits as u64 as i64
    }
}

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

impl Context {
    fn lowering(&mut self) -> Result<&mut Lowering, i64> {
        if !matches!(self.marshallings.last(), Some(Marshalling::Lowering(_))) {
            return Err(self.fail(err_marshalling("a value written with no lowering open")));
        }
        match self.marshallings.last_mut() {
            Some(Marshalling::Lowering(l)) => Ok(l),
            _ => unreachable!("the innermost marshalling is a lowering"),
        }
    }

    fn showing(&self) -> bool {
        matches!(self.marshallings.last(), Some(Marshalling::Lowering(l)) if l.shown)
    }

    fn lifting(&mut self) -> Result<&mut Decoding, i64> {
        if !matches!(self.marshallings.last(), Some(Marshalling::Decoding(_))) {
            return Err(self.fail(err_marshalling("a value read with no lifting open")));
        }
        match self.marshallings.last_mut() {
            Some(Marshalling::Decoding(l)) => Ok(l),
            _ => unreachable!("the innermost marshalling is a lifting"),
        }
    }

    fn read<T: Default>(&mut self, read: impl FnOnce(&mut Decoding) -> Result<T, String>) -> T {
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

    pub(crate) fn give(&mut self, bytes: Vec<u8>) -> I64 {
        self.taken = bytes;
        I64(self.taken.len() as i64)
    }

    pub(crate) fn given(&mut self, bytes: Vec<u8>) -> TakenBytes {
        TakenBytes(self.give(bytes).0)
    }
}

pub(crate) unsafe fn borrowed<'a>(p: Address, n: I64) -> &'a [u8] {
    if n.0 <= 0 {
        return &[];
    }
    unsafe { std::slice::from_raw_parts(p.0, n.0 as usize) }
}

pub unsafe extern "C" fn rt_out_tag(context: *mut Context, t: I64) {
    let c = unsafe { &mut *context };
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

pub unsafe extern "C" fn rt_out_int(context: *mut Context, v: I64) {
    unsafe { &mut *context }.write(|l| l.w.int(v.0));
}

pub unsafe extern "C" fn rt_out_float(context: *mut Context, bits: F64) {
    let canonical = ply_core::float::bits(f64::from_bits(bits.0));
    unsafe { &mut *context }.write(|l| l.w.float(canonical));
}

pub unsafe extern "C" fn rt_out_decimal(context: *mut Context, lo: I64, hi: I64) {
    let mut d = [0u8; 16];
    d[..8].copy_from_slice(&lo.0.to_le_bytes());
    d[8..].copy_from_slice(&hi.0.to_le_bytes());
    unsafe { &mut *context }.write(|l| l.w.decimal(d));
}

pub unsafe extern "C" fn rt_out_fixed(context: *mut Context, width: I64, lo: I64, hi: I64) {
    let c = unsafe { &mut *context };
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

pub unsafe extern "C" fn rt_out_char(context: *mut Context, c: I64) {
    let cx = unsafe { &mut *context };
    match u32::try_from(c.0).ok().and_then(char::from_u32) {
        Some(ch) => cx.write(|l| l.w.char(u32::from(ch))),
        None => {
            cx.fail(err_marshalling(&format!("{} is no character", c.0)));
        }
    }
}

pub unsafe extern "C" fn rt_out_str(context: *mut Context, p: Address, n: I64) {
    let s = unsafe { borrowed(p, n) };
    unsafe { &mut *context }.write(|l| {
        l.begin_shared_value();
        l.w.str(s);
    });
}

pub unsafe extern "C" fn rt_out_bytes(context: *mut Context, p: Address, n: I64) {
    let b = unsafe { borrowed(p, n) };
    unsafe { &mut *context }.write(|l| {
        l.begin_shared_value();
        l.w.bytes(b);
    });
}

pub unsafe extern "C" fn rt_out_count(context: *mut Context, t: I64, n: I64) {
    let c = unsafe { &mut *context };
    let (Ok(t @ (tag::LIST | tag::ARRAY | tag::MAP | tag::RECORD)), Ok(n)) =
        (u8::try_from(t.0), u64::try_from(n.0))
    else {
        c.fail(err_marshalling(&format!("tag {} counting {}", t.0, n.0)));
        return;
    };
    c.write(|l| {
        if codec::shareable(t) {
            l.begin_shared_value();
        }
        l.w.count(t, n);
    });
}

pub unsafe extern "C" fn rt_out_ctor(context: *mut Context, name: Address, len: I64, n: I64) {
    let name = unsafe { borrowed(name, len) };
    unsafe { &mut *context }.write(|l| l.w.ctor(name, n.0.max(0) as u64));
}

pub unsafe extern "C" fn rt_out_name(context: *mut Context, p: Address, n: I64) {
    let name = unsafe { borrowed(p, n) };
    unsafe { &mut *context }.write(|l| l.w.name(name));
}

pub unsafe extern "C" fn rt_out_handle(context: *mut Context, h: Handle) {
    let c = unsafe { &mut *context };
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
        && l.writes_entry_result
    {
        l.escaping.get_or_insert(escaping);
    }
    c.write(Lowering::handle);
}

pub unsafe extern "C" fn rt_out_carried(context: *mut Context, w: Handed, ty: *const PlyType) {
    if unsafe { &*context }.showing() {
        unsafe { ((*ty).lower)(context, w.word()) };
        return;
    }
    unsafe { &mut *context }.write(Lowering::handle);
}

pub unsafe extern "C" fn rt_out_fun(context: *mut Context, f: Handed, name: Address, len: I64) {
    let name = unsafe { borrowed(name, len) };
    let leaves = !name.is_empty() || unsafe { Block::of(f.word()).share(context) };
    unsafe { &mut *context }.write(|l| {
        if !leaves {
            l.resumes = true;
            l.w.fun(fun::RESUMPTION);
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

pub unsafe extern "C" fn rt_out_again(context: *mut Context, w: Handed) -> I64 {
    let c = unsafe { &mut *context };
    let Ok(l) = c.lowering() else {
        return I64(0);
    };
    let Some(s) = &mut l.sharing else {
        return I64(0);
    };
    let w = w.word();
    if heap::is_object(w) && unsafe { heap::count_of(heap::object(w)) } == heap::IMMORTAL {
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

pub unsafe extern "C" fn rt_in_tag(context: *mut Context) -> I64 {
    I64(unsafe { &mut *context }.read(|l| match l.r.byte()? {
        t @ (tag::UNIT | tag::FALSE | tag::TRUE) => Ok(i64::from(t)),
        t => Err(format!(
            "a value tagged {t} where one that is its tag alone was read"
        )),
    }))
}

pub unsafe extern "C" fn rt_in_int(context: *mut Context) -> I64 {
    I64(unsafe { &mut *context }.read(|l| {
        l.r.expect(tag::INT)?;
        l.r.int()
    }))
}

pub unsafe extern "C" fn rt_in_float(context: *mut Context) -> F64 {
    F64(unsafe { &mut *context }.read(|l| {
        l.r.expect(tag::FLOAT)?;
        Ok(u64::from_le_bytes(l.r.array()?))
    }))
}

pub unsafe extern "C" fn rt_in_decimal(context: *mut Context) -> I64 {
    I64(unsafe { &mut *context }.read(|l| {
        l.r.expect(tag::DECIMAL)?;
        let d = u128::from_le_bytes(l.r.array()?);
        Ok(l.store_high_return_low(d))
    }))
}

pub unsafe extern "C" fn rt_in_fixed(context: *mut Context, width: I64) -> I64 {
    I64(unsafe { &mut *context }.read(|l| {
        l.r.expect(tag::FIXED)?;
        let f = l.r.fixed()?;
        let at = ply_core::INT_TYPES.iter().position(|t| *t == f.ty);
        if at != usize::try_from(width.0).ok() {
            return Err(format!("a {:?} where width {} was read", f.ty, width.0));
        }
        Ok(l.store_high_return_low(f.bits()))
    }))
}

pub unsafe extern "C" fn rt_in_high(context: *mut Context) -> I64 {
    I64(unsafe { &mut *context }.read(|l| {
        l.high
            .take()
            .ok_or_else(|| "a high word asked where no wide value was read".into())
    }))
}

pub unsafe extern "C" fn rt_in_char(context: *mut Context) -> I64 {
    I64(unsafe { &mut *context }.read(|l| {
        l.r.expect(tag::CHAR)?;
        Ok(i64::from(u32::from(l.r.char()?)))
    }))
}

pub unsafe extern "C" fn rt_in_str(context: *mut Context) -> I64 {
    let c = unsafe { &mut *context };
    let s: Vec<u8> = c.read(|l| {
        l.r.expect(tag::STR)?;
        Ok(l.r.str()?.as_bytes().to_vec())
    });
    c.give(s)
}

pub unsafe extern "C" fn rt_in_bytes(context: *mut Context) -> I64 {
    let c = unsafe { &mut *context };
    let b: Vec<u8> = c.read(|l| {
        l.r.expect(tag::BYTES)?;
        Ok(l.r.blob()?.to_vec())
    });
    c.give(b)
}

pub unsafe extern "C" fn rt_in_count(context: *mut Context, t: I64) -> I64 {
    I64(unsafe { &mut *context }.read(|l| {
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

pub unsafe extern "C" fn rt_in_field(context: *mut Context, p: Address, n: I64) {
    let want = unsafe { borrowed(p, n) };
    unsafe { &mut *context }.read(|l| {
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

pub unsafe extern "C" fn rt_in_name_is(context: *mut Context, p: Address, n: I64) -> I64 {
    let want = unsafe { borrowed(p, n) };
    I64(unsafe { &mut *context }.read(|l| match &l.name {
        Some(name) => Ok(i64::from(name.as_bytes() == want)),
        None => Err("a constructor's name asked before one was read".into()),
    }))
}

pub unsafe extern "C" fn rt_never_lifted(context: *mut Context) {
    unsafe { &mut *context }.fail(err_marshalling(
        "a value lifted through a descriptor that lifts none: the runtime lifts only a `String`",
    ));
}

pub unsafe extern "C" fn rt_never_shown(context: *mut Context) {
    unsafe { &mut *context }.fail(err_marshalling(
        "a value shown through a descriptor that does not lower it",
    ));
}

pub unsafe extern "C" fn rt_shown_open(context: *mut Context) {
    unsafe { &mut *context }
        .marshallings
        .push(Marshalling::Lowering(Lowering::shown()));
}

pub unsafe extern "C" fn rt_lower_close(context: *mut Context) -> I64 {
    let c = unsafe { &mut *context };
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

pub unsafe extern "C" fn rt_lift_close(context: *mut Context) {
    let c = unsafe { &mut *context };
    if c.lifting().is_err() {
        return;
    }
    if let Err(reason) = c.close_decoding() {
        c.fail(err_marshalling(&reason));
    }
}

pub unsafe extern "C" fn rt_taken(context: *mut Context, dst: Address, n: I64) {
    let c = unsafe { &mut *context };
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

impl Context {
    pub fn open_decoding(&mut self, bytes: Vec<u8>) {
        let r = match Reader::new(bytes) {
            Ok(r) => r,
            Err(reason) => {
                self.fail(err_marshalling(&reason));
                Reader::new(codec::FORMAT.to_vec()).expect("the format alone opens")
            }
        };
        self.marshallings.push(Marshalling::Decoding(Decoding {
            r,
            name: None,
            high: None,
        }));
    }

    fn close_decoding(&mut self) -> Result<(), String> {
        let Some(Marshalling::Decoding(l)) = self.marshallings.pop() else {
            unreachable!("the innermost marshalling is a lifting");
        };
        l.r.end()
    }
}

pub(crate) fn shown(context: *mut Context, v: &Carried) -> Result<Vec<u8>, String> {
    let l = lowering(context, Lowering::shown(), |context| unsafe {
        (v.ty().lower)(context, v.word());
    })?;
    Ok(l.w.finish())
}

fn lowering(
    context: *mut Context,
    opened: Lowering,
    write: impl FnOnce(*mut Context),
) -> Result<Lowering, String> {
    let depth = unsafe { &*context }.marshallings.len();
    unsafe { &mut *context }
        .marshallings
        .push(Marshalling::Lowering(opened));
    write(context);
    let c = unsafe { &mut *context };
    if c.marshallings.len() != depth + 1 {
        c.marshallings.truncate(depth + 1);
        return Err("a lowering left a marshalling of its own open".into());
    }
    let Some(Marshalling::Lowering(l)) = c.marshallings.pop() else {
        return Err("a lowering ended a marshalling it did not begin".into());
    };
    Ok(l)
}

pub(crate) fn decode_string(context: *mut Context, s: &str, string: &PlyType) -> Option<Handed> {
    let mut w = codec::Writer::new();
    w.str(s.as_bytes());
    decode_bytes(context, w.finish(), string)
}

fn decode_bytes(context: *mut Context, bytes: Vec<u8>, ty: &PlyType) -> Option<Handed> {
    let c = unsafe { &mut *context };
    let depth = c.marshallings.len();
    c.open_decoding(bytes);
    let w = unsafe { (ty.lift)(context) };
    let c = unsafe { &mut *context };
    if c.marshallings.len() != depth + 1 {
        c.marshallings.truncate(depth + 1);
        c.fail(err_marshalling(
            "a lifting left a marshalling of its own open",
        ));
    }
    if let Err(reason) = c.close_decoding() {
        c.fail(err_marshalling(&reason));
    }
    (c.failed == 0).then(|| <Handed as crate::native::Returned>::of(w))
}

impl Context {
    pub(crate) fn open_entry_call(&mut self, bytes: Vec<u8>) {
        self.marshallings.push(Marshalling::Lowering(Lowering {
            writes_entry_result: true,
            ..Lowering::new(false)
        }));
        self.open_decoding(bytes);
    }
}

pub(crate) enum EntryBytes {
    Bytes(Vec<u8>),
    RetainsHandle,
    Function,
    EscapingHandle(EscapedHandle),
}

pub(crate) fn close_entry_call(context: *mut Context) -> Result<EntryBytes, String> {
    let l = pop_entry_result(context)?;
    let held = l.handles > 0;
    let bytes = l.w.finish();
    if let Some(handle) = l.escaping {
        return Ok(EntryBytes::EscapingHandle(EscapedHandle {
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
        return Ok(EntryBytes::EscapingHandle(EscapedHandle {
            handle: escape::Handle::Continuation,
            route,
        }));
    }
    if held || l.resumes {
        return Ok(EntryBytes::RetainsHandle);
    }
    Ok(EntryBytes::Bytes(bytes))
}

pub fn continuation_in(bytes: &[u8]) -> Result<Option<Vec<String>>, String> {
    let mut r = Reader::new(bytes)?;
    let mut route = Vec::new();
    if !walked(&mut r, &mut route)? {
        return Ok(None);
    }
    route.reverse();
    Ok(Some(route))
}

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
        tag::CHAR | tag::HANDLE | tag::BACK_REFERENCE => {
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
            fun::RESUMPTION => return Ok(true),
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

fn pop_entry_result(context: *mut Context) -> Result<Lowering, String> {
    let c = unsafe { &mut *context };
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
