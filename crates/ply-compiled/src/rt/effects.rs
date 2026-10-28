//! The runtime's evidence objects, raises left pending, detached bodies' entries and `parallel`.

use super::*;

/// The label a clause written `[*t]` reads, lifted through the `String` type its `handle` site
/// handed; 0 with the context failed where it does not lift.
pub(crate) fn label_word(ctx: *mut Ctx, string: &'static PlyType, label: &Symbol) -> Word {
    string_lifted(ctx, label.as_str(), string).map_or(0, Handed::word)
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

/// The bridge: what code that holds no word for an effect performs it through: an operation is
/// unhandled, and a raise is left unanswered.
pub static BRIDGE: RuntimeObject = RuntimeObject {
    flags: 1,
    slot: Slot {
        entry: bridged,
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
/// unanswered, and any other operation refused, as unhandled where the object is the bridge and
/// as unserved otherwise.
unsafe fn at_entry(
    ctx: *mut Ctx,
    op: *const PlyOp,
    mode: I64,
    resource: I64,
    args: *const Handed,
    unhandled: bool,
) -> Handed {
    let c = unsafe { &mut *ctx };
    let described = unsafe { &*op };
    let (effect, name) = named_op(c, described);
    let args = args_of(args.cast(), described.params_len);
    if mode.0 == MODE_RAISE || is_abort(effect.as_str(), name.as_str()) {
        return Handed::of(unanswered(c, &effect, &name, raised_of(args, described)));
    }
    let resource = (resource.0 >= 0).then(|| c.tables.fields[resource.0 as usize].clone());
    if unhandled {
        let mode = if mode.0 != 0 { Mode::Write } else { Mode::Read };
        recorded(c, &effect, &name, resource.as_ref(), mode);
    }
    let_go(ctx, args, described);
    let c = unsafe { &mut *ctx };
    let site = c.site();
    let d = if unhandled {
        ply_core::handler::err_unhandled(site, &effect, &name, resource.as_ref())
    } else {
        let operation = ply_core::host::operation_label(&effect, &name, resource.as_ref());
        ply_core::host::err_unserved(site, &operation)
    };
    Handed::of(c.fail(d))
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
/// for none) in `mode`, that a clause answered, as the entry's objects note each perform they are
/// handed. Compiled code calls it only while [`Ctx::recording`] is armed.
pub unsafe extern "C" fn rt_performed(ctx: *mut Ctx, op: *const PlyOp, resource: I64, mode: I64) {
    let c = unsafe { &mut *ctx };
    let (effect, name) = named_op(c, unsafe { &*op });
    let resource = (resource.0 >= 0).then(|| c.tables.fields[resource.0 as usize].clone());
    let mode = if mode.0 != 0 { Mode::Write } else { Mode::Read };
    recorded(c, &effect, &name, resource.as_ref(), mode);
}

/// Refuses a clause leaving an instance another task opened, the operation `op` describes performed
/// in a task: what it would leave is the spawner's, not the task's. 0 with the context failed.
pub unsafe extern "C" fn rt_task_abandons(ctx: *mut Ctx, op: *const PlyOp) -> Word {
    let c = unsafe { &mut *ctx };
    let (effect, name) = named_op(c, unsafe { &*op });
    let site = c.site();
    c.fail(err_task_abandoned(&effect, &name, site))
}

/// Refuses a clause resuming off its tail that would stop a detached body another task began, the
/// operation `op` describes performed in a task: what it would capture is the spawner's
/// continuation, not the task's. 0 with the context failed.
pub unsafe extern "C" fn rt_task_captures(ctx: *mut Ctx, op: *const PlyOp) -> Word {
    let c = unsafe { &mut *ctx };
    let (effect, name) = named_op(c, unsafe { &*op });
    let site = c.site();
    c.fail(err_task_captures(&effect, &name, site))
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

/// A task's perform answered by a clause, of a `handle` around its spawn, that resumes off its tail:
/// what it would capture is the continuation of the spawner's body, not the task's.
#[cold]
#[inline(never)]
fn err_task_captures(effect: &Symbol, op: &Symbol, site: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("`{effect}.{op}` was performed in a task, and its handler resumes off the tail"),
    )
    .primary(site, "performed here")
    .note(
        "the clause would capture the continuation of the body the task was spawned in, which is \
         not the task's own: handle the operation inside the task, or resume on the tail",
    )
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
    // Only a raise nothing answers is shown what it carried: one a clause takes is not read.
    let carried: Result<Vec<Vec<u8>>, String> = args.0.iter().map(|v| shown(c, v)).collect();
    let carried = match carried {
        Ok(carried) => carried,
        Err(why) => {
            args.release();
            return c.fail(err_crossing(&why));
        }
    };
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
    let text = |bytes: Vec<u8>| -> Option<String> {
        let mut r = ply_core::codec::Reader::new(bytes).ok()?;
        r.expect(ply_core::codec::tag::STR).ok()?;
        let s = r.str().ok()?.to_string();
        r.end().ok().map(|()| s)
    };
    args.0
        .first()
        .and_then(|v| shown(c, v).ok())
        .and_then(text)
        .unwrap_or_default()
}

/// The message of the raise [`FAILED_PENDING`] stands for, as `string`, the type of `String`, lays
/// it out, the entry no longer failed: for compiled code to raise it through the word that answers
/// it. Where that word's slot is [`BRIDGE`]'s or [`UNSERVED`]'s (`bridged` not 0), the raise
/// stays pending as it is, its diagnostic kept, for the code around to take. 0 with the entry
/// failed as it was where nothing is pending or the raise stays so.
pub unsafe extern "C" fn rt_pending_raised(
    ctx: *mut Ctx,
    string: *const PlyType,
    bridged: I64,
) -> Handed {
    let c = unsafe { &mut *ctx };
    if c.failed != FAILED_PENDING || bridged.0 != 0 {
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
    c.diagnostic = None;
    string_lifted(ctx, &message, unsafe { &*string }).unwrap_or(Handed::of(0))
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

/// One more holder of a shared block, for compiled code, which has no atomics of its own.
pub unsafe extern "C" fn rt_inc_shared(_ctx: *mut Ctx, w: i64) {
    heap::inc_shared(w);
}

/// `parallel { .. }`: the `n` slots at `slots` hold closure blocks, which this takes, and each
/// branch's answer is written over its closure uncounted. Where the block fails, a branch that
/// failed or never ran leaves 0, and one that answered its answer, for emitted code to let go at
/// its type.
pub unsafe extern "C" fn rt_branches(ctx: *mut Ctx, slots: *const Handed, n: I64) {
    let allowed = unsafe { &*ctx }.runs_branches_at_once();
    let slots = slots.cast::<Word>().cast_mut();
    unsafe { crate::parallel::run(ctx, slots, n.0 as usize, allowed) }
}
