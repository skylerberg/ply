//! Handlers, `perform` and `parallel`.

use super::*;

/// Installs a handler and answers its depth: the clause closures and `ret`, the `return` clause or
/// 0 where it has none, are taken until it pops.
pub unsafe extern "C" fn rt_handle_install(
    ctx: *mut Ctx,
    clauses: *const Word,
    n: I64,
    ret: Handed,
) -> I64 {
    I64(installed(unsafe { &mut *ctx }, clauses, n, ret).0)
}

/// [`rt_handle_install`] for a `handle` whose evidence objects reach its frame by its handler,
/// which this writes to `cell`.
pub unsafe extern "C" fn rt_handle_install_at(
    ctx: *mut Ctx,
    clauses: *const Word,
    n: I64,
    ret: Handed,
    cell: Out,
) -> I64 {
    let (depth, handler) = installed(unsafe { &mut *ctx }, clauses, n, ret);
    unsafe { cell.0.write(handler as i64) };
    I64(depth)
}

/// A handler installed on the running stack: its depth there and its handler.
fn installed(c: &mut Ctx, clauses: *const Word, n: I64, ret: Handed) -> (i64, u64) {
    let clauses = clauses_of(c, clauses, n.0);
    let fuel = c.fuel;
    let handler = c.next_handler();
    let frames = c.frames();
    frames.push(HandlerFrame {
        clauses,
        ret: ret.word(),
        detached: None,
        fuel,
        handler,
        vars: None,
        inherited: false,
    });
    ((frames.len() - 1) as i64, handler)
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
pub(crate) fn label_word(ctx: *mut Ctx, string: &'static PlyType, label: &Symbol) -> Word {
    let label = Value::Str(Arc::from(label.as_str()));
    lifted(ctx, &label, string).map_or(0, Handed::word)
}

/// The label the field-table entry `resource` names, lifted through `string`, the type of `String`:
/// what a clause written `[*t]` is handed beside its arguments. 0 with the context failed where it
/// names no label or does not lift.
pub unsafe extern "C" fn rt_label_string(
    ctx: *mut Ctx,
    resource: I64,
    string: *const PlyType,
) -> Handed {
    let c = unsafe { &mut *ctx };
    let Some(label) = usize::try_from(resource.0)
        .ok()
        .and_then(|i| c.tables.fields.get(i))
        .cloned()
    else {
        return Handed::of(c.fail(error("a clause on every label was performed at no label")));
    };
    Handed::of(label_word(ctx, unsafe { &*string }, &label))
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
    Handed::of(unsafe { crate::detached::open(ctx, clauses, ret.word(), body, None) })
}

/// [`rt_handle_install_detached`] for a body, clauses and `return` clause that hold the words of
/// `forwards`, a block laid out as [`crate::detached::Forwards`] says, which this takes: the body
/// keeps it as long as anything can resume it.
pub unsafe extern "C" fn rt_handle_forwarded(
    ctx: *mut Ctx,
    clauses: *const Word,
    n: I64,
    ret: Handed,
    body: Block,
    forwards: Block,
) -> Handed {
    let c = unsafe { &mut *ctx };
    let clauses = clauses_of(c, clauses, n.0);
    let body = unsafe { std::mem::transmute::<Block, Word>(body) };
    let forwards = unsafe { std::mem::transmute::<Block, Word>(forwards) };
    Handed::of(unsafe { crate::detached::open(ctx, clauses, ret.word(), body, Some(forwards)) })
}

/// The body of a `handle` whose objects enter its clauses, `body` a nullary closure, run on a stack
/// of its own until it stops or ends, its landing its own; answers the `handle`'s value. Takes the
/// body and `forwards`, the block forwarding what is around the `handle`, and holds `instance`, the
/// instance's counted block, once more.
pub unsafe extern "C" fn rt_detached_open(
    ctx: *mut Ctx,
    body: Block,
    forwards: Block,
    instance: Block,
) -> Handed {
    let body = unsafe { std::mem::transmute::<Block, Word>(body) };
    let forwards = unsafe { std::mem::transmute::<Block, Word>(forwards) };
    let instance = unsafe { std::mem::transmute::<Block, Word>(instance) };
    let forwards = (forwards != 0).then_some(forwards);
    Handed::of(unsafe { crate::detached::open_over(ctx, body, forwards, instance) })
}

/// The perform of the operation `op` describes, on the field-table entry `resource` and with the
/// words at `args`, which this takes, answered off its tail by the clause whose record is at
/// `record`: the `handle`'s body stops there, and the clause runs where it was entered or resumed
/// from, which may resume it more than once where `again`. Answers what the body is resumed with.
pub unsafe extern "C" fn rt_detached_stop(
    ctx: *mut Ctx,
    record: *const Word,
    op: *const PlyOp,
    resource: I64,
    args: *const Handed,
    again: I64,
) -> Handed {
    let described = unsafe { &*op };
    let args = args_of(args.cast(), described.params_len).to_vec();
    Handed::of(unsafe { crate::detached::stop_over(ctx, record, args, resource.0, again.0 != 0) })
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
    let c = unsafe { &mut *ctx };
    // A tail resumption reads no words; those its caller set are no other closure's.
    c.entering_desc = 0;
    c.entering_words = 0;
    c.resumed = Some(v);
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
    unsafe { dispatched(ctx, None, op, resource, mode, args) }
}

/// [`rt_dispatch`] reaching the frame of handler `handler` alone: what the evidence object of a
/// `handle` the runtime's frames hold hands it, so the perform reaches that `handle` whatever frames
/// stand nearer it.
pub unsafe extern "C" fn rt_dispatch_at(
    ctx: *mut Ctx,
    handler: I64,
    op: *const PlyOp,
    resource: I64,
    mode: I64,
    args: *const Handed,
) -> Handed {
    unsafe { dispatched(ctx, Some(handler.0 as u64), op, resource, mode, args) }
}

/// A perform the frames answer, by the nearest that answers it or by the one of handler `target`.
unsafe fn dispatched(
    ctx: *mut Ctx,
    target: Option<u64>,
    op: *const PlyOp,
    resource: I64,
    mode: I64,
    args: *const Handed,
) -> Handed {
    let c = unsafe { &mut *ctx };
    let described = unsafe { &*op };
    let (effect, name) = named_op(c, described);
    let args = args_of(args.cast(), described.params_len);
    if mode.0 == MODE_RAISE || is_abort(effect.as_str(), name.as_str()) {
        let raised = raised_of(args, described);
        return Handed::of(raise_from_perform(c, &effect, &name, raised, target));
    }
    let performing = Performing {
        effect,
        op: name,
        resource: resource.0,
        mode: mode.0,
        target,
    };
    Handed::of(unsafe { performed(ctx, performing, args, described) })
}

/// What [`performed`] searches the frames for: an operation on a resource (negative for none) in a
/// mode, answered by the nearest frame or, with a `target`, by that handler's frame alone.
struct Performing {
    effect: Symbol,
    op: Symbol,
    resource: i64,
    mode: i64,
    target: Option<u64>,
}

/// An operation's entry in an evidence object, as compiled code calls it: the slot's environment,
/// the operation's descriptor, its mode, the resource (negative for none) and its arguments' words.
pub type OpEntry =
    unsafe extern "C" fn(*mut Ctx, Word, *const PlyOp, I64, I64, *const Handed) -> Handed;

/// An evidence object of the runtime's own, laid out as an evidence object is: its flags, then an
/// entry and its environment per operation, or, with the flags set as here, one slot that answers
/// every operation, which reads the operation from its descriptor.
#[repr(C)]
pub struct RuntimeObject {
    pub flags: i64,
    pub slot: Slot,
}

/// An operation's slot in an evidence object.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Slot {
    pub entry: OpEntry,
    pub env: Word,
}

/// The frame bridge, which hands every perform to [`rt_dispatch`], which searches the frames: what
/// code that holds no word for an effect performs it through.
pub static BRIDGE: RuntimeObject = RuntimeObject {
    flags: 1,
    slot: Slot {
        entry: bridged,
        env: 0,
    },
};

/// What an entry hands its root for an effect the Rust binding serves: an operation goes to the
/// binding, no frame searched, and a raise is left unanswered.
pub static HOSTED: RuntimeObject = RuntimeObject {
    flags: 1,
    slot: Slot {
        entry: hosted,
        env: 0,
    },
};

/// What an entry hands its root for an effect nothing answers: an operation is refused (`E0424`),
/// and a raise is left unanswered.
pub static UNSERVED: RuntimeObject = RuntimeObject {
    flags: 1,
    slot: Slot {
        entry: unserved,
        env: 0,
    },
};

/// The word compiled code holds `object` by.
pub(crate) fn object_word(object: &'static RuntimeObject) -> Word {
    std::ptr::from_ref(object) as Word
}

unsafe extern "C" fn bridged(
    ctx: *mut Ctx,
    _env: Word,
    op: *const PlyOp,
    mode: I64,
    resource: I64,
    args: *const Handed,
) -> Handed {
    unsafe { rt_dispatch(ctx, op, resource, mode, args) }
}

unsafe extern "C" fn hosted(
    ctx: *mut Ctx,
    _env: Word,
    op: *const PlyOp,
    mode: I64,
    resource: I64,
    args: *const Handed,
) -> Handed {
    unsafe { at_entry(ctx, op, mode, resource, args, true) }
}

unsafe extern "C" fn unserved(
    ctx: *mut Ctx,
    _env: Word,
    op: *const PlyOp,
    mode: I64,
    resource: I64,
    args: *const Handed,
) -> Handed {
    unsafe { at_entry(ctx, op, mode, resource, args, false) }
}

/// A perform the object its entry hands answers, since nothing between answers it: a raise left
/// unanswered, and any other operation handed to the Rust binding where `hosted`, else refused.
unsafe fn at_entry(
    ctx: *mut Ctx,
    op: *const PlyOp,
    mode: I64,
    resource: I64,
    args: *const Handed,
    hosted: bool,
) -> Handed {
    let c = unsafe { &mut *ctx };
    let described = unsafe { &*op };
    let (effect, name) = named_op(c, described);
    let args = args_of(args.cast(), described.params_len);
    if mode.0 == MODE_RAISE || is_abort(effect.as_str(), name.as_str()) {
        return Handed::of(unanswered(c, &effect, &name, raised_of(args, described)));
    }
    let resource = (resource.0 >= 0).then(|| c.tables.fields[resource.0 as usize].clone());
    let mode = if mode.0 != 0 { Mode::Write } else { Mode::Read };
    if !hosted {
        let_go(ctx, args, described);
        let c = unsafe { &mut *ctx };
        let operation = ply_core::host::operation_label(&effect, &name, resource.as_ref());
        let d = ply_core::host::err_unserved(c.site(), &operation);
        return Handed::of(c.fail(d));
    }
    recorded(c, &effect, &name, resource.as_ref(), mode);
    Handed::of(unsafe {
        to_host(
            ctx,
            described,
            &effect,
            &name,
            resource.as_ref(),
            mode,
            args,
        )
    })
}

/// The effect and the operation `described` describes, by their program-wide names.
fn named_op(c: &Ctx, described: &PlyOp) -> (Symbol, Symbol) {
    let name = |i: i64| c.tables.fields[i as usize].clone();
    (name(described.effect), name(described.op))
}

/// Lets go of `args`, what a perform handed the operation `described` describes, through the
/// operation's parameters' types.
fn let_go(ctx: *mut Ctx, args: &[Word], described: &PlyOp) {
    for (w, ty) in args.iter().zip(described.params()) {
        unsafe { Carried::new(Handed::of(*w), *ty) }.release(ctx);
    }
}

/// What a raise hands the operation `described` describes, `args` its words, held through the
/// operation's parameters' types.
fn raised_of(args: &[Word], described: &PlyOp) -> Raised {
    Raised(
        args.iter()
            .zip(described.params())
            .map(|(w, ty)| unsafe { Carried::new(Handed::of(*w), *ty) })
            .collect(),
    )
}

/// A perform of `effect.op` on `resource` in `mode` noted where this entry's performs are read: in
/// its trace, and as a region's step access.
fn recorded(c: &mut Ctx, effect: &Symbol, op: &Symbol, resource: Option<&Symbol>, mode: Mode) {
    let atom = EffectAtom::operation(
        effect.clone(),
        resource
            .cloned()
            .map_or(Resource::Singleton, Resource::Named),
        mode,
        op.clone(),
    );
    c.touched(crate::tasks::Touched::Atom {
        effect: effect.clone(),
        resource: resource.cloned(),
        write: mode == Mode::Write,
    });
    c.performed.push(atom);
}

/// Notes a perform of the operation `op` describes, on the field-table entry `resource` (negative
/// for none) in `mode`, that a clause answered in place, as [`rt_dispatch`] notes every perform it
/// is handed. Compiled code calls it only while [`Ctx::recording`] is armed.
pub unsafe extern "C" fn rt_performed(ctx: *mut Ctx, op: *const PlyOp, resource: I64, mode: I64) {
    let c = unsafe { &mut *ctx };
    let (effect, name) = named_op(c, unsafe { &*op });
    let resource = (resource.0 >= 0).then(|| c.tables.fields[resource.0 as usize].clone());
    let mode = if mode.0 != 0 { Mode::Write } else { Mode::Read };
    recorded(c, &effect, &name, resource.as_ref(), mode);
}

/// [`rt_dispatch`] past a raise.
unsafe fn performed(ctx: *mut Ctx, p: Performing, args: &[Word], described: &PlyOp) -> i64 {
    let Performing {
        effect,
        op,
        resource,
        mode,
        target,
    } = p;
    let c = unsafe { &mut *ctx };
    let n = args.len() as i64;
    let args = args.as_ptr();
    let resource = (resource >= 0).then(|| c.tables.fields[resource as usize].clone());
    let mode = if mode != 0 { Mode::Write } else { Mode::Read };
    recorded(c, &effect, &op, resource.as_ref(), mode);
    let mut found = None;
    let mut inherited_off_tail = false;
    let mut stack = c.current;
    'search: loop {
        for (i, f) in c.stacks[stack].list.iter().enumerate().rev() {
            if target.is_some_and(|t| t != f.handler) {
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
                found = Some((stack, i, cl.closure, cl.resumes != 0, cl.label, f.inherited));
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
    let Some((stack, depth, closure, resumes, label, inherited)) = found else {
        if target.is_some() {
            return c.fail(err_handle_gone(&effect, &op));
        }
        // Inside an open production region a `task` op is the scheduler's; outside, the host's.
        if effect.as_str() == "task"
            && ply_core::sim::TASK_OPS.contains(&op.as_str())
            && !c.sims.is_empty()
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
    // Where nothing on the task's stack lands for the frame, a clause that does not resume is refused.
    let abandoning = (inherited && resumes).then(|| (effect.clone(), op.clone(), c.site()));
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
    match (c.resumed.take(), abandoning) {
        (Some(v), _) => v,
        // What the clause answered is emitted code's, abandoned uncounted as an unwind's is.
        (None, Some((effect, op, site))) => c.fail(err_task_abandoned(&effect, &op, site)),
        (None, None) => {
            c.unwind = Some((stack, depth, r));
            c.failed = FAILED_UNWIND;
            0
        }
    }
}

/// Refuses a clause leaving an instance another task opened, the operation `op` describes performed
/// in a task: what it would leave is the spawner's, not the task's. 0 with the context failed.
pub unsafe extern "C" fn rt_task_abandons(ctx: *mut Ctx, op: *const PlyOp) -> Word {
    let c = unsafe { &mut *ctx };
    let (effect, name) = named_op(c, unsafe { &*op });
    let site = c.site();
    c.fail(err_task_abandoned(&effect, &name, site))
}

/// Leaves unanswered a raise, the operation `op` describes with the `args` it takes, that a task
/// made and a clause of an instance another task opened would answer: no clause for a raise
/// answers a task from around its spawn, so the raise leaves the task, and its region raises it
/// again where the region was opened. 0 with the context failed.
pub unsafe extern "C" fn rt_task_raises(
    ctx: *mut Ctx,
    op: *const PlyOp,
    args: *const Handed,
) -> Word {
    let c = unsafe { &mut *ctx };
    let described = unsafe { &*op };
    let (effect, name) = named_op(c, described);
    let args = args_of(args.cast(), described.params_len);
    unanswered(c, &effect, &name, raised_of(args, described))
}

/// A task's perform answered by a clause, of a `handle` around its spawn, that stopped the body
/// rather than resume it: what it would abandon is the spawner's, not the task's.
#[cold]
#[inline(never)]
fn err_task_abandoned(effect: &Symbol, op: &Symbol, site: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("`{effect}.{op}` was performed in a task, and its handler did not resume it"),
    )
    .primary(site, "performed here")
    .note(
        "the clause would abandon the body the task was spawned in, which is not the task's own: \
         handle the operation inside the task, or resume on every path",
    )
}

/// A perform or raise sent to a `handle` whose frame is no longer installed where it was made: an
/// evidence object outlived the `handle` it was laid out for.
#[cold]
#[inline(never)]
fn err_handle_gone(effect: &Symbol, op: &Symbol) -> Diagnostic {
    error(format!(
        "`{effect}.{op}` reached for a `handle` that is no longer installed"
    ))
    .note("this is Ply's fault, not the program's")
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
    let_go(ctx, args, described);
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
pub(super) fn raise_from_perform(
    c: &mut Ctx,
    effect: &Symbol,
    op: &Symbol,
    args: Raised,
    target: Option<u64>,
) -> i64 {
    if c.failed != 0 {
        args.release();
        return 0;
    }
    let Some(at) = c.raise_handler(effect.as_str(), op.as_str(), target) else {
        if target.is_some() {
            args.release();
            return c.fail(err_handle_gone(effect, op));
        }
        return unanswered(c, effect, op, args);
    };
    let d = if is_abort(effect.as_str(), op.as_str()) {
        Diagnostic::error(
            codes::RUNTIME_ERROR,
            format!("panic: {}", raised_message(c, &args)),
        )
    } else {
        Diagnostic::error(codes::RUNTIME_ERROR, format!("`{effect}.{op}` was raised"))
    };
    let d = d.primary(c.site(), "raised here");
    c.bind_raise(at, args, d)
}

/// A raise no `handle` answers, left pending for the code around it, as [`Ctx::pend`] says, with
/// what the entry fails with should nothing take it: `abort.raise` a panic of its message, any other
/// raise naming the operation and what it carried. Takes the arguments.
pub(super) fn unanswered(c: &mut Ctx, effect: &Symbol, op: &Symbol, args: Raised) -> i64 {
    if c.failed != 0 {
        args.release();
        return 0;
    }
    if is_abort(effect.as_str(), op.as_str()) {
        let message = raised_message(c, &args);
        args.release();
        let d = Diagnostic::error(codes::RUNTIME_ERROR, format!("panic: {message}"))
            .primary(c.site(), "raised here");
        return c.pend(d, message);
    }
    // Only a raise no frame answers is shown what it carried: one a clause takes is not read.
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
    let d = Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("`{effect}.{op}` was raised{with}, and nothing answers it"),
    )
    .note("a `try` around the call answers it as an `Err`, and a `handle` clause as it likes")
    .showing(carried)
    .primary(c.site(), "raised here");
    c.pend_raised(d, effect.clone(), op.clone(), args)
}

/// The message an `abort.raise` carries, empty where it does not lower to a string.
fn raised_message(c: &mut Ctx, args: &Raised) -> String {
    match raised_values(c, args).first() {
        Some(Ok(Value::Str(s))) => s.to_string(),
        _ => String::new(),
    }
}

/// The message of the raise [`FAILED_PENDING`] stands for, as `string`, the type of `String`, lays
/// it out, the entry no longer failed: for compiled code to raise it through the word that answers
/// it. Where that word's slot is the frame bridge's or [`UNSERVED`]'s (`bridged` not 0), the raise
/// goes on from here as the one it is, its diagnostic kept: to a frame that answers it, or pending
/// again, and this answers 0 with the entry failed. 0 with the entry failed as it was where nothing
/// is pending.
pub unsafe extern "C" fn rt_pending_raised(
    ctx: *mut Ctx,
    string: *const PlyType,
    bridged: I64,
) -> Handed {
    let c = unsafe { &mut *ctx };
    if c.failed != FAILED_PENDING {
        return Handed::of(0);
    }
    let message = match c.pending.take() {
        Some(Unanswered::Abort(message)) => message,
        other => {
            c.pending = other;
            return Handed::of(0);
        }
    };
    c.failed = 0;
    let diagnostic = c.diagnostic.take();
    if bridged.0 != 0 {
        let d = diagnostic.unwrap_or_else(|| {
            Diagnostic::error(codes::RUNTIME_ERROR, format!("panic: {message}"))
        });
        c.raise(d, message);
        return Handed::of(0);
    }
    let message = Value::str(message);
    lifted(ctx, &message, unsafe { &*string }).unwrap_or(Handed::of(0))
}

/// The words of the raise [`FAILED_PENDING`] stands for, where it is a raise of the effect and
/// operation the field-table entries `effect` and `op` name and not `abort.raise`, the entry no
/// longer failed: for compiled code to hand the clause that answers it, which takes them. The array
/// has a word to spare past them and lasts until the next such raise. 0 with the entry failed as it
/// was otherwise.
pub unsafe extern "C" fn rt_pending_raised_at(ctx: *mut Ctx, effect: I64, op: I64) -> Handed {
    let c = unsafe { &mut *ctx };
    if c.failed != FAILED_PENDING {
        return Handed::of(0);
    }
    let named = |s: &Symbol, i: I64| c.tables.fields.get(i.0 as usize) == Some(s);
    let ours = match &c.pending {
        Some(Unanswered::Raised {
            effect: e, op: o, ..
        }) => named(e, effect) && named(o, op),
        _ => false,
    };
    if !ours {
        return Handed::of(0);
    }
    let Some(Unanswered::Raised { args, .. }) = c.pending.take() else {
        unreachable!("the pending raise was just read");
    };
    c.failed = 0;
    c.diagnostic = None;
    c.pending_words = args.handed();
    c.pending_words.push(0);
    Handed::of(c.pending_words.as_ptr() as Word)
}

/// Refuses a `simulate` an older emitter wrote as a call of the runtime, letting go of its body: a
/// region is `std.internal.schedule`'s. A released `ply` refuses a runtime that lacks this helper.
pub unsafe extern "C" fn rt_region(ctx: *mut Ctx, body: Block, _answer: *const PlyType) -> Handed {
    unsafe { body.let_go_function(ctx) };
    let c = unsafe { &mut *ctx };
    Handed::of(c.fail(Diagnostic::error(
        codes::INTERNAL_ERROR,
        "a `simulate` was emitted as a call of the runtime, which runs no region",
    )))
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
    // A raise left pending that nothing inside the frame took is the frame's where a clause of it
    // answers it.
    if c.failed == FAILED_PENDING
        && let Some(f) = &mine
        && let Some(clause) =
            (c.pending.as_ref()).and_then(|p| f.clauses.iter().position(|cl| p.answered_by(cl)))
    {
        let string = f.clauses[clause].string;
        c.bind_pending((stack, depth, clause), string);
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
