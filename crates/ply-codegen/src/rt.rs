//! The runtime compiled code calls into: every helper a body reaches for what it does not lower
//! inline, over the words [`crate::heap`] lays out, and the context an entry runs in.
//!
//! Ownership follows Perceus as the code generator marks it: a helper that *takes* an argument
//! owns it from then on and releases it when it is done or keeps it inside what it builds; a
//! helper that *reads* one leaves its count alone; a helper answers a word its caller owns.
//! Nothing here recurses over a value: a count reaching zero dismantles with a worklist.

use crate::heap::{
    self, CLOSURE_CAPTURES, CLOSURE_CODE, Heap, KIND_BRIDGE, KIND_CLOSURE, KIND_CTOR, KIND_LIST,
    KIND_MAP, KIND_RECORD, Layouts, Word, bridged, is_unique, obj, set_word, word_at,
};
use crate::jit::Entry;
use ply_eval::{Builtin, Closure, ClosureKind, Step, Value, values_equal};
use ply_span::{Diagnostic, Span, Symbol, codes};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

pub struct Tables {
    /// The constant pool as values, for a literal rebuilt per evaluation.
    pub consts: Vec<Value>,
    /// The same constants as immortal words, which a folded literal is an immediate of.
    pub const_words: Vec<Word>,
    pub layouts: Layouts,
    /// Every field name a compiled body reads by name, so that a field access is an index rather
    /// than a `Symbol` rebuilt per evaluation.
    pub fields: Vec<Symbol>,
    /// Every builtin a compiled body may call.
    pub builtins: Vec<Builtin>,
    /// Every compiled function by index, as its finalized address: what a native closure's
    /// `code` is read from when the closure is built, since no address exists until the module
    /// is finalized and every closure is built after that.
    pub functions: Vec<usize>,
    /// What each pure nullary function evaluated to, by the index above, once it has: the
    /// machine's memo (`ply_eval::memo`) for compiled code, held as immortal words.
    pub memo: RefCell<Vec<Option<Word>>>,
    /// Owns the constant pool's and the memo's objects for as long as the unit lives.
    pub immortals: RefCell<Heap>,
}

impl Tables {
    /// Whether the constant pool holds a value that must never sit in a table outliving the call
    /// that made it.
    pub fn retains_a_handle(&self) -> Option<&'static str> {
        self.consts.iter().find_map(holds_a_handle)
    }
}

/// The same question of one value, to its leaves.
pub(crate) fn holds_a_handle(value: &Value) -> Option<&'static str> {
    match value {
        Value::Secret(_) => Some("a Secret"),
        Value::Cell(_) => Some("a Cell"),
        Value::Task(_) => Some("a Task"),
        Value::Continuation(_) => Some("a Continuation"),
        Value::Closure(_) => Some("a Closure"),
        Value::List(items) => items.iter().find_map(holds_a_handle),
        Value::Map(entries) => entries
            .iter()
            .find_map(|(k, v)| holds_a_handle(k).or_else(|| holds_a_handle(v))),
        Value::Record(fields) => fields.values().find_map(holds_a_handle),
        Value::Ctor { args, .. } => args.iter().find_map(holds_a_handle),
        Value::Int(_)
        | Value::Bool(_)
        | Value::Float(_)
        | Value::Decimal(_)
        | Value::Str(_)
        | Value::Bytes(_)
        | Value::Unit => None,
    }
}

/// The failure a compiled function reports by, and the fuel it spends.
pub const FAILED_OUT_OF_FUEL: i64 = 2;

#[repr(C)]
pub struct Ctx {
    pub failed: i64,
    /// Nested native calls still allowed, counted down on entry to a compiled function and back up
    /// on its normal return.
    pub fuel: i64,
    pub heap: Heap,
    /// The objects the entry that just finished allocated, kept because [`Ctx::end`] clears the
    /// log and the number is otherwise gone.
    last_entry: usize,
    unclosed_entries: u64,
    pub tables: Rc<Tables>,
    /// The arena `ply_eval::builtins::call` insists on.
    cells: ply_eval::TaskRegions,
    /// `(allocations, regions_opened)` when the arena above was built, so [`Ctx::touched_cells`]
    /// compares against what an empty one costs rather than against zero.
    cells_baseline: (u64, u64),
    /// Why the last entry failed.
    pub diagnostic: Option<Diagnostic>,
    pub builtin_calls: u64,
}

impl Ctx {
    pub fn new(tables: Rc<Tables>) -> Ctx {
        let cells = ply_eval::TaskRegions::new();
        let stats = cells.arena().stats();
        Ctx {
            failed: 0,
            fuel: 0,
            heap: Heap::new(),
            last_entry: 0,
            unclosed_entries: 0,
            tables,
            cells,
            cells_baseline: (stats.allocations, stats.regions_opened),
            diagnostic: None,
            builtin_calls: 0,
        }
    }

    /// Between calls, and only between calls.
    pub fn begin(&mut self, fuel: i64) {
        self.failed = 0;
        self.fuel = fuel;
        self.diagnostic = None;
        // Every path out of an entry calls `end`, so this is one comparison against an empty log.
        if self.heap.allocated() != 0 {
            self.unclosed_entries += 1;
            self.end();
        }
    }

    /// The other end of [`Ctx::begin`]: the entry gives back what it used.
    pub fn end(&mut self) {
        self.last_entry = self.heap.allocated();
        self.heap.end();
    }

    /// How many objects the entry that just finished allocated.
    pub fn allocated_by_entry(&self) -> usize {
        if self.heap.allocated() == 0 {
            self.last_entry
        } else {
            self.heap.allocated()
        }
    }

    /// Entries that reached [`Ctx::begin`] without their predecessor having closed itself.
    pub fn unclosed_entries(&self) -> u64 {
        self.unclosed_entries
    }

    /// Whether a builtin allocated a cell in the private arena.
    pub fn touched_cells(&self) -> bool {
        let stats = self.cells.arena().stats();
        (stats.allocations, stats.regions_opened) != self.cells_baseline
    }

    fn fail(&mut self, d: Diagnostic) -> i64 {
        self.fail_with(1, d)
    }

    fn fail_with(&mut self, code: i64, d: Diagnostic) -> i64 {
        if self.failed == 0 {
            self.failed = code;
        }
        if self.diagnostic.is_none() {
            self.diagnostic = Some(d);
        }
        0
    }

    pub fn take_failure(&mut self) -> Option<Diagnostic> {
        self.diagnostic.take()
    }

    /// The interpreter value a word denotes, for a builtin or an error message.
    fn value(&self, w: Word) -> Value {
        Heap::to_value(&self.tables.layouts, w)
    }

    fn word(&mut self, v: &Value) -> Word {
        let tables = Rc::clone(&self.tables);
        self.heap.to_word(&tables.layouts, v)
    }

    fn type_name(&self, w: Word) -> &'static str {
        self.value(w).type_name()
    }
}

/// A failure inside compiled code, for the spike's own reporting.
fn error(message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(codes::RUNTIME_ERROR, message.into()).primary(Span::DUMMY, "in compiled code")
}

fn args_of<'a>(ptr: *const i64, n: i64) -> &'a [Word] {
    unsafe { std::slice::from_raw_parts(ptr, n as usize) }
}

/// The arguments a builtin consumes, as the values it reads: each word released once copied.
fn values_taken(ctx: &mut Ctx, args: &[Word]) -> Vec<Value> {
    let mut out = ply_eval::argv::take(args.len());
    for w in args {
        out.push(ctx.value(*w));
        heap::dec(*w);
    }
    out
}

/// Perceus's `dup`: the same word, held once more.
pub unsafe extern "C" fn rt_dup(_ctx: *mut Ctx, w: i64) -> i64 {
    heap::inc(w);
    w
}

/// Perceus's `drop`: one holder fewer.
pub unsafe extern "C" fn rt_dec(_ctx: *mut Ctx, w: i64) {
    heap::dec(w);
}

pub unsafe extern "C" fn rt_box_int(ctx: *mut Ctx, v: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    ctx.heap.boxed_int(v)
}

pub unsafe extern "C" fn rt_box_bool(_ctx: *mut Ctx, v: i64) -> i64 {
    heap::bool(v != 0)
}

pub unsafe extern "C" fn rt_unbox_int(ctx: *mut Ctx, w: i64) -> i64 {
    match heap::as_int(w) {
        Some(i) => i,
        None => {
            let ctx = unsafe { &mut *ctx };
            let d = error(format!("an `Int` operation on a {}", ctx.type_name(w)));
            ctx.fail(d)
        }
    }
}

pub unsafe extern "C" fn rt_unbox_bool(ctx: *mut Ctx, w: i64) -> i64 {
    match heap::as_bool(w) {
        Some(b) => i64::from(b),
        None => {
            let ctx = unsafe { &mut *ctx };
            let d = error(format!("a condition of type {}", ctx.type_name(w)));
            ctx.fail(d)
        }
    }
}

/// The prologue's refusal: this call would nest past the budget the machine handed the entry.
pub unsafe extern "C" fn rt_no_fuel(ctx: *mut Ctx) {
    let ctx = unsafe { &mut *ctx };
    let d = error("this call would nest past the machine's own bound on nested calls");
    ctx.fail_with(FAILED_OUT_OF_FUEL, d);
}

/// Whichever of `checked_mul`, `checked_div` and `checked_rem` the operator was.
pub unsafe extern "C" fn rt_arith(ctx: *mut Ctx, op: i64, a: i64, b: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let (result, what) = match op {
        0 => (a.checked_mul(b), "multiplication"),
        1 if b == 0 => (None, "division"),
        1 => (a.checked_div(b), "division"),
        2 if b == 0 => (None, "remainder"),
        _ => (a.checked_rem(b), "remainder"),
    };
    match result {
        Some(n) => n,
        None => {
            let d = error(format!("{what} of {a} and {b} is not representable"));
            ctx.fail(d)
        }
    }
}

/// A literal, built the way the interpreter builds it: a fresh allocation per evaluation.
pub unsafe extern "C" fn rt_lit(ctx: *mut Ctx, index: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let tables = Rc::clone(&ctx.tables);
    let value = &tables.consts[index as usize];
    let rebuilt = match value {
        Value::Bytes(b) => Value::bytes(b.as_ref()),
        Value::Str(s) => Value::str(s.as_ref()),
        Value::Ctor { name, args } => Value::ctor(name.clone(), args.as_ref().clone()),
        other => other.clone(),
    };
    ctx.word(&rebuilt)
}

/// A `match` whose arms did not cover the value.
pub unsafe extern "C" fn rt_no_match(ctx: *mut Ctx) {
    let ctx = unsafe { &mut *ctx };
    let d = error("no arm of this `match` matched");
    ctx.fail(d);
}

pub unsafe extern "C" fn rt_overflow(ctx: *mut Ctx, what: i64) {
    let ctx = unsafe { &mut *ctx };
    let name = match what {
        0 => "addition",
        1 => "subtraction",
        _ => "negation",
    };
    let d = error(format!("this {name} overflowed"));
    ctx.fail(d);
}

/// `==` and `!=` on anything that is not a pair of `Int`s or a pair of `Bool`s: two immediates
/// compare as themselves, and anything else is answered by the evaluator's own comparison so the
/// two cannot disagree. Reads both.
pub unsafe extern "C" fn rt_equal(ctx: *mut Ctx, a: i64, b: i64) -> i64 {
    if heap::is_imm(a) && heap::is_imm(b) {
        return i64::from(a == b);
    }
    let ctx = unsafe { &mut *ctx };
    let (l, r) = (ctx.value(a), ctx.value(b));
    match values_equal(&l, &r, Span::DUMMY) {
        Ok(eq) => i64::from(eq),
        Err(d) => ctx.fail(d),
    }
}

/// `++`, which is `String` concatenation only, built from the same two `Value::as_str` calls
/// `interp::strict_binary` uses so a non-`Str` operand raises the identical error. Reads both.
pub unsafe extern "C" fn rt_concat(ctx: *mut Ctx, a: i64, b: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let (l, r) = (ctx.value(a), ctx.value(b));
    let joined = match (l.as_str(Span::DUMMY, "`++`"), r.as_str(Span::DUMMY, "`++`")) {
        (Ok(x), Ok(y)) => format!("{x}{y}"),
        (Err(d), _) | (_, Err(d)) => return ctx.fail(d),
    };
    ctx.word(&Value::str(joined))
}

/// A builtin over taken arguments: the few a front end leans on are answered over the words
/// themselves, and the rest through the interpreter's own implementation over the values the
/// words denote.
pub unsafe extern "C" fn rt_builtin(ctx: *mut Ctx, index: i64, args: *const i64, n: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let b = ctx.tables.builtins[index as usize];
    let args = args_of(args, n);
    ctx.builtin_calls += 1;
    if let Some(w) = native_builtin(ctx, b, args) {
        return w;
    }
    let values = values_taken(ctx, args);
    match ply_eval::builtins::call(b, values, ctx.cells.arena_mut(), Span::DUMMY) {
        Ok(Step::Done(v)) => ctx.word(&v),
        // Unreachable: `jit::admissible_builtin` refuses every higher-order builtin at compile
        // time, because answering `Step::Apply` here would need user code run from inside a native
        // frame.
        Ok(_) => {
            let d = error(format!(
                "`{}` suspended, which the fragment excludes",
                b.name()
            ));
            ctx.fail(d)
        }
        Err(d) => ctx.fail(d),
    }
}

/// The builtins answered over words, when their arguments have the native kinds; `None` hands the
/// call to the interpreter's implementation.
fn native_builtin(ctx: &mut Ctx, b: Builtin, args: &[Word]) -> Option<Word> {
    match (b, args) {
        (Builtin::Len, [xs]) if heap::kind(*xs) == KIND_LIST => {
            let n = unsafe { (*obj(*xs)).len } as i64;
            heap::dec(*xs);
            Some(heap::imm(n))
        }
        (Builtin::Push, [xs, x]) if heap::kind(*xs) == KIND_LIST => {
            let o = obj(*xs);
            let (len, cap) = unsafe { ((*o).len, (*o).layout) };
            if is_unique(*xs) && len < cap {
                unsafe {
                    set_word(o, len as usize, *x);
                    (*o).len = len + 1;
                }
                return Some(*xs);
            }
            let room = if len < cap { cap } else { (len * 2).max(4) };
            let out = ctx.heap.alloc_list(room);
            unsafe {
                for i in 0..len as usize {
                    let w = word_at(o, i);
                    heap::inc(w);
                    set_word(out, i, w);
                }
                set_word(out, len as usize, *x);
                (*out).len = len + 1;
            }
            heap::dec(*xs);
            Some(out as Word)
        }
        (Builtin::ListAt, [xs, i]) if heap::kind(*xs) == KIND_LIST => {
            let o = obj(*xs);
            let index = heap::as_int(*i)?;
            let len = unsafe { (*o).len } as i64;
            let some = ctx.tables.layouts.some?;
            let none = ctx.tables.layouts.none?;
            let answer = if (0..len).contains(&index) {
                let item = unsafe { word_at(o, index as usize) };
                heap::inc(item);
                let c = ctx.heap.alloc(KIND_CTOR, 0, 1, some);
                unsafe { set_word(c, 0, item) };
                c as Word
            } else {
                ctx.heap.alloc(KIND_CTOR, 0, 0, none) as Word
            };
            heap::dec(*xs);
            Some(answer)
        }
        (Builtin::Range, [lo, hi]) => {
            let (a, b) = (heap::as_int(*lo)?, heap::as_int(*hi)?);
            let n = b.saturating_sub(a).clamp(0, 1 << 20);
            if b - a > (1 << 20) {
                return None;
            }
            let out = ctx.heap.alloc_list((n as u32).max(4));
            unsafe {
                for (i, v) in (a..b).enumerate() {
                    set_word(out, i, heap::imm(v));
                }
                (*out).len = n as u32;
            }
            Some(out as Word)
        }
        // A bridged `Bytes` is read in place: its buffer is shared, so nothing is copied.
        (Builtin::BytesLen, [b]) if heap::kind(*b) == KIND_BRIDGE => {
            let Value::Bytes(bytes) = (unsafe { bridged(obj(*b)) }) else {
                return None;
            };
            let n = bytes.len() as i64;
            heap::dec(*b);
            Some(heap::imm(n))
        }
        (Builtin::BytesAt, [b, i]) if heap::kind(*b) == KIND_BRIDGE => {
            let Value::Bytes(bytes) = (unsafe { bridged(obj(*b)) }) else {
                return None;
            };
            let index = heap::as_int(*i)?;
            // Out of range is the interpreter's diagnostic to raise.
            let byte = *bytes.get(usize::try_from(index).ok()?)?;
            heap::dec(*b);
            Some(heap::imm(i64::from(byte)))
        }
        // Concatenation over bridged buffers, in one allocation; a piece that is not bytes is
        // the interpreter's diagnostic to raise.
        (Builtin::BytesConcat, [a, b])
            if heap::kind(*a) == KIND_BRIDGE && heap::kind(*b) == KIND_BRIDGE =>
        {
            let (Value::Bytes(x), Value::Bytes(y)) =
                (unsafe { bridged(obj(*a)) }, unsafe { bridged(obj(*b)) })
            else {
                return None;
            };
            let mut out = Vec::with_capacity(x.len() + y.len());
            out.extend_from_slice(x);
            out.extend_from_slice(y);
            heap::dec(*a);
            heap::dec(*b);
            Some(ctx.heap.bridge(Value::bytes(out)))
        }
        (Builtin::BytesConcatAll, [xs]) if heap::kind(*xs) == KIND_LIST => {
            let o = obj(*xs);
            let n = unsafe { (*o).len } as usize;
            let mut total = 0;
            for i in 0..n {
                let w = unsafe { word_at(o, i) };
                if heap::kind(w) != KIND_BRIDGE {
                    return None;
                }
                let Value::Bytes(piece) = (unsafe { bridged(obj(w)) }) else {
                    return None;
                };
                total += piece.len();
            }
            let mut out = Vec::with_capacity(total);
            for i in 0..n {
                let w = unsafe { word_at(o, i) };
                if let Value::Bytes(piece) = unsafe { bridged(obj(w)) } {
                    out.extend_from_slice(piece);
                }
            }
            heap::dec(*xs);
            Some(ctx.heap.bridge(Value::bytes(out)))
        }
        (Builtin::ByteOfInt, [n]) => {
            let v = heap::as_int(*n)?;
            let byte = u8::try_from(v).ok()?;
            Some(ctx.heap.bridge(Value::bytes([byte])))
        }
        (Builtin::MapNew, []) => Some(ctx.heap.alloc_map(4) as Word),
        (Builtin::MapInsert, [m, k, v]) if heap::kind(*m) == KIND_MAP && heap::native_key(*k) => {
            let tables = Rc::clone(&ctx.tables);
            Some(ctx.heap.map_insert(&tables.layouts, *m, *k, *v))
        }
        (Builtin::MapGet, [m, k]) if heap::kind(*m) == KIND_MAP && heap::native_key(*k) => {
            let some = ctx.tables.layouts.some?;
            let none = ctx.tables.layouts.none?;
            let o = obj(*m);
            let answer = match heap::map_find(&ctx.tables.layouts, o, *k) {
                Ok(i) => {
                    let v = unsafe { heap::map_value(o, i) };
                    heap::inc(v);
                    let c = ctx.heap.alloc(KIND_CTOR, 0, 1, some);
                    unsafe { set_word(c, 0, v) };
                    c as Word
                }
                Err(_) => ctx.heap.alloc(KIND_CTOR, 0, 0, none) as Word,
            };
            heap::dec(*m);
            heap::dec(*k);
            Some(answer)
        }
        (Builtin::MapContains, [m, k]) if heap::kind(*m) == KIND_MAP && heap::native_key(*k) => {
            let found = heap::map_find(&ctx.tables.layouts, obj(*m), *k).is_ok();
            heap::dec(*m);
            heap::dec(*k);
            Some(heap::bool(found))
        }
        (Builtin::MapRemove, [m, k]) if heap::kind(*m) == KIND_MAP && heap::native_key(*k) => {
            let tables = Rc::clone(&ctx.tables);
            let out = ctx.heap.map_remove(&tables.layouts, *m, *k);
            heap::dec(*k);
            Some(out)
        }
        (Builtin::MapLen, [m]) if heap::kind(*m) == KIND_MAP => {
            let n = unsafe { (*obj(*m)).len } as i64;
            heap::dec(*m);
            Some(heap::imm(n))
        }
        (Builtin::MapKeys | Builtin::MapValues, [m]) if heap::kind(*m) == KIND_MAP => {
            let o = obj(*m);
            let n = unsafe { (*o).len };
            let out = ctx.heap.alloc_list(n.max(4));
            let at = if b == Builtin::MapKeys { 0 } else { 1 };
            unsafe {
                for i in 0..n as usize {
                    let w = word_at(o, 2 * i + at);
                    heap::inc(w);
                    set_word(out, i, w);
                }
                (*out).len = n;
            }
            heap::dec(*m);
            Some(out as Word)
        }
        (Builtin::MapEntries, [m]) if heap::kind(*m) == KIND_MAP => {
            let o = obj(*m);
            let n = unsafe { (*o).len };
            let shape = ctx.tables.layouts.entry_shape();
            let out = ctx.heap.alloc_list(n.max(4));
            unsafe {
                for i in 0..n as usize {
                    let (k, v) = (heap::map_key(o, i), heap::map_value(o, i));
                    heap::inc(k);
                    heap::inc(v);
                    let e = ctx.heap.alloc(KIND_RECORD, 0, 2, shape);
                    set_word(e, 0, k);
                    set_word(e, 1, v);
                    set_word(out, i, e as Word);
                }
                (*out).len = n;
            }
            heap::dec(*m);
            Some(out as Word)
        }
        (Builtin::MapOfEntries, [xs]) if heap::kind(*xs) == KIND_LIST => {
            let tables = Rc::clone(&ctx.tables);
            let o = obj(*xs);
            let n = unsafe { (*o).len } as usize;
            let (key, value) = (Symbol::new("key"), Symbol::new("value"));
            // Every entry must be a record with both fields and a key the map can order, or the
            // interpreter's implementation raises the right diagnostic instead.
            let mut pairs = Vec::with_capacity(n);
            for i in 0..n {
                let e = unsafe { word_at(o, i) };
                if heap::kind(e) != KIND_RECORD {
                    return None;
                }
                let shape = unsafe { (*obj(e)).layout };
                let (Some(ka), Some(va)) = (
                    tables.layouts.offset(shape, &key),
                    tables.layouts.offset(shape, &value),
                ) else {
                    return None;
                };
                let (k, v) = unsafe { (word_at(obj(e), ka), word_at(obj(e), va)) };
                if !heap::native_key(k) {
                    return None;
                }
                pairs.push((k, v));
            }
            let mut m = ctx.heap.alloc_map((n as u32).max(4)) as Word;
            for (k, v) in pairs {
                heap::inc(k);
                heap::inc(v);
                m = ctx.heap.map_insert(&tables.layouts, m, k, v);
            }
            heap::dec(*xs);
            Some(m)
        }
        (Builtin::MapMerge, [a, bm])
            if heap::kind(*a) == KIND_MAP && heap::kind(*bm) == KIND_MAP =>
        {
            let tables = Rc::clone(&ctx.tables);
            let o = obj(*bm);
            let n = unsafe { (*o).len } as usize;
            let mut m = *a;
            for i in 0..n {
                let (k, v) = unsafe { (heap::map_key(o, i), heap::map_value(o, i)) };
                heap::inc(k);
                heap::inc(v);
                m = ctx.heap.map_insert(&tables.layouts, m, k, v);
            }
            heap::dec(*bm);
            Some(m)
        }
        _ => None,
    }
}

/// A closure over `env`: the compiled function `index` names, entered with the captured values
/// as its leading arguments. Takes the captures.
pub unsafe extern "C" fn rt_closure(
    ctx: *mut Ctx,
    index: i64,
    arity: i64,
    env: *const i64,
    n: i64,
) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let code = ctx.tables.functions[index as usize];
    let env = args_of(env, n);
    let o = ctx.heap.alloc(
        KIND_CLOSURE,
        0,
        (env.len() + CLOSURE_CAPTURES) as u32,
        arity as u32,
    );
    unsafe {
        set_word(o, CLOSURE_CODE, code as Word);
        for (i, w) in env.iter().enumerate() {
            set_word(o, CLOSURE_CAPTURES + i, *w);
        }
    }
    o as Word
}

/// A builtin used as a value: the interpreter's own closure kind for it.
pub unsafe extern "C" fn rt_builtin_value(ctx: *mut Ctx, index: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let b = ctx.tables.builtins[index as usize];
    ctx.heap.bridge(Value::builtin(b))
}

/// A constructor used as a value, likewise.
pub unsafe extern "C" fn rt_ctor_value(ctx: *mut Ctx, index: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let (name, arity) = ctx.tables.layouts.ctors[index as usize].clone();
    ctx.heap.bridge(Value::Closure(Arc::new(Closure {
        name: Some(name.clone()),
        kind: ClosureKind::Ctor { name, arity },
    })))
}

/// The value of the pure nullary function at `index`: remembered on its first evaluation when
/// it is world-independent, as an immortal word, and evaluated on every call otherwise.
pub unsafe extern "C" fn rt_constant(ctx: *mut Ctx, index: i64) -> i64 {
    let tables = Rc::clone(&unsafe { &*ctx }.tables);
    if let Some(Some(w)) = tables.memo.borrow().get(index as usize) {
        return *w;
    }
    // SAFETY: as in `call_value`: a finalized address of this unit's own function, alive for as
    // long as the context is, with the signature every compiled function has; a nullary function
    // reads no argument, so the pointer is never dereferenced.
    let f: Entry = unsafe { std::mem::transmute::<usize, Entry>(tables.functions[index as usize]) };
    let w = unsafe { f(ctx, std::ptr::null()) };
    let c = unsafe { &mut *ctx };
    if c.failed != 0 {
        return 0;
    }
    // Remembered as a copy in the tables' own heap, which outlives the entry's memory; the
    // entry keeps using its own word.
    if !heap::world_independent(w) {
        return w;
    }
    let kept = tables.immortals.borrow_mut().adopt(w);
    let mut memo = tables.memo.borrow_mut();
    let index = index as usize;
    if memo.len() <= index {
        memo.resize(index + 1, None);
    }
    memo[index] = Some(kept);
    w
}

/// A call through a value: a local binding, a callee that is an expression, or a callback. Takes
/// the callee and the arguments.
pub unsafe extern "C" fn rt_call(ctx: *mut Ctx, callee: i64, args: *const i64, n: i64) -> i64 {
    let args = args_of(args, n);
    let r = call_value(ctx, callee, args);
    heap::dec(callee);
    r
}

/// Applies `callee` to `args` and answers the word of the result, or 0 with the context failed.
/// Reads the callee and takes the arguments. A native closure is entered directly; a builtin or
/// a constructor is the interpreter's own; an interpreted closure cannot be here, because the
/// seam carries no function.
fn call_value(ctx: *mut Ctx, callee: Word, args: &[Word]) -> i64 {
    let c = unsafe { &mut *ctx };
    match heap::kind(callee) {
        KIND_CLOSURE => {
            let o = obj(callee);
            let (arity, len) = unsafe { ((*o).layout as usize, (*o).len as usize) };
            if args.len() != arity {
                let d = error(format!(
                    "a compiled function takes {arity} arguments and was given {}",
                    args.len()
                ));
                return c.fail(d);
            }
            // The callee owns every parameter: the captures are held once more, since the
            // closure may be called again, and the arguments were taken by the caller's mask. A
            // stack array holds the words for any arity a body has, so a call allocates nothing.
            let mut handles = [0i64; 64];
            let mut spilled: Vec<i64> = Vec::new();
            let captures = len - CLOSURE_CAPTURES;
            let total = captures + args.len();
            let mut push = |w: Word, i: usize| {
                if total <= handles.len() {
                    handles[i] = w;
                } else {
                    spilled.push(w);
                }
            };
            for i in 0..captures {
                let w = unsafe { word_at(o, CLOSURE_CAPTURES + i) };
                heap::inc(w);
                push(w, i);
            }
            for (i, w) in args.iter().enumerate() {
                push(*w, captures + i);
            }
            let ptr = if total <= handles.len() {
                handles.as_ptr()
            } else {
                spilled.as_ptr()
            };
            let code = unsafe { word_at(o, CLOSURE_CODE) } as usize;
            // SAFETY: `code` came out of `Tables::functions`, the finalized addresses of this
            // unit's own functions, which `Bodies` keeps alive for as long as this context
            // exists; the signature is the one every compiled function has.
            let f: Entry = unsafe { std::mem::transmute::<usize, Entry>(code) };
            unsafe { f(ctx, ptr) }
        }
        KIND_BRIDGE => {
            let value = unsafe { bridged(obj(callee)) };
            let Value::Closure(closure) = value else {
                let d = error(format!(
                    "a call needs a function, and this is {}",
                    value.type_name()
                ));
                return c.fail(d);
            };
            match &closure.kind {
                ClosureKind::Builtin(b) => {
                    let b = *b;
                    let values = values_taken(c, args);
                    c.builtin_calls += 1;
                    match ply_eval::builtins::call(b, values, c.cells.arena_mut(), Span::DUMMY) {
                        Ok(Step::Done(v)) => c.word(&v),
                        Ok(_) => {
                            let d = error(format!(
                                "`{}` suspended, which the fragment excludes",
                                b.name()
                            ));
                            c.fail(d)
                        }
                        Err(d) => c.fail(d),
                    }
                }
                ClosureKind::Ctor { name, arity } => {
                    if args.len() != *arity {
                        let d = error(format!(
                            "the constructor `{name}` takes {arity} fields and was given {}",
                            args.len()
                        ));
                        return c.fail(d);
                    }
                    match c.tables.layouts.ctor_index(name) {
                        Some(index) => {
                            let o = c.heap.alloc(KIND_CTOR, 0, args.len() as u32, index);
                            for (i, w) in args.iter().enumerate() {
                                unsafe { set_word(o, i, *w) };
                            }
                            o as Word
                        }
                        None => {
                            let values = values_taken(c, args);
                            c.word(&Value::ctor(name.clone(), values))
                        }
                    }
                }
                _ => {
                    let d = error(
                        "an interpreted closure reached compiled code, which has no machine to run it on",
                    );
                    c.fail(d)
                }
            }
        }
        _ => {
            let d = error(format!(
                "a call needs a function, and this is {}",
                c.type_name(callee)
            ));
            c.fail(d)
        }
    }
}

/// A native list, or a failure naming the builtin that needed one.
fn native_list(ctx: &mut Ctx, w: Word, what: &str) -> Option<*mut heap::Obj> {
    if heap::kind(w) == KIND_LIST {
        return Some(obj(w));
    }
    let d = error(format!(
        "`{what}` needs a List, and this is {}",
        ctx.type_name(w)
    ));
    ctx.fail(d);
    None
}

/// `map(xs, f)`: `f` on every element, in order. Takes the list and the function.
pub unsafe extern "C" fn rt_map(ctx: *mut Ctx, list: i64, f: i64) -> i64 {
    let c = unsafe { &mut *ctx };
    let Some(items) = native_list(c, list, "map") else {
        return 0;
    };
    let n = unsafe { (*items).len };
    let out = c.heap.alloc_list(n.max(4));
    for i in 0..n as usize {
        let x = unsafe { word_at(items, i) };
        heap::inc(x);
        let r = call_value(ctx, f, &[x]);
        let c = unsafe { &mut *ctx };
        if c.failed != 0 {
            return 0;
        }
        unsafe {
            set_word(out, i, r);
            (*out).len = i as u32 + 1;
        }
    }
    heap::dec(list);
    heap::dec(f);
    out as Word
}

/// `filter(xs, p)`: the elements `p` answers `true` for. Takes the list and the predicate.
pub unsafe extern "C" fn rt_filter(ctx: *mut Ctx, list: i64, p: i64) -> i64 {
    let c = unsafe { &mut *ctx };
    let Some(items) = native_list(c, list, "filter") else {
        return 0;
    };
    let n = unsafe { (*items).len };
    let out = c.heap.alloc_list(n.max(4));
    let mut kept = 0u32;
    for i in 0..n as usize {
        let x = unsafe { word_at(items, i) };
        heap::inc(x);
        let r = call_value(ctx, p, &[x]);
        let c = unsafe { &mut *ctx };
        if c.failed != 0 {
            return 0;
        }
        match heap::as_bool(r) {
            Some(true) => {
                heap::inc(x);
                unsafe {
                    set_word(out, kept as usize, x);
                    (*out).len = kept + 1;
                }
                kept += 1;
            }
            Some(false) => {}
            None => {
                let d = error(format!(
                    "the predicate given to `filter` answered {}, not a Bool",
                    c.type_name(r)
                ));
                return c.fail(d);
            }
        }
    }
    heap::dec(list);
    heap::dec(p);
    out as Word
}

/// `fold(xs, init, f)`. Takes all three.
pub unsafe extern "C" fn rt_fold(ctx: *mut Ctx, list: i64, init: i64, f: i64) -> i64 {
    let c = unsafe { &mut *ctx };
    let Some(items) = native_list(c, list, "fold") else {
        return 0;
    };
    let n = unsafe { (*items).len };
    let mut acc = init;
    for i in 0..n as usize {
        let x = unsafe { word_at(items, i) };
        heap::inc(x);
        acc = call_value(ctx, f, &[acc, x]);
        let c = unsafe { &mut *ctx };
        if c.failed != 0 {
            return 0;
        }
    }
    heap::dec(list);
    heap::dec(f);
    acc
}

/// `map_fold(m, init, f)`: `f` on every entry in ascending key order, over a snapshot of the
/// entries as the interpreter's loop takes one. Takes all three.
pub unsafe extern "C" fn rt_map_fold(ctx: *mut Ctx, map: i64, init: i64, f: i64) -> i64 {
    let c = unsafe { &mut *ctx };
    if heap::kind(map) == KIND_MAP {
        let o = obj(map);
        let n = unsafe { (*o).len } as usize;
        let mut acc = init;
        for i in 0..n {
            let (k, v) = unsafe { (heap::map_key(o, i), heap::map_value(o, i)) };
            heap::inc(k);
            heap::inc(v);
            acc = call_value(ctx, f, &[acc, k, v]);
            if unsafe { &*ctx }.failed != 0 {
                return 0;
            }
        }
        heap::dec(map);
        heap::dec(f);
        return acc;
    }
    let value = c.value(map);
    let Value::Map(entries) = &value else {
        let d = error(format!(
            "`map_fold` needs a Map, and this is {}",
            value.type_name()
        ));
        return c.fail(d);
    };
    let entries: Vec<(Value, Value)> = entries
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let mut acc = init;
    for (k, v) in &entries {
        let c = unsafe { &mut *ctx };
        let kw = c.word(k);
        let vw = c.word(v);
        acc = call_value(ctx, f, &[acc, kw, vw]);
        let c = unsafe { &mut *ctx };
        if c.failed != 0 {
            return 0;
        }
    }
    heap::dec(map);
    heap::dec(f);
    acc
}

/// `iterate(seed, budget, f)`: `f` until it answers `Stop`, or the budget runs out and the call
/// fails the way the interpreter's raises. Takes all three.
pub unsafe extern "C" fn rt_iterate(ctx: *mut Ctx, seed: i64, budget: i64, f: i64) -> i64 {
    let c = unsafe { &mut *ctx };
    let budget = match heap::as_int(budget) {
        Some(n) if n >= 1 => n,
        Some(n) => {
            let d = error(format!(
                "`iterate` needs a budget of at least 1, and this is {n}"
            ));
            return c.fail(d);
        }
        None => {
            let d = error(format!(
                "`iterate` needs an Int budget, and this is {}",
                c.type_name(budget)
            ));
            return c.fail(d);
        }
    };
    let stop = c.tables.layouts.stop;
    let go = c.tables.layouts.go;
    let mut state = seed;
    let mut left = budget;
    loop {
        if left <= 0 {
            let c = unsafe { &mut *ctx };
            let d = error(format!(
                "`iterate` did not stop within its budget of {budget}"
            ));
            return c.fail(d);
        }
        left -= 1;
        let r = call_value(ctx, f, &[state]);
        let c = unsafe { &mut *ctx };
        if c.failed != 0 {
            return 0;
        }
        // The step's answer gives up its payload, so the state threaded through the loop stays
        // uniquely held.
        let tag = if heap::kind(r) == KIND_CTOR {
            Some(unsafe { ((*obj(r)).layout, (*obj(r)).len) })
        } else {
            None
        };
        match tag {
            Some((index, 1)) if Some(index) == go || Some(index) == stop => {
                let payload = unsafe { word_at(obj(r), 0) };
                unsafe { set_word(obj(r), 0, heap::unit()) };
                heap::dec(r);
                if Some(index) == stop {
                    heap::dec(f);
                    return payload;
                }
                state = payload;
            }
            _ => {
                let d = error(format!(
                    "the step given to `iterate` answered {}, not `Continue` or `Stop`",
                    c.type_name(r)
                ));
                return c.fail(d);
            }
        }
    }
}

/// A shift count outside `0..64`, which the interpreter refuses too.
pub unsafe extern "C" fn rt_shift_count(ctx: *mut Ctx, n: i64) {
    let ctx = unsafe { &mut *ctx };
    let d = error(format!(
        "a shift count must be between 0 and 63, and this is {n}"
    ));
    ctx.fail(d);
}

/// An applied constructor. Takes the arguments.
pub unsafe extern "C" fn rt_ctor(ctx: *mut Ctx, index: i64, args: *const i64, n: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let args = args_of(args, n);
    let o = ctx.heap.alloc(KIND_CTOR, 0, n as u32, index as u32);
    for (i, w) in args.iter().enumerate() {
        unsafe { set_word(o, i, *w) };
    }
    o as Word
}

/// A record literal: the fields in the shape's own sorted order. Takes the fields.
pub unsafe extern "C" fn rt_record(ctx: *mut Ctx, shape: i64, args: *const i64, n: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let args = args_of(args, n);
    let o = ctx.heap.alloc(KIND_RECORD, 0, n as u32, shape as u32);
    for (i, w) in args.iter().enumerate() {
        unsafe { set_word(o, i, *w) };
    }
    o as Word
}

/// A record update: `n` written fields at the `offsets` they take in `shape`, written into the
/// base itself when it has that shape and nothing else holds it, and into a fresh record with
/// the rest copied out of the base otherwise. Takes the base and the written fields.
pub unsafe extern "C" fn rt_record_update(
    ctx: *mut Ctx,
    shape: i64,
    base: i64,
    args: *const i64,
    offsets: *const i64,
    n: i64,
) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let written = args_of(args, n);
    let offsets = args_of(offsets, n);
    if heap::kind(base) != KIND_RECORD {
        let d = error(format!(
            "a record update needs a record, and this is {}",
            ctx.type_name(base)
        ));
        return ctx.fail(d);
    }
    let o = obj(base);
    let shape = shape as u32;
    if unsafe { (*o).layout } == shape && is_unique(base) {
        for (w, at) in written.iter().zip(offsets) {
            unsafe {
                heap::dec(word_at(o, *at as usize));
                set_word(o, *at as usize, *w);
            }
        }
        return base;
    }
    let tables = Rc::clone(&ctx.tables);
    let names = tables.layouts.shape_names(shape);
    let out = ctx.heap.alloc(KIND_RECORD, 0, names.len() as u32, shape);
    let base_shape = unsafe { (*o).layout };
    for (i, name) in names.iter().enumerate() {
        let w = match offsets.iter().position(|at| *at as usize == i) {
            Some(k) => written[k],
            None => match tables.layouts.offset(base_shape, name) {
                Some(at) => {
                    let w = unsafe { word_at(o, at) };
                    heap::inc(w);
                    w
                }
                None => {
                    let d = error(format!("this record has no field `{name}`"));
                    return ctx.fail(d);
                }
            },
        };
        unsafe { set_word(out, i, w) };
    }
    heap::dec(base);
    out as Word
}

/// One field of a record, by name, with the two failures the interpreter has here kept as
/// failures rather than answered. `own` is the lowering's mark: 0 reads a local's record and
/// answers a field held once more; 2 is the last use of this *field* while the record stays,
/// which moves the field out when the record is held by nobody else; 1 is the base's last use
/// and 3 a base that is a temporary, both of which take the base and release it.
pub unsafe extern "C" fn rt_field(ctx: *mut Ctx, base: i64, index: i64, own: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    if heap::kind(base) != KIND_RECORD {
        let d = error(format!(
            "a field access needs a record, and this is {}",
            ctx.type_name(base)
        ));
        return ctx.fail(d);
    }
    let o = obj(base);
    let shape = unsafe { (*o).layout };
    let Some(at) = ctx
        .tables
        .layouts
        .offset_by_index(shape, index as usize, &ctx.tables.fields)
    else {
        let d = error(format!(
            "this record has no field `{}`",
            ctx.tables.fields[index as usize]
        ));
        return ctx.fail(d);
    };
    let w = unsafe { word_at(o, at) };
    match own {
        0 => heap::inc(w),
        2 => {
            if is_unique(base) {
                unsafe { set_word(o, at, heap::unit()) };
            } else {
                heap::inc(w);
            }
        }
        _ => {
            if is_unique(base) {
                unsafe { set_word(o, at, heap::unit()) };
            } else {
                heap::inc(w);
            }
            heap::dec(base);
        }
    }
    w
}

/// A list literal. Takes the items.
pub unsafe extern "C" fn rt_list(ctx: *mut Ctx, args: *const i64, n: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let args = args_of(args, n);
    let o = ctx.heap.alloc_list((n as u32).max(4));
    for (i, w) in args.iter().enumerate() {
        unsafe { set_word(o, i, *w) };
    }
    unsafe { (*o).len = n as u32 };
    o as Word
}

/// Whether a value is a record whose field count a pattern admits: `exact` demands the count,
/// because a pattern without `..` matches only a record of exactly its fields. Reads.
pub unsafe extern "C" fn rt_record_fits(_ctx: *mut Ctx, value: i64, len: i64, exact: i64) -> i64 {
    if heap::kind(value) != KIND_RECORD {
        return 0;
    }
    i64::from(exact == 0 || unsafe { (*obj(value)).len } as i64 == len)
}

/// Whether a record holds the field a pattern names, a missing one being a failed match rather
/// than the error [`rt_field`] raises. Reads.
pub unsafe extern "C" fn rt_record_has(ctx: *mut Ctx, value: i64, index: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    if heap::kind(value) != KIND_RECORD {
        return 0;
    }
    let shape = unsafe { (*obj(value)).layout };
    i64::from(
        ctx.tables
            .layouts
            .offset_by_index(shape, index as usize, &ctx.tables.fields)
            .is_some(),
    )
}

/// Whether a value is a list long enough for a pattern: `exact` demands the length, and otherwise
/// `len` is a minimum because a `..rest` takes the remainder. Reads.
pub unsafe extern "C" fn rt_list_fits(_ctx: *mut Ctx, value: i64, len: i64, exact: i64) -> i64 {
    if heap::kind(value) != KIND_LIST {
        return 0;
    }
    let n = unsafe { (*obj(value)).len } as i64;
    i64::from(if exact != 0 { n == len } else { n >= len })
}

/// One element of a list, once [`rt_list_fits`] has admitted its length: held once more. Reads.
pub unsafe extern "C" fn rt_list_at(ctx: *mut Ctx, value: i64, i: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    if heap::kind(value) != KIND_LIST {
        let d = error("a list pattern bound a value that is not a list");
        return ctx.fail(d);
    }
    let o = obj(value);
    if i < 0 || i >= unsafe { (*o).len } as i64 {
        let d = error("a list pattern read past the end of the list");
        return ctx.fail(d);
    }
    let w = unsafe { word_at(o, i as usize) };
    heap::inc(w);
    w
}

/// What a `..rest` binds: a fresh list of everything from `from` on, which is the copy `ply_eval`
/// makes at the same point rather than a shared tail. Reads.
pub unsafe extern "C" fn rt_list_rest(ctx: *mut Ctx, value: i64, from: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    if heap::kind(value) != KIND_LIST {
        let d = error("a list pattern bound a value that is not a list");
        return ctx.fail(d);
    }
    let o = obj(value);
    let len = unsafe { (*o).len } as i64;
    let from = from.clamp(0, len) as usize;
    let n = len as usize - from;
    let out = ctx.heap.alloc_list((n as u32).max(4));
    unsafe {
        for i in 0..n {
            let w = word_at(o, from + i);
            heap::inc(w);
            set_word(out, i, w);
        }
        (*out).len = n as u32;
    }
    out as Word
}

/// Whether a value is the constructor at `index`, name and arity both. Reads.
pub unsafe extern "C" fn rt_ctor_is(ctx: *mut Ctx, value: i64, index: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    if heap::kind(value) != KIND_CTOR {
        return 0;
    }
    let arity = ctx.tables.layouts.ctors[index as usize].1;
    let o = obj(value);
    i64::from(unsafe { (*o).layout } as i64 == index && unsafe { (*o).len } as usize == arity)
}

/// One argument of a constructor value, once [`rt_ctor_is`] has said it is that constructor:
/// moved out when `take` is set and nothing else holds the constructor, held once more otherwise.
/// Reads the constructor.
pub unsafe extern "C" fn rt_ctor_arg(ctx: *mut Ctx, value: i64, i: i64, take: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    if heap::kind(value) != KIND_CTOR {
        let d = error("a constructor pattern bound a value that is not a constructor");
        return ctx.fail(d);
    }
    let o = obj(value);
    if i < 0 || i >= unsafe { (*o).len } as i64 {
        let d = error("a constructor pattern read an argument that is not there");
        return ctx.fail(d);
    }
    let w = unsafe { word_at(o, i as usize) };
    if take != 0 && is_unique(value) {
        unsafe { set_word(o, i as usize, heap::unit()) };
    } else {
        heap::inc(w);
    }
    w
}

/// Every symbol the JIT registers, in one place so the compiler and the linker cannot drift.
pub fn symbols() -> Vec<(&'static str, *const u8)> {
    vec![
        ("rt_box_int", rt_box_int as *const u8),
        ("rt_box_bool", rt_box_bool as *const u8),
        ("rt_unbox_int", rt_unbox_int as *const u8),
        ("rt_unbox_bool", rt_unbox_bool as *const u8),
        ("rt_arith", rt_arith as *const u8),
        ("rt_overflow", rt_overflow as *const u8),
        ("rt_no_match", rt_no_match as *const u8),
        ("rt_lit", rt_lit as *const u8),
        ("rt_equal", rt_equal as *const u8),
        ("rt_concat", rt_concat as *const u8),
        ("rt_record_fits", rt_record_fits as *const u8),
        ("rt_record_has", rt_record_has as *const u8),
        ("rt_builtin", rt_builtin as *const u8),
        ("rt_ctor", rt_ctor as *const u8),
        ("rt_list", rt_list as *const u8),
        ("rt_list_fits", rt_list_fits as *const u8),
        ("rt_list_at", rt_list_at as *const u8),
        ("rt_list_rest", rt_list_rest as *const u8),
        ("rt_ctor_is", rt_ctor_is as *const u8),
        ("rt_ctor_arg", rt_ctor_arg as *const u8),
        ("rt_record", rt_record as *const u8),
        ("rt_record_update", rt_record_update as *const u8),
        ("rt_field", rt_field as *const u8),
        ("rt_no_fuel", rt_no_fuel as *const u8),
        ("rt_closure", rt_closure as *const u8),
        ("rt_builtin_value", rt_builtin_value as *const u8),
        ("rt_ctor_value", rt_ctor_value as *const u8),
        ("rt_call", rt_call as *const u8),
        ("rt_map", rt_map as *const u8),
        ("rt_filter", rt_filter as *const u8),
        ("rt_fold", rt_fold as *const u8),
        ("rt_map_fold", rt_map_fold as *const u8),
        ("rt_iterate", rt_iterate as *const u8),
        ("rt_shift_count", rt_shift_count as *const u8),
        ("rt_dup", rt_dup as *const u8),
        ("rt_dec", rt_dec as *const u8),
        ("rt_constant", rt_constant as *const u8),
    ]
}
