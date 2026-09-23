//! The launcher: enter a shipped artifact with the command line, its shelf beside it, and its
//! environment lent.
//!
//! This is the whole of what stands between the operating system and the program `ply` is: the
//! big-stack thread, the artifact open, the `cwd` and `shelf` roots, the process and environment
//! bindings, and the exit code.

use ply_machine::artifact::{self, Binds};
use ply_span::{Diagnostic, codes};
use std::path::{Path, PathBuf};

/// The front end and emitter recurse once per node on the native stack.
const STACK: usize = 256 << 20;

/// What a launched program is: its artifact, the shelf its modules lay out as, the root its `cwd`
/// names, and the binary's version for the environment it may ask about.
pub struct Program {
    pub artifact: Vec<u8>,
    pub artifact_name: String,
    pub shelf: Vec<(String, String)>,
    /// The stage's identity: the shelf lands beside the unit cache under it.
    pub stage: String,
    pub version: String,
}

/// The marker that says a shelf directory is whole; landed last, so a reader never sees half of
/// one. Not a `.ply` file, so the program's own listing passes over it.
const SHELF_MARKER: &str = "SHELF.ok";

/// The shelf laid out once per identity. Each file lands by a rename and the marker lands last,
/// so a run that finds the marker finds every module whole.
pub fn shelf(program: &Program) -> Result<PathBuf, Diagnostic> {
    let dir = ply_codegen::c::bundle::stage_dir(&program.stage).join("shelf");
    if dir.join(SHELF_MARKER).exists() {
        return Ok(dir);
    }
    lay_out(&dir, &program.shelf).map_err(|e| {
        Diagnostic::error(
            codes::INTERNAL_ERROR,
            format!(
                "the shipped modules could not be placed in `{}`: {e}",
                dir.display()
            ),
        )
        .primary(
            ply_span::Span::DUMMY,
            "this is Ply's fault, not the program's",
        )
    })?;
    Ok(dir)
}

fn lay_out(dir: &Path, sources: &[(String, String)]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for (name, text) in sources {
        land_in(dir, &format!("{name}.ply"), text.as_bytes())?;
    }
    land_in(dir, SHELF_MARKER, b"ok")
}

fn land_in(dir: &Path, name: &str, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = dir.join(format!("{name}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, dir.join(name))
}

/// Enter the program on the command's own big-stack thread; the answer is the exit code it asked
/// for. `binds` lends whatever the caller's command configured on top of the launcher's own.
pub fn run(
    program: &Program,
    root: &Path,
    argv: Vec<String>,
    mut binds: Binds,
) -> Result<i32, Diagnostic> {
    let shelf = shelf(program)?;
    let path = PathBuf::from(&program.artifact_name);
    let (artifact, _) = artifact::decode(&program.artifact, &path)?;
    let opened = artifact::open(&artifact, &path).map_err(|diagnostics| {
        diagnostics.into_iter().next().unwrap_or_else(|| {
            Diagnostic::error(
                codes::INTERNAL_ERROR,
                "the program did not open, and nothing said why",
            )
        })
    })?;
    let mut roots = vec![
        ply_host::fs::RootSpec {
            name: "cwd".to_string(),
            path: root.to_path_buf(),
        },
        ply_host::fs::RootSpec {
            name: "shelf".to_string(),
            path: shelf,
        },
    ];
    roots.append(&mut binds.roots);
    binds.roots = roots;
    binds
        .lent
        .extend(crate::env::registrations(&program.version));
    // The program is the tool's own work rather than a program under test, so the budgets a run
    // gives a program are not its. The entry runs on a thread of its own, with the stack a front
    // end's recursion wants.
    let work = std::thread::Builder::new()
        .name("ply".to_string())
        .stack_size(STACK)
        .spawn(move || {
            ply_codegen::rt::unbounded(|| artifact::enter(&artifact, &opened, argv, binds))
        });
    match work {
        Ok(thread) => match thread.join() {
            Ok(answer) => answer,
            Err(panic) => std::panic::resume_unwind(panic),
        },
        Err(e) => Err(Diagnostic::error(
            codes::INTERNAL_ERROR,
            format!("the program could not be started on a thread of its own: {e}"),
        )
        .primary(ply_span::Span::DUMMY, "this is Ply's fault")),
    }
}

pub mod env;
