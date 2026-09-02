//! What compiled code calls when it leaves native instructions.

// # Safety, once, for every `rt_*` helper below
//
// Each is called **only** from code Cranelift emitted in [`crate::jit`], which
// passes the `*mut Ctx` that [`crate::entry`] created for the entry currently
// running and handles this same context produced. `Ctx` outlives every call into
// the body — `Bodies` holds it in a `RefCell` for the duration — and an entry
// cannot nest, so the `&mut` taken here is unique. A handle is either an index
// into `Ctx::slots` or a negative index into the constant pool, and `Ctx::read`
// bounds-checks both.
//
// Nothing outside this crate may call one: they are `pub` because [`symbols`]
// hands their addresses to the JIT, not because they are an API.
#![allow(clippy::missing_safety_doc)]

use crate::jit::Entry;
use ply_eval::{Builtin, Closure, ClosureKind, Fields, Step, Value, values_equal};
use ply_span::{Diagnostic, Span, Symbol, codes};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

/// The compile-time tables a compiled program reads through its context: shared, immutable, and one
/// per [`crate::jit::Unit`] rather than one per call.
pub struct Tables {
    /// The constant pool negative handles index.
    pub consts: Vec<Value>,
    pub ctors: Vec<(Symbol, usize)>,
    pub shapes: Vec<Vec<Symbol>>,
    /// Every field name a compiled body reads, so that a field access is an index rather than a
    /// `Symbol` rebuilt per evaluation.
    pub fields: Vec<Symbol>,
    /// Every builtin a compiled body may call.
    pub builtins: Vec<Builtin>,
    /// Every compiled function by index, as its finalized address: what a native closure's
    /// `code` is read from when the closure is built, since no address exists until the module
    /// is finalized and every closure is built after that.
    pub functions: Vec<usize>,
    /// What each pure nullary function evaluated to, by the index above, once it has: the
    /// machine's memo (`ply_eval::memo`) for compiled code, so a placeholder value built in Ply
    /// is built once per unit rather than once per call.
    pub memo: RefCell<Vec<Option<Value>>>,
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

/// The floor under the value arena between entries.
const RETAINED_SLOTS: usize = 4096;

/// How far above what an entry used the buffer may sit before [`Ctx::end`] hands the slack back.
pub const SLACK: usize = 2;

/// The failure a compiled function reports by, and the fuel it spends.
pub const FAILED_OUT_OF_FUEL: i64 = 2;

#[repr(C)]
pub struct Ctx {
    pub failed: i64,
    /// Nested native calls still allowed, counted down on entry to a compiled function and back up
    /// on its normal return.
    pub fuel: i64,
    pub slots: Vec<Value>,
    /// The slots the entry that just finished used, kept because [`Ctx::end`] clears the arena and
    /// the number is otherwise gone.
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
            slots: Vec::with_capacity(64),
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
        // Every path out of an entry calls `end`, so this is one comparison against an empty arena.
        if !self.slots.is_empty() {
            self.unclosed_entries += 1;
            self.end();
        }
    }

    /// The other end of [`Ctx::begin`]: the entry gives back what it used.
    pub fn end(&mut self) {
        self.last_entry = self.slots.len();
        self.slots.clear();
        let keep = self.last_entry.max(RETAINED_SLOTS);
        let trigger = self.last_entry.saturating_mul(SLACK).max(RETAINED_SLOTS);
        if self.slots.capacity() > trigger {
            self.slots.shrink_to(keep);
        }
    }

    /// How much of the value arena the entry that just finished used.
    pub fn arena_after_entry(&self) -> usize {
        if self.slots.is_empty() {
            self.last_entry
        } else {
            self.slots.len()
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

    pub fn read(&self, handle: i64) -> &Value {
        if handle < 0 {
            &self.tables.consts[(-handle - 1) as usize]
        } else {
            &self.slots[handle as usize]
        }
    }

    pub fn push(&mut self, value: Value) -> i64 {
        self.slots.push(value);
        (self.slots.len() - 1) as i64
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
}

/// A value out of its slot when `owned` — nothing reads the slot again, so the value is handed
/// over rather than cloned — and a clone otherwise. A constant (a negative handle) is never taken.
fn take_or_clone(ctx: &mut Ctx, handle: i64, owned: bool) -> Value {
    if owned && handle >= 0 {
        std::mem::replace(&mut ctx.slots[handle as usize], Value::Unit)
    } else {
        ctx.read(handle).clone()
    }
}

/// The arguments a helper consumes, taken out of their slots: every handle the code generator
/// hands a consuming helper is either a binding at its last use or a fresh temporary — a read
/// that is not the last is duplicated into a slot of its own first (`rt_dup`) — so nothing reads
/// these slots again.
fn args_take(ctx: &mut Ctx, ptr: *const i64, n: i64) -> Vec<Value> {
    let handles = unsafe { std::slice::from_raw_parts(ptr, n as usize) };
    // The interpreter's own free list: `builtins::call` gives the vector back, so a builtin call
    // allocates no argument vector in the steady state here either.
    let mut out = ply_eval::argv::take(handles.len());
    out.extend(handles.iter().map(|h| take_or_clone(ctx, *h, true)));
    out
}

/// A fresh slot holding a clone — Perceus's `dup`: what a read that is not the binding's last
/// evaluates to, so that whoever consumes the handle may take it while the binding's slot stays.
pub unsafe extern "C" fn rt_dup(ctx: *mut Ctx, handle: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let v = ctx.read(handle).clone();
    ctx.push(v)
}

/// A failure inside compiled code, for the spike's own reporting.
fn error(message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(codes::RUNTIME_ERROR, message.into()).primary(Span::DUMMY, "in compiled code")
}

pub unsafe extern "C" fn rt_box_int(ctx: *mut Ctx, v: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    ctx.push(Value::Int(v))
}

pub unsafe extern "C" fn rt_box_bool(ctx: *mut Ctx, v: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    ctx.push(Value::Bool(v != 0))
}

pub unsafe extern "C" fn rt_unbox_int(ctx: *mut Ctx, handle: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    match ctx.read(handle) {
        Value::Int(i) => *i,
        other => {
            let d = error(format!("an `Int` operation on a {}", other.type_name()));
            ctx.fail(d)
        }
    }
}

pub unsafe extern "C" fn rt_unbox_bool(ctx: *mut Ctx, handle: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    match ctx.read(handle) {
        Value::Bool(b) => i64::from(*b),
        other => {
            let d = error(format!("a condition of type {}", other.type_name()));
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
    let value = ctx.read(index).clone();
    let rebuilt = match &value {
        Value::Bytes(b) => Value::bytes(b.as_ref()),
        Value::Str(s) => Value::str(s.as_ref()),
        Value::Ctor { name, args } => Value::ctor(name.clone(), args.as_ref().clone()),
        other => other.clone(),
    };
    ctx.push(rebuilt)
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

/// `==` and `!=` on anything that is not a pair of `Int`s or a pair of `Bool`s, answered by the
/// evaluator's own comparison so the two cannot disagree.
pub unsafe extern "C" fn rt_equal(ctx: *mut Ctx, a: i64, b: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let (l, r) = (ctx.read(a).clone(), ctx.read(b).clone());
    match values_equal(&l, &r, Span::DUMMY) {
        Ok(eq) => i64::from(eq),
        Err(d) => ctx.fail(d),
    }
}

/// `++`, which is `String` concatenation only, built from the same two `Value::as_str` calls
/// `interp::strict_binary` uses so a non-`Str` operand raises the identical error.
pub unsafe extern "C" fn rt_concat(ctx: *mut Ctx, a: i64, b: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let (l, r) = (ctx.read(a).clone(), ctx.read(b).clone());
    let joined = match (l.as_str(Span::DUMMY, "`++`"), r.as_str(Span::DUMMY, "`++`")) {
        (Ok(x), Ok(y)) => format!("{x}{y}"),
        (Err(d), _) | (_, Err(d)) => return ctx.fail(d),
    };
    ctx.push(Value::str(joined))
}

pub unsafe extern "C" fn rt_builtin(ctx: *mut Ctx, index: i64, args: *const i64, n: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let b = ctx.tables.builtins[index as usize];
    let args = args_take(ctx, args, n);
    ctx.builtin_calls += 1;
    match ply_eval::builtins::call(b, args, ctx.cells.arena_mut(), Span::DUMMY) {
        Ok(Step::Done(v)) => ctx.push(v),
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

/// A closure over `env`: the compiled function `index` names, entered with the captured values
/// as its leading arguments.
pub unsafe extern "C" fn rt_closure(
    ctx: *mut Ctx,
    index: i64,
    arity: i64,
    env: *const i64,
    n: i64,
) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let code = ctx.tables.functions[index as usize];
    // The captures are the closure's own exact vector, not a pooled buffer.
    let env = unsafe { std::slice::from_raw_parts(env, n as usize) };
    let captured: Vec<Value> = env.iter().map(|h| take_or_clone(ctx, *h, true)).collect();
    ctx.push(Value::Closure(Arc::new(Closure {
        name: None,
        kind: ClosureKind::Native {
            code,
            arity: arity as usize,
            captured,
        },
    })))
}

/// A builtin used as a value: the interpreter's own closure kind for it.
pub unsafe extern "C" fn rt_builtin_value(ctx: *mut Ctx, index: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let b = ctx.tables.builtins[index as usize];
    ctx.push(Value::builtin(b))
}

/// A constructor used as a value, likewise.
/// The value of the pure nullary function at `index`: remembered on its first evaluation when
/// it is world-independent, evaluated on every call otherwise.
pub unsafe extern "C" fn rt_constant(ctx: *mut Ctx, index: i64) -> i64 {
    let tables = Rc::clone(&unsafe { &*ctx }.tables);
    if let Some(Some(value)) = tables.memo.borrow().get(index as usize) {
        let value = value.clone();
        return unsafe { &mut *ctx }.push(value);
    }
    // SAFETY: as in `call_value`: a finalized address of this unit's own function, alive for as
    // long as the context is, with the signature every compiled function has; a nullary function
    // reads no argument, so the pointer is never dereferenced.
    let f: Entry = unsafe { std::mem::transmute::<usize, Entry>(tables.functions[index as usize]) };
    let handle = unsafe { f(ctx, std::ptr::null()) };
    let c = unsafe { &mut *ctx };
    if c.failed == 0 && ply_eval::memo::world_independent(c.read(handle)) {
        let mut memo = tables.memo.borrow_mut();
        let index = index as usize;
        if memo.len() <= index {
            memo.resize(index + 1, None);
        }
        memo[index] = Some(c.read(handle).clone());
    }
    handle
}

pub unsafe extern "C" fn rt_ctor_value(ctx: *mut Ctx, index: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let (name, arity) = ctx.tables.ctors[index as usize].clone();
    ctx.push(Value::Closure(Arc::new(Closure {
        name: Some(name.clone()),
        kind: ClosureKind::Ctor { name, arity },
    })))
}

/// A call through a value: a local binding, a callee that is an expression, or a callback.
pub unsafe extern "C" fn rt_call(ctx: *mut Ctx, callee: i64, args: *const i64, n: i64) -> i64 {
    let (callee, args) = {
        let c = unsafe { &mut *ctx };
        let args = args_take(c, args, n);
        (take_or_clone(c, callee, true), args)
    };
    call_value(ctx, &callee, args)
}

/// Applies `callee` to `args` and answers the handle of the result, or 0 with the context failed.
/// A native closure is entered directly; a builtin or a constructor is the interpreter's own; an
/// interpreted closure cannot be here, because the seam carries no function.
fn call_value(ctx: *mut Ctx, callee: &Value, args: Vec<Value>) -> i64 {
    let Value::Closure(closure) = callee else {
        let c = unsafe { &mut *ctx };
        let d = error(format!(
            "a call needs a function, and this is {}",
            callee.type_name()
        ));
        return c.fail(d);
    };
    match &closure.kind {
        ClosureKind::Native {
            code,
            arity,
            captured,
        } => {
            if args.len() != *arity {
                let c = unsafe { &mut *ctx };
                let d = error(format!(
                    "a compiled function takes {arity} arguments and was given {}",
                    args.len()
                ));
                return c.fail(d);
            }
            // The callee owns every parameter: the captures are cloned, since the closure may be
            // called again, and the arguments were taken or cloned by the caller's mask. A
            // stack array holds the handles for any arity a body has, so a call allocates nothing.
            let mut handles = [0i64; 64];
            let mut spilled: Vec<i64> = Vec::new();
            let total = captured.len() + args.len();
            {
                let c = unsafe { &mut *ctx };
                let mut push = |h: i64, i: usize| {
                    if total <= handles.len() {
                        handles[i] = h;
                    } else {
                        spilled.push(h);
                    }
                };
                for (i, v) in captured.iter().enumerate() {
                    push(c.push(v.clone()), i);
                }
                let mut args = args;
                for (i, v) in args.drain(..).enumerate() {
                    push(c.push(v), captured.len() + i);
                }
                ply_eval::argv::give(args);
            }
            let ptr = if total <= handles.len() {
                handles.as_ptr()
            } else {
                spilled.as_ptr()
            };
            // SAFETY: `code` came out of `Tables::functions`, the finalized addresses of this
            // unit's own functions, which `Bodies` keeps alive for as long as this context
            // exists; the signature is the one every compiled function has.
            let f: Entry = unsafe { std::mem::transmute::<usize, Entry>(*code) };
            unsafe { f(ctx, ptr) }
        }
        ClosureKind::Builtin(b) => {
            let c = unsafe { &mut *ctx };
            c.builtin_calls += 1;
            match ply_eval::builtins::call(*b, args, c.cells.arena_mut(), Span::DUMMY) {
                Ok(Step::Done(v)) => c.push(v),
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
            let c = unsafe { &mut *ctx };
            if args.len() != *arity {
                let d = error(format!(
                    "the constructor `{name}` takes {arity} fields and was given {}",
                    args.len()
                ));
                return c.fail(d);
            }
            c.push(Value::ctor_pooled(name.clone(), args))
        }
        ClosureKind::Fn { .. } | ClosureKind::Code { .. } => {
            let c = unsafe { &mut *ctx };
            let d = error(
                "an interpreted closure reached compiled code, which has no machine to run it on",
            );
            c.fail(d)
        }
    }
}

/// The list a callback builtin walks, or a failure: taken, and walked by index, so the elements
/// are cloned one at a time as the interpreter clones them rather than all at once first.
fn list_of(ctx: *mut Ctx, handle: i64, what: &str) -> Option<ply_eval::List> {
    let c = unsafe { &mut *ctx };
    let value = take_or_clone(c, handle, true);
    match &value {
        Value::List(xs) => Some(xs.clone()),
        other => {
            let d = error(format!(
                "`{what}` needs a List, and this is {}",
                other.type_name()
            ));
            c.fail(d);
            None
        }
    }
}

/// `map(xs, f)`: `f` on every element, in order.
pub unsafe extern "C" fn rt_map(ctx: *mut Ctx, list: i64, f: i64) -> i64 {
    let Some(items) = list_of(ctx, list, "map") else {
        return 0;
    };
    let f = take_or_clone(unsafe { &mut *ctx }, f, true);
    let mut out = Vec::with_capacity(items.len());
    for x in items.iter().cloned() {
        let r = call_value(ctx, &f, ply_eval::argv::of([x]));
        let c = unsafe { &mut *ctx };
        if c.failed != 0 {
            return 0;
        }
        out.push(take_or_clone(c, r, true));
    }
    unsafe { &mut *ctx }.push(Value::list(out))
}

/// `filter(xs, p)`: the elements `p` answers `true` for.
pub unsafe extern "C" fn rt_filter(ctx: *mut Ctx, list: i64, p: i64) -> i64 {
    let Some(items) = list_of(ctx, list, "filter") else {
        return 0;
    };
    let p = take_or_clone(unsafe { &mut *ctx }, p, true);
    let mut out = Vec::new();
    for x in items.iter().cloned() {
        let r = call_value(ctx, &p, ply_eval::argv::of([x.clone()]));
        let c = unsafe { &mut *ctx };
        if c.failed != 0 {
            return 0;
        }
        match c.read(r) {
            Value::Bool(true) => out.push(x),
            Value::Bool(false) => {}
            other => {
                let d = error(format!(
                    "the predicate given to `filter` answered {}, not a Bool",
                    other.type_name()
                ));
                return c.fail(d);
            }
        }
    }
    unsafe { &mut *ctx }.push(Value::list(out))
}

/// `fold(xs, init, f)`.
pub unsafe extern "C" fn rt_fold(ctx: *mut Ctx, list: i64, init: i64, f: i64) -> i64 {
    let Some(items) = list_of(ctx, list, "fold") else {
        return 0;
    };
    let (f, mut acc) = {
        let c = unsafe { &mut *ctx };
        let acc = take_or_clone(c, init, true);
        (take_or_clone(c, f, true), acc)
    };
    for x in items.iter().cloned() {
        let r = call_value(ctx, &f, ply_eval::argv::of([acc, x]));
        let c = unsafe { &mut *ctx };
        if c.failed != 0 {
            return 0;
        }
        // Out of its slot, so the accumulator the next step receives is held by nobody else and
        // a `push` onto it writes in place.
        acc = take_or_clone(c, r, true);
    }
    unsafe { &mut *ctx }.push(acc)
}

/// `map_fold(m, init, f)`: `f` on every entry in ascending key order, over a snapshot of the
/// entries as the interpreter's loop takes one.
pub unsafe extern "C" fn rt_map_fold(ctx: *mut Ctx, map: i64, init: i64, f: i64) -> i64 {
    let (entries, f, mut acc) = {
        let c = unsafe { &mut *ctx };
        let value = take_or_clone(c, map, true);
        let entries: Vec<(Value, Value)> = match &value {
            Value::Map(m) => m.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            other => {
                let d = error(format!(
                    "`map_fold` needs a Map, and this is {}",
                    other.type_name()
                ));
                return c.fail(d);
            }
        };
        let acc = take_or_clone(c, init, true);
        (entries, take_or_clone(c, f, true), acc)
    };
    for (k, v) in entries {
        let r = call_value(ctx, &f, ply_eval::argv::of([acc, k, v]));
        let c = unsafe { &mut *ctx };
        if c.failed != 0 {
            return 0;
        }
        acc = take_or_clone(c, r, true);
    }
    unsafe { &mut *ctx }.push(acc)
}

/// `iterate(seed, budget, f)`: `f` until it answers `Stop`, or the budget runs out and the call
/// fails the way the interpreter's raises.
pub unsafe extern "C" fn rt_iterate(ctx: *mut Ctx, seed: i64, budget: i64, f: i64) -> i64 {
    let (f, mut state, budget) = {
        let c = unsafe { &mut *ctx };
        let budget = match c.read(budget) {
            Value::Int(n) if *n >= 1 => *n,
            Value::Int(n) => {
                let d = error(format!(
                    "`iterate` needs a budget of at least 1, and this is {n}"
                ));
                return c.fail(d);
            }
            other => {
                let d = error(format!(
                    "`iterate` needs an Int budget, and this is {}",
                    other.type_name()
                ));
                return c.fail(d);
            }
        };
        let seed = take_or_clone(c, seed, true);
        (take_or_clone(c, f, true), seed, budget)
    };
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
        let r = call_value(ctx, &f, ply_eval::argv::of([state]));
        let c = unsafe { &mut *ctx };
        if c.failed != 0 {
            return 0;
        }
        // The step's answer leaves its slot, and its payload leaves the constructor when nothing
        // else holds it, so the state threaded through the loop stays uniquely held.
        let mut answer = take_or_clone(c, r, true);
        let payload = match &mut answer {
            Value::Ctor { name, args } if args.len() == 1 => match Arc::get_mut(args) {
                Some(args) => Some((name.as_str() == "Stop", args.pop().expect("one field"))),
                None => Some((name.as_str() == "Stop", args[0].clone())),
            },
            _ => None,
        };
        let stop = matches!(&answer, Value::Ctor { name, .. } if name.as_str() == "Stop");
        let go = matches!(&answer, Value::Ctor { name, .. } if name.as_str() == "Continue");
        match payload {
            Some((_, v)) if go => {
                state = v;
            }
            Some((_, v)) if stop => {
                return c.push(v);
            }
            _ => {
                let d = error(format!(
                    "the step given to `iterate` answered {}, not `Continue` or `Stop`",
                    answer.type_name()
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

pub unsafe extern "C" fn rt_ctor(ctx: *mut Ctx, index: i64, args: *const i64, n: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let name = ctx.tables.ctors[index as usize].0.clone();
    // The payload is the arguments' own exact vector, not a pooled buffer.
    let handles = unsafe { std::slice::from_raw_parts(args, n as usize) };
    let payload: Vec<Value> = handles
        .iter()
        .map(|h| take_or_clone(ctx, *h, true))
        .collect();
    ctx.push(Value::ctor(name, payload))
}

pub unsafe extern "C" fn rt_record(ctx: *mut Ctx, shape: i64, args: *const i64, n: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let tables = Rc::clone(&ctx.tables);
    let names = &tables.shapes[shape as usize];
    let mut args = args_take(ctx, args, n);
    let fields = names
        .iter()
        .cloned()
        .zip(args.drain(..))
        .collect::<Vec<_>>();
    ply_eval::argv::give(args);
    ctx.push(Value::Record(Arc::new(Fields::from_unsorted(fields))))
}

/// A record update: written in place when the literal names exactly the base's fields and
/// nothing else holds the base, built as written otherwise.
pub unsafe extern "C" fn rt_record_update(
    ctx: *mut Ctx,
    shape: i64,
    base: i64,
    args: *const i64,
    n: i64,
) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let tables = Rc::clone(&ctx.tables);
    let names = &tables.shapes[shape as usize];
    let mut written = args_take(ctx, args, n);
    // The base is taken too: at its last use that is the record itself, written in place when
    // nothing else holds it — ADR 0034's reuse reaching compiled code — and otherwise it is the
    // duplicate the read made, whose `make_mut` copies once.
    let mut base = take_or_clone(ctx, base, true);
    let Value::Record(fields) = &mut base else {
        let d = error(format!(
            "a record update needs a record, and this is {}",
            base.type_name()
        ));
        return ctx.fail(d);
    };
    let (sets, copies) = names.split_at(n as usize);
    let exact = fields.len() == names.len()
        && copies.iter().all(|c| fields.contains_key(c))
        && sets.iter().all(|s| fields.contains_key(s));
    if exact {
        let record = Arc::make_mut(fields);
        for (name, value) in sets.iter().zip(written.drain(..)) {
            record.insert(name.clone(), value);
        }
        ply_eval::argv::give(written);
        return ctx.push(base);
    }
    let mut out = Vec::with_capacity(names.len());
    for name in copies {
        match fields.get(name) {
            Some(v) => out.push((name.clone(), v.clone())),
            None => {
                let d = error(format!("this record has no field `{name}`"));
                return ctx.fail(d);
            }
        }
    }
    out.extend(sets.iter().cloned().zip(written.drain(..)));
    ply_eval::argv::give(written);
    ctx.push(Value::Record(Arc::new(Fields::from_unsorted(out))))
}

/// One field of a record, with the two failures the interpreter has here kept as failures rather
/// than answered.
pub unsafe extern "C" fn rt_field(ctx: *mut Ctx, base: i64, index: i64, own: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let name = ctx.tables.fields[index as usize].clone();
    // `own` is the lowering's mark: 1 is the base's last use, so the record leaves its slot and
    // a field of a record nothing else holds is moved out rather than cloned; 2 is the last use
    // of this *field* while the record stays, which takes the field in place when the record is
    // unshared — ADR 0034's fifth gate pair, reaching compiled code; 0 reads a local's record.
    if own == 2
        && base >= 0
        && let Value::Record(fields) = &mut ctx.slots[base as usize]
        && let Some(map) = Arc::get_mut(fields)
        && let Some(v) = map.insert(name.clone(), Value::Unit)
    {
        return ctx.push(v);
    }
    // 3: the base is a temporary — an expression's answer nobody else reads — so it is taken
    // as well, which keeps it from pinning its record in the arena until the entry ends.
    if (own == 1 || own == 3) && base >= 0 {
        let mut record = take_or_clone(ctx, base, true);
        if let Value::Record(fields) = &mut record
            && let Some(map) = Arc::get_mut(fields)
            && let Some(v) = map.insert(name.clone(), Value::Unit)
        {
            return ctx.push(v);
        }
        return match &record {
            Value::Record(fields) => match fields.get(&name) {
                Some(v) => {
                    let v = v.clone();
                    ctx.push(v)
                }
                None => {
                    let d = error(format!("this record has no field `{name}`"));
                    ctx.fail(d)
                }
            },
            other => {
                let d = error(format!(
                    "a field access needs a record, and this is {}",
                    other.type_name()
                ));
                ctx.fail(d)
            }
        };
    }
    match ctx.read(base) {
        Value::Record(fields) => match fields.get(&name) {
            Some(v) => {
                let v = v.clone();
                ctx.push(v)
            }
            None => {
                let d = error(format!("this record has no field `{name}`"));
                ctx.fail(d)
            }
        },
        other => {
            let d = error(format!(
                "a field access needs a record, and this is {}",
                other.type_name()
            ));
            ctx.fail(d)
        }
    }
}

/// A list literal, built from handles the way [`rt_record`] builds a record.
pub unsafe extern "C" fn rt_list(ctx: *mut Ctx, args: *const i64, n: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let args = args_take(ctx, args, n);
    ctx.push(Value::list(args))
}

/// Whether a value is a record whose field count a pattern admits: `exact` demands the count,
/// because a pattern without `..` matches only a record of exactly its fields.
pub unsafe extern "C" fn rt_record_fits(ctx: *mut Ctx, value: i64, len: i64, exact: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    match ctx.read(value) {
        Value::Record(fields) => i64::from(exact == 0 || fields.len() as i64 == len),
        _ => 0,
    }
}

/// Whether a record holds the field a pattern names, a missing one being a failed match rather
/// than the error [`rt_field`] raises.
pub unsafe extern "C" fn rt_record_has(ctx: *mut Ctx, value: i64, index: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let name = ctx.tables.fields[index as usize].clone();
    match ctx.read(value) {
        Value::Record(fields) => i64::from(fields.get(&name).is_some()),
        _ => 0,
    }
}

/// Whether a value is a list long enough for a pattern: `exact` demands the length, and otherwise
/// `len` is a minimum because a `..rest` takes the remainder.
pub unsafe extern "C" fn rt_list_fits(ctx: *mut Ctx, value: i64, len: i64, exact: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    match ctx.read(value) {
        Value::List(xs) => {
            let n = xs.len() as i64;
            i64::from(if exact != 0 { n == len } else { n >= len })
        }
        _ => 0,
    }
}

/// One element of a list, once [`rt_list_fits`] has admitted its length.
pub unsafe extern "C" fn rt_list_at(ctx: *mut Ctx, value: i64, i: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    match ctx.read(value) {
        Value::List(xs) => match xs.get(i as usize) {
            Some(v) => {
                let v = v.clone();
                ctx.push(v)
            }
            None => {
                let d = error("a list pattern read past the end of the list");
                ctx.fail(d)
            }
        },
        _ => {
            let d = error("a list pattern bound a value that is not a list");
            ctx.fail(d)
        }
    }
}

/// What a `..rest` binds: a fresh list of everything from `from` on, which is the copy `ply_eval`
/// makes at the same point rather than a shared tail.
pub unsafe extern "C" fn rt_list_rest(ctx: *mut Ctx, value: i64, from: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    match ctx.read(value) {
        Value::List(xs) => {
            let tail = xs.skip(from as usize);
            ctx.push(Value::List(tail))
        }
        _ => {
            let d = error("a list pattern bound a value that is not a list");
            ctx.fail(d)
        }
    }
}

/// Whether a value is the constructor at `index`, name and arity both.
pub unsafe extern "C" fn rt_ctor_is(ctx: *mut Ctx, value: i64, index: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let (name, arity) = ctx.tables.ctors[index as usize].clone();
    match ctx.read(value) {
        Value::Ctor { name: n, args } => i64::from(*n == name && args.len() == arity),
        _ => 0,
    }
}

/// One argument of a constructor value, once [`rt_ctor_is`] has said it is that constructor.
pub unsafe extern "C" fn rt_ctor_arg(ctx: *mut Ctx, value: i64, i: i64, take: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    // A pattern binding out of a constructor nothing else holds moves the argument out.
    if take != 0
        && value >= 0
        && let Value::Ctor { args, .. } = &mut ctx.slots[value as usize]
        && let Some(args) = Arc::get_mut(args)
        && let Some(slot) = args.get_mut(i as usize)
    {
        let v = std::mem::replace(slot, Value::Unit);
        return ctx.push(v);
    }
    match ctx.read(value) {
        Value::Ctor { args, .. } => match args.get(i as usize) {
            Some(v) => {
                let v = v.clone();
                ctx.push(v)
            }
            None => {
                let d = error("a constructor pattern read an argument that is not there");
                ctx.fail(d)
            }
        },
        _ => {
            let d = error("a constructor pattern bound a value that is not a constructor");
            ctx.fail(d)
        }
    }
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
        ("rt_no_fuel", rt_no_fuel as *const u8),
        ("rt_lit", rt_lit as *const u8),
        ("rt_equal", rt_equal as *const u8),
        ("rt_concat", rt_concat as *const u8),
        ("rt_builtin", rt_builtin as *const u8),
        ("rt_ctor", rt_ctor as *const u8),
        ("rt_dup", rt_dup as *const u8),
        ("rt_constant", rt_constant as *const u8),
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
        ("rt_record", rt_record as *const u8),
        ("rt_record_update", rt_record_update as *const u8),
        ("rt_field", rt_field as *const u8),
        ("rt_list", rt_list as *const u8),
        ("rt_record_fits", rt_record_fits as *const u8),
        ("rt_record_has", rt_record_has as *const u8),
        ("rt_list_fits", rt_list_fits as *const u8),
        ("rt_list_at", rt_list_at as *const u8),
        ("rt_list_rest", rt_list_rest as *const u8),
        ("rt_ctor_is", rt_ctor_is as *const u8),
        ("rt_ctor_arg", rt_ctor_arg as *const u8),
    ]
}
