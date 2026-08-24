//! What compiled code calls when it leaves native instructions.
//!
//! Values stay boxed. A handle is an index: non-negative into the per-call
//! arena, negative into the constant pool the compiler filled — which is what
//! makes a literal an immediate rather than an allocation. Nothing here is
//! clever on purpose: ADR 0016 §3.2 pins the fragment so that the ceiling is
//! one a real backend could reach, and a runtime that unboxed or interned would
//! price a language Ply is not.
//!
//! # What R5 removed, and why it had to be removed
//!
//! There used to be a `rt_call_machine` here: a compiled body that reached a
//! function outside the compiled set called `Machine::call` on a second,
//! privately-held `Machine`. That is a whole entry point — `escape::check`,
//! `reset()`, `close_regions`, `end_entry_point` — so it discarded the handler
//! stack, the trail and the region generations of whatever run was underneath
//! it. It was invisible while the only entry into compiled code was at the top
//! of a pure integer kernel. The moment the interpreter can *enter* compiled
//! code (`ply_eval::Compiled`), a trampoline is a route from inside a live
//! machine into a different one, so it is deleted rather than guarded: a call
//! to an uncompiled function now refuses the enclosing function at compile time
//! (`crate::jit::Denotes::Uncompiled`).
//!
//! What is left cannot reach a `Machine` at all. The only ways out of native
//! instructions are the helpers below, and each one is either arithmetic, an
//! allocation into [`Ctx::slots`], or `ply_eval::builtins::call` on a builtin
//! the compiler already proved cannot suspend and cannot touch a cell.

use ply_eval::{Builtin, Step, Value, values_equal};
use ply_span::{Diagnostic, Span, Symbol, codes};
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

/// The compile-time tables a compiled program reads through its context:
/// shared, immutable, and one per [`crate::jit::Compiled`] rather than one per
/// call.
pub struct Tables {
    /// The constant pool negative handles index.
    pub consts: Vec<Value>,
    pub ctors: Vec<(Symbol, usize)>,
    pub shapes: Vec<Vec<Symbol>>,
    /// Every builtin a compiled body may call. The compiler refuses the ones
    /// that can suspend or reach a cell, so this holds only total functions of
    /// their arguments — see [`crate::jit::admissible_builtin`].
    pub builtins: Vec<Builtin>,
}

impl Tables {
    /// Whether the constant pool holds a value that must never sit in a table
    /// outliving the call that made it.
    ///
    /// A [`Value::Secret`] is the one ADR 0019 §0.1 arms `argv`'s pool against
    /// by name; a `Cell`, `Task`, `Continuation` or `Closure` is a handle into a
    /// run this table has no business outliving. The compiler cannot mint any of
    /// them — `secret_of_string` is refused, a lambda is refused, and there is
    /// no literal for the rest — so this is a registration-time assertion that
    /// the claim is true rather than a filter expected to fire.
    pub fn retains_a_handle(&self) -> Option<&'static str> {
        self.consts.iter().find_map(holds_a_handle)
    }
}

/// The same question of one value, to its leaves.
///
/// No wildcard: a `Value` variant added later fails to compile here until
/// somebody decides whether a constant pool may hold one. Checking only the top
/// level would be the kind of guarantee that reads as armed and is not — a
/// `Secret` inside a record is still a `Secret` the pool outlives.
fn holds_a_handle(value: &Value) -> Option<&'static str> {
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

/// How much of the value arena is kept between entries. Enough that an ordinary
/// call never reallocates, small enough that one outsized call does not hold
/// memory for the life of the process.
const RETAINED_SLOTS: usize = 4096;

/// The failure a compiled function reports by, and the fuel it spends.
///
/// `#[repr(C)]` with `failed` at offset 0 and `fuel` at offset 8, because
/// compiled code reads both with a load at a fixed offset: `failed` after every
/// call that can fail, `fuel` in every function's prologue.
/// [`Ctx::failed`] when a body ran out of fuel, as against `1` for anything
/// else. Compiled code only ever tests it against zero, so the two are the same
/// branch; the caller needs them apart, because "this recursion would pass the
/// machine's bound" and "this multiplication overflowed" are different facts
/// about a run and reading a depth-exhausted frame's leftover `fuel` cannot tell
/// them apart.
pub const FAILED_OUT_OF_FUEL: i64 = 2;

#[repr(C)]
pub struct Ctx {
    pub failed: i64,
    /// Nested native calls still allowed, counted down on entry to a compiled
    /// function and back up on its normal return.
    ///
    /// This is the compiled half of `ply_eval::limit`: the machine bounds nested
    /// calls at `DEFAULT_MAX_CALLS` and answers a breach with a diagnostic, and
    /// before R5 the fragment compiled a self-call to a native call with no
    /// bound at all, so the same program was a diagnostic in one engine and a
    /// `SIGABRT` in the other. `ply_eval::Compiled::enter` is handed the
    /// machine's remaining budget and seeds this from it; a body that would pass
    /// it fails, the entry declines, and the machine raises **its own**
    /// `recursion limit of 10000 nested calls exceeded`.
    pub fuel: i64,
    pub slots: Vec<Value>,
    pub tables: Rc<Tables>,
    /// The arena `ply_eval::builtins::call` insists on. Structurally unreachable:
    /// `cell_get` and `cell_set` are the only builtins that touch it and the
    /// compiler refuses both, so nothing a compiled body can call allocates here.
    /// [`Ctx::touched_cells`] is the armed form of that claim — a private arena
    /// that quietly started serving cells would be the exact hazard the
    /// trampoline's removal exists to close.
    cells: ply_eval::TaskRegions,
    /// `(allocations, regions_opened)` when the arena above was built, so
    /// [`Ctx::touched_cells`] compares against what an empty one costs rather
    /// than against zero.
    cells_baseline: (u64, u64),
    /// Why the last entry failed. **Never crosses into a machine.**
    /// `crate::entry::SpikeBodies::enter` maps a set `failed` to `None` and
    /// never reads this, so the diagnostic the program sees is the interpreter's
    /// own, at the interpreter's own span, with the interpreter's own code and
    /// notes. It is kept because the spike's *direct* harness — ADR 0016's
    /// `read_line` measurement and this crate's own tests — needs to say what
    /// went wrong, and a bare flag would make a refusal unreportable.
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
            tables,
            cells,
            cells_baseline: (stats.allocations, stats.regions_opened),
            diagnostic: None,
            builtin_calls: 0,
        }
    }

    /// Between calls, and only between calls.
    ///
    /// One context serves every entry, which is safe exactly while an entry
    /// cannot nest inside another: `slots` is a bump arena with no pop, so a
    /// nested reset would leave the outer activation's handles indexing
    /// different values of usually the same type — a plausible integer and no
    /// crash. Nothing here can call back into a machine and no builtin the
    /// compiler admits can call user code, so nesting has no route; the
    /// `RefCell` in `crate::entry::SpikeBodies` is the armed form of that claim,
    /// and it declines rather than resetting if the route is ever opened.
    pub fn begin(&mut self, fuel: i64) {
        self.failed = 0;
        self.fuel = fuel;
        self.slots.clear();
        // One pathological entry — a recursion that runs out of fuel ten
        // thousand frames down — boxes an intermediate per operation and
        // `clear` keeps the capacity. Held for the provider's life that is a
        // leak proportional to the worst call ever made rather than to the
        // work in front of it, so the buffer is given back.
        if self.slots.capacity() > RETAINED_SLOTS {
            self.slots.shrink_to(RETAINED_SLOTS);
        }
        self.diagnostic = None;
    }

    /// Whether a builtin allocated a cell in the private arena. Always `false`,
    /// and checked after every entry rather than asserted in a comment.
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

fn args_of(ctx: &Ctx, ptr: *const i64, n: i64) -> Vec<Value> {
    let handles = unsafe { std::slice::from_raw_parts(ptr, n as usize) };
    handles.iter().map(|h| ctx.read(*h).clone()).collect()
}

/// A failure inside compiled code, for the spike's own reporting.
///
/// `Span::DUMMY` and `RUNTIME_ERROR` are wrong for a program — the machine
/// raises `err_zero_divisor`, `err_overflow` and the rest at real spans — which
/// is why no diagnostic built here may reach one. It cannot: a failed entry
/// answers `None` and the machine evaluates the definition itself.
fn error(message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(codes::RUNTIME_ERROR, message.into()).primary(Span::DUMMY, "in compiled code")
}

pub extern "C" fn rt_box_int(ctx: *mut Ctx, v: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    ctx.push(Value::Int(v))
}

pub extern "C" fn rt_box_bool(ctx: *mut Ctx, v: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    ctx.push(Value::Bool(v != 0))
}

pub extern "C" fn rt_unbox_int(ctx: *mut Ctx, handle: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    match ctx.read(handle) {
        Value::Int(i) => *i,
        other => {
            let d = error(format!("an `Int` operation on a {}", other.type_name()));
            ctx.fail(d)
        }
    }
}

pub extern "C" fn rt_unbox_bool(ctx: *mut Ctx, handle: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    match ctx.read(handle) {
        Value::Bool(b) => i64::from(*b),
        other => {
            let d = error(format!("a condition of type {}", other.type_name()));
            ctx.fail(d)
        }
    }
}

/// The prologue's refusal: this call would nest past the budget the machine
/// handed the entry. Not a program error — the machine re-evaluates and raises
/// its own bound.
pub extern "C" fn rt_no_fuel(ctx: *mut Ctx) {
    let ctx = unsafe { &mut *ctx };
    let d = error("this call would nest past the machine's own bound on nested calls");
    ctx.fail_with(FAILED_OUT_OF_FUEL, d);
}

/// Whichever of `checked_mul`, `checked_div` and `checked_rem` the operator was.
/// Multiplication and division are not on the request path this spike prices,
/// so they are a call rather than an instruction; addition and subtraction are
/// compiled with `sadd_overflow` and `ssub_overflow` inline.
pub extern "C" fn rt_arith(ctx: *mut Ctx, op: i64, a: i64, b: i64) -> i64 {
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

/// A literal, built the way the interpreter builds it: a fresh allocation per
/// evaluation. Compiled code reaches this only when literal folding is switched
/// off, which is how the spike separates "dispatch removed" from "the constant
/// stopped being rebuilt per call".
pub extern "C" fn rt_lit(ctx: *mut Ctx, index: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let value = ctx.read(index).clone();
    let rebuilt = match &value {
        Value::Bytes(b) => Value::bytes(b.as_ref()),
        Value::Str(s) => Value::str(s.as_ref()),
        Value::Ctor { name, args } => Value::Ctor {
            name: name.clone(),
            args: Arc::new(args.as_ref().clone()),
        },
        other => other.clone(),
    };
    ctx.push(rebuilt)
}

/// A `match` whose arms did not cover the value. The machine raises here, so
/// this fails too rather than answering something.
pub extern "C" fn rt_no_match(ctx: *mut Ctx) {
    let ctx = unsafe { &mut *ctx };
    let d = error("no arm of this `match` matched");
    ctx.fail(d);
}

pub extern "C" fn rt_overflow(ctx: *mut Ctx, what: i64) {
    let ctx = unsafe { &mut *ctx };
    let name = if what == 0 { "addition" } else { "subtraction" };
    let d = error(format!("this {name} overflowed"));
    ctx.fail(d);
}

/// `==` and `!=` on anything that is not a pair of `Int`s or a pair of `Bool`s,
/// answered by the evaluator's own comparison so the two cannot disagree.
pub extern "C" fn rt_equal(ctx: *mut Ctx, a: i64, b: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let (l, r) = (ctx.read(a).clone(), ctx.read(b).clone());
    match values_equal(&l, &r, Span::DUMMY) {
        Ok(eq) => i64::from(eq),
        Err(d) => ctx.fail(d),
    }
}

pub extern "C" fn rt_builtin(ctx: *mut Ctx, index: i64, args: *const i64, n: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let b = ctx.tables.builtins[index as usize];
    let args = args_of(ctx, args, n);
    ctx.builtin_calls += 1;
    match ply_eval::builtins::call(b, args, ctx.cells.arena_mut(), Span::DUMMY) {
        Ok(Step::Done(v)) => ctx.push(v),
        // Unreachable: `jit::admissible_builtin` refuses every higher-order
        // builtin at compile time, because answering `Step::Apply` here would
        // need user code run from inside a native frame.
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

pub extern "C" fn rt_ctor(ctx: *mut Ctx, index: i64, args: *const i64, n: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let name = ctx.tables.ctors[index as usize].0.clone();
    let args = args_of(ctx, args, n);
    ctx.push(Value::Ctor {
        name,
        args: Arc::new(args),
    })
}

pub extern "C" fn rt_record(ctx: *mut Ctx, shape: i64, args: *const i64, n: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let names = ctx.tables.shapes[shape as usize].clone();
    let args = args_of(ctx, args, n);
    let mut map = BTreeMap::new();
    for (name, value) in names.into_iter().zip(args) {
        map.insert(name, value);
    }
    ctx.push(Value::Record(Arc::new(map)))
}

/// Every symbol the JIT registers, in one place so the compiler and the linker
/// cannot drift.
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
        ("rt_builtin", rt_builtin as *const u8),
        ("rt_ctor", rt_ctor as *const u8),
        ("rt_record", rt_record as *const u8),
    ]
}
