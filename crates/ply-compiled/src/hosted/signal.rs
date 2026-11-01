use super::{Nondeterministic, asked, handed, with_host_facilities};
use crate::c::{MachineNative, runtime_native};
use crate::native::I64;
use crate::rt::Ctx;
use ply_host::signal::{Asked, err_signal_watch_failed};

pub(super) fn natives() -> Vec<MachineNative> {
    vec![
        runtime_native(
            "signal_stopping",
            rt_signal_stopping as unsafe extern "C" fn(_) -> _,
        ),
        runtime_native(
            "signal_deadline_ms",
            rt_signal_deadline_ms as unsafe extern "C" fn(_) -> _,
        ),
        runtime_native(
            "signal_arrived",
            rt_signal_arrived as unsafe extern "C" fn(_, _) -> _,
        ),
    ]
}

pub unsafe extern "C" fn rt_signal_stopping(ctx: *mut Ctx) -> I64 {
    asked(
        ctx,
        "signal_stopping",
        Nondeterministic,
        I64(0),
        |c: &mut Ctx| {
            let stopping = with_host_facilities(c, |facilities| {
                facilities.stop().is_some_and(|stop| stop.stopping())
            });
            I64(i64::from(stopping.unwrap_or(false)))
        },
    )
}

pub unsafe extern "C" fn rt_signal_deadline_ms(ctx: *mut Ctx) -> I64 {
    asked(
        ctx,
        "signal_deadline_ms",
        Nondeterministic,
        I64(-1),
        |c: &mut Ctx| {
            let left = with_host_facilities(c, |facilities| {
                facilities.stop().map_or(-1, |stop| stop.deadline_ms())
            });
            I64(left.unwrap_or(-1))
        },
    )
}

pub unsafe extern "C" fn rt_signal_arrived(ctx: *mut Ctx, which: I64) -> I64 {
    asked(
        ctx,
        "signal_arrived",
        Nondeterministic,
        I64(0),
        |c: &mut Ctx| {
            let (asked, what) = match which.0 {
                0 => (Some(Asked::Hangup), "`signal.hangup`"),
                1 => (Some(Asked::User1), "`signal.user1`"),
                2 => (Some(Asked::User2), "`signal.user2`"),
                3 => (Some(Asked::WindowChange), "`signal.resized`"),
                _ => (None, ""),
            };
            let Some(asked) = handed(c, "signal_arrived", "a signal", asked) else {
                return I64(0);
            };
            let heard = with_host_facilities(c, |facilities| {
                facilities.stop().map(|stop| stop.arrived(asked))
            });
            match heard.flatten() {
                None | Some(Ok(false)) => I64(0),
                Some(Ok(true)) => I64(1),
                Some(Err(e)) => {
                    let span = c.site();
                    c.fail(err_signal_watch_failed(what, asked, &e, span));
                    I64(0)
                }
            }
        },
    )
}
