//! The launcher: enter a shipped program with the command line, its shelf beside it, and its
//! environment lent.
//!
//! This is the whole of what stands between the operating system and the program `ply` is: the
//! big-stack thread, the runnable opened, the `cwd` and `shelf` roots, the process and environment
//! bindings, and the exit code.

use ply_eval::{Diagnostic, Ended, codes};
use ply_machine::artifact::{self, Binds};
use ply_machine::runnable::Runnable;
use std::path::{Path, PathBuf};

/// The front end and emitter recurse once per node on the native stack.
const STACK: usize = 256 << 20;

/// What a launched program is: the runnable it is entered from, the shelf its modules lay out as,
/// the root its `cwd` names, and the binary's version for the environment it may ask about.
pub struct Program {
    pub runnable: Runnable,
    pub shelf: Vec<(String, String)>,
    /// The stage's identity: the shelf lands beside the unit cache under it.
    pub stage: String,
    pub version: String,
}

/// The marker that says a shelf directory is whole; landed last, so a reader never sees half of
/// one. Not a `.ply` file, so the program's own listing passes over it.
const SHELF_MARKER: &str = "SHELF.ok";

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

/// Not a `.ply` file, so the program's own listing passes over it.
const STAMPS: &str = "stamps";

const RUNTIME_SOURCES: &str = env!("PLY_RUNTIME_SOURCES");

const FRONT_CODE: &str = env!("PLY_FRONT_CODE");

const VERDICT_CODE: &str = env!("PLY_VERDICT_CODE");

/// What a store's groups are a function of beyond their own keys, as `name hex` lines. A front-end
/// entry is the evaluator's and the Ply that loads and files it; a pass or a claim's evidence is the
/// runtime's, the Ply that decides and files it, and the emitter's that compiled what ran. A test's
/// own hash covers every definition it reaches, so the rest of `ply` is in neither.
pub fn stamps() -> String {
    let stamp = |parts: &[&str]| {
        let mut h = blake3::Hasher::new();
        for part in parts {
            h.update(part.as_bytes());
            h.update(&[0]);
        }
        h.finalize().to_hex().to_string()
    };
    let emitter =
        ply_codegen::c::producer::identity_of(&ply_codegen::c::producer::Sources::Embedded);
    format!(
        "frontend {}\nruntime {}\n",
        stamp(&[
            "ply.stamp.frontend.3",
            ply_codegen::c::semantics_digest(),
            FRONT_CODE
        ]),
        stamp(&[
            "ply.stamp.runtime.2",
            RUNTIME_SOURCES,
            VERDICT_CODE,
            &emitter
        ])
    )
}

/// Each file lands by a rename and the marker last, so a run that finds the marker finds it whole.
pub fn shelf(program: &Program) -> Result<PathBuf, Diagnostic> {
    let stamps = stamps();
    let dir = ply_codegen::c::bundle::stage_dir(&program.stage).join(format!(
        "shelf-{}",
        &blake3::hash(stamps.as_bytes()).to_hex()[..16]
    ));
    if dir.join(SHELF_MARKER).exists() {
        ply_codegen::c::sweep::used(&ply_codegen::c::bundle::stage_dir(&program.stage));
        return Ok(dir);
    }
    lay_out(&dir, &program.shelf, &stamps).map_err(|e| {
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

fn lay_out(dir: &Path, sources: &[(String, String)], stamps: &str) -> std::io::Result<()> {
    use ply_eval::files::write_atomically;
    std::fs::create_dir_all(dir)?;
    for (name, text) in sources {
        write_atomically(&dir.join(format!("{name}.ply")), text.as_bytes())?;
    }
    write_atomically(&dir.join(STAMPS), stamps.as_bytes())?;
    write_atomically(&dir.join(SHELF_MARKER), b"ok")
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
    let shelf = match shelf(&program) {
        Ok(shelf) => shelf,
        Err(refused) => return Ended::refused(refused),
    };
    ply_machine::shipped::stamp(stamps());
    let version = program.version.clone();
    let opened = match artifact::opened_runnable(program.runnable, Path::new(shipped::ROOT)) {
        Ok(opened) => opened,
        Err(refused) => return Ended::refused(refused),
    };
    let mut roots = vec![
        ply_host::fs::RootSpec {
            name: "cwd".to_string(),
            path: root.to_path_buf(),
        },
        ply_host::fs::RootSpec {
            name: "shelf".to_string(),
            path: shelf,
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
                                artifact::enter_runnable(opened, argv, binds)
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
                None => {
                    ply_codegen::rt::unbounded(|| artifact::enter_runnable(opened, argv, binds))
                }
            }
        });
    match work {
        Ok(thread) => match thread.join() {
            Ok(answer) => answer,
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

pub mod builder;
pub mod code;
pub mod count;
pub mod env;
pub mod shipped;
