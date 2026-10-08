//! The launcher: enter a shipped program with the command line, the shipped modules beside it, and
//! its environment lent.
//!
//! This is the whole of what stands between the operating system and the program `ply` is: the
//! big-stack thread, the runnable opened, the `cwd` and `shipped` roots, the process and
//! environment bindings, and the exit code.

use ply_eval::{Diagnostic, Ended, codes};
use ply_machine::enter::{self, Binds};
use ply_machine::runnable::Runnable;
use std::path::{Path, PathBuf};

/// The front end and emitter recurse once per node on the native stack.
const STACK: usize = 256 << 20;

/// What a launched program is: the runnable it is entered from and the binary's version for the
/// environment it may ask about. It reads the pack's shipped modules as files.
pub struct Program {
    pub runnable: Runnable,
    /// The stage's identity: the shipped modules land under it, beside the C cache.
    pub stage: String,
    pub version: String,
}

/// The marker that says a shipped modules directory is whole; landed last, so a reader never sees
/// half of one. Not a `.ply` file, so the program's own listing passes over it.
const SHIPPED_MARKER: &str = "SHIPPED.ok";

/// The pack appended to this binary, installed for everything after it to read.
pub fn install_own_pack() -> Result<(), String> {
    let binary =
        std::env::current_exe().map_err(|e| format!("this binary cannot be found: {e}"))?;
    match ply_pack::Pack::of_binary(&binary)? {
        Some(pack) => {
            ply_pack::install(pack);
            Ok(())
        }
        None => Err(format!(
            "`{}` carries no pack; `cargo pack {}` appends the checkout's",
            binary.display(),
            binary.display()
        )),
    }
}

/// The PEM files `PLY_TRUST` names, separated as `PATH` separates directories: roots the program's
/// own `net.connect_tls` accepts beside the built-in ones, as `--trust` gives a program a command
/// runs. One that does not load is `E0430` before the program runs.
pub fn trust() -> Vec<PathBuf> {
    match std::env::var_os("PLY_TRUST") {
        None => Vec::new(),
        Some(value) => std::env::split_paths(&value)
            .filter(|path| !path.as_os_str().is_empty())
            .collect(),
    }
}

/// The programs the `ply` program's own `process.spawn` labels start: the `git` on `PATH`, which
/// fetches a git dependency. Where none is installed the label stays unbound, which the program
/// reads with `process.bound[git]`.
pub fn executables() -> ply_host::process::Executables {
    let mut executables = ply_host::process::Executables::new();
    let git = std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join("git"))
            .find(|program| program.is_file())
    });
    if let Some(git) = git {
        let _ = executables.bind("git", &git, ply_eval::Span::DUMMY);
    }
    executables
}

/// Not `.ply` files, so the program's own listing passes over them.
const STAMPS: &str = "stamps";

const DEFINITIONS: &str = "definitions";

const RUNTIME_SOURCES: &str = env!("PLY_RUNTIME_SOURCES");

/// The runtime's side of what kept answers are a function of beyond their own keys, as `name hex`
/// lines: a query's answer is the evaluator's, and a pass or a claim's evidence the whole runtime's.
/// The program adds its own side to each, from the definitions it is told it has.
pub fn stamps() -> String {
    let stamp = |parts: &[&str]| {
        let mut h = blake3::Hasher::new();
        for part in parts {
            h.update(part.as_bytes());
            h.update(&[0]);
        }
        h.finalize().to_hex().to_string()
    };
    format!(
        "evaluator {}\nruntime {}\n",
        stamp(&["ply.stamp.evaluator.1", ply_codegen::c::semantics_digest()]),
        stamp(&["ply.stamp.runtime.4", RUNTIME_SOURCES])
    )
}

/// The shipped modules as files, `<dotted name>.ply` each, the data files they embed below them by
/// name, and the stamps and definitions beside them. Each file lands by a rename and the marker
/// last, so a run that finds the marker finds it whole, and reads nothing of the pack for it.
pub fn shipped_modules(program: &Program, definitions: &str) -> Result<PathBuf, Diagnostic> {
    let stamps = stamps();
    let mut laid = blake3::Hasher::new();
    laid.update(stamps.as_bytes());
    laid.update(&[0]);
    laid.update(definitions.as_bytes());
    let dir = ply_codegen::c::stage::stage_dir(&program.stage)
        .join(format!("shipped-{}", &laid.finalize().to_hex()[..16]));
    if dir.join(SHIPPED_MARKER).exists() {
        ply_codegen::c::sweep::used(&ply_codegen::c::stage::stage_dir(&program.stage));
        return Ok(dir);
    }
    lay_out(&dir, &stamps, definitions).map_err(|e| {
        Diagnostic::error(
            codes::INTERNAL_ERROR,
            format!(
                "the shipped modules could not be placed in `{}`: {e}",
                dir.display()
            ),
        )
        .primary(
            ply_eval::Span::DUMMY,
            "this is Ply's fault, not the program's",
        )
    })?;
    Ok(dir)
}

fn lay_out(dir: &Path, stamps: &str, definitions: &str) -> std::io::Result<()> {
    use ply_eval::files::write_atomically;
    use ply_machine::shipped_modules;
    std::fs::create_dir_all(dir)?;
    for (name, text) in shipped_modules::sources() {
        write_atomically(&dir.join(format!("{name}.ply")), text.as_bytes())?;
    }
    for name in shipped_modules::data_names() {
        let at = dir.join(&name);
        if let Some(parent) = at.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let bytes = shipped_modules::data(&name).expect("a listed data file is carried");
        write_atomically(&at, bytes)?;
    }
    write_atomically(&dir.join(STAMPS), stamps.as_bytes())?;
    write_atomically(&dir.join(DEFINITIONS), definitions.as_bytes())?;
    write_atomically(&dir.join(SHIPPED_MARKER), b"ok")
}

/// Writes what this `ply` read where the `ply` that started it asked, begun now so that a report
/// that never finishes is told from one never asked for. It is written when the entry ends, or
/// wherever the process ends first.
fn reporting() -> Option<std::sync::Arc<dyn Fn() + Send + Sync>> {
    let file = PathBuf::from(std::env::var_os(ply_host::observe::TRACE_VAR)?);
    std::fs::write(&file, b"").ok()?;
    let recorder = ply_host::observe::begin_process();
    recorder.ran_program();
    let write: std::sync::Arc<dyn Fn() + Send + Sync> = std::sync::Arc::new(move || {
        let binary = ply_host::observe::Binary {
            shipped: &ply_machine::shipped::module_digest,
            program: ply_machine::shipped::program_digest(),
        };
        let _ = std::fs::write(&file, ply_host::observe::report(&recorder, &binary));
    });
    let at_exit = std::sync::Arc::clone(&write);
    ply_host::observe::reports_with(Box::new(move || at_exit()));
    Some(write)
}

/// Enter the program on the command's own big-stack thread; the answer is the exit code it asked
/// for, with what the entry ended with. `binds` lends whatever the caller's command configured on
/// top of the launcher's own.
pub fn run(
    program: Program,
    root: &Path,
    argv: Vec<String>,
    mut binds: Binds,
    count: Option<crate::count::Asked>,
) -> Ended<i32> {
    let definitions = ply_machine::shipped::definitions(
        &program.runnable.front.answer,
        Some(&program.runnable.entry),
    );
    let shipped = match shipped_modules(&program, &definitions) {
        Ok(shipped) => shipped,
        Err(refused) => return Ended::refused(refused),
    };
    ply_host::observe::laid_out(&shipped, ply_codegen::c::kept_dirs());
    let reporting = reporting();
    ply_machine::shipped::stamp(stamps());
    ply_machine::shipped::entered(&program.runnable.entry, definitions);
    let version = program.version.clone();
    let opened = match enter::opened_runnable(program.runnable, Path::new(shipped::ROOT)) {
        Ok(opened) => opened,
        Err(refused) => return Ended::refused(refused),
    };
    let mut roots = vec![
        ply_host::fs::RootSpec {
            name: "cwd".to_string(),
            path: root.to_path_buf(),
        },
        ply_host::fs::RootSpec {
            name: "shipped".to_string(),
            path: shipped,
        },
        // The program is the tool: a path that leaves the working directory is addressed under
        // `abs`, the filesystem's root.
        ply_host::fs::RootSpec {
            name: "abs".to_string(),
            path: PathBuf::from("/"),
        },
    ];
    roots.append(&mut binds.roots);
    binds.roots = roots;
    binds.lent.extend(crate::env::registrations(&version));
    // The program is the tool's own work rather than a program under test, so the budgets a run
    // gives a program are not its. The entry runs on a thread of its own, with the stack a front
    // end's recursion wants.
    let work = std::thread::Builder::new()
        .name("ply".to_string())
        .stack_size(STACK)
        .spawn(move || {
            match count {
                Some(asked) => {
                    let (answer, counted, sites) = crate::count::window_sampled(
                        || {
                            ply_codegen::rt::unbounded(|| {
                                enter::enter_runnable(opened, argv, binds)
                            })
                        },
                        asked.every(),
                    );
                    // The entry's answer stands: the run happened, and a count that could not
                    // be written is reported rather than replacing what the program did.
                    if let Err(e) = crate::count::write(&asked.path, counted, &sites, asked.every())
                    {
                        eprintln!(
                            "the allocation count could not be written to {}: {e}",
                            asked.path.display()
                        );
                    }
                    answer
                }
                None => ply_codegen::rt::unbounded(|| enter::enter_runnable(opened, argv, binds)),
            }
        });
    match work {
        Ok(thread) => match thread.join() {
            Ok(answer) => {
                if let Some(write) = reporting {
                    write();
                }
                answer
            }
            Err(panic) => std::panic::resume_unwind(panic),
        },
        Err(e) => Ended::refused(
            Diagnostic::error(
                codes::INTERNAL_ERROR,
                format!("the program could not be started on a thread of its own: {e}"),
            )
            .primary(ply_eval::Span::DUMMY, "this is Ply's fault"),
        ),
    }
}

pub mod code;
pub mod count;
pub mod env;
pub mod shipped;
