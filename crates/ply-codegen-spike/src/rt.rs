//! What compiled code calls when it leaves native instructions.
//!
//! Values stay boxed. A handle is an index: non-negative into the per-call
//! arena, negative into the constant pool the compiler filled — which is what
//! makes a literal an immediate rather than an allocation. Nothing here is
//! clever on purpose: ADR 0016 §3.2 pins the fragment so that the ceiling is
//! one a real backend could reach, and a runtime that unboxed or interned would
//! price a language Ply is not.

use ply_eval::{Builtin, Machine, Step, Value, values_equal};
use ply_span::{Diagnostic, Span, Symbol, codes};
use std::collections::BTreeMap;
use std::sync::Arc;

/// The failure a compiled function reports by. `failed` is first and the struct
/// is `repr(C)` because compiled code tests it with a load at offset zero after
/// every call that can fail.
#[repr(C)]
pub struct Ctx {
    pub failed: i64,
    pub slots: Vec<Value>,
    pub consts: Vec<Value>,
    pub ctors: Vec<(Symbol, usize)>,
    pub shapes: Vec<Vec<Symbol>>,
    pub builtins: Vec<Builtin>,
    /// Program-wide names the machine trampoline calls, by index.
    pub targets: Vec<String>,
    /// The region stack the compiled fragment's cells live in. ADR 0017 §5.
    pub regions: ply_eval::TaskRegions,
    pub diagnostic: Option<Diagnostic>,
    pub machine: Option<Box<Machine<'static>>>,
    pub builtin_calls: u64,
    pub machine_calls: u64,
    /// The same total, split by `targets` index, so that a hybrid run reports
    /// which Ply functions the compiled fragment had to leave to reach.
    pub machine_calls_by_target: Vec<u64>,
    /// And by `builtins` index, for the same reason.
    pub builtin_calls_by_index: Vec<u64>,
}

impl Ctx {
    pub fn new() -> Ctx {
        Ctx {
            failed: 0,
            slots: Vec::with_capacity(64),
            consts: Vec::new(),
            ctors: Vec::new(),
            shapes: Vec::new(),
            builtins: Vec::new(),
            targets: Vec::new(),
            regions: ply_eval::TaskRegions::new(),
            diagnostic: None,
            machine: None,
            builtin_calls: 0,
            machine_calls: 0,
            machine_calls_by_target: Vec::new(),
            builtin_calls_by_index: Vec::new(),
        }
    }

    /// Zeroes the per-target and per-builtin tallies and sizes them to the
    /// tables the compiled program declared.
    pub fn reset_counts(&mut self) {
        self.builtin_calls = 0;
        self.machine_calls = 0;
        self.machine_calls_by_target = vec![0; self.targets.len()];
        self.builtin_calls_by_index = vec![0; self.builtins.len()];
    }

    /// Between calls. The arena is the whole of this spike's memory management,
    /// and saying so is part of the result: a real backend owes reference
    /// counting or a collector here, and this one owes neither.
    pub fn reset(&mut self) {
        self.failed = 0;
        self.slots.clear();
        self.diagnostic = None;
    }

    pub fn read(&self, handle: i64) -> &Value {
        if handle < 0 {
            &self.consts[(-handle - 1) as usize]
        } else {
            &self.slots[handle as usize]
        }
    }

    pub fn push(&mut self, value: Value) -> i64 {
        self.slots.push(value);
        (self.slots.len() - 1) as i64
    }

    /// The handle a constant is reachable by, minted at compile time.
    pub fn intern(&mut self, value: Value) -> i64 {
        self.consts.push(value);
        -(self.consts.len() as i64)
    }

    fn fail(&mut self, d: Diagnostic) -> i64 {
        self.failed = 1;
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
            let d = error(format!("arithmetic on a {}", other.type_name()));
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
/// this does too rather than answering something.
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
    let b = ctx.builtins[index as usize];
    let args = args_of(ctx, args, n);
    ctx.builtin_calls += 1;
    if let Some(slot) = ctx.builtin_calls_by_index.get_mut(index as usize) {
        *slot += 1;
    }
    match ply_eval::builtins::call(b, args, &mut ctx.regions, Span::DUMMY) {
        Ok(Step::Done(v)) => ctx.push(v),
        Ok(_) => {
            let d = error(format!("`{}` suspended, which the fragment excludes", b.name()));
            ctx.fail(d)
        }
        Err(d) => ctx.fail(d),
    }
}

pub extern "C" fn rt_ctor(ctx: *mut Ctx, index: i64, args: *const i64, n: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let name = ctx.ctors[index as usize].0.clone();
    let args = args_of(ctx, args, n);
    ctx.push(Value::Ctor {
        name,
        args: Arc::new(args),
    })
}

pub extern "C" fn rt_record(ctx: *mut Ctx, shape: i64, args: *const i64, n: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let names = ctx.shapes[shape as usize].clone();
    let args = args_of(ctx, args, n);
    let mut map = BTreeMap::new();
    for (name, value) in names.into_iter().zip(args) {
        map.insert(name, value);
    }
    ctx.push(Value::Record(Arc::new(map)))
}

/// The escape hatch ADR 0016 §3.2 allows: a call to a Ply function the spike did
/// not compile goes back into the machine. It is a whole entry point rather than
/// a frame push, because `Machine::apply` is not public — which is itself part
/// of what a backend would have to open up.
pub extern "C" fn rt_call_machine(ctx: *mut Ctx, index: i64, args: *const i64, n: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let name = ctx.targets[index as usize].clone();
    let args = args_of(ctx, args, n);
    ctx.machine_calls += 1;
    if let Some(slot) = ctx.machine_calls_by_target.get_mut(index as usize) {
        *slot += 1;
    }
    let Some(machine) = ctx.machine.as_mut() else {
        let d = error(format!("`{name}` needs the machine trampoline and none was installed"));
        return ctx.fail(d);
    };
    match machine.call(&name, args, Span::DUMMY) {
        Ok(v) => ctx.push(v),
        Err(d) => ctx.fail(d),
    }
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
        ("rt_lit", rt_lit as *const u8),
        ("rt_equal", rt_equal as *const u8),
        ("rt_builtin", rt_builtin as *const u8),
        ("rt_ctor", rt_ctor as *const u8),
        ("rt_record", rt_record as *const u8),
        ("rt_call_machine", rt_call_machine as *const u8),
    ]
}
