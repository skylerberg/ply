use super::{Determinism, Deterministic, Nondeterministic, granted, text_at, texts_at};
use crate::c::{MachineNative, runtime_native};
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
        runtime_native(
            "compilers_found",
            rt_compilers_found as unsafe extern "C" fn(_, _, _) -> _,
        ),
        runtime_native(
            "compilers_started",
            rt_compilers_started as unsafe extern "C" fn(_, _, _, _, _) -> _,
        ),
        runtime_native(
            "toolchain_cache",
            rt_toolchain_cache as unsafe extern "C" fn(_) -> _,
        ),
        runtime_native(
            "toolchain_reused",
            rt_toolchain_reused as unsafe extern "C" fn(_) -> _,
        ),
        runtime_native(
            "toolchain_bodies",
            rt_toolchain_bodies as unsafe extern "C" fn(_) -> _,
        ),
        runtime_native(
            "toolchain_holds",
            rt_toolchain_holds as unsafe extern "C" fn(_, _, _) -> _,
        ),
        runtime_native(
            "toolchain_held",
            rt_toolchain_held as unsafe extern "C" fn(_, _, _) -> _,
        ),
        runtime_native(
            "toolchain_written",
            rt_toolchain_written as unsafe extern "C" fn(_, _, _, _, _) -> _,
        ),
        runtime_native(
            "toolchain_moved",
            rt_toolchain_moved as unsafe extern "C" fn(_, _, _, _, _) -> _,
        ),
        runtime_native(
            "toolchain_appended",
            rt_toolchain_appended as unsafe extern "C" fn(_, _, _, _, _) -> _,
        ),
        runtime_native(
            "toolchain_scratch",
            rt_toolchain_scratch as unsafe extern "C" fn(_) -> _,
        ),
        runtime_native(
            "toolchain_cleared",
            rt_toolchain_cleared as unsafe extern "C" fn(_, _, _),
        ),
        runtime_native(
            "toolchain_libraries",
            rt_toolchain_libraries as unsafe extern "C" fn(_) -> _,
        ),
        runtime_native(
            "toolchain_ran",
            rt_toolchain_ran as unsafe extern "C" fn(_, _, _, _, _) -> _,
        ),
        runtime_native(
            "memory_heap",
            rt_memory_heap as unsafe extern "C" fn(_) -> _,
        ),
        runtime_native(
            "memory_census",
            rt_memory_census as unsafe extern "C" fn(_) -> _,
        ),
    ]
}

fn run_if_toolchain_allowed<R>(
    ctx: *mut Ctx,
    native: &'static str,
    determinism: Determinism,
    rejected: R,
    result: impl FnOnce(&mut Ctx) -> R,
) -> R {
    granted(ctx, "toolchain", native, determinism, rejected, result)
}

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

pub fn tinycc_library_flags(resolved: &Path) -> Vec<String> {
    let is_tcc = resolved
        .file_name()
        .is_some_and(|name| name.to_string_lossy().ends_with("tcc"));
    let Some(dir) = resolved.parent().filter(|_| is_tcc) else {
        return Vec::new();
    };
    [dir.to_path_buf(), dir.join("../lib/tcc")]
        .into_iter()
        .find(|candidate| candidate.join("libtcc1.a").is_file())
        .map(|candidate| vec![format!("-B{}", candidate.display())])
        .unwrap_or_default()
}

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

fn cache_path_from_text(c: &mut Ctx, p: Addr, n: I64) -> Option<PathBuf> {
    let path = text_at(c, p, n)?;
    below(c, &path)
}

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

pub unsafe extern "C" fn rt_compilers_found(ctx: *mut Ctx, p: Addr, n: I64) -> TakenBytes {
    run_if_toolchain_allowed(ctx, "compilers_found", Deterministic, TakenBytes(0), |c| {
        let Some(label) = text_at(c, p, n) else {
            return TakenBytes(0);
        };
        let Some(path) = compiler_path(&label) else {
            return c.given(found(None::<Vec<u8>>));
        };
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
        let support = tinycc_library_flags(&resolved);
        let support: Vec<&[u8]> = support.iter().map(|s| s.as_bytes()).collect();
        let flags = parts(&support);
        c.given(found(Some(parts(&[
            shown.as_bytes(),
            identity.as_bytes(),
            flags.as_slice(),
        ]))))
    })
}

pub unsafe extern "C" fn rt_compilers_started(
    ctx: *mut Ctx,
    lp: Addr,
    ln: I64,
    ap: Addr,
    an: I64,
) -> TakenBytes {
    run_if_toolchain_allowed(
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

pub unsafe extern "C" fn rt_toolchain_cache(ctx: *mut Ctx) -> TakenBytes {
    run_if_toolchain_allowed(ctx, "toolchain_cache", Deterministic, TakenBytes(0), |c| {
        c.given(crate::c::cache_dir().display().to_string().into_bytes())
    })
}

pub unsafe extern "C" fn rt_toolchain_reused(ctx: *mut Ctx) -> TakenBytes {
    run_if_toolchain_allowed(ctx, "toolchain_reused", Deterministic, TakenBytes(0), |c| {
        let dir = crate::c::stage::stage_dir(crate::c::sweep::REUSED);
        c.given(dir.display().to_string().into_bytes())
    })
}

pub unsafe extern "C" fn rt_toolchain_bodies(ctx: *mut Ctx) -> TakenBytes {
    run_if_toolchain_allowed(ctx, "toolchain_bodies", Deterministic, TakenBytes(0), |c| {
        c.given(crate::c::bodies_dir().display().to_string().into_bytes())
    })
}

pub unsafe extern "C" fn rt_toolchain_holds(ctx: *mut Ctx, p: Addr, n: I64) -> I64 {
    run_if_toolchain_allowed(ctx, "toolchain_holds", Deterministic, I64(0), |c| {
        let Some(at) = cache_path_from_text(c, p, n) else {
            return I64(0);
        };
        let held = at.is_file();
        if held {
            crate::c::sweep::used(&at);
        }
        I64(i64::from(held))
    })
}

pub unsafe extern "C" fn rt_toolchain_held(ctx: *mut Ctx, p: Addr, n: I64) -> TakenBytes {
    run_if_toolchain_allowed(ctx, "toolchain_held", Deterministic, TakenBytes(0), |c| {
        let Some(at) = cache_path_from_text(c, p, n) else {
            return TakenBytes(0);
        };
        c.given(found(std::fs::read(&at).ok()))
    })
}

pub unsafe extern "C" fn rt_toolchain_written(
    ctx: *mut Ctx,
    p: Addr,
    n: I64,
    bp: Addr,
    bn: I64,
) -> I64 {
    run_if_toolchain_allowed(ctx, "toolchain_written", Deterministic, I64(0), |c| {
        let Some(at) = cache_path_from_text(c, p, n) else {
            return I64(0);
        };
        I64(i64::from(landed(&at, unsafe { borrowed(bp, bn) })))
    })
}

pub unsafe extern "C" fn rt_toolchain_moved(
    ctx: *mut Ctx,
    fp: Addr,
    source_path_length: I64,
    tp: Addr,
    tn: I64,
) -> I64 {
    run_if_toolchain_allowed(ctx, "toolchain_moved", Deterministic, I64(0), |c| {
        let (Some(from), Some(to)) = (
            cache_path_from_text(c, fp, source_path_length),
            cache_path_from_text(c, tp, tn),
        ) else {
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

pub unsafe extern "C" fn rt_toolchain_appended(
    ctx: *mut Ctx,
    p: Addr,
    n: I64,
    bp: Addr,
    bn: I64,
) -> I64 {
    use std::io::Write;
    run_if_toolchain_allowed(ctx, "toolchain_appended", Deterministic, I64(0), |c| {
        let Some(at) = cache_path_from_text(c, p, n) else {
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

pub unsafe extern "C" fn rt_toolchain_scratch(ctx: *mut Ctx) -> TakenBytes {
    run_if_toolchain_allowed(
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
            let made = at
                .parent()
                .is_some_and(|parent| std::fs::create_dir_all(parent).is_ok())
                && std::fs::create_dir(&at).is_ok();
            c.given(found(made.then(|| name.into_bytes())))
        },
    )
}

pub unsafe extern "C" fn rt_toolchain_cleared(ctx: *mut Ctx, p: Addr, n: I64) {
    run_if_toolchain_allowed(ctx, "toolchain_cleared", Deterministic, (), |c| {
        if let Some(at) = cache_path_from_text(c, p, n) {
            let _ = std::fs::remove_dir_all(at);
        }
    });
}

pub unsafe extern "C" fn rt_toolchain_libraries(ctx: *mut Ctx) -> TakenBytes {
    run_if_toolchain_allowed(
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

pub unsafe extern "C" fn rt_toolchain_ran(
    ctx: *mut Ctx,
    p: Addr,
    n: I64,
    ap: Addr,
    an: I64,
) -> I64 {
    run_if_toolchain_allowed(ctx, "toolchain_ran", Deterministic, I64(0), |c| {
        let (Some(program), Some(args)) = (cache_path_from_text(c, p, n), texts_at(c, ap, an))
        else {
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

pub unsafe extern "C" fn rt_memory_heap(ctx: *mut Ctx) -> TakenBytes {
    run_if_toolchain_allowed(ctx, "memory_heap", Nondeterministic, TakenBytes(0), |c| {
        let (live, held) = (c.live_bytes() as i64, c.retained_bytes() as i64);
        c.given(parts(&[&int(live)[..], &int(held)[..]]))
    })
}

pub unsafe extern "C" fn rt_memory_census(ctx: *mut Ctx) -> TakenBytes {
    run_if_toolchain_allowed(ctx, "memory_census", Nondeterministic, TakenBytes(0), |c| {
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
