use super::*;

pub(crate) fn label_word(ctx: *mut Ctx, string: &'static PlyType, label: &Symbol) -> Word {
    decode_string(ctx, label.as_str(), string).map_or(0, Handed::word)
}

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
    Handed::of(unsafe { crate::detached::open_detached(ctx, body, forwards, instance) })
}

pub unsafe extern "C" fn rt_detached_perform(
    ctx: *mut Ctx,
    record: *const Word,
    op: *const PlyOp,
    resource: I64,
    args: *const Handed,
) -> Handed {
    let described = unsafe { &*op };
    let args = args_of(args.cast(), described.params_len).to_vec();
    Handed::of(unsafe { crate::detached::stop_detached(ctx, record, args, resource.0) })
}

pub unsafe extern "C" fn rt_detached_stop(
    ctx: *mut Ctx,
    record: *const Word,
    op: *const PlyOp,
    resource: I64,
    args: *const Handed,
    _again: I64,
) -> Handed {
    unsafe { rt_detached_perform(ctx, record, op, resource, args) }
}

pub type OpEntry =
    unsafe extern "C" fn(*mut Ctx, Word, *const PlyOp, I64, I64, *const Handed) -> Handed;

pub const EVIDENCE_ONE_SLOT: i64 = 1;

pub const EVIDENCE_SHARED: i64 = 2;

#[repr(C)]
pub struct RuntimeObject {
    pub flags: i64,
    pub slot: Slot,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Slot {
    pub entry: OpEntry,
    pub env: Word,
}

pub static NO_HANDLER_OBJECT: RuntimeObject = RuntimeObject {
    flags: EVIDENCE_ONE_SLOT | EVIDENCE_SHARED,
    slot: Slot {
        entry: bridged,
        env: 0,
    },
};

pub static UNHANDLED_OPERATION: RuntimeObject = RuntimeObject {
    flags: EVIDENCE_ONE_SLOT | EVIDENCE_SHARED,
    slot: Slot {
        entry: unserved,
        env: 0,
    },
};

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
    unsafe { perform_operation(ctx, op, mode, resource, args, true) }
}

unsafe extern "C" fn unserved(
    ctx: *mut Ctx,
    _env: Word,
    op: *const PlyOp,
    mode: I64,
    resource: I64,
    args: *const Handed,
) -> Handed {
    unsafe { perform_operation(ctx, op, mode, resource, args, false) }
}

unsafe fn perform_operation(
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
        return Handed::of(pending_raise(c, &effect, &name, raised_of(args, described)));
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
        ply_core::host::err_unhandled_operation(site, &operation)
    };
    Handed::of(c.fail(d))
}

fn named_op(c: &Ctx, described: &PlyOp) -> (Symbol, Symbol) {
    let name = |i: i64| c.tables.fields[i as usize].clone();
    (name(described.effect), name(described.op))
}

fn let_go(ctx: *mut Ctx, args: &[Word], described: &PlyOp) {
    for (w, ty) in args.iter().zip(described.params()) {
        unsafe { Carried::new(Handed::of(*w), *ty) }.release(ctx);
    }
}

fn raised_of(args: &[Word], described: &PlyOp) -> Raised {
    Raised(
        args.iter()
            .zip(described.params())
            .map(|(w, ty)| unsafe { Carried::new(Handed::of(*w), *ty) })
            .collect(),
    )
}

fn recorded(c: &mut Ctx, effect: &Symbol, op: &Symbol, resource: Option<&Symbol>, mode: Mode) {
    let performed = Effect::operation(
        effect.clone(),
        resource
            .cloned()
            .map_or(Resource::Singleton, Resource::Named),
        mode,
        op.clone(),
    );
    c.touched(crate::tasks::Access::Effect {
        effect: effect.clone(),
        resource: resource.cloned(),
        write: mode == Mode::Write,
    });
    c.performed.push(performed);
}

pub unsafe extern "C" fn rt_performed(ctx: *mut Ctx, op: *const PlyOp, resource: I64, mode: I64) {
    let c = unsafe { &mut *ctx };
    let (effect, name) = named_op(c, unsafe { &*op });
    let resource = (resource.0 >= 0).then(|| c.tables.fields[resource.0 as usize].clone());
    let mode = if mode.0 != 0 { Mode::Write } else { Mode::Read };
    recorded(c, &effect, &name, resource.as_ref(), mode);
}

pub unsafe extern "C" fn rt_task_abandons(ctx: *mut Ctx, op: *const PlyOp) -> Word {
    let c = unsafe { &mut *ctx };
    let (effect, name) = named_op(c, unsafe { &*op });
    let site = c.site();
    c.fail(err_task_not_resumed(&effect, &name, site))
}

pub unsafe extern "C" fn rt_task_captures(ctx: *mut Ctx, op: *const PlyOp) -> Word {
    let c = unsafe { &mut *ctx };
    let (effect, name) = named_op(c, unsafe { &*op });
    let site = c.site();
    c.fail(err_task_captures(&effect, &name, site))
}

pub unsafe extern "C" fn rt_task_raises(
    ctx: *mut Ctx,
    op: *const PlyOp,
    args: *const Handed,
) -> Word {
    let c = unsafe { &mut *ctx };
    let described = unsafe { &*op };
    let (effect, name) = named_op(c, described);
    let args = args_of(args.cast(), described.params_len);
    pending_raise(c, &effect, &name, raised_of(args, described))
}

#[cold]
#[inline(never)]
fn err_task_not_resumed(effect: &Symbol, op: &Symbol, site: Span) -> Diagnostic {
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

pub(super) fn pending_raise(c: &mut Ctx, effect: &Symbol, op: &Symbol, args: Raised) -> i64 {
    if c.failed != 0 {
        args.release();
        return 0;
    }
    if is_abort(effect.as_str(), op.as_str()) {
        let message = raised_message(c, &args);
        args.release();
        let d = Diagnostic::error(codes::RUNTIME_ERROR, format!("panic: {message}"))
            .primary(c.site(), "raised here");
        return c.fail_unhandled_abort(d, message);
    }
    let carried: Result<Vec<Vec<u8>>, String> = args.0.iter().map(|v| shown(c, v)).collect();
    let carried = match carried {
        Ok(carried) => carried,
        Err(reason) => {
            args.release();
            return c.fail(err_marshalling(&reason));
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
        format!("`{effect}.{op}` was raised{with}, and nothing handles it"),
    )
    .note("a `try` around the call handles it as an `Err`, and a `handle` clause as it likes")
    .showing(carried)
    .primary(c.site(), "raised here");
    c.fail_with_pending_raise(d, effect.clone(), op.clone(), args)
}

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

pub unsafe extern "C" fn rt_pending_raised(
    ctx: *mut Ctx,
    string: *const PlyType,
    bridged: I64,
) -> Handed {
    let c = unsafe { &mut *ctx };
    if c.failed != FAILED_PENDING_RAISE || bridged.0 != 0 {
        return Handed::of(0);
    }
    let message = match c.pending.take() {
        Some(Unhandled::Abort(message)) => message,
        other => {
            c.pending = other;
            return Handed::of(0);
        }
    };
    c.failed = 0;
    c.diagnostic = None;
    decode_string(ctx, &message, unsafe { &*string }).unwrap_or(Handed::of(0))
}

pub unsafe extern "C" fn rt_pending_raised_at(ctx: *mut Ctx, effect: I64, op: I64) -> Handed {
    let c = unsafe { &mut *ctx };
    if c.failed != FAILED_PENDING_RAISE {
        return Handed::of(0);
    }
    let named = |s: &Symbol, i: I64| c.tables.fields.get(i.0 as usize) == Some(s);
    let ours = match &c.pending {
        Some(Unhandled::Raised {
            effect: e, op: o, ..
        }) => named(e, effect) && named(o, op),
        _ => false,
    };
    if !ours {
        return Handed::of(0);
    }
    let Some(Unhandled::Raised { args, .. }) = c.pending.take() else {
        unreachable!("the pending raise was just read");
    };
    c.failed = 0;
    c.diagnostic = None;
    c.pending_words = args.handed();
    c.pending_words.push(0);
    Handed::of(c.pending_words.as_ptr() as Word)
}

pub unsafe extern "C" fn rt_inc_shared(_ctx: *mut Ctx, w: i64) {
    heap::inc_shared(w);
}

pub unsafe extern "C" fn rt_branches(ctx: *mut Ctx, slots: *const Handed, n: I64) {
    let allowed = unsafe { &*ctx }.runs_branches_at_once();
    let slots = slots.cast::<Word>().cast_mut();
    unsafe { crate::parallel::run(ctx, slots, n.0 as usize, allowed) }
}
