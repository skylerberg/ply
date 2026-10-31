//! `std.trace`'s natives: records written to the run's sink, and the spans the run keeps open, each
//! the running task's of this machine. A record's fields cross flat as `ply_host::trace` reads them,
//! and only where the sink wants the record.

use super::{Nondeterministic, asked, done, from_host, handed};
use crate::c::{MachineNative, machine_native};
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
        machine_native(
            "trace_wants",
            rt_trace_wants as unsafe extern "C" fn(_, _) -> _,
        ),
        machine_native(
            "trace_record",
            rt_trace_record as unsafe extern "C" fn(_, _, _, _, _, _, _, _, _, _, _, _),
        ),
        machine_native(
            "trace_enter",
            rt_trace_enter as unsafe extern "C" fn(_, _, _, _, _, _, _) -> _,
        ),
        machine_native(
            "trace_exit",
            rt_trace_exit as unsafe extern "C" fn(_, _, _, _, _, _, _),
        ),
    ]
}

/// The levels as the host handler numbers them: `Debug` 0, `Info` 1, `Warn` 2 and `Error` 3.
fn level_of(n: I64) -> Option<Level> {
    match n.0 {
        0 => Some(Level::Debug),
        1 => Some(Level::Info),
        2 => Some(Level::Warn),
        3 => Some(Level::Error),
        _ => None,
    }
}

/// The run's trace, and who performs on this machine: the running task of a production scheduler,
/// or the machine itself outside one.
fn traced(c: &mut Ctx) -> Option<(Arc<Trace>, Owner)> {
    let trace = from_host(c, |facilities| Arc::clone(facilities.tracing()))?;
    Some((trace, (c.id, crate::tasks::production_task(c))))
}

/// The fields at `at`, as the host handler writes them.
fn fields(c: &mut Ctx, native: &str, at: Addr, n: I64) -> Option<Vec<(String, Field)>> {
    handed(
        c,
        native,
        "a record's fields",
        fields_read(unsafe { borrowed(at, n) }),
    )
}

/// 1 where the run's sink writes a record at the level numbered `level`.
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
            let wants = from_host(c, |facilities| facilities.tracing().wants(level));
            I64(i64::from(wants.unwrap_or(false)))
        },
    )
}

/// One record of the kind numbered `kind` (`event` 0, `count` 1, `gauge` 2, `time` 3) on the
/// channel at `channel_at`: a count's delta or a time's microseconds in `amount`, and a gauge's
/// value as the decimal text at `value_at`.
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn rt_trace_record(
    ctx: *mut Ctx,
    kind: I64,
    channel_at: Addr,
    channel_n: I64,
    level: I64,
    name_at: Addr,
    name_n: I64,
    amount: I64,
    value_at: Addr,
    value_n: I64,
    fields_at: Addr,
    fields_n: I64,
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
            c.text(unsafe { borrowed(channel_at, channel_n) }),
            c.text(unsafe { borrowed(name_at, name_n) }),
            c.text(unsafe { borrowed(value_at, value_n) }),
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
        let Some(fields) = fields(c, "trace_record", fields_at, fields_n) else {
            return;
        };
        let Some((trace, owner)) = traced(c) else {
            return;
        };
        trace.record(owner, kind, level, channel, name, amount, value, &fields);
    });
}

/// Opens a span on the channel at `channel_at`, returning its id.
pub unsafe extern "C" fn rt_trace_enter(
    ctx: *mut Ctx,
    channel_at: Addr,
    channel_n: I64,
    name_at: Addr,
    name_n: I64,
    fields_at: Addr,
    fields_n: I64,
) -> I64 {
    asked(
        ctx,
        "trace_enter",
        Nondeterministic,
        I64(0),
        |c: &mut Ctx| {
            let (Some(channel), Some(name)) = (
                c.text(unsafe { borrowed(channel_at, channel_n) }),
                c.text(unsafe { borrowed(name_at, name_n) }),
            ) else {
                return I64(0);
            };
            let Some(fields) = fields(c, "trace_enter", fields_at, fields_n) else {
                return I64(0);
            };
            let Some((trace, owner)) = traced(c) else {
                return I64(0);
            };
            I64(trace.enter(owner, channel, name, &fields))
        },
    )
}

/// Closes span `id` on the channel at `channel_at` with the outcome numbered `outcome` (`Ok` 0,
/// `Failed` 1 with the reason at `reason_at`, `Abandoned` 2), and every span opened inside it.
/// One that is not open fails the run.
pub unsafe extern "C" fn rt_trace_exit(
    ctx: *mut Ctx,
    channel_at: Addr,
    channel_n: I64,
    id: I64,
    outcome: I64,
    reason_at: Addr,
    reason_n: I64,
) {
    asked(ctx, "trace_exit", Nondeterministic, (), |c: &mut Ctx| {
        let (Some(channel), Some(reason)) = (
            c.text(unsafe { borrowed(channel_at, channel_n) }),
            c.text(unsafe { borrowed(reason_at, reason_n) }),
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
