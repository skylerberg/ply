//! `std.term`'s natives: whether the process's streams are a terminal, the terminal's size and
//! input mode, and what is typed at it, read from the buffer `process.line` reads.
//!
//! A line read for a `Secret` is marshalled alone: the job that reads it keeps it with the run
//! that asked, under a number, and returns the number found, and `term_secret_taken` makes the
//! secret of what the run keeps.

use super::{Nondeterministic, asked, from_host, handed, waited};
use crate::c::{MachineNative, machine_native};
use crate::emitted::SecretHandle;
use crate::native::{I64, TakenBytes};
use crate::rt::Ctx;
use crate::secret;
use ply_core::{Diagnostic, Span, codes};
use ply_host::jobs::Job;
use ply_host::native::{found, int};
use ply_host::stdio::{self, Mode, Standard};
use std::sync::Arc;
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

pub(super) fn natives() -> Vec<MachineNative> {
    vec![
        machine_native(
            "term_is_terminal",
            rt_term_is_terminal as unsafe extern "C" fn(_, _) -> _,
        ),
        machine_native("term_size", rt_term_size as unsafe extern "C" fn(_) -> _),
        machine_native("term_mode", rt_term_mode as unsafe extern "C" fn(_, _) -> _),
        machine_native(
            "term_input",
            rt_term_input as unsafe extern "C" fn(_, _, _) -> _,
        ),
        machine_native(
            "term_secret_line",
            rt_term_secret_line as unsafe extern "C" fn(_) -> _,
        ),
        machine_native(
            "term_secret_taken",
            rt_term_secret_taken as unsafe extern "C" fn(_, _) -> _,
        ),
    ]
}

/// The modes as the host handler numbers them: `Cooked` 0, `Silent` 1 and `Raw` 2.
fn mode_of(n: i64) -> Option<Mode> {
    match n {
        0 => Some(Mode::Cooked),
        1 => Some(Mode::Silent),
        2 => Some(Mode::Raw),
        _ => None,
    }
}

fn mode_number(mode: Mode) -> i64 {
    match mode {
        Mode::Cooked => 0,
        Mode::Silent => 1,
        Mode::Raw => 2,
    }
}

/// 1 where the stream numbered `stream` (standard input 0, output 1, error 2) is a terminal.
pub unsafe extern "C" fn rt_term_is_terminal(ctx: *mut Ctx, stream: I64) -> I64 {
    asked(
        ctx,
        "term_is_terminal",
        Nondeterministic,
        I64(0),
        |c: &mut Ctx| {
            let stream = match stream.0 {
                0 => Some(Standard::In),
                1 => Some(Standard::Out),
                2 => Some(Standard::Err),
                _ => None,
            };
            match handed(c, "term_is_terminal", "a stream", stream) {
                Some(stream) => I64(i64::from(stdio::is_terminal(stream))),
                None => I64(0),
            }
        },
    )
}

/// The terminal's columns and rows, each in eight bytes, found.
pub unsafe extern "C" fn rt_term_size(ctx: *mut Ctx) -> TakenBytes {
    asked(
        ctx,
        "term_size",
        Nondeterministic,
        TakenBytes(0),
        |c: &mut Ctx| {
            let size = stdio::terminal_size()
                .map(|(columns, rows)| [int(i64::from(columns)), int(i64::from(rows))].concat());
            c.given(found(size))
        },
    )
}

/// The terminal standard input is put in the mode numbered `mode`, returning the mode it was in,
/// or -1, nothing changed, where standard input is no terminal.
pub unsafe extern "C" fn rt_term_mode(ctx: *mut Ctx, mode: I64) -> I64 {
    asked(
        ctx,
        "term_mode",
        Nondeterministic,
        I64(-1),
        |c: &mut Ctx| {
            let Some(mode) = handed(c, "term_mode", "a mode", mode_of(mode.0)) else {
                return I64(-1);
            };
            match stdio::set_mode(mode) {
                Ok(was) => I64(was.map_or(-1, mode_number)),
                Err(e) => {
                    let span = c.site();
                    c.fail(unset(mode, &e, span));
                    I64(-1)
                }
            }
        },
    )
}

/// The ticket of what has been typed, at most `max` bytes, found; none where nothing came within
/// `timeout_ms`, which waits as long as it takes where it is negative.
pub unsafe extern "C" fn rt_term_input(ctx: *mut Ctx, max: I64, timeout_ms: I64) -> I64 {
    asked(
        ctx,
        "term_input",
        Nondeterministic,
        I64(0),
        |c: &mut Ctx| {
            let Some(max) = handed(c, "term_input", "a length", usize::try_from(max.0).ok()) else {
                return I64(0);
            };
            let deadline = u64::try_from(timeout_ms.0)
                .ok()
                .and_then(|ms| Instant::now().checked_add(Duration::from_millis(ms)));
            let job: Job = Box::new(move || match stdio::read_within(max, deadline) {
                Ok(bytes) => Ok(found(bytes)),
                Err(e) => Err(format!("the terminal could not be read: {e}")),
            });
            waited(c, "term_input", "`term.input`", Ok(job))
        },
    )
}

/// The ticket of one line of standard input kept for a `Secret` by the run that asked: the number
/// it is kept under, found, or none at the end of input. A line read after its run ended is wiped
/// as it is dropped.
pub unsafe extern "C" fn rt_term_secret_line(ctx: *mut Ctx) -> I64 {
    asked(
        ctx,
        "term_secret_line",
        Nondeterministic,
        I64(0),
        |c: &mut Ctx| {
            let Some(run) = from_host(c, |facilities| Arc::downgrade(facilities.run())) else {
                return I64(0);
            };
            let job: Job = Box::new(move || match stdio::read_secret_line() {
                Ok(Some(mut line)) => {
                    let kept = ply_host::pool::Kept::new(std::mem::take(&mut *line).into_bytes());
                    Ok(found(run.upgrade().map(|run| int(run.keep_secret(kept)))))
                }
                Ok(None) => Ok(found(None::<[u8; 8]>)),
                Err(e) => Err(format!("standard input could not be read: {e}")),
            });
            waited(c, "term_secret_line", "`term.secret_line`", Ok(job))
        },
    )
}

/// The line the run keeps under `number`, as a secret that only a `Secret`'s own readers read.
pub unsafe extern "C" fn rt_term_secret_taken(ctx: *mut Ctx, number: I64) -> SecretHandle {
    asked(
        ctx,
        "term_secret_taken",
        Nondeterministic,
        SecretHandle::of(0),
        |c: &mut Ctx| {
            let line = from_host(c, |facilities| facilities.run().take_secret(number.0)).flatten();
            match handed(c, "term_secret_taken", "a line's number", line) {
                Some(line) if std::str::from_utf8(&line).is_ok() => secret::sealed(c, true, line),
                Some(_) => secret::sealed(c, true, Zeroizing::new(Vec::new())),
                None => SecretHandle::of(0),
            }
        },
    )
}

#[cold]
fn unset(mode: Mode, e: &std::io::Error, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("`term.mode` could not put the terminal in `{mode:?}`: {e}"),
    )
    .primary(span, "the terminal rejected its settings")
}
