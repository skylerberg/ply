//! The builder this binary carries, and what it builds: the compiler's own `build.main`, a
//! runnable like any other. It is the committed one when that was built for this binary's shelf
//! and runtime, else the one an earlier process staged, else one the committed builder builds now
//! from the shelf's compiler.

use crate::enter::{self, Binds};
use crate::runnable::{self, Runnable};
use ply_codegen::c::{stage, sweep};
use ply_eval::{Diagnostic, Severity, Span, codes};
use std::path::{Path, PathBuf};

/// Where the compiler's package sits in a stage laid out from the shelf, as `ply bootstrap` reads
/// it in a checkout.
const ROOT: &str = "crates/ply-compiler/ply";

const ENTRY: &str = "build.main";

const RUNNABLE: &str = "builder.run";

/// The front end and emitter recurse once per node on the native stack.
const STACK: usize = 256 << 20;

/// What the builder is asked for: a program to ship, refused when it does not check or compile,
/// or whatever the front end and the emitter made of it.
#[derive(Clone, Copy)]
enum Asked {
    Ship,
    Answer,
}

/// What a set of modules is, as `(path, text)` pairs in any order.
pub fn digest_of(modules: &[(String, String)]) -> String {
    let mut sorted: Vec<&(String, String)> = modules.iter().collect();
    sorted.sort();
    let mut h = blake3::Hasher::new();
    for (name, text) in sorted {
        h.update(name.as_bytes());
        h.update(&[0]);
        h.update(text.as_bytes());
        h.update(&[0]);
    }
    h.finalize().to_hex()[..16].to_string()
}

/// What a builder is a function of: the shelf it is built from, the compiler among it, and the
/// runtime its unit is compiled against.
pub fn identity() -> String {
    static IDENTITY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    IDENTITY
        .get_or_init(|| {
            let mut hasher = blake3::Hasher::new();
            hasher.update(b"ply builder 1\0");
            hasher.update(digest_of(crate::shelf::sources()).as_bytes());
            hasher.update(&[0]);
            hasher.update(ply_codegen::c::runtime_digest().as_bytes());
            hasher.finalize().to_hex()[..16].to_string()
        })
        .clone()
}

fn stage() -> PathBuf {
    stage::stage_dir(&format!("builder-{}", identity()))
}

/// Where the rows builds of `program` published are kept. The builder names each file for the front
/// end that published it, since another's rows seed nothing.
fn rows(program: &str) -> PathBuf {
    stage::stage_dir(sweep::ROWS).join(program)
}

/// The builder: committed, staged, or built now by the committed one and staged.
pub fn builder() -> Result<Runnable, Diagnostic> {
    let current = ply_compiler::bootstrap::BUILDER_DIGEST.trim() == identity();
    let staged = stage().join(RUNNABLE);
    if !current
        && let Ok(bytes) = std::fs::read(&staged)
        && let Ok(builder) = runnable::decode(&bytes)
    {
        sweep::used(&stage());
        return Ok(builder);
    }
    let committed = runnable::decode(ply_compiler::bootstrap::BUILDER).map_err(|why| {
        unbuilt(format!(
            "the committed builder does not read: {why}; a field the runtime requires of a front \
             end's answer lands after main's builder writes it"
        ))
    })?;
    if current {
        return Ok(committed);
    }
    let src = laid_out()?;
    let fresh = staged.with_extension(format!("run.{}", std::process::id()));
    // Behind the shelf. A builder that files what it keeps under its own definitions keeps and
    // seeds as it does anywhere; one from before that would file under this shelf's, so it keeps
    // nothing.
    let own = declared(&committed.front.answer).contains("definitions");
    let seeds = rows("builder");
    build_with(
        committed,
        &src,
        ROOT,
        ENTRY,
        &fresh,
        own.then_some(seeds.as_path()),
        own,
        Asked::Ship,
    )?;
    landed(&fresh, &staged)
}

/// `program`'s root below `src` built by the builder into `out`, seeded with the rows its last
/// build of `rows_of` kept and keeping the emitter's answers.
pub fn build(
    src: &Path,
    root: &str,
    entry: &str,
    out: &Path,
    rows_of: &str,
) -> Result<(), Diagnostic> {
    let started = std::time::Instant::now();
    let builder = builder()?;
    let ready = started.elapsed();
    let fresh = out.with_extension(format!("run.{}", std::process::id()));
    build_with(
        builder,
        src,
        root,
        entry,
        &fresh,
        Some(&rows(rows_of)),
        true,
        Asked::Ship,
    )?;
    let built = started.elapsed();
    landed(&fresh, out)?;
    if std::env::var_os("PLY_C_PHASES").is_some() {
        eprintln!(
            "phases: builder ready {}ms, built {entry} {}ms, read back {}ms",
            ready.as_millis(),
            (built - ready).as_millis(),
            (started.elapsed() - built).as_millis()
        );
    }
    Ok(())
}

/// What the front end and the emitter make of `files`, one package of `(path, text)` held in
/// memory: the runnable the builder writes of it, a refusal's diagnostics and no unit included.
/// Kept under the stage by what it is a function of, so a second asking reads it back.
pub fn answered(files: &[(String, String)]) -> Result<Vec<u8>, Diagnostic> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"ply answered 1\0");
    hasher.update(identity().as_bytes());
    for (path, text) in files {
        hasher.update(&[0]);
        hasher.update(path.as_bytes());
        hasher.update(&[0]);
        hasher.update(text.as_bytes());
    }
    let key = hasher.finalize().to_hex()[..32].to_string();
    let at = stage::stage_dir(sweep::ANSWERED).join(format!("{key}.run"));
    if let Ok(bytes) = std::fs::read(&at)
        && runnable::decode(&bytes).is_ok()
    {
        sweep::used(&at);
        return Ok(bytes);
    }
    let src = stage::stage_dir(sweep::ANSWERED).join(format!("{key}.src.{}", std::process::id()));
    let written = files.iter().try_for_each(|(path, text)| {
        let file = src.join(path);
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(file, text)
    });
    written.map_err(|e| {
        unbuilt(format!(
            "the sources could not be placed in `{}`: {e}",
            src.display()
        ))
    })?;
    let fresh = at.with_extension(format!("run.{}", std::process::id()));
    let built = build_with(builder()?, &src, ".", "", &fresh, None, true, Asked::Answer);
    let _ = std::fs::remove_dir_all(&src);
    built?;
    landed(&fresh, &at)?;
    std::fs::read(&at).map_err(|e| unbuilt(format!("`{}` could not be read: {e}", at.display())))
}

/// [`answered`] read back: the front end's answer, a refusal's included, and the unit's C.
pub fn answered_program(files: &[(String, String)]) -> Result<Runnable, Diagnostic> {
    let bytes = answered(files)?;
    runnable::decode(&bytes)
        .map_err(|why| unbuilt(format!("what it answered does not read: {why}")))
}

/// [`answered_program`] of a program that has to check: one the front end refused is the first
/// error it was refused with, the rest as its notes.
pub fn checked_program(files: &[(String, String)]) -> Result<Runnable, Diagnostic> {
    let program = answered_program(files)?;
    let mut errors = program
        .front
        .answer
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error);
    match errors.next() {
        None => Ok(program),
        Some(first) => Err(errors.fold(first.clone(), |refused, more| {
            refused.note(format!("and {}: {}", more.code, more.message))
        })),
    }
}

/// Each `(module name, text)` as the file its name spells, `a.b` at `a/b.ply`.
pub fn module_files(modules: &[(&str, &str)]) -> Vec<(String, String)> {
    modules
        .iter()
        .map(|(name, text)| {
            (
                format!("{}.ply", name.replace('.', "/")),
                (*text).to_string(),
            )
        })
        .collect()
}

/// The shipped operations `front` declares: a builder behind this binary's shelf was built before
/// any added since, and is lent only those it names.
fn lent_to(front: &ply_eval::Analysis) -> Vec<crate::hosts::LentOp> {
    let declared = declared(front);
    crate::shipped::lent_over(front)
        .into_iter()
        .filter(|(op, _)| declared.contains(op.op.as_str()))
        .collect()
}

/// The shipped operations a program's own `shipped` effect names.
fn declared(front: &ply_eval::Analysis) -> std::collections::HashSet<&str> {
    front
        .check
        .effects
        .values()
        .filter(|effect| effect.name.as_str().rsplit('.').next() == Some("shipped"))
        .flat_map(|effect| effect.ops.keys().map(|op| op.as_str()))
        .collect()
}

/// `fresh` renamed into place at `at` and read back: another process that landed one first wins,
/// and its copy is the same program.
fn landed(fresh: &Path, at: &Path) -> Result<Runnable, Diagnostic> {
    if std::fs::rename(fresh, at).is_err() {
        let _ = std::fs::remove_file(fresh);
    }
    let bytes = std::fs::read(at).map_err(|e| {
        unbuilt(format!(
            "what it built could not be read at `{}`: {e}",
            at.display()
        ))
    })?;
    runnable::decode(&bytes).map_err(|why| unbuilt(format!("what it built does not read: {why}")))
}

/// The compiler's sources laid out once under the stage, as `ply bootstrap` reads them in a
/// checkout. Laid out aside and renamed in whole: a directory that is there is complete.
fn laid_out() -> Result<PathBuf, Diagnostic> {
    let at = stage().join("src");
    if at.is_dir() {
        return Ok(at);
    }
    let aside = stage().join(format!("src.{}", std::process::id()));
    let package = aside.join(ROOT);
    let written = std::fs::create_dir_all(&package).and_then(|()| {
        ply_compiler::MODULES
            .iter()
            .try_for_each(|(name, text)| std::fs::write(package.join(format!("{name}.ply")), text))
    });
    written.map_err(|e| {
        unbuilt(format!(
            "the compiler's sources could not be placed in `{}`: {e}",
            aside.display()
        ))
    })?;
    if std::fs::rename(&aside, &at).is_err() {
        let _ = std::fs::remove_dir_all(&aside);
    }
    Ok(at)
}

/// The program below `root` of `src` built by `builder` into `out`: the builder entered on a thread
/// of its own, with the sources, the stages and the emitter's kept answers as the roots it reads
/// and writes.
#[allow(clippy::too_many_arguments)]
fn build_with(
    builder: Runnable,
    src: &Path,
    root: &str,
    entry: &str,
    out: &Path,
    rows: Option<&Path>,
    kept: bool,
    asked: Asked,
) -> Result<(), Diagnostic> {
    let stages = stage::stage_root();
    let below = |path: &Path| -> Result<String, Diagnostic> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| unbuilt(format!("`{}` could not be made: {e}", parent.display())))?;
        }
        path.strip_prefix(&stages)
            .map(|rel| rel.display().to_string())
            .map_err(|_| unbuilt(format!("`{}` is not under the stages", path.display())))
    };
    let bodies = ply_codegen::c::bodies_dir();
    let _ = std::fs::create_dir_all(&bodies);
    let argv = vec![
        root.to_string(),
        entry.to_string(),
        below(out)?,
        match rows {
            Some(rows) => below(rows)?,
            None => String::new(),
        },
        if kept { "kept" } else { "fresh" }.to_string(),
        if std::env::var_os("PLY_C_PHASES").is_some() {
            "phases"
        } else {
            "quiet"
        }
        .to_string(),
    ];
    let argv = match asked {
        // A builder behind the shelf is only ever asked to ship, in the words it was built to read.
        Asked::Ship => argv,
        Asked::Answer => [argv, vec!["answer".to_string()]].concat(),
    };
    let opened = enter::opened_runnable(builder, Path::new(ROOT))?;
    let lent = lent_to(&opened.opened.front);
    let root_named = |name: &str, path: PathBuf| ply_host::fs::RootSpec {
        name: name.to_string(),
        path,
    };
    let binds = Binds {
        roots: vec![
            root_named("src", src.to_path_buf()),
            root_named("out", stages),
            root_named("bodies", bodies),
        ],
        lent,
        ..Binds::default()
    };
    let entered = std::thread::Builder::new()
        .name("ply builder".to_string())
        .stack_size(STACK)
        .spawn(move || ply_codegen::rt::unbounded(|| enter::enter_runnable(opened, argv, binds)))
        .map(|thread| thread.join());
    match entered {
        Ok(Ok(ended)) => match ended.into_parts().0 {
            Ok(0) => Ok(()),
            Ok(code) => Err(unbuilt(format!("the builder exited {code}"))),
            Err(diagnostic) => Err(unbuilt(format!("the builder raised: {diagnostic}"))),
        },
        Ok(Err(panic)) => std::panic::resume_unwind(panic),
        Err(e) => Err(unbuilt(format!(
            "the builder could not be started on a thread of its own: {e}"
        ))),
    }
}

#[cold]
fn unbuilt(why: String) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("the builder could not build: {why}"),
    )
    .primary(Span::DUMMY, "this is Ply's fault, not the program's")
    .note("the builder is `crates/ply-compiler/ply`'s `build.main`, the compiler this binary ships")
}
