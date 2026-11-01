use super::{Determinism, Nondeterministic, if_host_allowed, text_at, texts_at, with_host_facilities};
use crate::c::{MachineNative, runtime_native};
use crate::native::{Addr, I64, TakenBytes};
use crate::rt::{Ctx, borrowed};
use ply_core::{Diagnostic, codes};
use ply_host::jobs::Work;
use ply_host::native::{int, parts_of};
use ply_host::observe;
use ply_host::process::{Io, OperationTarget, Output, ProcessHost, Signal, Stream, withheld};
use std::path::Path;

pub(super) fn natives() -> Vec<MachineNative> {
    vec![
        runtime_native(
            "process_args",
            rt_process_args as unsafe extern "C" fn(_) -> _,
        ),
        runtime_native(
            "process_bound",
            rt_process_bound as unsafe extern "C" fn(_, _, _) -> _,
        ),
        runtime_native(
            "process_line",
            rt_process_line as unsafe extern "C" fn(_, _, _, _),
        ),
        runtime_native(
            "process_write",
            rt_process_write as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
        runtime_native(
            "process_flush",
            rt_process_flush as unsafe extern "C" fn(_) -> _,
        ),
        runtime_native(
            "process_read_line",
            rt_process_read_line as unsafe extern "C" fn(_) -> _,
        ),
        runtime_native(
            "process_read",
            rt_process_read as unsafe extern "C" fn(_, _) -> _,
        ),
        runtime_native(
            "process_exit",
            rt_process_exit as unsafe extern "C" fn(_, _),
        ),
        runtime_native(
            "process_spawn",
            rt_process_spawn as unsafe extern "C" fn(_, _, _, _, _, _, _, _, _) -> _,
        ),
        runtime_native(
            "process_start",
            rt_process_start as unsafe extern "C" fn(_, _, _, _, _, _, _, _, _, _, _) -> _,
        ),
        runtime_native(
            "process_wait",
            rt_process_wait as unsafe extern "C" fn(_, _, _, _, _) -> _,
        ),
        runtime_native(
            "process_signal",
            rt_process_signal as unsafe extern "C" fn(_, _, _, _, _) -> _,
        ),
        runtime_native(
            "process_input",
            rt_process_input as unsafe extern "C" fn(_, _, _, _, _, _) -> _,
        ),
        runtime_native(
            "process_end_input",
            rt_process_end_input as unsafe extern "C" fn(_, _, _, _),
        ),
        runtime_native(
            "process_output_line",
            rt_process_output_line as unsafe extern "C" fn(_, _, _, _, _) -> _,
        ),
    ]
}

fn hosted<R: Copy>(
    ctx: *mut Ctx,
    native: &'static str,
    op: &'static str,
    determinism: Determinism,
    reach: OperationTarget,
    rejected: R,
    result: impl FnOnce(&mut Ctx, &ProcessHost) -> Result<R, Diagnostic>,
) -> R {
    if_host_allowed(ctx, native, determinism, rejected, |c| {
        let Some(found) = with_host_facilities(c, |facilities| facilities.process().cloned())
        else {
            return rejected;
        };
        let returned = match found {
            Some(host) => result(c, &host),
            None => Err(withheld(op, reach, c.site())),
        };
        returned.unwrap_or_else(|d| {
            c.fail(d);
            rejected
        })
    })
}

fn stream_of(number: I64) -> Stream {
    if number.0 == 0 {
        Stream::Out
    } else {
        Stream::Err
    }
}

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

pub unsafe extern "C" fn rt_process_args(ctx: *mut Ctx) -> TakenBytes {
    let none = TakenBytes(0);
    hosted(
        ctx,
        "process_args",
        "args",
        Nondeterministic,
        OperationTarget::OwnProcess,
        none,
        |c, host| {
            let args = host.args(c.site())?;
            let texts: Vec<&[u8]> = args.iter().map(|arg| arg.as_bytes()).collect();
            Ok(c.given(ply_host::native::parts(&texts)))
        },
    )
}

pub unsafe extern "C" fn rt_process_bound(ctx: *mut Ctx, p: Addr, n: I64) -> I64 {
    hosted(
        ctx,
        "process_bound",
        "bound",
        Nondeterministic,
        OperationTarget::Executable,
        I64(0),
        |c, host| Ok(text_at(c, p, n).map_or(I64(0), |label| I64(i64::from(host.bound(&label))))),
    )
}

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
        OperationTarget::OwnProcess,
        (),
        |c, host| match text_at(c, p, n) {
            Some(text) => host.line(stream_of(stream), &text, c.site()),
            None => Ok(()),
        },
    )
}

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
        OperationTarget::OwnProcess,
        I64(0),
        |c, host| {
            let bytes = unsafe { borrowed(p, n) };
            Ok(I64(i64::from(host.write(
                stream_of(stream),
                bytes,
                c.site(),
            )?)))
        },
    )
}

pub unsafe extern "C" fn rt_process_flush(ctx: *mut Ctx) -> I64 {
    hosted(
        ctx,
        "process_flush",
        "flush",
        Nondeterministic,
        OperationTarget::OwnProcess,
        I64(0),
        |c, host| Ok(I64(i64::from(host.flush(c.site())?))),
    )
}

pub unsafe extern "C" fn rt_process_read_line(ctx: *mut Ctx) -> I64 {
    hosted(
        ctx,
        "process_read_line",
        "line",
        Nondeterministic,
        OperationTarget::OwnProcess,
        I64(0),
        |c, host| {
            let job = host.read_line(c.site())?;
            Ok(c.submitted("process_read_line", "`process.line`", Work::Waiting, job))
        },
    )
}

pub unsafe extern "C" fn rt_process_read(ctx: *mut Ctx, max: I64) -> I64 {
    hosted(
        ctx,
        "process_read",
        "in_bytes",
        Nondeterministic,
        OperationTarget::OwnProcess,
        I64(0),
        |c, host| {
            let Some(max) =
                super::required_argument(c, "process_read", "a length", usize::try_from(max.0).ok())
            else {
                return Ok(I64(0));
            };
            let job = host.read(max, c.site())?;
            Ok(c.submitted("process_read", "`process.in_bytes`", Work::Waiting, job))
        },
    )
}

pub unsafe extern "C" fn rt_process_exit(ctx: *mut Ctx, code: I64) {
    hosted(
        ctx,
        "process_exit",
        "exit",
        Nondeterministic,
        OperationTarget::OwnProcess,
        (),
        |c, host| {
            let Some(code) = super::required_argument(
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
        OperationTarget::Executable,
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
            Ok(c.queued_fallible("process_spawn", "`process.spawn`", Work::Waiting, job))
        },
    )
}

fn parse_stdio(wiring: &[u8]) -> Option<Io> {
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
        OperationTarget::Executable,
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
            let wiring = parse_stdio(unsafe { borrowed(wp, wn) });
            let Some(io) = super::required_argument(c, "process_start", "a wiring", wiring) else {
                return Ok(TakenBytes(0));
            };
            let said = match host.start(&label, &args, &dir, &env, &io, c.site())? {
                Ok(handle) => [&[1u8][..], &int(handle)].concat(),
                Err(reason) => [&[0u8][..], reason.as_bytes()].concat(),
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
        OperationTarget::Executable,
        I64(0),
        |c, host| {
            let Some(label) = text_at(c, lp, ln) else {
                return Ok(I64(0));
            };
            let job = host.wait(&label, child.0, timeout_ms.0, c.site())?;
            Ok(c.queued_fallible("process_wait", "`process.wait`", Work::Waiting, job))
        },
    )
}

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
        OperationTarget::Executable,
        I64(0),
        |c, host| {
            let (Some(label), Some(signal)) = (
                text_at(c, lp, ln),
                super::required_argument(c, "process_signal", "a signal", Signal::of(signal.0)),
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
        OperationTarget::Executable,
        I64(0),
        |c, host| {
            let Some(label) = text_at(c, lp, ln) else {
                return Ok(I64(0));
            };
            let bytes = unsafe { borrowed(p, n) }.to_vec();
            let job = host.input(&label, child.0, bytes, c.site())?;
            Ok(c.submitted("process_input", "`process.input`", Work::Waiting, job))
        },
    )
}

pub unsafe extern "C" fn rt_process_end_input(ctx: *mut Ctx, lp: Addr, ln: I64, child: I64) {
    hosted(
        ctx,
        "process_end_input",
        "end_input",
        Nondeterministic,
        OperationTarget::Executable,
        (),
        |c, host| match text_at(c, lp, ln) {
            Some(label) => host.end_input(&label, child.0, c.site()),
            None => Ok(()),
        },
    )
}

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
        OperationTarget::Executable,
        I64(0),
        |c, host| {
            let Some(label) = text_at(c, lp, ln) else {
                return Ok(I64(0));
            };
            let job = host.output_line(&label, child.0, timeout_ms.0, c.site())?;
            Ok(c.queued_fallible(
                "process_output_line",
                "`process.output_line`",
                Work::Waiting,
                job,
            ))
        },
    )
}
