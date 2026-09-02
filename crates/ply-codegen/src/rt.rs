//! The runtime compiled code calls into: every helper a body reaches for what it does not lower
//! inline, over the words [`crate::heap`] lays out, and the context an entry runs in.
//!
//! Ownership follows Perceus as the code generator marks it: a helper that *takes* an argument
//! owns it from then on and releases it when it is done or keeps it inside what it builds; a
//! helper that *reads* one leaves its count alone; a helper answers a word its caller owns.
//! Nothing here recurses over a value: a count reaching zero dismantles with a worklist.

use crate::heap::{
    self, CLOSURE_CAPTURES, CLOSURE_CODE, Heap, KIND_BRIDGE, KIND_BYTES, KIND_CLOSURE, KIND_CTOR,
    KIND_LIST, KIND_MAP, KIND_RECORD, KIND_STR, Layouts, Word, bridged, bytes_of, is_unique, obj,
    set_word, str_of, word_at,
};
use crate::jit::Entry;
use crate::list;
use crate::map;
use ply_eval::{Builtin, Closure, ClosureKind, Step, Value, values_equal};
use ply_span::{Diagnostic, Span, Symbol, codes};
use std::cell::RefCell;
use std::collections::HashMap;
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
    /// The two hundred and fifty-six one-byte values, each made immortal the first time it is
    /// asked for, so `byte_of_int` allocates nothing.
    pub bytes: RefCell<[Word; 256]>,
    /// Per constructor index, the immortal singleton a nullary one is, or `0`; and the empty
    /// list and the empty map, made once — no body allocates any of these.
    pub nullaries: Vec<Word>,
    pub empty_list: Word,
    pub empty_map: Word,
    /// The memo's words as the values the seam converted them to, once each, and those values'
    /// identities back to the words: what lets a phase's tree cross the seam and come back
    /// without being rebuilt either way.
    pub memo_values: RefCell<HashMap<Word, Value>>,
    pub memo_words: RefCell<HashMap<Identity, Word>>,
    /// The answers of roots called with nothing but memo words, by the root and the words:
    /// a pure function of remembered inputs, remembered in turn, up to a bound.
    pub calls: RefCell<HashMap<(Symbol, Vec<Word>), Word>>,
}

/// How many calls of roots over memo words a unit remembers; past it, a call is run and
/// converted as any other.
pub const CALL_MEMO_LIMIT: usize = 64;

/// How many of an answer's parts are given identities of their own, one level down: the fields
/// a body pulls out of a record or a constructor, and the elements of a short list.
const PARTS_LIMIT: usize = 64;

/// What identifies a value the seam handed out, without walking it: the allocation behind it
/// and, for a list, the window it shows of that allocation. A value with no allocation of its
/// own has no identity and is converted like any other.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Identity {
    Record(usize),
    Str(usize),
    Bytes(usize),
    List(usize, usize, usize, usize),
}

fn identity(v: &Value) -> Option<Identity> {
    Some(match v {
        Value::Record(fields) => Identity::Record(Arc::as_ptr(fields) as usize),
        Value::Str(s) => Identity::Str(Arc::as_ptr(s) as *const u8 as usize),
        Value::Bytes(b) => Identity::Bytes(Arc::as_ptr(b) as *const u8 as usize),
        Value::List(items) => {
            let (tail, root, len, start) = items.identity();
            Identity::List(tail, root, len, start)
        }
        _ => return None,
    })
}

impl Tables {
    /// The memo's word for the pure nullary function at `index`, if it has one.
    pub fn memoized(&self, index: usize) -> Option<Word> {
        self.memo.borrow().get(index).copied().flatten()
    }

    /// Remembers `w`, world-independent, as the answer of the pure nullary function at `index`:
    /// a copy in the tables' own heap, which outlives every entry, answered from then on.
    pub fn memoize(&self, index: usize, w: Word) -> Word {
        let kept = self.immortals.borrow_mut().adopt(w);
        let mut memo = self.memo.borrow_mut();
        if memo.len() <= index {
            memo.resize(index + 1, None);
        }
        memo[index] = Some(kept);
        kept
    }

    /// The value a memo word was converted to before, if it was.
    pub fn memo_value(&self, w: Word) -> Option<Value> {
        self.memo_values.borrow().get(&w).cloned()
    }

    /// The memo word a value came from, if the seam handed that value out. The values kept in
    /// `memo_values` hold their allocations, so an identity here cannot be a later value's.
    pub fn memo_word(&self, v: &Value) -> Option<Word> {
        let id = identity(v)?;
        self.memo_words.borrow().get(&id).copied()
    }

    /// Keeps the value a memo word was just converted to, and gives the value's parts one level
    /// down — a record's fields, a constructor's arguments, a short list's elements — the words
    /// they came from, since a body that takes a memo value apart hands those parts back in.
    pub fn remember(&self, w: Word, v: &Value) {
        self.memo_values.borrow_mut().insert(w, v.clone());
        let mut words = self.memo_words.borrow_mut();
        if let Some(id) = identity(v) {
            words.insert(id, w);
        }
        // A scalar answer is its own word, with no parts to remember.
        if heap::is_imm(w) || w == 0 {
            return;
        }
        let o = obj(w);
        let parts: Vec<(Word, &Value)> = match v {
            Value::Record(fields) => fields
                .iter()
                .enumerate()
                .map(|(i, (_, part))| (unsafe { word_at(o, i) }, part))
                .collect(),
            Value::Ctor { args, .. } => args
                .iter()
                .enumerate()
                .map(|(i, part)| (unsafe { word_at(o, i) }, part))
                .collect(),
            Value::List(items) if items.len() <= PARTS_LIMIT => items
                .iter()
                .enumerate()
                .map(|(i, part)| (list::get(o, i), part))
                .collect(),
            _ => Vec::new(),
        };
        for (part_word, part) in parts.into_iter().take(PARTS_LIMIT) {
            if let Some(id) = identity(part) {
                words.insert(id, part_word);
            }
        }
    }

    /// The remembered answer of `root` over exactly these memo words, if it has one.
    pub fn memo_call(&self, root: &Symbol, words: &[Word]) -> Option<Value> {
        let kept = *self.calls.borrow().get(&(root.clone(), words.to_vec()))?;
        self.memo_value(kept)
    }

    /// Remembers `out`, world-independent, as the answer of `root` over these memo words, up to
    /// the bound; answers the word to keep, or nothing when the bound is reached.
    pub fn memoize_call(&self, root: &Symbol, words: &[Word], out: Word) -> Option<Word> {
        let mut calls = self.calls.borrow_mut();
        if calls.len() >= CALL_MEMO_LIMIT {
            return None;
        }
        let kept = self.immortals.borrow_mut().adopt(out);
        calls.insert((root.clone(), words.to_vec()), kept);
        Some(kept)
    }

    /// The immortal `Bytes` holding just `b`.
    pub fn byte(&self, b: u8) -> Word {
        let cached = self.bytes.borrow()[b as usize];
        if cached != 0 {
            return cached;
        }
        let w = self.immortals.borrow_mut().bytes(&[b]);
        heap::mark_immortal(w);
        self.bytes.borrow_mut()[b as usize] = w;
        w
    }

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
        heap::enter(&mut self.heap);
    }

    /// The other end of [`Ctx::begin`]: the entry gives back what it used.
    pub fn end(&mut self) {
        heap::leave();
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

    /// The singleton a nullary constructor is.
    pub fn nullary(&self, index: u32) -> Word {
        self.tables.nullaries[index as usize]
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
pub unsafe extern "C" fn rt_dec(ctx: *mut Ctx, w: i64) {
    let ctx = unsafe { &mut *ctx };
    ctx.heap.release_last(w);
}

/// Perceus's `reset`: a record held once, at its last use in a body that builds another of its
/// width, keeps its memory for that one. Answers the word kept, or `0` once released.
pub unsafe extern "C" fn rt_reset(_ctx: *mut Ctx, w: i64) -> i64 {
    heap::reset(w)
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
    if !heap::is_imm(a) && !heap::is_imm(b) {
        let (ka, kb) = (heap::kind(a), heap::kind(b));
        if ka == kb && (ka == KIND_STR || ka == KIND_BYTES) {
            return i64::from(unsafe { bytes_of(obj(a)) == bytes_of(obj(b)) });
        }
    }
    let ctx = unsafe { &mut *ctx };
    let (l, r) = (ctx.value(a), ctx.value(b));
    match values_equal(&l, &r, Span::DUMMY) {
        Ok(eq) => i64::from(eq),
        Err(d) => ctx.fail(d),
    }
}

/// `++`, which is `String` concatenation only. Takes both: two native strings append, in place
/// when the left one is held by nobody else and has the room; anything else goes through the
/// same two `Value::as_str` calls `interp::strict_binary` uses, so a non-`Str` operand raises
/// the identical error.
pub unsafe extern "C" fn rt_concat(ctx: *mut Ctx, a: i64, b: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    if heap::kind(a) == KIND_STR && heap::kind(b) == KIND_STR {
        let out = ctx.heap.append(a, unsafe { bytes_of(obj(b)) });
        heap::dec(b);
        return out;
    }
    let (l, r) = (ctx.value(a), ctx.value(b));
    heap::dec(a);
    heap::dec(b);
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
    builtin_over_values(ctx, b, args)
}

/// The interpreter's own implementation of `b` over the values the words denote: what answers
/// when no native path does. Takes the arguments.
fn builtin_over_values(ctx: &mut Ctx, b: Builtin, args: &[Word]) -> Word {
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

/// `bytes_concat_all` over the pieces of a list literal, without the list: one value holding
/// them all, or the interpreter's answer over the list when a piece is not bytes. Takes the
/// pieces.
pub unsafe extern "C" fn rt_bytes_join(ctx: *mut Ctx, args: *const i64, n: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let pieces = args_of(args, n);
    ctx.builtin_calls += 1;
    if pieces.iter().any(|w| heap::kind(*w) != KIND_BYTES) {
        let xs = ctx.heap.list_from(pieces);
        return builtin_over_values(ctx, Builtin::BytesConcatAll, &[xs]);
    }
    let total: usize = pieces
        .iter()
        .map(|w| unsafe { (*obj(*w)).len } as usize)
        .sum();
    let out = ctx.heap.alloc_bytes(KIND_BYTES, total as u32);
    let mut at = 0;
    for w in pieces {
        let piece = unsafe { bytes_of(obj(*w)) };
        unsafe {
            std::ptr::copy_nonoverlapping(
                piece.as_ptr(),
                heap::bytes_ptr(out).add(at),
                piece.len(),
            );
        }
        at += piece.len();
        heap::dec(*w);
    }
    unsafe { (*out).len = total as u32 };
    out as Word
}

/// The builtins answered over words, when their arguments have the native kinds; `None` hands the
/// call to the interpreter's implementation.
fn native_builtin(ctx: &mut Ctx, which: Builtin, args: &[Word]) -> Option<Word> {
    match (which, args) {
        (Builtin::Len, [xs]) if heap::kind(*xs) == KIND_LIST => {
            let n = unsafe { (*obj(*xs)).len } as i64;
            heap::dec(*xs);
            Some(heap::imm(n))
        }
        (Builtin::Push, [xs, x]) if heap::kind(*xs) == KIND_LIST => {
            Some(ctx.heap.list_push(*xs, *x))
        }
        (Builtin::ListAt, [xs, i]) if heap::kind(*xs) == KIND_LIST => {
            let o = obj(*xs);
            let index = heap::as_int(*i)?;
            let len = list::len(o) as i64;
            let some = ctx.tables.layouts.some?;
            let none = ctx.tables.layouts.none?;
            let answer = if (0..len).contains(&index) {
                let item = list::get(o, index as usize);
                heap::inc(item);
                let c = ctx.heap.alloc(KIND_CTOR, 0, 1, some);
                unsafe { set_word(c, 0, item) };
                c as Word
            } else {
                ctx.nullary(none)
            };
            heap::dec(*xs);
            Some(answer)
        }
        (Builtin::Range, [lo, hi]) => {
            let (a, b) = (heap::as_int(*lo)?, heap::as_int(*hi)?);
            if b - a > (1 << 20) {
                return None;
            }
            let items: Vec<Word> = (a..b).map(heap::imm).collect();
            Some(ctx.heap.list_from(&items))
        }
        // Strings and bytes over their own payloads. Anything out of range, not a byte, not
        // UTF-8 or not the kind the builtin wants is the interpreter's diagnostic to raise, so
        // those answer `None` before touching a count.
        (Builtin::BytesLen, [b]) if heap::kind(*b) == KIND_BYTES => {
            let n = unsafe { (*obj(*b)).len } as i64;
            heap::dec(*b);
            Some(heap::imm(n))
        }
        (Builtin::BytesAt, [b, i]) if heap::kind(*b) == KIND_BYTES => {
            let index = usize::try_from(heap::as_int(*i)?).ok()?;
            let byte = *unsafe { bytes_of(obj(*b)) }.get(index)?;
            heap::dec(*b);
            Some(heap::imm(i64::from(byte)))
        }
        (Builtin::BytesSlice, [b, s, e]) if heap::kind(*b) == KIND_BYTES => {
            let bytes = unsafe { bytes_of(obj(*b)) };
            let (start, end) = slice_range(*s, *e, bytes.len())?;
            let out = ctx.heap.bytes(&bytes[start..end]);
            heap::dec(*b);
            Some(out)
        }
        (Builtin::BytesConcat, [a, b])
            if heap::kind(*a) == KIND_BYTES && heap::kind(*b) == KIND_BYTES =>
        {
            let out = ctx.heap.append(*a, unsafe { bytes_of(obj(*b)) });
            heap::dec(*b);
            Some(out)
        }
        (Builtin::BytesConcatAll, [xs]) if heap::kind(*xs) == KIND_LIST => {
            let o = obj(*xs);
            let mut total = 0;
            let mut all_bytes = true;
            list::for_each(o, &mut |w| {
                if heap::kind(w) != KIND_BYTES {
                    all_bytes = false;
                } else {
                    total += unsafe { (*obj(w)).len } as usize;
                }
            });
            if !all_bytes {
                return None;
            }
            let out = ctx.heap.alloc_bytes(KIND_BYTES, total as u32);
            let mut at = 0;
            list::for_each(o, &mut |w| {
                let piece = unsafe { bytes_of(obj(w)) };
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        piece.as_ptr(),
                        heap::bytes_ptr(out).add(at),
                        piece.len(),
                    );
                }
                at += piece.len();
            });
            unsafe { (*out).len = total as u32 };
            heap::dec(*xs);
            Some(out as Word)
        }
        (Builtin::ByteOfInt, [n]) => {
            let byte = u8::try_from(heap::as_int(*n)?).ok()?;
            Some(ctx.tables.byte(byte))
        }
        (Builtin::BytesOfString, [s]) if heap::kind(*s) == KIND_STR => {
            let out = ctx.heap.bytes(unsafe { bytes_of(obj(*s)) });
            heap::dec(*s);
            Some(out)
        }
        (Builtin::StringOfBytes, [b]) if heap::kind(*b) == KIND_BYTES => {
            let text = std::str::from_utf8(unsafe { bytes_of(obj(*b)) }).ok()?;
            let out = ctx.heap.str(text);
            heap::dec(*b);
            Some(out)
        }
        (Builtin::BytesIndexOf, [hay, needle])
            if heap::kind(*hay) == KIND_BYTES && heap::kind(*needle) == KIND_BYTES =>
        {
            let at = find_bytes(
                unsafe { bytes_of(obj(*hay)) },
                unsafe { bytes_of(obj(*needle)) },
                0,
            );
            let out = position(ctx, at)?;
            heap::dec(*hay);
            heap::dec(*needle);
            Some(out)
        }
        (Builtin::BytesIndexOfFrom, [hay, needle, from])
            if heap::kind(*hay) == KIND_BYTES && heap::kind(*needle) == KIND_BYTES =>
        {
            let h = unsafe { bytes_of(obj(*hay)) };
            let from = usize::try_from(heap::as_int(*from)?).ok()?;
            if from > h.len() {
                return None;
            }
            let at = find_bytes(h, unsafe { bytes_of(obj(*needle)) }, from);
            let out = position(ctx, at)?;
            heap::dec(*hay);
            heap::dec(*needle);
            Some(out)
        }
        (Builtin::BytesIndexOfByte, [hay, byte]) if heap::kind(*hay) == KIND_BYTES => {
            let byte = u8::try_from(heap::as_int(*byte)?).ok()?;
            let at = memchr::memchr(byte, unsafe { bytes_of(obj(*hay)) });
            let out = position(ctx, at)?;
            heap::dec(*hay);
            Some(out)
        }
        (Builtin::BytesStartsWith | Builtin::BytesEndsWith, [a, b])
            if heap::kind(*a) == KIND_BYTES && heap::kind(*b) == KIND_BYTES =>
        {
            let (x, y) = unsafe { (bytes_of(obj(*a)), bytes_of(obj(*b))) };
            let answer = if which == Builtin::BytesStartsWith {
                x.starts_with(y)
            } else {
                x.ends_with(y)
            };
            heap::dec(*a);
            heap::dec(*b);
            Some(heap::bool(answer))
        }
        (Builtin::StringConcat, [a, b])
            if heap::kind(*a) == KIND_STR && heap::kind(*b) == KIND_STR =>
        {
            let out = ctx.heap.append(*a, unsafe { bytes_of(obj(*b)) });
            heap::dec(*b);
            Some(out)
        }
        (Builtin::StringLen, [s]) if heap::kind(*s) == KIND_STR => {
            let n = unsafe { str_of(obj(*s)) }.chars().count() as i64;
            heap::dec(*s);
            Some(heap::imm(n))
        }
        (Builtin::IntToString, [n]) => {
            let text = heap::as_int(*n)?.to_string();
            Some(ctx.heap.str(&text))
        }
        (Builtin::StringStartsWith | Builtin::StringEndsWith | Builtin::StringContains, [a, b])
            if heap::kind(*a) == KIND_STR && heap::kind(*b) == KIND_STR =>
        {
            let (x, y) = unsafe { (str_of(obj(*a)), str_of(obj(*b))) };
            let answer = match which {
                Builtin::StringStartsWith => x.starts_with(y),
                Builtin::StringEndsWith => x.ends_with(y),
                _ => x.contains(y),
            };
            heap::dec(*a);
            heap::dec(*b);
            Some(heap::bool(answer))
        }
        (Builtin::Len, [s]) if heap::kind(*s) == KIND_STR => {
            let n = unsafe { str_of(obj(*s)) }.chars().count() as i64;
            heap::dec(*s);
            Some(heap::imm(n))
        }
        // The bounded scans, as `scan` answers them: a window of at most `max` bytes from `from`,
        // the position of the first byte in the class (or off it), or the window's end.
        (Builtin::BytesScan | Builtin::BytesScanUntil, [hay, from, members, max])
            if heap::kind(*hay) == KIND_BYTES && heap::kind(*members) == KIND_BYTES =>
        {
            let h = unsafe { bytes_of(obj(*hay)) };
            let from = usize::try_from(heap::as_int(*from)?).ok()?;
            if from > h.len() {
                return None;
            }
            let max = usize::try_from(heap::as_int(*max)?).ok()?;
            let window = &h[from..h.len().min(from.saturating_add(max))];
            let want = which == Builtin::BytesScanUntil;
            let found = match (want, unsafe { bytes_of(obj(*members)) }) {
                (true, []) => None,
                (true, [a]) => memchr::memchr(*a, window),
                (true, [a, b]) => memchr::memchr2(*a, *b, window),
                (true, [a, b, c]) => memchr::memchr3(*a, *b, *c, window),
                (_, set) => {
                    let mut bits = [0u64; 4];
                    for &b in set {
                        bits[usize::from(b >> 6)] |= 1 << (b & 63);
                    }
                    window
                        .iter()
                        .position(|&b| (bits[usize::from(b >> 6)] >> (b & 63) & 1 == 1) == want)
                }
            };
            let at = match found {
                Some(at) => from + at,
                None => from + window.len(),
            };
            heap::dec(*hay);
            heap::dec(*members);
            Some(heap::imm(at as i64))
        }
        (Builtin::BytesIsUtf8, [b]) if heap::kind(*b) == KIND_BYTES => {
            let ok = std::str::from_utf8(unsafe { bytes_of(obj(*b)) }).is_ok();
            heap::dec(*b);
            Some(heap::bool(ok))
        }
        (Builtin::StringOfBytesLossy, [b]) if heap::kind(*b) == KIND_BYTES => {
            let text = String::from_utf8_lossy(unsafe { bytes_of(obj(*b)) });
            let out = ctx.heap.str(&text);
            heap::dec(*b);
            Some(out)
        }
        (Builtin::BytesSplit, [b, sep])
            if heap::kind(*b) == KIND_BYTES && heap::kind(*sep) == KIND_BYTES =>
        {
            let (x, y) = unsafe { (bytes_of(obj(*b)), bytes_of(obj(*sep))) };
            if y.is_empty() {
                return None;
            }
            let mut pieces = Vec::new();
            let mut at = 0;
            for found in memchr::memmem::find_iter(x, y) {
                pieces.push(ctx.heap.bytes(&x[at..found]));
                at = found + y.len();
            }
            pieces.push(ctx.heap.bytes(&x[at..]));
            let out = list_of(ctx, &pieces);
            heap::dec(*b);
            heap::dec(*sep);
            Some(out)
        }
        (Builtin::StringSlice, [s, start, end]) if heap::kind(*s) == KIND_STR => {
            let text = unsafe { str_of(obj(*s)) };
            let chars = text.chars().count();
            let (from, to) = slice_range(*start, *end, chars)?;
            let (from, to) = (char_offset(text, from), char_offset(text, to));
            let out = ctx.heap.str(&text[from..to]);
            heap::dec(*s);
            Some(out)
        }
        (Builtin::StringSplit, [s, sep])
            if heap::kind(*s) == KIND_STR && heap::kind(*sep) == KIND_STR =>
        {
            let (x, y) = unsafe { (str_of(obj(*s)), str_of(obj(*sep))) };
            if y.is_empty() {
                return None;
            }
            let pieces: Vec<Word> = x.split(y).map(|piece| ctx.heap.str(piece)).collect();
            let out = list_of(ctx, &pieces);
            heap::dec(*s);
            heap::dec(*sep);
            Some(out)
        }
        (Builtin::StringTrim | Builtin::StringLower | Builtin::StringUpper, [s])
            if heap::kind(*s) == KIND_STR =>
        {
            let text = unsafe { str_of(obj(*s)) };
            let out = match which {
                Builtin::StringTrim => ctx.heap.str(text.trim()),
                Builtin::StringLower => ctx.heap.str(&text.to_lowercase()),
                _ => ctx.heap.str(&text.to_uppercase()),
            };
            heap::dec(*s);
            Some(out)
        }
        (Builtin::StringFind, [s, needle])
            if heap::kind(*s) == KIND_STR && heap::kind(*needle) == KIND_STR =>
        {
            let (x, y) = unsafe { (str_of(obj(*s)), str_of(obj(*needle))) };
            // Absent is the interpreter's diagnostic to raise.
            let at = x.find(y)?;
            let n = x[..at].chars().count() as i64;
            heap::dec(*s);
            heap::dec(*needle);
            Some(heap::imm(n))
        }
        (Builtin::Compare | Builtin::CompareValues, [a, b])
            if heap::native_key(*a) && heap::native_key(*b) =>
        {
            let layouts = &ctx.tables.layouts;
            let index = match heap::cmp_words(layouts, *a, *b) {
                std::cmp::Ordering::Less => layouts.less?,
                std::cmp::Ordering::Equal => layouts.equal?,
                std::cmp::Ordering::Greater => layouts.greater?,
            };
            heap::dec(*a);
            heap::dec(*b);
            Some(ctx.nullary(index))
        }
        (Builtin::MapNew, []) => Some(ctx.tables.empty_map),
        (Builtin::MapInsert, [m, k, v]) if heap::kind(*m) == KIND_MAP && heap::native_key(*k) => {
            let tables = Rc::clone(&ctx.tables);
            Some(ctx.heap.map_insert(&tables.layouts, *m, *k, *v))
        }
        (Builtin::MapGet, [m, k]) if heap::kind(*m) == KIND_MAP && heap::native_key(*k) => {
            let some = ctx.tables.layouts.some?;
            let none = ctx.tables.layouts.none?;
            let o = obj(*m);
            let answer = match map::get(&ctx.tables.layouts, o, *k) {
                Some(v) => {
                    heap::inc(v);
                    let c = ctx.heap.alloc(KIND_CTOR, 0, 1, some);
                    unsafe { set_word(c, 0, v) };
                    c as Word
                }
                None => ctx.nullary(none),
            };
            heap::dec(*m);
            heap::dec(*k);
            Some(answer)
        }
        (Builtin::MapContains, [m, k]) if heap::kind(*m) == KIND_MAP && heap::native_key(*k) => {
            let found = map::get(&ctx.tables.layouts, obj(*m), *k).is_some();
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
            let keys = which == Builtin::MapKeys;
            let items: Vec<Word> = map::to_vec(o)
                .into_iter()
                .map(|(k, v)| {
                    let w = if keys { k } else { v };
                    heap::inc(w);
                    w
                })
                .collect();
            let out = ctx.heap.list_from(&items);
            heap::dec(*m);
            Some(out)
        }
        (Builtin::MapEntries, [m]) if heap::kind(*m) == KIND_MAP => {
            let o = obj(*m);
            let shape = ctx.tables.layouts.entry_shape();
            let mut items = Vec::with_capacity(map::len(o));
            for (k, v) in map::to_vec(o) {
                heap::inc(k);
                heap::inc(v);
                let e = ctx.heap.alloc(KIND_RECORD, 0, 2, shape);
                unsafe {
                    set_word(e, 0, k);
                    set_word(e, 1, v);
                }
                items.push(e as Word);
            }
            let out = ctx.heap.list_from(&items);
            heap::dec(*m);
            Some(out)
        }
        (Builtin::MapOfEntries, [xs]) if heap::kind(*xs) == KIND_LIST => {
            let tables = Rc::clone(&ctx.tables);
            let entries = list::to_vec(obj(*xs));
            let n = entries.len();
            let (key, value) = (Symbol::new("key"), Symbol::new("value"));
            // Every entry must be a record with both fields and a key the map can order, or the
            // interpreter's implementation raises the right diagnostic instead.
            let mut pairs = Vec::with_capacity(n);
            for e in entries {
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
            let mut m = ctx.heap.map_new();
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
            let mut m = *a;
            for (k, v) in map::to_vec(o) {
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

/// The half-open range a slicing builtin was given over `len` bytes, as `range_args` admits it:
/// never clamped, so anything outside is `None` and the interpreter's to refuse.
fn slice_range(start: Word, end: Word, len: usize) -> Option<(usize, usize)> {
    let (start, end) = (heap::as_int(start)?, heap::as_int(end)?);
    if start < 0 || end < start || !usize::try_from(end).is_ok_and(|e| e <= len) {
        return None;
    }
    Some((start as usize, end as usize))
}

/// Where `needle` first occurs in `hay` at or after `from`: an empty needle occurs at `from`,
/// as the interpreter's `find` answers.
fn find_bytes(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() {
        return Some(from);
    }
    memchr::memmem::find(&hay[from..], needle).map(|at| from + at)
}

/// The byte offset of the `n`-th character boundary, as the interpreter's `char_offset` finds it.
fn char_offset(s: &str, n: usize) -> usize {
    s.char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(s.len()))
        .nth(n)
        .unwrap_or(s.len())
}

/// A list of `items`, which it takes.
fn list_of(ctx: &mut Ctx, items: &[Word]) -> Word {
    ctx.heap.list_from(items)
}

/// `Some(at)` or `None` as the prelude's constructors, when the unit knows them.
fn position(ctx: &mut Ctx, at: Option<usize>) -> Option<Word> {
    let some = ctx.tables.layouts.some?;
    let none = ctx.tables.layouts.none?;
    Some(match at {
        Some(i) => {
            let c = ctx.heap.alloc(KIND_CTOR, 0, 1, some);
            unsafe { set_word(c, 0, heap::imm(i as i64)) };
            c as Word
        }
        None => ctx.nullary(none),
    })
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
    if heap::world_independent(w) {
        tables.memoize(index as usize, w);
    }
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
    let items = list::to_vec(items);
    let mut out = Vec::with_capacity(items.len());
    for x in items {
        heap::inc(x);
        let r = call_value(ctx, f, &[x]);
        let c = unsafe { &mut *ctx };
        if c.failed != 0 {
            return 0;
        }
        out.push(r);
    }
    let c = unsafe { &mut *ctx };
    let out = c.heap.list_from(&out);
    heap::dec(list);
    heap::dec(f);
    out
}

/// `filter(xs, p)`: the elements `p` answers `true` for. Takes the list and the predicate.
pub unsafe extern "C" fn rt_filter(ctx: *mut Ctx, list: i64, p: i64) -> i64 {
    let c = unsafe { &mut *ctx };
    let Some(items) = native_list(c, list, "filter") else {
        return 0;
    };
    let items = list::to_vec(items);
    let mut kept = Vec::new();
    for x in items {
        heap::inc(x);
        let r = call_value(ctx, p, &[x]);
        let c = unsafe { &mut *ctx };
        if c.failed != 0 {
            return 0;
        }
        match heap::as_bool(r) {
            Some(true) => {
                heap::inc(x);
                kept.push(x);
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
    let c = unsafe { &mut *ctx };
    let out = c.heap.list_from(&kept);
    heap::dec(list);
    heap::dec(p);
    out
}

/// `fold(xs, init, f)`. Takes all three.
pub unsafe extern "C" fn rt_fold(ctx: *mut Ctx, list: i64, init: i64, f: i64) -> i64 {
    let c = unsafe { &mut *ctx };
    let Some(items) = native_list(c, list, "fold") else {
        return 0;
    };
    // The list is held until the walk ends, so `f` may run whatever it likes over it.
    let mut acc = init;
    let mut failed = false;
    list::for_each(items, &mut |x| {
        if failed {
            return;
        }
        heap::inc(x);
        acc = call_value(ctx, f, &[acc, x]);
        failed = unsafe { (*ctx).failed } != 0;
    });
    if failed {
        return 0;
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
        let mut acc = init;
        for (k, v) in map::to_vec(o) {
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

/// What a fused `iterate` loop raises: `what` 0 for a budget under one (`n` the budget), 1 for
/// a budget run out (`n` the budget), 2 for a step that answered neither `Continue` nor `Stop`
/// (`n` the answer). Takes the answer in the last case.
pub unsafe extern "C" fn rt_iterate_bad(ctx: *mut Ctx, what: i64, n: i64) {
    let ctx = unsafe { &mut *ctx };
    let d = match what {
        0 => error(format!(
            "`iterate` needs a budget of at least 1, and this is {n}"
        )),
        1 => error(format!("`iterate` did not stop within its budget of {n}")),
        _ => {
            let d = error(format!(
                "the step given to `iterate` answered {}, not `Continue` or `Stop`",
                ctx.type_name(n)
            ));
            heap::dec(n);
            d
        }
    };
    ctx.fail(d);
}

/// A range longer than the interpreter's `range` admits, raised where a fused loop would have
/// walked it.
pub unsafe extern "C" fn rt_bad_range(ctx: *mut Ctx, lo: i64, hi: i64) {
    let ctx = unsafe { &mut *ctx };
    let d = error(format!(
        "`range` of {} elements exceeds the limit of {RANGE_LIMIT}",
        hi.saturating_sub(lo)
    ));
    ctx.fail(d);
}

/// The most elements the interpreter's `range` builds, which a fused loop holds to as well.
pub const RANGE_LIMIT: i64 = 10_000_000;

/// The element at `i` of a list a fused loop walks by index, held once more. Reads the list;
/// the loop checked the kind and the bound.
pub unsafe extern "C" fn rt_list_get(_ctx: *mut Ctx, list: i64, i: i64) -> i64 {
    let w = list::get(obj(list), i as usize);
    heap::inc(w);
    w
}

/// `xs` with `x` appended, for a fused `map` or `filter` building its answer. Takes both.
pub unsafe extern "C" fn rt_list_push(ctx: *mut Ctx, xs: i64, x: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    ctx.heap.list_push(xs, x)
}

/// A fused loop was handed something other than a list, which the runtime's loop refuses the
/// same way.
pub unsafe extern "C" fn rt_not_a_list(ctx: *mut Ctx, which: i64, value: i64) {
    let ctx = unsafe { &mut *ctx };
    let what = match which {
        0 => "fold",
        1 => "map",
        _ => "filter",
    };
    let d = error(format!(
        "`{what}` needs a List, and this is {}",
        ctx.type_name(value)
    ));
    heap::dec(value);
    ctx.fail(d);
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
    if n == 0 {
        return ctx.nullary(index as u32);
    }
    let args = args_of(args, n);
    let o = ctx
        .heap
        .alloc(KIND_CTOR, flat_over(args), n as u32, index as u32);
    for (i, w) in args.iter().enumerate() {
        unsafe { set_word(o, i, *w) };
    }
    o as Word
}

/// A record literal: the fields in the shape's own sorted order. Takes the fields.
pub unsafe extern "C" fn rt_record(ctx: *mut Ctx, shape: i64, args: *const i64, n: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let args = args_of(args, n);
    let o = ctx
        .heap
        .alloc(KIND_RECORD, flat_over(args), n as u32, shape as u32);
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
        unsafe { (*o).flags &= flat_over(written) | !heap::FLAT };
        return base;
    }
    // A fresh record: the written fields at their offsets, and the rest copied out of the base
    // by offset when it has the shape, or by name when the lowering's guess at a base was a
    // record of another shape — which is only ever a value that dies here, so nothing is read
    // from it unless the literal left a field unwritten.
    let tables = Rc::clone(&ctx.tables);
    let width = tables.layouts.shape_width(shape);
    let flat = flat_over(written) & unsafe { (*o).flags };
    let out = ctx.heap.alloc(KIND_RECORD, flat, width as u32, shape);
    // Which offsets the literal wrote: a bit each for a shape a word of bits covers, which is
    // every shape a front end has, and a list for a wider one.
    let mut mask = 0u128;
    let mut wide = Vec::new();
    if width > 128 {
        wide = vec![false; width];
    }
    for (w, at) in written.iter().zip(offsets) {
        let at = *at as usize;
        unsafe { set_word(out, at, *w) };
        if width > 128 {
            wide[at] = true;
        } else {
            mask |= 1 << at;
        }
    }
    let filled = |i: usize| {
        if width > 128 {
            wide[i]
        } else {
            mask >> i & 1 == 1
        }
    };
    if written.len() < width {
        let base_shape = unsafe { (*o).layout };
        if base_shape == shape {
            for i in (0..width).filter(|i| !filled(*i)) {
                let w = unsafe { word_at(o, i) };
                heap::inc(w);
                unsafe { set_word(out, i, w) };
            }
        } else {
            let names = tables.layouts.shape_names(shape);
            for i in (0..width).filter(|i| !filled(*i)) {
                let Some(at) = tables.layouts.offset(base_shape, &names[i]) else {
                    let d = error(format!("this record has no field `{}`", names[i]));
                    return ctx.fail(d);
                };
                let w = unsafe { word_at(o, at) };
                heap::inc(w);
                unsafe { set_word(out, i, w) };
            }
        }
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
    ctx.heap.list_from(args_of(args, n))
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
    if i < 0 || i >= list::len(o) as i64 {
        let d = error("a list pattern read past the end of the list");
        return ctx.fail(d);
    }
    let w = list::get(o, i as usize);
    heap::inc(w);
    w
}

/// What a `..rest` binds: the list from `from` on, sharing the trie and copying at most a
/// tail, as `ply_eval`'s `skip` does at the same point. Reads.
pub unsafe extern "C" fn rt_list_rest(ctx: *mut Ctx, value: i64, from: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    if heap::kind(value) != KIND_LIST {
        let d = error("a list pattern bound a value that is not a list");
        return ctx.fail(d);
    }
    ctx.heap.list_skip(value, from.max(0) as usize)
}

/// One argument of a constructor value, once the compiled test has said it is that constructor:
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

/// `map_get` for a `match` that unwraps its answer at once: the value held once more, or `0`
/// for a key the map does not hold, with no constructor built between. Takes the map and the
/// key; a map or a key the native path does not serve is answered through the interpreter's
/// `map_get` and unwrapped here.
pub unsafe extern "C" fn rt_map_lookup(ctx: *mut Ctx, m: i64, k: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    if heap::kind(m) == KIND_MAP && heap::native_key(k) {
        let found = map::get(&ctx.tables.layouts, obj(m), k);
        if let Some(v) = found {
            heap::inc(v);
        }
        heap::dec(m);
        heap::dec(k);
        return found.unwrap_or(0);
    }
    let answer = builtin_over_values(ctx, Builtin::MapGet, &[m, k]);
    unwrapped(ctx, answer)
}

/// [`heap::FLAT`] when none of `words` holds a count.
fn flat_over(words: &[Word]) -> u8 {
    if words.iter().all(|w| heap::is_imm(*w)) {
        heap::FLAT
    } else {
        0
    }
}

/// A fresh object of `kind` with `len` words, its header written and its fields left for the
/// compiled code that asked, which stores them itself.
pub unsafe extern "C" fn rt_alloc(
    ctx: *mut Ctx,
    kind: i64,
    len: i64,
    layout: i64,
    flags: i64,
) -> i64 {
    let ctx = unsafe { &mut *ctx };
    ctx.heap
        .alloc(kind as u8, flags as u8, len as u32, layout as u32) as Word
}

/// A builtin called directly by compiled code, with no dispatch on its index and no argument
/// array: the native path, or the interpreter's over the values. Takes the arguments.
fn direct(ctx: &mut Ctx, b: Builtin, args: &[Word]) -> Word {
    ctx.builtin_calls += 1;
    match native_builtin(ctx, b, args) {
        Some(w) => w,
        None => builtin_over_values(ctx, b, args),
    }
}

/// The value inside an `Option` answer, held once more, or `0` for `None`; the answer is let go.
fn unwrapped(ctx: &mut Ctx, answer: Word) -> Word {
    if ctx.failed != 0 {
        return 0;
    }
    let o = obj(answer);
    let held = unsafe { (*o).len == 1 && ctx.tables.layouts.some == Some((*o).layout) };
    let v = if held {
        let v = unsafe { word_at(o, 0) };
        heap::inc(v);
        v
    } else {
        0
    };
    heap::dec(answer);
    v
}

pub unsafe extern "C" fn rt_list_index(ctx: *mut Ctx, xs: i64, i: i64) -> i64 {
    direct(unsafe { &mut *ctx }, Builtin::ListAt, &[xs, i])
}

/// `list_at` for a `match` that unwraps its answer at once, like [`rt_map_lookup`].
pub unsafe extern "C" fn rt_list_lookup(ctx: *mut Ctx, xs: i64, i: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    if heap::kind(xs) == KIND_LIST
        && let Some(index) = heap::as_int(i)
    {
        ctx.builtin_calls += 1;
        let o = obj(xs);
        let w = if index >= 0 && (index as usize) < list::len(o) {
            let item = list::get(o, index as usize);
            heap::inc(item);
            item
        } else {
            0
        };
        heap::dec(xs);
        return w;
    }
    let answer = builtin_over_values(ctx, Builtin::ListAt, &[xs, i]);
    unwrapped(ctx, answer)
}

pub unsafe extern "C" fn rt_push(ctx: *mut Ctx, xs: i64, x: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    if heap::kind(xs) == KIND_LIST {
        ctx.builtin_calls += 1;
        return ctx.heap.list_push(xs, x);
    }
    direct(ctx, Builtin::Push, &[xs, x])
}

pub unsafe extern "C" fn rt_map_insert(ctx: *mut Ctx, m: i64, k: i64, v: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    if heap::kind(m) == KIND_MAP && heap::native_key(k) {
        ctx.builtin_calls += 1;
        let tables = Rc::clone(&ctx.tables);
        return ctx.heap.map_insert(&tables.layouts, m, k, v);
    }
    direct(ctx, Builtin::MapInsert, &[m, k, v])
}

pub unsafe extern "C" fn rt_map_contains(ctx: *mut Ctx, m: i64, k: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    if heap::kind(m) == KIND_MAP && heap::native_key(k) {
        ctx.builtin_calls += 1;
        let found = map::get(&ctx.tables.layouts, obj(m), k).is_some();
        heap::dec(m);
        heap::dec(k);
        return heap::bool(found);
    }
    direct(ctx, Builtin::MapContains, &[m, k])
}

pub unsafe extern "C" fn rt_map_get(ctx: *mut Ctx, m: i64, k: i64) -> i64 {
    direct(unsafe { &mut *ctx }, Builtin::MapGet, &[m, k])
}

pub unsafe extern "C" fn rt_compare(ctx: *mut Ctx, a: i64, b: i64) -> i64 {
    direct(unsafe { &mut *ctx }, Builtin::Compare, &[a, b])
}

pub unsafe extern "C" fn rt_byte_of_int(ctx: *mut Ctx, n: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    match heap::as_int(n).and_then(|v| u8::try_from(v).ok()) {
        Some(b) => {
            ctx.builtin_calls += 1;
            ctx.tables.byte(b)
        }
        None => direct(ctx, Builtin::ByteOfInt, &[n]),
    }
}

pub unsafe extern "C" fn rt_bytes_scan(
    ctx: *mut Ctx,
    hay: i64,
    from: i64,
    members: i64,
    max: i64,
) -> i64 {
    direct(
        unsafe { &mut *ctx },
        Builtin::BytesScan,
        &[hay, from, members, max],
    )
}

pub unsafe extern "C" fn rt_bytes_scan_until(
    ctx: *mut Ctx,
    hay: i64,
    from: i64,
    members: i64,
    max: i64,
) -> i64 {
    direct(
        unsafe { &mut *ctx },
        Builtin::BytesScanUntil,
        &[hay, from, members, max],
    )
}

pub unsafe extern "C" fn rt_bytes_slice(ctx: *mut Ctx, b: i64, s: i64, e: i64) -> i64 {
    direct(unsafe { &mut *ctx }, Builtin::BytesSlice, &[b, s, e])
}

pub unsafe extern "C" fn rt_bytes_concat(ctx: *mut Ctx, a: i64, b: i64) -> i64 {
    direct(unsafe { &mut *ctx }, Builtin::BytesConcat, &[a, b])
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
        ("rt_ctor_arg", rt_ctor_arg as *const u8),
        ("rt_map_lookup", rt_map_lookup as *const u8),
        ("rt_list_index", rt_list_index as *const u8),
        ("rt_list_lookup", rt_list_lookup as *const u8),
        ("rt_push", rt_push as *const u8),
        ("rt_map_insert", rt_map_insert as *const u8),
        ("rt_map_contains", rt_map_contains as *const u8),
        ("rt_map_get", rt_map_get as *const u8),
        ("rt_compare", rt_compare as *const u8),
        ("rt_byte_of_int", rt_byte_of_int as *const u8),
        ("rt_bytes_scan", rt_bytes_scan as *const u8),
        ("rt_bytes_scan_until", rt_bytes_scan_until as *const u8),
        ("rt_bytes_slice", rt_bytes_slice as *const u8),
        ("rt_bytes_concat", rt_bytes_concat as *const u8),
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
        ("rt_iterate_bad", rt_iterate_bad as *const u8),
        ("rt_bytes_join", rt_bytes_join as *const u8),
        ("rt_list_get", rt_list_get as *const u8),
        ("rt_list_push", rt_list_push as *const u8),
        ("rt_not_a_list", rt_not_a_list as *const u8),
        ("rt_bad_range", rt_bad_range as *const u8),
        ("rt_shift_count", rt_shift_count as *const u8),
        ("rt_dup", rt_dup as *const u8),
        ("rt_dec", rt_dec as *const u8),
        ("rt_alloc", rt_alloc as *const u8),
        ("rt_reset", rt_reset as *const u8),
        ("rt_constant", rt_constant as *const u8),
    ]
}
