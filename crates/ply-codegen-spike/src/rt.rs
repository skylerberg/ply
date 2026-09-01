//! What compiled code calls when it leaves native instructions.

// # Safety, once, for every `rt_*` helper below
//
// Each is called **only** from code Cranelift emitted in [`crate::jit`], which
// passes the `*mut Ctx` that [`crate::entry`] created for the entry currently
// running and handles this same context produced. `Ctx` outlives every call into
// the body — `SpikeBodies` holds it in a `RefCell` for the duration — and an entry
// cannot nest, so the `&mut` taken here is unique. A handle is either an index
// into `Ctx::slots` or a negative index into the constant pool, and `Ctx::read`
// bounds-checks both.
//
// Nothing outside this crate may call one: they are `pub` because [`symbols`]
// hands their addresses to the JIT, not because they are an API.
#![allow(clippy::missing_safety_doc)]

use ply_eval::{Builtin, Step, Value, values_equal};
use ply_span::{Diagnostic, Span, Symbol, codes};
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

/// The compile-time tables a compiled program reads through its context: shared, immutable, and one
/// per [`crate::jit::Compiled`] rather than one per call.
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
}

impl Tables {
    /// Whether the constant pool holds a value that must never sit in a table outliving the call
    /// that made it.
    pub fn retains_a_handle(&self) -> Option<&'static str> {
        self.consts.iter().find_map(holds_a_handle)
    }
}

/// The same question of one value, to its leaves.
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

fn args_of(ctx: &Ctx, ptr: *const i64, n: i64) -> Vec<Value> {
    let handles = unsafe { std::slice::from_raw_parts(ptr, n as usize) };
    handles.iter().map(|h| ctx.read(*h).clone()).collect()
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
        Value::Ctor { name, args } => Value::Ctor {
            name: name.clone(),
            args: Arc::new(args.as_ref().clone()),
        },
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

pub unsafe extern "C" fn rt_builtin(ctx: *mut Ctx, index: i64, args: *const i64, n: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let b = ctx.tables.builtins[index as usize];
    let args = args_of(ctx, args, n);
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

pub unsafe extern "C" fn rt_ctor(ctx: *mut Ctx, index: i64, args: *const i64, n: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let name = ctx.tables.ctors[index as usize].0.clone();
    let args = args_of(ctx, args, n);
    ctx.push(Value::Ctor {
        name,
        args: Arc::new(args),
    })
}

pub unsafe extern "C" fn rt_record(ctx: *mut Ctx, shape: i64, args: *const i64, n: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let names = ctx.tables.shapes[shape as usize].clone();
    let args = args_of(ctx, args, n);
    let mut map = BTreeMap::new();
    for (name, value) in names.into_iter().zip(args) {
        map.insert(name, value);
    }
    ctx.push(Value::Record(Arc::new(map.into_iter().collect())))
}

/// One field of a record, with the two failures the interpreter has here kept as failures rather
/// than answered.
pub unsafe extern "C" fn rt_field(ctx: *mut Ctx, base: i64, index: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let name = ctx.tables.fields[index as usize].clone();
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
    let args = args_of(ctx, args, n);
    ctx.push(Value::list(args))
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
            let from = (from as usize).min(xs.len());
            let tail = xs[from..].to_vec();
            ctx.push(Value::list(tail))
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
pub unsafe extern "C" fn rt_ctor_arg(ctx: *mut Ctx, value: i64, i: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
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
        ("rt_builtin", rt_builtin as *const u8),
        ("rt_ctor", rt_ctor as *const u8),
        ("rt_record", rt_record as *const u8),
        ("rt_field", rt_field as *const u8),
        ("rt_list", rt_list as *const u8),
        ("rt_list_fits", rt_list_fits as *const u8),
        ("rt_list_at", rt_list_at as *const u8),
        ("rt_list_rest", rt_list_rest as *const u8),
        ("rt_ctor_is", rt_ctor_is as *const u8),
        ("rt_ctor_arg", rt_ctor_arg as *const u8),
    ]
}
