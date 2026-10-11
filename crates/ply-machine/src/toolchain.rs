//! The `toolchain` family: what a program that compiles C asks of the host, as `compiler.cc`
//! declares it. `compilers` names the C compilers by label, `cc` (`PLY_CC` where it is set) and
//! `tcc`, and starts and waits for them; `toolchain` reads and writes files below the toolchain's
//! cache and nowhere else, and runs the programs linked there. What to compile, with what flags and what to keep is the program's: this
//! side only runs what it is asked to and confines where it writes. A compiler's output is a
//! function of what it is given, so the family is deterministic.

use ply_eval::host::{HostAnswer, HostHandler, HostOp, HostRequest, HostRuntime, Linearity};
use ply_eval::{Diagnostic, Resource, Span, Value as PlyValue, codes};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

const OPERATIONS: [(&str, &str, &str); 14] = [
    ("compilers", "found", "ply_machine::toolchain::found"),
    ("compilers", "started", "ply_machine::toolchain::started"),
    ("compilers", "finished", "ply_machine::toolchain::finished"),
    ("toolchain", "cache", "ply_machine::toolchain::cache"),
    ("toolchain", "jobs", "ply_machine::toolchain::jobs"),
    ("toolchain", "holds", "ply_machine::toolchain::holds"),
    ("toolchain", "held", "ply_machine::toolchain::held"),
    ("toolchain", "written", "ply_machine::toolchain::written"),
    ("toolchain", "moved", "ply_machine::toolchain::moved"),
    ("toolchain", "appended", "ply_machine::toolchain::appended"),
    ("toolchain", "scratch", "ply_machine::toolchain::scratch"),
    ("toolchain", "cleared", "ply_machine::toolchain::cleared"),
    (
        "toolchain",
        "libraries",
        "ply_machine::toolchain::libraries",
    ),
    ("toolchain", "ran", "ply_machine::toolchain::ran"),
];

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

/// The family's operations, over one table of the jobs it started.
pub fn lent() -> Vec<(HostOp, Arc<dyn HostHandler>)> {
    let handler: Arc<dyn HostHandler> = Arc::new(Toolchain::default());
    OPERATIONS
        .into_iter()
        .map(|(effect, op, path)| {
            let op = crate::hosts::hermetic_op(effect, op, Linearity::Repeatable, path);
            (op, Arc::clone(&handler))
        })
        .collect()
}

#[derive(Default)]
struct Toolchain {
    jobs: Mutex<HashMap<i64, Child>>,
    next: AtomicU64,
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

fn compiler_value(label: &str) -> PlyValue {
    let Some(path) = compiler_path(label) else {
        return PlyValue::ctor("None", Vec::new());
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
    PlyValue::ctor(
        "Some",
        vec![crate::payload::record(vec![
            ("path", PlyValue::str(&shown)),
            (
                "identity",
                PlyValue::bytes(format!("{shown}\0{stamp}").as_bytes()),
            ),
            (
                "support",
                PlyValue::list(
                    support_flags(&resolved)
                        .into_iter()
                        .map(PlyValue::str)
                        .collect(),
                ),
            ),
        ])],
    )
}

/// `path` below the cache, or a refusal for one that names a place outside it.
fn below(path: &str, span: Span) -> Result<PathBuf, Diagnostic> {
    let relative = Path::new(path);
    if relative.as_os_str().is_empty()
        || !relative
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
    {
        return Err(Diagnostic::error(
            codes::FS_PATH_ESCAPES_ROOT,
            format!("`{path}` is not a path below the toolchain's cache"),
        )
        .primary(span, "named here")
        .note("the toolchain reads and writes below its cache and nowhere else"));
    }
    Ok(ply_codegen::c::cache_dir().join(relative))
}

/// `bytes` at `target`, written beside it and renamed, so a reader never sees half of it.
fn landed(target: &Path, bytes: &[u8]) -> bool {
    let Some(parent) = target.parent() else {
        return false;
    };
    let tmp = ply_eval::files::temp_beside(target);
    let ok = std::fs::create_dir_all(parent).is_ok()
        && std::fs::write(&tmp, bytes).is_ok()
        && std::fs::rename(&tmp, target).is_ok();
    if !ok {
        let _ = std::fs::remove_file(&tmp);
    }
    ok
}

fn label_of(req: &HostRequest<'_>) -> Result<String, Diagnostic> {
    match &req.atom.resource {
        Resource::Named(name) => Ok(name.to_string()),
        _ => Err(Diagnostic::error(
            codes::INTERNAL_ERROR,
            "a compiler is named by a label: `compilers.started[cc](..)`",
        )
        .primary(req.span, "named here")),
    }
}

impl HostHandler for Toolchain {
    fn call(&self, _: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        let span = req.span;
        let value = match (req.op.op.as_str(), req.args) {
            ("found", []) => compiler_value(&label_of(req)?),
            ("started", [args]) => {
                let label = label_of(req)?;
                let args = args
                    .as_list(span, "a compiler's arguments")?
                    .iter()
                    .map(|a| a.as_str(span, "an argument").map(str::to_string))
                    .collect::<Result<Vec<_>, _>>()?;
                let Some(program) = compiler_path(&label) else {
                    return Ok(HostAnswer::Value(PlyValue::ctor(
                        "Err",
                        vec![PlyValue::str(format!("no `{label}` is installed"))],
                    )));
                };
                match Command::new(&program)
                    .args(&args)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::piped())
                    .spawn()
                {
                    Ok(child) => {
                        let job = self.next.fetch_add(1, Ordering::Relaxed) as i64;
                        self.jobs
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert(job, child);
                        PlyValue::ctor("Ok", vec![PlyValue::Int(job)])
                    }
                    Err(e) => PlyValue::ctor(
                        "Err",
                        vec![PlyValue::str(format!("`{}`: {e}", program.display()))],
                    ),
                }
            }
            ("finished", [job]) => {
                let job = job.as_int(span, "a job")?;
                let child = self
                    .jobs
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&job);
                match child.map(Child::wait_with_output) {
                    Some(Ok(out)) if out.status.success() => PlyValue::ctor("None", Vec::new()),
                    Some(Ok(out)) => PlyValue::ctor("Some", vec![PlyValue::bytes(&out.stderr)]),
                    Some(Err(e)) => {
                        PlyValue::ctor("Some", vec![PlyValue::bytes(e.to_string().as_bytes())])
                    }
                    None => PlyValue::ctor(
                        "Some",
                        vec![PlyValue::bytes(
                            b"the job was not started, or was waited for",
                        )],
                    ),
                }
            }
            ("cache", []) => PlyValue::str(ply_codegen::c::cache_dir().display().to_string()),
            ("jobs", []) => {
                PlyValue::Int(std::thread::available_parallelism().map_or(1, |n| n.get()) as i64)
            }
            ("holds", [path]) => {
                let at = below(path.as_str(span, "a path")?, span)?;
                let held = at.is_file();
                if held {
                    ply_codegen::c::sweep::used(&at);
                }
                PlyValue::Bool(held)
            }
            ("held", [path]) => {
                let at = below(path.as_str(span, "a path")?, span)?;
                match std::fs::read(&at) {
                    Ok(bytes) => PlyValue::ctor("Some", vec![PlyValue::bytes(&bytes)]),
                    Err(_) => PlyValue::ctor("None", Vec::new()),
                }
            }
            ("written", [path, bytes]) => {
                let at = below(path.as_str(span, "a path")?, span)?;
                PlyValue::Bool(landed(&at, bytes.as_bytes(span, "what is written")?))
            }
            ("moved", [from, to]) => {
                let from = below(from.as_str(span, "a path")?, span)?;
                let to = below(to.as_str(span, "a path")?, span)?;
                let parent_made = to
                    .parent()
                    .is_some_and(|p| std::fs::create_dir_all(p).is_ok());
                PlyValue::Bool(parent_made && std::fs::rename(&from, &to).is_ok())
            }
            ("appended", [path, bytes]) => {
                use std::io::Write;
                let at = below(path.as_str(span, "a path")?, span)?;
                let bytes = bytes.as_bytes(span, "what is appended")?;
                let done = std::fs::OpenOptions::new()
                    .append(true)
                    .open(&at)
                    .and_then(|mut f| f.write_all(bytes));
                PlyValue::Bool(done.is_ok())
            }
            ("scratch", []) => {
                static NEXT: AtomicU64 = AtomicU64::new(0);
                let nanos = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_nanos());
                let name = format!(
                    "work/{}-{nanos}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                );
                let at = ply_codegen::c::cache_dir().join(&name);
                // A new directory, never one another build is writing into.
                let made = at
                    .parent()
                    .is_some_and(|p| std::fs::create_dir_all(p).is_ok())
                    && std::fs::create_dir(&at).is_ok();
                if made {
                    PlyValue::ctor("Some", vec![PlyValue::str(name)])
                } else {
                    PlyValue::ctor("None", Vec::new())
                }
            }
            ("libraries", []) => {
                PlyValue::list(libraries().iter().map(|l| PlyValue::str(*l)).collect())
            }
            ("ran", [path, args]) => {
                let program = below(path.as_str(span, "a path")?, span)?;
                let args = args
                    .as_list(span, "a program's arguments")?
                    .iter()
                    .map(|a| a.as_str(span, "an argument").map(str::to_string))
                    .collect::<Result<Vec<_>, _>>()?;
                match Command::new(&program)
                    .args(&args)
                    .stdin(Stdio::null())
                    .output()
                {
                    Ok(out) => crate::payload::record(vec![
                        (
                            "code",
                            PlyValue::Int(out.status.code().map_or(-1, i64::from)),
                        ),
                        ("out", PlyValue::bytes(&out.stdout)),
                        ("err", PlyValue::bytes(&out.stderr)),
                    ]),
                    Err(e) => crate::payload::record(vec![
                        ("code", PlyValue::Int(-1)),
                        ("out", PlyValue::bytes(b"")),
                        ("err", PlyValue::bytes(e.to_string().as_bytes())),
                    ]),
                }
            }
            ("cleared", [dir]) => {
                let at = below(dir.as_str(span, "a path")?, span)?;
                let _ = std::fs::remove_dir_all(at);
                PlyValue::Unit
            }
            (op, _) => return Err(crate::hosts::unserved(req.op.effect.as_str(), op, span)),
        };
        Ok(HostAnswer::Value(value))
    }
}

impl Drop for Toolchain {
    /// A job no one waited for is killed and reaped, so no compiler outlives the run that started it.
    fn drop(&mut self) {
        for (_, mut child) in self.jobs.lock().unwrap_or_else(|e| e.into_inner()).drain() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
