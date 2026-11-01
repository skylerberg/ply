//! `compiler.cc`'s natives, the family `toolchain` that `--allow toolchain` grants: the C
//! compilers by label, each run a job of the run's in line for a core; the toolchain's cache,
//! read and written below it and nowhere else, and the programs linked there, each run a job too;
//! and the asking program's own heap. What to compile, with what flags and what to keep is the
//! program's: this side only runs what it is asked to and confines where it writes. A compiler's
//! output is a function of what it is given, so compiling and the cache are asked as computing.

use super::{Determinism, Deterministic, Nondeterministic, granted, text_at, texts_at};
use crate::c::{MachineNative, machine_native};
use crate::native::{Addr, I64, TakenBytes};
use crate::rt::{Ctx, borrowed};
use ply_core::{Diagnostic, codes};
use ply_host::jobs::Work;
use ply_host::native::{found, int, parts};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

pub(super) fn natives() -> Vec<MachineNative> {
    vec![
        machine_native(
            "compilers_found",
            rt_compilers_found as unsafe extern "C" fn(_, _, _) -> _,
        ),
        machine_native(
            "compilers_started",
            rt_compilers_started as unsafe extern "C" fn(_, _, _, _, _) -> _,
        ),
        machine_native(
            "toolchain_cache",
            rt_toolchain_cache as unsafe extern "C" fn(_) -> _,
        ),
        machine_native(
            "toolchain_reused",
            rt_toolchain_reused as unsafe extern "C" fn(_) -> _,
        ),
        machine_native(
            "toolchain_bodies",
            rt_toolchain_bodies as unsafe extern "C" fn(_) -> _,
        ),
        machine_native(
            "toolchain_holds",
            rt_toolchain_holds as unsafe extern "C" fn(_, _, _) -> _,
        ),
        machine_native(
            "toolchain_held",
            rt_toolchain_held as unsafe extern "C" fn(_, _, _) -> _,
        ),
        machine_native(
            "toolchain_written",
            rt_toolchain_written as unsafe extern "C" fn(_, _, _, _, _) -> _,
        ),
        machine_native(
            "toolchain_moved",
            rt_toolchain_moved as unsafe extern "C" fn(_, _, _, _, _) -> _,
        ),
        machine_native(
            "toolchain_appended",
            rt_toolchain_appended as unsafe extern "C" fn(_, _, _, _, _) -> _,
        ),
        machine_native(
            "toolchain_scratch",
            rt_toolchain_scratch as unsafe extern "C" fn(_) -> _,
        ),
        machine_native(
            "toolchain_cleared",
            rt_toolchain_cleared as unsafe extern "C" fn(_, _, _),
        ),
        machine_native(
            "toolchain_libraries",
            rt_toolchain_libraries as unsafe extern "C" fn(_) -> _,
        ),
        machine_native(
            "toolchain_ran",
            rt_toolchain_ran as unsafe extern "C" fn(_, _, _, _, _) -> _,
        ),
        machine_native(
            "memory_heap",
            rt_memory_heap as unsafe extern "C" fn(_) -> _,
        ),
        machine_native(
            "memory_census",
            rt_memory_census as unsafe extern "C" fn(_) -> _,
        ),
    ]
}

fn borrowed_toolchain<R>(
    ctx: *mut Ctx,
    native: &'static str,
    determinism: Determinism,
    rejected: R,
    result: impl FnOnce(&mut Ctx) -> R,
) -> R {
    granted(ctx, "toolchain", native, determinism, rejected, result)
}

/// Where the compiler `label` names is: `cc` as `PLY_CC` names it, a path or a name on `PATH`, and
/// any other label on `PATH` by its own name.
pub fn compiler_path(label: &str) -> Option<PathBuf> {
    let name = match (label, std::env::var("PLY_CC")) {
        ("cc", Ok(cc)) => cc,
        _ => label.to_string(),
    };
    if name.contains('/') {
        let path = PathBuf::from(name);
        return path.is_file().then_some(path);
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(&name))
        .find(|program| program.is_file())
}

/// What a compiler needs on every call that others do not. tcc finds `libtcc1.a`, which every
/// object it makes needs, relative to `-B`, and a tcc built from source and not installed has no
/// default that finds it: the compile succeeds and the object will not load.
pub fn support_flags(resolved: &Path) -> Vec<String> {
    let is_tcc = resolved
        .file_name()
        .is_some_and(|name| name.to_string_lossy().ends_with("tcc"));
    let Some(dir) = resolved.parent().filter(|_| is_tcc) else {
        return Vec::new();
    };
    // Beside the binary is where a source build leaves it; `../lib/tcc` is where `make install`
    // puts it. An installed tcc finds its own and needs neither.
    [dir.to_path_buf(), dir.join("../lib/tcc")]
        .into_iter()
        .find(|candidate| candidate.join("libtcc1.a").is_file())
        .map(|candidate| vec![format!("-B{}", candidate.display())])
        .unwrap_or_default()
}

/// The code `program` ended with, and what a person is told where a signal stopped it, which the
/// program had no chance to say: -1 and the signal then.
pub fn ended(program: &Path, status: std::process::ExitStatus) -> (i64, Option<String>) {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            let name = match signal {
                6 => " (SIGABRT)",
                9 => " (SIGKILL)",
                11 => " (SIGSEGV)",
                15 => " (SIGTERM)",
                _ => "",
            };
            return (
                -1,
                Some(format!(
                    "{} was stopped by signal {signal}{name}",
                    program.display()
                )),
            );
        }
    }
    (status.code().map_or(-1, i64::from), None)
}

/// What the runtime's library needs linked after it on this platform, as `rustc` names its native
/// libraries.
fn libraries() -> &'static [&'static str] {
    if cfg!(target_os = "macos") {
        &["-liconv", "-lSystem", "-lc", "-lm"]
    } else {
        &[
            "-lgcc_s",
            "-lutil",
            "-lrt",
            "-lpthread",
            "-lm",
            "-ldl",
            "-lc",
        ]
    }
}

/// `path` below the cache; `None`, with the run failed, for one that names a place outside it.
fn below(c: &mut Ctx, path: &str) -> Option<PathBuf> {
    let relative = Path::new(path);
    if relative.as_os_str().is_empty()
        || !relative
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        let span = c.site();
        c.fail(
            Diagnostic::error(
                codes::FS_PATH_ESCAPES_ROOT,
                format!("`{path}` is not a path below the toolchain's cache"),
            )
            .primary(span, "named here")
            .note("the toolchain reads and writes below its cache and nowhere else"),
        );
        return None;
    }
    Some(crate::c::cache_dir().join(relative))
}

/// The path at `p` below the cache.
fn below_at(c: &mut Ctx, p: Addr, n: I64) -> Option<PathBuf> {
    let path = text_at(c, p, n)?;
    below(c, &path)
}

/// `bytes` at `target`, written beside it and renamed, so a reader never sees half of it.
fn landed(target: &Path, bytes: &[u8]) -> bool {
    let Some(parent) = target.parent() else {
        return false;
    };
    let tmp = ply_core::files::temp_beside(target);
    let ok = std::fs::create_dir_all(parent).is_ok()
        && std::fs::write(&tmp, bytes).is_ok()
        && std::fs::rename(&tmp, target).is_ok();
    if !ok {
        let _ = std::fs::remove_file(&tmp);
    }
    ok
}

/// The compiler the label at `p` names, found: its path, what identifies it, and its support
/// flags as parts, as parts.
pub unsafe extern "C" fn rt_compilers_found(ctx: *mut Ctx, p: Addr, n: I64) -> TakenBytes {
    borrowed_toolchain(ctx, "compilers_found", Deterministic, TakenBytes(0), |c| {
        let Some(label) = text_at(c, p, n) else {
            return TakenBytes(0);
        };
        let Some(path) = compiler_path(&label) else {
            return c.given(found(None::<Vec<u8>>));
        };
        // Through the link: what sits beside the binary is the question, and a name on `PATH` is
        // often a link into a tree that has what the link's directory has not.
        let resolved = std::fs::canonicalize(&path).unwrap_or(path);
        let stamp = std::fs::metadata(&resolved)
            .ok()
            .map(|m| {
                let modified = m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_secs());
                format!("{}:{modified}", m.len())
            })
            .unwrap_or_default();
        let shown = resolved.display().to_string();
        let identity = format!("{shown}\0{stamp}");
        let support = support_flags(&resolved);
        let support: Vec<&[u8]> = support.iter().map(|s| s.as_bytes()).collect();
        let flags = parts(&support);
        c.given(found(Some(parts(&[
            shown.as_bytes(),
            identity.as_bytes(),
            flags.as_slice(),
        ]))))
    })
}

/// Queues the compiler named by the label at `lp` to run with the arguments at `ap` when a core is
/// free. Returns byte 1 and an eight-byte ticket, or byte 0 and the reason if no compiler has that
/// label. The ticket returns byte 0 if the compiler succeeded. Otherwise it returns byte 1 and what
/// the compiler wrote to standard error, or why it did not start. Compilers start in the order
/// queued, as many at once as the machine has cores, so a program can queue every run before it
/// waits for any.
pub unsafe extern "C" fn rt_compilers_started(
    ctx: *mut Ctx,
    lp: Addr,
    ln: I64,
    ap: Addr,
    an: I64,
) -> TakenBytes {
    borrowed_toolchain(
        ctx,
        "compilers_started",
        Deterministic,
        TakenBytes(0),
        |c| {
            let (Some(label), Some(args)) = (text_at(c, lp, ln), texts_at(c, ap, an)) else {
                return TakenBytes(0);
            };
            let Some(program) = compiler_path(&label) else {
                let reason = format!("no `{label}` is installed");
                return c.given([&[0u8][..], reason.as_bytes()].concat());
            };
            let ticket = c.queued(
                "compilers_started",
                "a compiler",
                Work::Computing,
                move || {
                    let ran = Command::new(&program)
                        .args(&args)
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::piped())
                        .output();
                    Ok(match ran {
                        Ok(out) if out.status.success() => vec![0],
                        Ok(out) => found(Some(out.stderr)),
                        Err(e) => found(Some(format!("`{}`: {e}", program.display()).into_bytes())),
                    })
                },
            );
            if ticket.0 == 0 {
                return TakenBytes(0);
            }
            c.given([&[1u8][..], &int(ticket.0)[..]].concat())
        },
    )
}

/// The cache's directory.
pub unsafe extern "C" fn rt_toolchain_cache(ctx: *mut Ctx) -> TakenBytes {
    borrowed_toolchain(ctx, "toolchain_cache", Deterministic, TakenBytes(0), |c| {
        c.given(crate::c::cache_dir().display().to_string().into_bytes())
    })
}

/// The directory what `ply run` and `ply check` reuse is stored in, under the stage root, where the
/// sweep keeps it to the cache's budget.
pub unsafe extern "C" fn rt_toolchain_reused(ctx: *mut Ctx) -> TakenBytes {
    borrowed_toolchain(ctx, "toolchain_reused", Deterministic, TakenBytes(0), |c| {
        let dir = crate::c::stage::stage_dir(crate::c::sweep::REUSED);
        c.given(dir.display().to_string().into_bytes())
    })
}

/// The directory the emitter's results are cached in between runs, under the toolchain's sweep.
pub unsafe extern "C" fn rt_toolchain_bodies(ctx: *mut Ctx) -> TakenBytes {
    borrowed_toolchain(ctx, "toolchain_bodies", Deterministic, TakenBytes(0), |c| {
        c.given(crate::c::bodies_dir().display().to_string().into_bytes())
    })
}

/// 1 where the cache holds a file at the path at `p`, which records a use of it; else 0.
pub unsafe extern "C" fn rt_toolchain_holds(ctx: *mut Ctx, p: Addr, n: I64) -> I64 {
    borrowed_toolchain(ctx, "toolchain_holds", Deterministic, I64(0), |c| {
        let Some(at) = below_at(c, p, n) else {
            return I64(0);
        };
        let held = at.is_file();
        if held {
            crate::c::sweep::used(&at);
        }
        I64(i64::from(held))
    })
}

/// What the cache holds at the path at `p`, found.
pub unsafe extern "C" fn rt_toolchain_held(ctx: *mut Ctx, p: Addr, n: I64) -> TakenBytes {
    borrowed_toolchain(ctx, "toolchain_held", Deterministic, TakenBytes(0), |c| {
        let Some(at) = below_at(c, p, n) else {
            return TakenBytes(0);
        };
        c.given(found(std::fs::read(&at).ok()))
    })
}

/// 1 where the bytes at `bp` were written whole at the path at `p`, beside it and renamed.
pub unsafe extern "C" fn rt_toolchain_written(
    ctx: *mut Ctx,
    p: Addr,
    n: I64,
    bp: Addr,
    bn: I64,
) -> I64 {
    borrowed_toolchain(ctx, "toolchain_written", Deterministic, I64(0), |c| {
        let Some(at) = below_at(c, p, n) else {
            return I64(0);
        };
        I64(i64::from(landed(&at, unsafe { borrowed(bp, bn) })))
    })
}

/// 1 where the path at `fp` was moved whole to the one at `tp`.
pub unsafe extern "C" fn rt_toolchain_moved(
    ctx: *mut Ctx,
    fp: Addr,
    fnn: I64,
    tp: Addr,
    tn: I64,
) -> I64 {
    borrowed_toolchain(ctx, "toolchain_moved", Deterministic, I64(0), |c| {
        let (Some(from), Some(to)) = (below_at(c, fp, fnn), below_at(c, tp, tn)) else {
            return I64(0);
        };
        let parent_made = to
            .parent()
            .is_some_and(|parent| std::fs::create_dir_all(parent).is_ok());
        I64(i64::from(
            parent_made && std::fs::rename(&from, &to).is_ok(),
        ))
    })
}

/// 1 where the bytes at `bp` were added after what the path at `p` holds.
pub unsafe extern "C" fn rt_toolchain_appended(
    ctx: *mut Ctx,
    p: Addr,
    n: I64,
    bp: Addr,
    bn: I64,
) -> I64 {
    use std::io::Write;
    borrowed_toolchain(ctx, "toolchain_appended", Deterministic, I64(0), |c| {
        let Some(at) = below_at(c, p, n) else {
            return I64(0);
        };
        let bytes = unsafe { borrowed(bp, bn) };
        let done = std::fs::OpenOptions::new()
            .append(true)
            .open(&at)
            .and_then(|mut file| file.write_all(bytes));
        I64(i64::from(done.is_ok()))
    })
}

/// A new directory of the asker's own below the cache, its path there found.
pub unsafe extern "C" fn rt_toolchain_scratch(ctx: *mut Ctx) -> TakenBytes {
    borrowed_toolchain(
        ctx,
        "toolchain_scratch",
        Deterministic,
        TakenBytes(0),
        |c| {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos());
            let name = format!(
                "work/{}-{nanos}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            );
            let at = crate::c::cache_dir().join(&name);
            // A new directory, never one another build is writing into.
            let made = at
                .parent()
                .is_some_and(|parent| std::fs::create_dir_all(parent).is_ok())
                && std::fs::create_dir(&at).is_ok();
            c.given(found(made.then(|| name.into_bytes())))
        },
    )
}

/// The directory at `p` below the cache removed with everything under it.
pub unsafe extern "C" fn rt_toolchain_cleared(ctx: *mut Ctx, p: Addr, n: I64) {
    borrowed_toolchain(ctx, "toolchain_cleared", Deterministic, (), |c| {
        if let Some(at) = below_at(c, p, n) {
            let _ = std::fs::remove_dir_all(at);
        }
    });
}

/// What the runtime's library needs linked after it on this platform, as parts.
pub unsafe extern "C" fn rt_toolchain_libraries(ctx: *mut Ctx) -> TakenBytes {
    borrowed_toolchain(
        ctx,
        "toolchain_libraries",
        Deterministic,
        TakenBytes(0),
        |c| {
            let names: Vec<&[u8]> = libraries().iter().map(|l| l.as_bytes()).collect();
            c.given(parts(&names))
        },
    )
}

/// The ticket of the program at the path at `p` below the cache run to its end with the arguments
/// at `ap`, as parts: what it ended with, eight bytes, and what it wrote to standard output, as
/// parts. What it writes to standard error is this run's.
pub unsafe extern "C" fn rt_toolchain_ran(
    ctx: *mut Ctx,
    p: Addr,
    n: I64,
    ap: Addr,
    an: I64,
) -> I64 {
    borrowed_toolchain(ctx, "toolchain_ran", Deterministic, I64(0), |c| {
        let (Some(program), Some(args)) = (below_at(c, p, n), texts_at(c, ap, an)) else {
            return I64(0);
        };
        c.submitted(
            "toolchain_ran",
            "a linked program",
            Work::Waiting,
            move || {
                let ran = Command::new(&program)
                    .args(&args)
                    .stdin(Stdio::null())
                    .stderr(Stdio::inherit())
                    .output();
                let (code, out) = match ran {
                    Ok(out) => {
                        let (code, said) = ended(&program, out.status);
                        if let Some(said) = said {
                            eprintln!("{said}");
                        }
                        (code, out.stdout)
                    }
                    Err(e) => {
                        eprintln!("{} did not start: {e}", program.display());
                        (-1, Vec::new())
                    }
                };
                Ok(parts(&[&int(code)[..], &out[..]]))
            },
        )
    })
}

/// What the asking program's heap holds, read without a walk: its live bytes and those held for
/// them, each eight bytes, as parts.
pub unsafe extern "C" fn rt_memory_heap(ctx: *mut Ctx) -> TakenBytes {
    borrowed_toolchain(ctx, "memory_heap", Nondeterministic, TakenBytes(0), |c| {
        let (live, held) = (c.live_bytes() as i64, c.held() as i64);
        c.given(parts(&[&int(live)[..], &int(held)[..]]))
    })
}

/// The asking program's live objects, the most bytes first: each a sort's words, how many there
/// are and their bytes past their headers, as parts of parts.
pub unsafe extern "C" fn rt_memory_census(ctx: *mut Ctx) -> TakenBytes {
    borrowed_toolchain(ctx, "memory_census", Nondeterministic, TakenBytes(0), |c| {
        let each: Vec<Vec<u8>> = c
            .census()
            .into_iter()
            .map(|(size, objects, bytes)| {
                let what = format!("blocks of {size} bytes past the header");
                parts(&[
                    what.as_bytes(),
                    &int(objects as i64)[..],
                    &int(bytes as i64)[..],
                ])
            })
            .collect();
        let each: Vec<&[u8]> = each.iter().map(Vec::as_slice).collect();
        c.given(parts(&each))
    })
}
