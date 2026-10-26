//! Handlers, `perform`, `simulate` and `parallel`.

use super::*;
use crate::simulate::Body;

/// Installs a handler and answers its depth: the clause closures and `ret`, the `return` clause or
/// 0 where it has none, are taken until it pops.
pub unsafe extern "C" fn rt_handle_install(
    ctx: *mut Ctx,
    clauses: *const Word,
    n: I64,
    ret: Handed,
) -> I64 {
    let c = unsafe { &mut *ctx };
    let clauses = clauses_of(c, clauses, n.0);
    let fuel = c.fuel;
    let handler = c.next_handler();
    let frames = c.frames();
    frames.push(HandlerFrame {
        clauses,
        ret: ret.word(),
        simulate: false,
        detached: None,
        fuel,
        handler,
        vars: None,
    });
    I64((frames.len() - 1) as i64)
}

/// A `handle` site's clause table: per clause the effect, resource (negative for none) and op
/// as field-table indices, the closure, its `resumes`, and the `String` type where the clause
/// binds its label or answers `abort.raise`, 0 otherwise.
pub(super) fn clauses_of(c: &Ctx, clauses: *const i64, n: i64) -> Vec<FrameClause> {
    let words = args_of(clauses, n * 6);
    let name = |i: i64| c.tables.fields[i as usize].clone();
    words
        .chunks(6)
        .map(|w| {
            let (effect, op) = (name(w[0]), name(w[2]));
            let raise = is_abort(effect.as_str(), op.as_str());
            let string = (w[5] != 0).then(|| unsafe { &*(w[5] as *const PlyType) });
            FrameClause {
                resource: (w[1] >= 0).then(|| name(w[1])),
                closure: w[3],
                resumes: w[4] as u8,
                label: string.filter(|_| !raise),
                string,
                effect,
                op,
            }
        })
        .collect()
}

/// The label a clause written `[*t]` reads, lifted through the `String` type its `handle` site
/// handed; 0 with the context failed where it does not lift.
fn label_word(ctx: *mut Ctx, string: &'static PlyType, label: &Symbol) -> Word {
    let label = Value::Str(Arc::from(label.as_str()));
    lifted(ctx, &label, string).map_or(0, Handed::word)
}

/// A `handle` whose clause resumes off the tail: `body`, a nullary closure, runs on its own stack.
/// The clauses, `return` clause (0 for none) and body are taken.
pub unsafe extern "C" fn rt_handle_install_detached(
    ctx: *mut Ctx,
    clauses: *const Word,
    n: I64,
    ret: Handed,
    body: Block,
) -> Handed {
    let c = unsafe { &mut *ctx };
    let clauses = clauses_of(c, clauses, n.0);
    let body = unsafe { std::mem::transmute::<Block, Word>(body) };
    Handed::of(unsafe { crate::detached::open(ctx, clauses, ret.word(), body) })
}

/// The `k` a clause is handed where it resumes on the tail: one immortal block holding nothing but
/// its descriptor, since a tail resumption only records its value.
#[repr(C)]
struct ResumeBlock {
    header: heap::Obj,
    desc: &'static PlyFnDesc,
}

static RESUME_DESC: PlyFnDesc = PlyFnDesc {
    uniform: resume_uniform,
    typed: None,
    convention: 0,
    release: never_released,
    share: resumes_here_only,
    keep: resumes_here_only,
};

static RESUME: ResumeBlock = ResumeBlock {
    header: heap::Obj { rc: heap::IMMORTAL },
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

/// A resumption is its perform's: it crosses to no other thread and outlives no entry.
unsafe extern "C" fn resumes_here_only(_ctx: *mut Ctx, _k: Word) -> Word {
    0
}

/// The `k` a tail-resuming clause is handed.
fn resume_token() -> Word {
    &raw const RESUME as Word
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
        return Handed::of(raise_from_perform(c, &effect, &name, Raised(carried)));
    }
    Handed::of(unsafe { performed(ctx, effect, name, resource.0, mode.0, args, described) })
}

/// [`rt_dispatch`] past a raise.
unsafe fn performed(
    ctx: *mut Ctx,
    effect: Symbol,
    op: Symbol,
    resource: i64,
    mode: i64,
    args: &[Word],
    described: &PlyOp,
) -> i64 {
    let c = unsafe { &mut *ctx };
    let n = args.len() as i64;
    let args = args.as_ptr();
    let resource = (resource >= 0).then(|| c.tables.fields[resource as usize].clone());
    let mode = if mode != 0 { Mode::Write } else { Mode::Read };
    let atom = EffectAtom::operation(
        effect.clone(),
        resource
            .clone()
            .map_or(Resource::Singleton, Resource::Named),
        mode,
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
                        crate::simulate::perform(ctx, &effect, &op, args_of(args, n), described)
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
                    let (closure, again) = (cl.closure, cl.again());
                    let mut bound: Vec<Word> = args_of(args, n).to_vec();
                    if let (Some(string), Some(label)) = (cl.label, &resource) {
                        bound.push(label_word(ctx, string, label));
                    }
                    // Moved in: a local owning memory across the switch is freed once per restore.
                    return unsafe {
                        crate::detached::stop(
                            ctx,
                            id,
                            closure,
                            bound,
                            again,
                            (effect, op, resource),
                        )
                    };
                }
                found = Some((stack, i, cl.closure, cl.resumes != 0, cl.label));
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
    let Some((stack, depth, closure, resumes, label)) = found else {
        // Inside an open production region a `task` op is the scheduler's; outside, the host's.
        if effect.as_str() == "task"
            && ply_core::sim::TASK_OPS.contains(&op.as_str())
            && c.sims.last().is_some_and(|sim| sim.is_production())
        {
            return unsafe {
                crate::simulate::perform(ctx, &effect, &op, args_of(args, n), described)
            };
        }
        return unsafe {
            to_host(
                ctx,
                described,
                &effect,
                &op,
                resource.as_ref(),
                mode,
                args_of(args, n),
            )
        };
    };
    let mut call_args: Vec<Word> = args_of(args, n).to_vec();
    // A clause written `[*t]` reads the label its call site named, before the continuation does.
    if let (Some(string), Some(label)) = (label, &resource) {
        call_args.push(label_word(ctx, string, label));
    }
    drop((effect, op, resource));
    // The clause runs outside its handler: that frame and all above it are hidden until it returns.
    let held = c.stacks[stack].list[depth].held();
    let hidden = hide_above(c, stack, depth);
    if resumes {
        call_args.push(resume_token());
        c.resumed = None;
    }
    let here = c.keep(Kept::Hidden(hidden));
    let r = call_clause(ctx, closure, call_args, held);
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
/// through the operation's. One the production scheduler answers is handed it as it was performed.
unsafe fn to_host(
    ctx: *mut Ctx,
    described: &PlyOp,
    effect: &Symbol,
    op: &Symbol,
    resource: Option<&Symbol>,
    mode: Mode,
    args: &[Word],
) -> i64 {
    if crate::host::production_scheduled(unsafe { &*ctx }, effect, op) {
        return unsafe { crate::host::schedule(ctx, described, effect, op, resource, args) };
    }
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
    let served = unsafe { crate::host::serve(ctx, effect, op, resource, mode, values) };
    answered(ctx, served, described)
}

/// What a perform is answered: a value lifted through `described`'s answer, or 0 with the context
/// failed.
fn answered(ctx: *mut Ctx, served: Served, described: &PlyOp) -> i64 {
    match served {
        Served::Word(w) => w,
        Served::Value(v) => lifted(ctx, &v, described.answer()).map_or(0, Handed::word),
    }
}

/// What a raise carries, as values a diagnostic shows, lowered through their types.
fn raised_values(c: &mut Ctx, args: &Raised) -> Vec<Result<Value, String>> {
    let ctx: *mut Ctx = c;
    args.0.iter().map(|v| v.lower(ctx)).collect()
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
        c.host_runtime().unanswered(effect, op);
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

/// A `simulate` region emitted code wrote: `body` is a closure block, which this takes,
/// and `answer` the type of what it answers, through which the region holds its answer until it
/// hands it back.
pub unsafe extern "C" fn rt_region(ctx: *mut Ctx, body: Block, answer: *const PlyType) -> Handed {
    Handed::of(unsafe { region(ctx, Body(body, &*answer)) })
}

unsafe fn region(ctx: *mut Ctx, body: Body) -> Word {
    let c = unsafe { &mut *ctx };
    if !c.sims.is_empty() {
        body.release(ctx);
        let outer = c.sims.last().map_or(Span::DUMMY, |sim| sim.site);
        let d = ply_core::sim::err_nested_simulation(c.site(), outer);
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
    r.map_or(0, |answer| crate::simulate::word_of(c, answer))
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
        let r = call_clause(ctx, clause.closure, a.args.handed(), held);
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
    let (ret, held) = (f.ret, f.held());
    let here = c.keep(Kept::Landing(f));
    let r = Ctx::holding(ctx, held, || call_fun(ctx, ret, &[value]));
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
        .set(i.0 as usize, v);
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

/// One more holder of a shared block, for compiled code, which has no atomics of its own.
pub unsafe extern "C" fn rt_inc_shared(_ctx: *mut Ctx, w: i64) {
    heap::inc_shared(w);
}

/// `parallel { .. }`: the `n` slots at `slots` hold closure blocks, which this takes, and each
/// branch's answer is written over its closure uncounted. Where the block fails, a branch that
/// failed or never ran leaves 0, and one that answered its answer, for emitted code to let go at
/// its type.
pub unsafe extern "C" fn rt_branches(ctx: *mut Ctx, slots: *const Handed, n: I64) {
    let allowed = unsafe { &*ctx }.runs_branches_at_once(None);
    let slots = slots.cast::<Word>().cast_mut();
    unsafe { crate::parallel::run(ctx, slots, n.0 as usize, allowed) }
}

/// [`rt_branches`] of a block whose site knows what its branches may perform: `reach` holds `count`
/// pairs of field-table indices, an effect and its resource (negative for any).
pub unsafe extern "C" fn rt_branches_past(
    ctx: *mut Ctx,
    slots: *const Handed,
    n: I64,
    reach: *const Word,
    count: I64,
) {
    let allowed = unsafe { reaching(ctx, reach, count.0) };
    let slots = slots.cast::<Word>().cast_mut();
    unsafe { crate::parallel::run(ctx, slots, n.0 as usize, allowed) }
}

/// Whether a `parallel` block's branches may run at once, `reach` holding what they may perform as
/// [`rt_branches_past`] takes it.
unsafe fn reaching(ctx: *mut Ctx, reach: *const i64, count: i64) -> bool {
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
    unsafe { &*ctx }.runs_branches_at_once(Some(&reach))
}
