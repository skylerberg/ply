//! Closures, handlers, `perform`, `simulate` and `parallel`.

use super::*;

/// A closure of compiled function `index` over `env`, its leading arguments. Takes the captures.
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

/// The closure over function `index` that captures nothing, taking `arity` arguments: made once
/// in the unit's immortal heap, so a body that hands it on allocates nothing and counts nothing.
/// `desc`, where it is not zero, is the immediate word of the function's descriptor, which a call
/// by type reads.
pub unsafe extern "C" fn rt_lambda_value(ctx: *mut Ctx, index: i64, arity: i64, desc: i64) -> i64 {
    let tables = Arc::clone(&unsafe { &*ctx }.tables);
    let slot = &tables.lambdas[index as usize];
    let made = slot.load(Acquire);
    if made != 0 {
        return made;
    }
    let held = usize::from(desc != 0);
    let o = lock(&tables.immortals).alloc(
        KIND_CLOSURE,
        if desc != 0 { CLOSURE_DESCRIBED } else { 0 },
        (held + CLOSURE_CAPTURES) as u32,
        arity as u32,
    );
    unsafe {
        set_word(o, CLOSURE_CODE, tables.functions[index as usize] as Word);
        if desc != 0 {
            set_word(o, CLOSURE_CAPTURES, desc as Word);
        }
    }
    heap::mark_immortal(o as Word);
    // Two threads making it at once keep one each, and the unit's heap holds both.
    match slot.compare_exchange(0, o as i64, AcqRel, Acquire) {
        Ok(_) => o as i64,
        Err(first) => first,
    }
}

/// Installs a handler and answers its depth; the clause closures and `ret` are taken until it pops.
pub unsafe extern "C" fn rt_handle_push(
    ctx: *mut Ctx,
    clauses: *const i64,
    n: i64,
    ret: i64,
) -> i64 {
    unsafe { pushed(ctx, clauses, n, ret, false) }
}

/// [`rt_handle_push`] of a `handle` emitted code wrote, whose clauses and `return` clause, which
/// is 0 where it has none, are closures it made.
pub unsafe extern "C" fn rt_handle_install(
    ctx: *mut Ctx,
    clauses: *const Word,
    n: I64,
    ret: Handed,
) -> I64 {
    I64(unsafe { pushed(ctx, clauses, n.0, ret.word(), true) })
}

unsafe fn pushed(ctx: *mut Ctx, clauses: *const Word, n: i64, ret: Word, emitted: bool) -> i64 {
    let c = unsafe { &mut *ctx };
    let clauses = clauses_of(c, clauses, n, emitted);
    let fuel = c.fuel;
    let handler = c.next_handler();
    let frames = c.frames();
    frames.push(HandlerFrame {
        clauses,
        ret,
        emitted,
        simulate: false,
        detached: None,
        fuel,
        handler,
        vars: None,
    });
    (frames.len() - 1) as i64
}

/// A `handle` site's clause table: per clause the effect, resource (negative for none) and op
/// as field-table indices, the closure, its `resumes`, and whether it binds the label.
pub(super) fn clauses_of(c: &Ctx, clauses: *const i64, n: i64, emitted: bool) -> Vec<FrameClause> {
    let words = args_of(clauses, n * 6);
    let name = |i: i64| c.tables.fields[i as usize].clone();
    words
        .chunks(6)
        .map(|w| FrameClause {
            effect: name(w[0]),
            resource: (w[1] >= 0).then(|| name(w[1])),
            op: name(w[2]),
            closure: w[3],
            resumes: w[4] as u8,
            binds_label: w[5] != 0,
            emitted,
        })
        .collect()
}

/// A `handle` whose clause resumes off the tail: `body`, a nullary closure, runs on its own stack.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_handle_detached(
    ctx: *mut Ctx,
    clauses: *const i64,
    n: i64,
    ret: i64,
    body: i64,
) -> i64 {
    let c = unsafe { &mut *ctx };
    let clauses = clauses_of(c, clauses, n, false);
    unsafe { crate::detached::open(ctx, clauses, ret, body, false) }
}

/// [`rt_handle_detached`] of a `handle` emitted code wrote, whose clauses, `return` clause (0 for
/// none) and body are closures it made.
pub unsafe extern "C" fn rt_handle_install_detached(
    ctx: *mut Ctx,
    clauses: *const Word,
    n: I64,
    ret: Handed,
    body: Block,
) -> Handed {
    let c = unsafe { &mut *ctx };
    let clauses = clauses_of(c, clauses, n.0, true);
    let body = unsafe { std::mem::transmute::<Block, Word>(body) };
    Handed::of(unsafe { crate::detached::open(ctx, clauses, ret.word(), body, true) })
}

/// The `k` a clause emitted code made is handed where it resumes on the tail: one immortal block
/// holding nothing but its descriptor, since a tail resumption only records its value.
#[repr(C)]
struct ResumeBlock {
    header: heap::Obj,
    desc: &'static PlyFnDesc,
}

static RESUME_DESC: PlyFnDesc = PlyFnDesc {
    uniform: resume_uniform,
    typed: std::ptr::null(),
    convention: 0,
    release: never_released,
    share: std::ptr::null(),
    keep: std::ptr::null(),
};

static RESUME: ResumeBlock = ResumeBlock {
    header: heap::Obj {
        rc: heap::IMMORTAL,
        kind: heap::KIND_REFUSED,
        flags: 0,
        aux: 0,
        len: 0,
        layout: 0,
    },
    desc: &RESUME_DESC,
};

unsafe extern "C" fn resume_uniform(ctx: *mut Ctx, _k: Word, args: *const Word) -> Word {
    let v = unsafe { *args };
    unsafe { (*ctx).resumed = Some(v) };
    v
}

unsafe extern "C" fn never_released(_ctx: *mut Ctx, _k: Handed) -> Word {
    unreachable!("an immortal block is never released")
}

/// The `k` a tail-resuming clause emitted code made is handed.
fn resume_token_emitted() -> Word {
    &raw const RESUME as Word
}

/// The `k` a `resume`-binding clause gets: a tail call records the value for `rt_perform`.
pub(crate) unsafe extern "C" fn rt_resume_entry(ctx: *mut Ctx, args: *const i64) -> i64 {
    let c = unsafe { &mut *ctx };
    let v = unsafe { *args.add(1) };
    c.resumed = Some(v);
    v
}

pub(super) fn hide_above(
    c: &mut Ctx,
    stack: usize,
    depth: usize,
) -> Vec<(usize, Vec<HandlerFrame>)> {
    let mut hidden = Vec::new();
    let mut s = c.current;
    while s != stack {
        hidden.push((s, std::mem::take(&mut c.stacks[s].list)));
        s = c.stacks[s]
            .parent
            .expect("the handler's stack is in the chain");
    }
    hidden.push((stack, c.stacks[stack].list.split_off(depth)));
    hidden
}

pub(super) fn restore_hidden(c: &mut Ctx, hidden: Vec<(usize, Vec<HandlerFrame>)>) {
    for (s, frames) in hidden.into_iter().rev() {
        c.stacks[s].list.extend(frames);
    }
}

pub(super) fn resume_token(c: &mut Ctx, stack: usize, depth: usize) -> Word {
    closure_of(
        c,
        rt_resume_entry as *const () as usize,
        &[((stack << 32) | depth) as i64],
    )
}

/// A unary closure over immediate captures whose entry is a runtime function.
pub(crate) fn closure_of(c: &mut Ctx, entry: usize, captures: &[i64]) -> Word {
    let o = c.heap.alloc(
        KIND_CLOSURE,
        0,
        (captures.len() + CLOSURE_CAPTURES) as u32,
        1,
    );
    unsafe {
        set_word(o, CLOSURE_CODE, entry as Word);
        for (i, capture) in captures.iter().enumerate() {
            set_word(o, CLOSURE_CAPTURES + i, heap::imm(*capture));
        }
    }
    o as Word
}

/// Records the atom; then the innermost frame with a clause answers, with frames from its own
/// up set aside. A `resume`-binding clause that never resumes unwinds to its `handle`.
pub unsafe extern "C" fn rt_perform(
    ctx: *mut Ctx,
    effect: i64,
    op: i64,
    resource: i64,
    mode: i64,
    args: *const i64,
    n: i64,
) -> i64 {
    let c = unsafe { &mut *ctx };
    let effect = c.tables.fields[effect as usize].clone();
    let op = c.tables.fields[op as usize].clone();
    let args = args_of(args, n);
    if mode == MODE_RAISE || is_abort(effect.as_str(), op.as_str()) {
        return raise_from_perform(c, &effect, &op, Raised::Words(args.to_vec()));
    }
    unsafe { performed(ctx, effect, op, resource, mode, args, None) }
}

/// `perform` of the operation `op` describes, at the perform's types, `args` holding a word form
/// for each of its parameters, which this takes. A clause, a region's scheduler or a detached body
/// takes the words as they are; the host is handed what they lower to through the parameters'
/// types, and its answer is lifted through the operation's. A raise's payload is held carried,
/// through the same types.
pub unsafe extern "C" fn rt_dispatch(
    ctx: *mut Ctx,
    op: *const PlyOp,
    resource: I64,
    mode: I64,
    args: *const Handed,
) -> Handed {
    let c = unsafe { &mut *ctx };
    let described = unsafe { &*op };
    let effect = c.tables.fields[described.effect as usize].clone();
    let name = c.tables.fields[described.op as usize].clone();
    let args = args_of(args.cast(), described.params_len);
    if mode.0 == MODE_RAISE || is_abort(effect.as_str(), name.as_str()) {
        let carried = args
            .iter()
            .zip(described.params())
            .map(|(w, ty)| unsafe { Carried::new(Handed::of(*w), *ty) })
            .collect();
        return Handed::of(raise_from_perform(
            c,
            &effect,
            &name,
            Raised::Carried(carried),
        ));
    }
    Handed::of(unsafe { performed(ctx, effect, name, resource.0, mode.0, args, Some(described)) })
}

/// [`rt_perform`] past a raise, its arguments crossing to the host through `described` where the
/// perform passed one.
unsafe fn performed(
    ctx: *mut Ctx,
    effect: Symbol,
    op: Symbol,
    resource: i64,
    mode: i64,
    args: &[Word],
    described: Option<&PlyOp>,
) -> i64 {
    let c = unsafe { &mut *ctx };
    let n = args.len() as i64;
    let args = args.as_ptr();
    let resource = (resource >= 0).then(|| c.tables.fields[resource as usize].clone());
    let atom = EffectAtom::operation(
        effect.clone(),
        resource
            .clone()
            .map_or(Resource::Singleton, Resource::Named),
        if mode != 0 { Mode::Write } else { Mode::Read },
        op.clone(),
    );
    // A step conflicts on the mode, and a scheduled draw's access is recorded here alone.
    if !c.sims.is_empty() {
        c.record_access(Access::Atom(atom.mode_atom()));
    }
    c.performed.push(atom);
    let mut found = None;
    let mut inherited_off_tail = false;
    let mut stack = c.current;
    'search: loop {
        for (i, f) in c.stacks[stack].list.iter().enumerate().rev() {
            if f.simulate {
                if effect.as_str() == "sim" && op.as_str() == "seed" {
                    let root = Value::Int(c.seed.root as i64);
                    return answered(ctx, Served::Value(root), described);
                }
                if ply_core::sim::is_scheduled(effect.as_str(), op.as_str()) {
                    return unsafe {
                        crate::simulate::perform(ctx, &effect, &op, args_of(args, n))
                    };
                }
                continue;
            }
            if let Some(cl) = f
                .clauses
                .iter()
                .find(|cl| cl.answers(&effect, &op, resource.as_ref()))
            {
                if cl.off_tail() {
                    let Some(id) = f.detached else {
                        inherited_off_tail = true;
                        break 'search;
                    };
                    let (closure, emitted, again) = (cl.closure, cl.emitted, cl.again());
                    let mut bound: Vec<Word> = args_of(args, n).to_vec();
                    if cl.binds_label
                        && let Some(label) = &resource
                    {
                        bound.push(c.word(&Value::Str(Arc::from(label.as_str()))));
                    }
                    // Moved in: a local owning memory across the switch is freed once per restore.
                    return unsafe {
                        crate::detached::stop(
                            ctx,
                            id,
                            (closure, emitted),
                            bound,
                            again,
                            (effect, op, resource),
                        )
                    };
                }
                found = Some((
                    stack,
                    i,
                    (cl.closure, cl.emitted),
                    cl.resumes != 0,
                    cl.binds_label,
                ));
                break 'search;
            }
        }
        match c.stacks[stack].parent {
            Some(p) => stack = p,
            None => break,
        }
    }
    if inherited_off_tail {
        let d = Diagnostic::error(
            codes::RUNTIME_ERROR,
            format!(
                "`{effect}.{op}` was performed in a task, and its handler resumes off the tail"
            ),
        )
        .primary(c.site(), "performed here")
        .note(
            "the clause would capture the continuation of the body the task was spawned in, which \
             is not the task's own: handle the operation inside the task, or resume on the tail",
        );
        return c.fail(d);
    }
    let Some((stack, depth, (closure, emitted), resumes, binds_label)) = found else {
        // Inside an open production region a `task` op is the scheduler's; outside, the host's.
        if effect.as_str() == "task"
            && ply_core::sim::TASK_OPS.contains(&op.as_str())
            && c.sims.last().is_some_and(|sim| sim.is_production())
        {
            return unsafe { crate::simulate::perform(ctx, &effect, &op, args_of(args, n)) };
        }
        let args = args_of(args, n);
        return match described {
            None => unsafe { crate::host::perform(ctx, &effect, &op, resource.as_ref(), args) },
            Some(described) => unsafe {
                to_host(ctx, described, &effect, &op, resource.as_ref(), args)
            },
        };
    };
    let mut call_args: Vec<Word> = args_of(args, n).to_vec();
    // A clause written `[*t]` reads the label its call site named, before the continuation does.
    if binds_label && let Some(label) = &resource {
        call_args.push(c.word(&Value::Str(Arc::from(label.as_str()))));
    }
    drop((effect, op, resource));
    // The clause runs outside its handler: that frame and all above it are hidden until it returns.
    let held = c.stacks[stack].list[depth].held();
    let hidden = hide_above(c, stack, depth);
    if resumes {
        let k = if emitted {
            resume_token_emitted()
        } else {
            resume_token(c, stack, depth)
        };
        call_args.push(k);
        c.resumed = None;
    }
    let here = c.keep(Kept::Hidden(hidden));
    let r = call_clause(ctx, closure, emitted, call_args, held);
    let c = unsafe { &mut *ctx };
    let Kept::Hidden(hidden) = c.kept_back(here) else {
        unreachable!("a clause's call keeps the frames it hid");
    };
    restore_hidden(c, hidden);
    if c.failed != 0 {
        return 0;
    }
    if !resumes {
        return r;
    }
    match c.resumed.take() {
        Some(v) => v,
        None => {
            c.unwind = Some((stack, depth, r));
            c.failed = FAILED_UNWIND;
            0
        }
    }
}

/// The host's answer to an operation `described` describes, `args` its word forms, which this
/// takes: each lowered through its parameter's type and let go through it, and the answer lifted
/// through the operation's.
unsafe fn to_host(
    ctx: *mut Ctx,
    described: &PlyOp,
    effect: &Symbol,
    op: &Symbol,
    resource: Option<&Symbol>,
    args: &[Word],
) -> i64 {
    let values = lowered_values(ctx, args, described.params());
    for (w, ty) in args.iter().zip(described.params()) {
        unsafe { Carried::new(Handed::of(*w), *ty) }.release(ctx);
    }
    let values = match values {
        Ok(values) => values,
        Err(why) => {
            let operation = ply_core::host::operation_label(effect, op, resource);
            let d = error(format!(
                "an argument of `{operation}` does not lower: {why}"
            ))
            .note("this is Ply's fault, not the program's");
            return unsafe { &mut *ctx }.fail(d);
        }
    };
    let served = unsafe { crate::host::serve(ctx, effect, op, resource, values) };
    answered(ctx, served, Some(described))
}

/// What a perform is answered: a value as the word compiled code holds it, lifted through
/// `described`'s answer where the perform passed one; a word the scheduler made as it is.
fn answered(ctx: *mut Ctx, served: Served, described: Option<&PlyOp>) -> i64 {
    match (served, described) {
        (Served::Word(w), _) => w,
        (Served::Value(v), None) => unsafe { &mut *ctx }.word(&v),
        (Served::Value(v), Some(described)) => {
            lifted(ctx, &v, described.answer()).map_or(0, Handed::word)
        }
    }
}

/// What a raise carries, as values a diagnostic shows: the release's words read as their layouts
/// say, carried values lowered through their types.
fn raised_values(c: &mut Ctx, args: &Raised) -> Vec<Result<Value, String>> {
    match args {
        Raised::Words(ws) => ws.iter().map(|w| Ok(c.value(*w))).collect(),
        Raised::Carried(vs) => {
            let ctx: *mut Ctx = c;
            vs.iter().map(|v| v.lower(ctx)).collect()
        }
    }
}

/// A raise, which no clause answers where it is performed: the frame whose clause does is unwound
/// to first, and with none in reach the entry fails. Takes the arguments.
pub(super) fn raise_from_perform(c: &mut Ctx, effect: &Symbol, op: &Symbol, args: Raised) -> i64 {
    if c.failed != 0 {
        args.release();
        return 0;
    }
    let at = c.raise_handler(effect.as_str(), op.as_str());
    let d = if is_abort(effect.as_str(), op.as_str()) {
        let message = match raised_values(c, &args).first() {
            Some(Ok(Value::Str(s))) => s.to_string(),
            _ => String::new(),
        };
        Diagnostic::error(codes::RUNTIME_ERROR, format!("panic: {message}"))
    } else if at.is_some() {
        Diagnostic::error(codes::RUNTIME_ERROR, format!("`{effect}.{op}` was raised"))
    } else {
        if let Some(runtime) = c.host_runtime() {
            runtime.unanswered(effect, op);
        }
        // Only a raise nothing answers is shown what it carried: one a clause takes is not read.
        let carried: Vec<Plain> = raised_values(c, &args)
            .iter()
            .map(|v| v.as_ref().map_or(Plain::Elided(0), Plain::shown))
            .collect();
        let slots: Vec<String> = (0..carried.len()).map(ply_core::slot).collect();
        let with = if slots.is_empty() {
            String::new()
        } else {
            format!(" with {}", slots.join(", "))
        };
        Diagnostic::error(
            codes::RUNTIME_ERROR,
            format!("`{effect}.{op}` was raised{with}, and nothing answers it"),
        )
        .note("a `try` around the call answers it as an `Err`, and a `handle` clause as it likes")
        .showing(carried)
    };
    let d = d.primary(c.site(), "raised here");
    match at {
        Some(at) => c.bind_raise(at, args, d),
        None => {
            args.release();
            c.fail(d)
        }
    }
}

/// `simulate { body }`: runs the nullary `body` as a region's root task on this stack.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_simulate(ctx: *mut Ctx, body: i64) -> i64 {
    let c = unsafe { &mut *ctx };
    if !c.sims.is_empty() {
        heap::dec(body);
        let outer = c.sims.last().map_or(Span::DUMMY, |sim| sim.site);
        let d = ply_core::err_nested_simulation(c.site(), outer);
        return c.fail(d);
    }
    let id = ply_core::SimId(c.entered_sims);
    c.entered_sims += 1;
    let site = c.site();
    c.trail.enter(site);
    let stack = c.current;
    let depth = c.frames().len();
    let fuel = c.fuel;
    c.frames().push(HandlerFrame::simulate(fuel));
    let sim = crate::simulate::Simulation::new(
        ply_core::sched::Scheduler::new(id, site).with_step_budget(c.sim_steps),
        site,
        c.seed.root,
        c.trail.drawn(),
        stack,
        body,
    );
    c.sims.push(sim);
    let r = unsafe { crate::simulate::run(ctx) };
    let c = unsafe { &mut *ctx };
    c.current = stack;
    for f in c.stacks[stack].list.split_off(depth) {
        drop_frame(f);
    }
    crate::simulate::end(c);
    c.record = Some(c.trail.record());
    r
}

/// The `handle` at `depth` is over: pops its frames, catches an unwind to it, applies `return`.
pub unsafe extern "C" fn rt_handle_land(ctx: *mut Ctx, depth: i64, value: i64) -> i64 {
    let c = unsafe { &mut *ctx };
    let depth = depth as usize;
    let stack = c.current;
    let mut popped = c.frames().split_off(depth);
    let mine = if popped.is_empty() {
        None
    } else {
        Some(popped.remove(0))
    };
    for f in popped {
        drop_frame(f);
    }
    if let Some(f) = &mine {
        f.land(c);
    }
    if c.failed == FAILED_ABORT
        && let Some(a) = c.aborting.take_if(|a| a.stack == stack && a.depth == depth)
    {
        c.failed = 0;
        let mut f = mine.expect("a raise is bound for a frame still installed");
        let clause = f.take_clause(a.clause);
        let held = f.held();
        drop_frame(f);
        // The clause runs outside its `handle`, which is over: its value is the `handle`'s.
        let r = call_clause(ctx, clause.closure, clause.emitted, a.args.handed(), held);
        clause.release();
        return r;
    }
    if c.failed == FAILED_UNWIND
        && let Some((target_stack, target, v)) = c.unwind.take()
    {
        if target_stack == stack && target == depth {
            c.failed = 0;
            if let Some(f) = mine {
                drop_frame(f);
            }
            return v;
        }
        c.unwind = Some((target_stack, target, v));
    }
    if c.failed != 0 {
        if let Some(f) = mine {
            drop_frame(f);
        }
        return 0;
    }
    let Some(f) = mine else {
        return value;
    };
    if f.ret == 0 {
        drop_frame(f);
        return value;
    }
    let (ret, emitted, held) = (f.ret, f.emitted, f.held());
    let here = c.keep(Kept::Landing(f));
    let r = Ctx::holding(ctx, held, || call_fun(ctx, emitted, ret, &[value]));
    let c = unsafe { &mut *ctx };
    c.kept_back(here).release();
    r
}

/// Variable `i` of the `handle` whose body starts, given its first value `x` and `ty`, the type
/// every value it holds has.
pub unsafe extern "C" fn rt_handler_typed(ctx: *mut Ctx, i: I64, x: Handed, ty: *const PlyType) {
    let c = unsafe { &mut *ctx };
    let v = unsafe { Carried::new(x, ty) };
    let current = c.current;
    let Some(f) = c.stacks[current].list.last_mut() else {
        v.release(ctx);
        c.fail(no_handler_vars());
        return;
    };
    f.vars
        .get_or_insert_with(|| shared(HandlerVars(Vec::new())))
        .borrow_mut()
        .set(i.0 as usize, Var::Carried(v));
}

/// Variable `i` of the handler whose clause runs, held once more for the caller.
pub unsafe extern "C" fn rt_handler_get(ctx: *mut Ctx, i: I64) -> Handed {
    let c = unsafe { &mut *ctx };
    let Some((_, v)) = c.stacks[c.current].held.last() else {
        c.fail(no_handler_vars());
        return Handed::of(0);
    };
    Handed::of(v.borrow().get(i.0 as usize))
}

/// Variable `i` of the handler whose clause runs, given `x`; what it held is let go.
pub unsafe extern "C" fn rt_handler_put(ctx: *mut Ctx, i: I64, x: Handed) {
    let c = unsafe { &mut *ctx };
    let Some((_, v)) = c.stacks[c.current].held.last() else {
        c.fail(no_handler_vars());
        return;
    };
    let put = v.borrow_mut().put(i.0 as usize, x);
    if let Err(why) = put {
        c.fail(error(why).note("this is Ply's fault, not the program's"));
    }
}

/// A builtin used as a value, which a call through it answers as a named call would.
pub unsafe extern "C" fn rt_builtin_value(ctx: *mut Ctx, index: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let b = ctx.tables.builtins[index as usize];
    ctx.heap.bridge(Value::builtin(b))
}

/// The singleton a nullary constructor *is*, and `0` for any other constructor.
pub unsafe extern "C" fn rt_nullary(ctx: *mut Ctx, index: i64) -> i64 {
    unsafe { &*ctx }.nullary(index as u32)
}

/// A constructor named as a value: the singleton a nullary one is, and a function otherwise.
pub unsafe extern "C" fn rt_ctor_value(ctx: *mut Ctx, index: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let (name, arity) = &ctx.tables.layouts.ctors[index as usize];
    if *arity == 0 {
        return ctx.nullary(index as u32);
    }
    let (name, arity) = (name.clone(), *arity);
    ctx.heap.bridge(Value::Closure(Arc::new(Closure {
        name: Some(name.clone()),
        kind: ClosureKind::Ctor { name, arity },
    })))
}

/// One more holder of a shared object, for compiled code, which has no atomics of its own.
pub unsafe extern "C" fn rt_inc_shared(_ctx: *mut Ctx, w: i64) {
    heap::inc_shared(w);
}

/// One holder fewer of a shared object.
pub unsafe extern "C" fn rt_dec_shared(_ctx: *mut Ctx, w: i64) {
    heap::dec_shared(w);
}

/// `parallel { .. }`: the `n` nullary closures at `slots`, each answer written over its closure.
pub unsafe extern "C" fn rt_parallel(ctx: *mut Ctx, slots: i64, n: i64) {
    let allowed = unsafe { &*ctx }.runs_branches_at_once(None);
    unsafe { crate::parallel::run(ctx, slots as usize as *mut Word, n as usize, allowed) }
}

/// A `parallel` block whose site knows what its branches may perform: `reach` holds `count` pairs
/// of field-table indices, an effect and its resource (negative for any).
pub unsafe extern "C" fn rt_parallel_past(
    ctx: *mut Ctx,
    slots: i64,
    n: i64,
    reach: *const i64,
    count: i64,
) {
    let reach: Vec<Reached> = {
        let c = unsafe { &*ctx };
        let name = |i: i64| c.tables.fields[i as usize].clone();
        args_of(reach, count * 2)
            .chunks(2)
            .map(|w| Reached {
                effect: name(w[0]),
                resource: (w[1] >= 0).then(|| name(w[1])),
            })
            .collect()
    };
    // Decided and let go before a branch runs, which may stop a detached body on this stack.
    let allowed = unsafe { &*ctx }.runs_branches_at_once(Some(&reach));
    drop(reach);
    unsafe { crate::parallel::run(ctx, slots as usize as *mut Word, n as usize, allowed) }
}
