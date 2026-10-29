//! `std.process`'s natives: the run's own process, which a run that is not one withholds, and the
//! programs `--exec` binds, each by its label. What waits, on another program, on a person or on a
//! pipe a child drains at its own pace, is a job on the waiting pool. Where the run asking is
//! observed, a child's `File` outputs are entered into its record as writes, and a `ply` the child
//! is reports what it read there.

use super::{Determinism, Nondeterministic, asked, from_host, text_at, texts_at};
use crate::c::{MachineNative, machine_native};
use crate::native::{Addr, I64, TakenBytes};
use crate::rt::{Ctx, lent};
use ply_core::{Diagnostic, codes};
use ply_host::jobs::Work;
use ply_host::native::{int, parts_of};
use ply_host::observe;
use ply_host::process::{Io, Output, ProcessHost, Reach, Signal, Stream, withheld};
use std::path::Path;

pub(super) fn natives() -> Vec<MachineNative> {
    vec![
        machine_native(
            "process_args",
            rt_process_args as unsafe extern "C" fn(_) -> _,
        ),
        machine_native(
            "process_bound",
            rt_process_bound as unsafe extern "C" fn(_, _, _) -> _,
        ),
        machine_native(
            "process_line",
            rt_process_line as unsafe extern "C" fn(_, _, _, _),
        ),
        machine_native(
            "process_write",
            rt_process_write as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
        machine_native(
            "process_flush",
            rt_process_flush as unsafe extern "C" fn(_) -> _,
        ),
        machine_native(
            "process_read_line",
            rt_process_read_line as unsafe extern "C" fn(_) -> _,
        ),
        machine_native(
            "process_read",
            rt_process_read as unsafe extern "C" fn(_, _) -> _,
        ),
        machine_native(
            "process_exit",
            rt_process_exit as unsafe extern "C" fn(_, _),
        ),
        machine_native(
            "process_spawn",
            rt_process_spawn as unsafe extern "C" fn(_, _, _, _, _, _, _, _, _) -> _,
        ),
        machine_native(
            "process_start",
            rt_process_start as unsafe extern "C" fn(_, _, _, _, _, _, _, _, _, _, _) -> _,
        ),
        machine_native(
            "process_wait",
            rt_process_wait as unsafe extern "C" fn(_, _, _, _, _) -> _,
        ),
        machine_native(
            "process_signal",
            rt_process_signal as unsafe extern "C" fn(_, _, _, _, _) -> _,
        ),
        machine_native(
            "process_input",
            rt_process_input as unsafe extern "C" fn(_, _, _, _, _, _) -> _,
        ),
        machine_native(
            "process_end_input",
            rt_process_end_input as unsafe extern "C" fn(_, _, _, _),
        ),
        machine_native(
            "process_output_line",
            rt_process_output_line as unsafe extern "C" fn(_, _, _, _, _) -> _,
        ),
    ]
}

/// What `answer` answers of the run's process host, where the native may ask the host for an
/// answer of `determinism`; `refused`, with the run failed, where the run was lent no process host, or
/// `answer` refuses. `op` and `reach` name what a refusal names.
fn hosted<R: Copy>(
    ctx: *mut Ctx,
    native: &'static str,
    op: &'static str,
    determinism: Determinism,
    reach: Reach,
    refused: R,
    answer: impl FnOnce(&mut Ctx, &ProcessHost) -> Result<R, Diagnostic>,
) -> R {
    asked(ctx, native, determinism, refused, |c| {
        let Some(found) = from_host(c, |facilities| facilities.process().cloned()) else {
            return refused;
        };
        let answered = match found {
            Some(host) => answer(c, &host),
            None => Err(withheld(op, reach, c.site())),
        };
        answered.unwrap_or_else(|d| {
            c.fail(d);
            refused
        })
    })
}

/// The stream `number` numbers: standard output 0 and standard error 1.
fn stream_of(number: I64) -> Stream {
    if number.0 == 0 {
        Stream::Out
    } else {
        Stream::Err
    }
}

/// The environment lent as parts at `p`, each variable's name then its value, and, where the run
/// asking is observed, where a `ply` the child is reports what it read.
fn environment_at(c: &mut Ctx, p: Addr, n: I64) -> Option<Vec<(String, String)>> {
    let texts = texts_at(c, p, n)?;
    if texts.len() % 2 != 0 {
        c.fail(Diagnostic::error(
            codes::INTERNAL_ERROR,
            "an environment was handed with a name and no value",
        ));
        return None;
    }
    let mut env: Vec<(String, String)> = texts
        .chunks(2)
        .map(|pair| (pair[0].clone(), pair[1].clone()))
        .collect();
    if let Some(recorder) = observe::of(&*c.host_runtime()) {
        env.push((
            observe::TRACE_VAR.to_string(),
            recorder.child_trace().to_string_lossy().into_owned(),
        ));
    }
    Some(env)
}

/// The arguments the run was given, as parts.
pub unsafe extern "C" fn rt_process_args(ctx: *mut Ctx) -> TakenBytes {
    let none = TakenBytes(0);
    hosted(
        ctx,
        "process_args",
        "args",
        Nondeterministic,
        Reach::Own,
        none,
        |c, host| {
            let args = host.args(c.site())?;
            let texts: Vec<&[u8]> = args.iter().map(|arg| arg.as_bytes()).collect();
            Ok(c.given(ply_host::native::parts(&texts)))
        },
    )
}

/// 1 where the run bound a program to the label at `p`, else 0.
pub unsafe extern "C" fn rt_process_bound(ctx: *mut Ctx, p: Addr, n: I64) -> I64 {
    hosted(
        ctx,
        "process_bound",
        "bound",
        Nondeterministic,
        Reach::Executable,
        I64(0),
        |c, host| Ok(text_at(c, p, n).map_or(I64(0), |label| I64(i64::from(host.bound(&label))))),
    )
}

/// The text at `p` and a line ending written to the stream `stream` numbers.
pub unsafe extern "C" fn rt_process_line(ctx: *mut Ctx, stream: I64, p: Addr, n: I64) {
    let op = match stream_of(stream) {
        Stream::Out => "out",
        Stream::Err => "err",
    };
    hosted(
        ctx,
        "process_line",
        op,
        Nondeterministic,
        Reach::Own,
        (),
        |c, host| match text_at(c, p, n) {
            Some(text) => host.line(stream_of(stream), &text, c.site()),
            None => Ok(()),
        },
    )
}

/// 1 where the bytes at `p` reached the stream `stream` numbers, 0 once its reader has gone.
pub unsafe extern "C" fn rt_process_write(ctx: *mut Ctx, stream: I64, p: Addr, n: I64) -> I64 {
    let op = match stream_of(stream) {
        Stream::Out => "out_bytes",
        Stream::Err => "err_bytes",
    };
    hosted(
        ctx,
        "process_write",
        op,
        Nondeterministic,
        Reach::Own,
        I64(0),
        |c, host| {
            let bytes = unsafe { lent(p, n) };
            Ok(I64(i64::from(host.write(
                stream_of(stream),
                bytes,
                c.site(),
            )?)))
        },
    )
}

/// 1 where what standard output's buffer held reached it, 0 once its reader has gone.
pub unsafe extern "C" fn rt_process_flush(ctx: *mut Ctx) -> I64 {
    hosted(
        ctx,
        "process_flush",
        "flush",
        Nondeterministic,
        Reach::Own,
        I64(0),
        |c, host| Ok(I64(i64::from(host.flush(c.site())?))),
    )
}

/// The ticket of one line of standard input without its ending, found.
pub unsafe extern "C" fn rt_process_read_line(ctx: *mut Ctx) -> I64 {
    hosted(
        ctx,
        "process_read_line",
        "line",
        Nondeterministic,
        Reach::Own,
        I64(0),
        |c, host| {
            let job = host.read_line(c.site())?;
            Ok(c.submitted("process_read_line", "`process.line`", Work::Waiting, job))
        },
    )
}

/// The ticket of what comes next on standard input, at most `max` bytes.
pub unsafe extern "C" fn rt_process_read(ctx: *mut Ctx, max: I64) -> I64 {
    hosted(
        ctx,
        "process_read",
        "in_bytes",
        Nondeterministic,
        Reach::Own,
        I64(0),
        |c, host| {
            let Some(max) =
                super::handed(c, "process_read", "a length", usize::try_from(max.0).ok())
            else {
                return Ok(I64(0));
            };
            let job = host.read(max, c.site())?;
            Ok(c.submitted("process_read", "`process.in_bytes`", Work::Waiting, job))
        },
    )
}

/// The run ended with `code`, which it exits with once its host has torn down.
pub unsafe extern "C" fn rt_process_exit(ctx: *mut Ctx, code: I64) {
    hosted(
        ctx,
        "process_exit",
        "exit",
        Nondeterministic,
        Reach::Own,
        (),
        |c, host| {
            let Some(code) = super::handed(
                c,
                "process_exit",
                "an exit code",
                i32::try_from(code.0).ok(),
            ) else {
                return Ok(());
            };
            Err(host.exit(code, c.site()))
        },
    )
}

/// The ticket of the program the label at `lp` names run to its end with the arguments at `ap`,
/// in the directory at `dp` under the environment at `ep`: how it ended and both its streams.
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn rt_process_spawn(
    ctx: *mut Ctx,
    lp: Addr,
    ln: I64,
    ap: Addr,
    an: I64,
    dp: Addr,
    dn: I64,
    ep: Addr,
    en: I64,
) -> I64 {
    hosted(
        ctx,
        "process_spawn",
        "spawn",
        Nondeterministic,
        Reach::Executable,
        I64(0),
        |c, host| {
            let (Some(label), Some(args), Some(dir), Some(env)) = (
                text_at(c, lp, ln),
                texts_at(c, ap, an),
                text_at(c, dp, dn),
                environment_at(c, ep, en),
            ) else {
                return Ok(I64(0));
            };
            let job = host.spawn(&label, &args, &dir, &env, c.site())?;
            Ok(c.refusing("process_spawn", "`process.spawn`", Work::Waiting, job))
        },
    )
}

/// How a child is wired, lent as parts: three bytes, 1 where it reads input and each output
/// stream's number (`Keep` 0, `Discard` 1, `Inherit` 2, `Lines` 3 and `File` 4), then the path
/// of each stream that is a file.
fn wiring_of(wiring: &[u8]) -> Option<Io> {
    let parts = parts_of(wiring)?;
    let [kinds, out_file, err_file] = parts.as_slice() else {
        return None;
    };
    let &[input, out, err] = *kinds else {
        return None;
    };
    let file = |path: &[u8]| String::from_utf8(path.to_vec()).ok();
    Some(Io {
        input: input != 0,
        out: Output::of(i64::from(out), &file(out_file)?)?,
        err: Output::of(i64::from(err), &file(err_file)?)?,
    })
}

/// A child of the program the label at `lp` names, started beside the program as
/// [`rt_process_spawn`] reads its arguments, and wired as the parts at `wp` say: byte 1 and its
/// handle, or byte 0 and why it could not start.
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn rt_process_start(
    ctx: *mut Ctx,
    lp: Addr,
    ln: I64,
    ap: Addr,
    an: I64,
    dp: Addr,
    dn: I64,
    ep: Addr,
    en: I64,
    wp: Addr,
    wn: I64,
) -> TakenBytes {
    hosted(
        ctx,
        "process_start",
        "start",
        Nondeterministic,
        Reach::Executable,
        TakenBytes(0),
        |c, host| {
            let (Some(label), Some(args), Some(dir), Some(env)) = (
                text_at(c, lp, ln),
                texts_at(c, ap, an),
                text_at(c, dp, dn),
                environment_at(c, ep, en),
            ) else {
                return Ok(TakenBytes(0));
            };
            let wiring = wiring_of(unsafe { lent(wp, wn) });
            let Some(io) = super::handed(c, "process_start", "a wiring", wiring) else {
                return Ok(TakenBytes(0));
            };
            let said = match host.start(&label, &args, &dir, &env, &io, c.site())? {
                Ok(handle) => [&[1u8][..], &int(handle)].concat(),
                Err(why) => [&[0u8][..], why.as_bytes()].concat(),
            };
            if let Some(recorder) = observe::of(&*c.host_runtime()) {
                for output in [&io.out, &io.err] {
                    if let Output::File(path) = output
                        && let Ok(at) = Path::new(&dir).join(path).canonicalize()
                    {
                        recorder.wrote(&at);
                    }
                }
            }
            Ok(c.given(said))
        },
    )
}

/// The ticket of how the child `child` the label at `lp` started ended, found once it has: none
/// while it runs past `timeout_ms`.
pub unsafe extern "C" fn rt_process_wait(
    ctx: *mut Ctx,
    lp: Addr,
    ln: I64,
    child: I64,
    timeout_ms: I64,
) -> I64 {
    hosted(
        ctx,
        "process_wait",
        "wait",
        Nondeterministic,
        Reach::Executable,
        I64(0),
        |c, host| {
            let Some(label) = text_at(c, lp, ln) else {
                return Ok(I64(0));
            };
            let job = host.wait(&label, child.0, timeout_ms.0, c.site())?;
            Ok(c.refusing("process_wait", "`process.wait`", Work::Waiting, job))
        },
    )
}

/// 1 where the signal `signal` numbers reached the child, 0 where it had already ended.
pub unsafe extern "C" fn rt_process_signal(
    ctx: *mut Ctx,
    lp: Addr,
    ln: I64,
    child: I64,
    signal: I64,
) -> I64 {
    hosted(
        ctx,
        "process_signal",
        "signal",
        Nondeterministic,
        Reach::Executable,
        I64(0),
        |c, host| {
            let (Some(label), Some(signal)) = (
                text_at(c, lp, ln),
                super::handed(c, "process_signal", "a signal", Signal::of(signal.0)),
            ) else {
                return Ok(I64(0));
            };
            Ok(I64(i64::from(host.signal(
                &label,
                child.0,
                signal,
                c.site(),
            )?)))
        },
    )
}

/// The ticket of the bytes at `p` written to the child's input: byte 1 where every byte was.
pub unsafe extern "C" fn rt_process_input(
    ctx: *mut Ctx,
    lp: Addr,
    ln: I64,
    child: I64,
    p: Addr,
    n: I64,
) -> I64 {
    hosted(
        ctx,
        "process_input",
        "input",
        Nondeterministic,
        Reach::Executable,
        I64(0),
        |c, host| {
            let Some(label) = text_at(c, lp, ln) else {
                return Ok(I64(0));
            };
            let bytes = unsafe { lent(p, n) }.to_vec();
            let job = host.input(&label, child.0, bytes, c.site())?;
            Ok(c.submitted("process_input", "`process.input`", Work::Waiting, job))
        },
    )
}

/// The child's input closed.
pub unsafe extern "C" fn rt_process_end_input(ctx: *mut Ctx, lp: Addr, ln: I64, child: I64) {
    hosted(
        ctx,
        "process_end_input",
        "end_input",
        Nondeterministic,
        Reach::Executable,
        (),
        |c, host| match text_at(c, lp, ln) {
            Some(label) => host.end_input(&label, child.0, c.site()),
            None => Ok(()),
        },
    )
}

/// The ticket of the next line any of the child's `Lines` streams wrote: byte 0 and the line,
/// byte 1 for nothing before `timeout_ms`, or byte 2 for the end of every one.
pub unsafe extern "C" fn rt_process_output_line(
    ctx: *mut Ctx,
    lp: Addr,
    ln: I64,
    child: I64,
    timeout_ms: I64,
) -> I64 {
    hosted(
        ctx,
        "process_output_line",
        "output_line",
        Nondeterministic,
        Reach::Executable,
        I64(0),
        |c, host| {
            let Some(label) = text_at(c, lp, ln) else {
                return Ok(I64(0));
            };
            let job = host.output_line(&label, child.0, timeout_ms.0, c.site())?;
            Ok(c.refusing(
                "process_output_line",
                "`process.output_line`",
                Work::Waiting,
                job,
            ))
        },
    )
}
