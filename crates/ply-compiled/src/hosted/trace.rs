use super::{Nondeterministic, asked, done, handed, with_host_facilities};
use crate::c::{MachineNative, runtime_native};
use crate::native::{Addr, I64};
use crate::rt::{Ctx, borrowed};
use ply_core::Decimal;
use ply_host::trace::{
    Field, Level, Outcome, Owner, RecordKind, Trace, fields_read, spans::err_unbalanced,
};
use std::str::FromStr;
use std::sync::Arc;

pub(super) fn natives() -> Vec<MachineNative> {
    vec![
        runtime_native(
            "trace_wants",
            rt_trace_wants as unsafe extern "C" fn(_, _) -> _,
        ),
        runtime_native(
            "trace_record",
            rt_trace_record as unsafe extern "C" fn(_, _, _, _, _, _, _, _, _, _, _, _),
        ),
        runtime_native(
            "trace_enter",
            rt_trace_enter as unsafe extern "C" fn(_, _, _, _, _, _, _) -> _,
        ),
        runtime_native(
            "trace_exit",
            rt_trace_exit as unsafe extern "C" fn(_, _, _, _, _, _, _),
        ),
    ]
}

fn level_of(n: I64) -> Option<Level> {
    match n.0 {
        0 => Some(Level::Debug),
        1 => Some(Level::Info),
        2 => Some(Level::Warn),
        3 => Some(Level::Error),
        _ => None,
    }
}

fn traced(c: &mut Ctx) -> Option<(Arc<Trace>, Owner)> {
    let trace = with_host_facilities(c, |facilities| Arc::clone(facilities.tracing()))?;
    Some((trace, (c.id, crate::tasks::running_unseeded_task(c))))
}

fn fields(c: &mut Ctx, native: &str, at: Addr, n: I64) -> Option<Vec<(String, Field)>> {
    handed(
        c,
        native,
        "a record's fields",
        fields_read(unsafe { borrowed(at, n) }),
    )
}

pub unsafe extern "C" fn rt_trace_wants(ctx: *mut Ctx, level: I64) -> I64 {
    asked(
        ctx,
        "trace_wants",
        Nondeterministic,
        I64(0),
        |c: &mut Ctx| {
            let Some(level) = handed(c, "trace_wants", "a level", level_of(level)) else {
                return I64(0);
            };
            let wants = with_host_facilities(c, |facilities| facilities.tracing().wants(level));
            I64(i64::from(wants.unwrap_or(false)))
        },
    )
}

#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn rt_trace_record(
    ctx: *mut Ctx,
    kind: I64,
    channel_addr: Addr,
    channel_len: I64,
    level: I64,
    name_at: Addr,
    name_n: I64,
    amount: I64,
    value_addr: Addr,
    value_len: I64,
    fields_addr: Addr,
    fields_len: I64,
) {
    asked(ctx, "trace_record", Nondeterministic, (), |c: &mut Ctx| {
        let kind = match kind.0 {
            0 => Some(RecordKind::Event),
            1 => Some(RecordKind::Count),
            2 => Some(RecordKind::Gauge),
            3 => Some(RecordKind::Time),
            _ => None,
        };
        let (Some(kind), Some(level)) = (
            handed(c, "trace_record", "a record's kind", kind),
            handed(c, "trace_record", "a level", level_of(level)),
        ) else {
            return;
        };
        let (Some(channel), Some(name), Some(value)) = (
            c.text(unsafe { borrowed(channel_addr, channel_len) }),
            c.text(unsafe { borrowed(name_at, name_n) }),
            c.text(unsafe { borrowed(value_addr, value_len) }),
        ) else {
            return;
        };
        let value = match kind {
            RecordKind::Gauge => {
                let read = Decimal::from_str(value).ok();
                let Some(value) = handed(c, "trace_record", "a gauge's value", read) else {
                    return;
                };
                Some(value)
            }
            _ => None,
        };
        let amount = matches!(kind, RecordKind::Count | RecordKind::Time).then_some(amount.0);
        let Some(fields) = fields(c, "trace_record", fields_addr, fields_len) else {
            return;
        };
        let Some((trace, owner)) = traced(c) else {
            return;
        };
        trace.record(owner, kind, level, channel, name, amount, value, &fields);
    });
}

pub unsafe extern "C" fn rt_trace_enter(
    ctx: *mut Ctx,
    channel_addr: Addr,
    channel_len: I64,
    name_at: Addr,
    name_n: I64,
    fields_addr: Addr,
    fields_len: I64,
) -> I64 {
    asked(
        ctx,
        "trace_enter",
        Nondeterministic,
        I64(0),
        |c: &mut Ctx| {
            let (Some(channel), Some(name)) = (
                c.text(unsafe { borrowed(channel_addr, channel_len) }),
                c.text(unsafe { borrowed(name_at, name_n) }),
            ) else {
                return I64(0);
            };
            let Some(fields) = fields(c, "trace_enter", fields_addr, fields_len) else {
                return I64(0);
            };
            let Some((trace, owner)) = traced(c) else {
                return I64(0);
            };
            I64(trace.enter(owner, channel, name, &fields))
        },
    )
}

pub unsafe extern "C" fn rt_trace_exit(
    ctx: *mut Ctx,
    channel_addr: Addr,
    channel_len: I64,
    id: I64,
    outcome: I64,
    reason_addr: Addr,
    reason_len: I64,
) {
    asked(ctx, "trace_exit", Nondeterministic, (), |c: &mut Ctx| {
        let (Some(channel), Some(reason)) = (
            c.text(unsafe { borrowed(channel_addr, channel_len) }),
            c.text(unsafe { borrowed(reason_addr, reason_len) }),
        ) else {
            return;
        };
        let outcome = match outcome.0 {
            0 => Some(Outcome::Ok),
            1 => Some(Outcome::Failed(reason.to_string())),
            2 => Some(Outcome::Abandoned),
            _ => None,
        };
        let Some(outcome) = handed(c, "trace_exit", "an outcome", outcome) else {
            return;
        };
        let Some((trace, owner)) = traced(c) else {
            return;
        };
        let span = c.site();
        let closed = trace
            .exit(owner, channel, id.0, outcome)
            .map_err(|reason| err_unbalanced(span, "`trace.exit`", id.0, &reason));
        done(c, closed);
    });
}
