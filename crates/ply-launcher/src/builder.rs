//! The builder this binary makes the programs it ships with: the compiler's own `build.main`, a
//! runnable like any other. It is the committed one when that was built for this binary's shelf
//! and runtime, else the one an earlier process staged, else one the committed builder builds now
//! from the shelf's compiler. A checkout with no builder committed yet builds its first one with
//! the compiler this binary carries as a library.

use ply_codegen::c::{bundle, producer, sweep};
use ply_eval::{Diagnostic, Span, Value, codes};
use ply_machine::enter::{self, Binds};
use ply_machine::runnable::{self, Runnable};
use std::path::{Path, PathBuf};

/// Where the compiler's package sits in a stage laid out from the shelf, as `ply bootstrap` reads
/// it in a checkout.
const ROOT: &str = "crates/ply-compiler/ply";

const ENTRY: &str = "build.main";

const RUNNABLE: &str = "builder.run";

/// The front end and emitter recurse once per node on the native stack.
const STACK: usize = 256 << 20;

/// What a builder is a function of: the shelf it is built from, the compiler among it, and the
/// runtime its unit is compiled against.
pub fn identity() -> String {
    static IDENTITY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    IDENTITY
        .get_or_init(|| {
            let mut hasher = blake3::Hasher::new();
            hasher.update(b"ply builder 1\0");
            hasher.update(producer::digest_of(ply_machine::shelf::sources()).as_bytes());
            hasher.update(&[0]);
            hasher.update(ply_codegen::c::runtime_digest().as_bytes());
            hasher.finalize().to_hex()[..16].to_string()
        })
        .clone()
}

fn stage() -> PathBuf {
    bundle::stage_dir(&format!("builder-{}", identity()))
}

/// Where the rows a build of `program` published are kept: under the builder that published them,
/// since another compiler's rows seed nothing.
fn rows(program: &str) -> PathBuf {
    bundle::stage_dir(&format!("rows-{}", identity())).join(program)
}

/// The builder: committed, staged, or built now and staged.
pub fn builder() -> Result<Runnable, Diagnostic> {
    let committed = ply_compiler::bootstrap::BUILDER;
    if !committed.is_empty()
        && ply_compiler::bootstrap::BUILDER_DIGEST.trim() == identity()
        && let Ok(builder) = runnable::decode(committed)
    {
        return Ok(builder);
    }
    let staged = stage().join(RUNNABLE);
    if let Ok(bytes) = std::fs::read(&staged)
        && let Ok(builder) = runnable::decode(&bytes)
    {
        sweep::used(&stage());
        return Ok(builder);
    }
    let src = laid_out()?;
    let fresh = staged.with_extension(format!("run.{}", std::process::id()));
    match runnable::decode(committed) {
        // Behind the shelf: its emitter is not the one a kept answer would be filed under, so it
        // keeps none.
        Ok(behind) => build_with(behind, &src, ROOT, ENTRY, &fresh, None, false)?,
        Err(_) => first(&src, &fresh)?,
    }
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
fn build_with(
    builder: Runnable,
    src: &Path,
    root: &str,
    entry: &str,
    out: &Path,
    rows: Option<&Path>,
    kept: bool,
) -> Result<(), Diagnostic> {
    let stages = bundle::stage_root();
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
    let opened = enter::opened_runnable(builder, Path::new(ROOT))?;
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
        lent: ply_machine::shipped::lent(),
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

/// The first builder of a checkout that commits none: its front end and unit made by the compiler
/// this binary carries as a library, and written as the runnable a committed builder would be.
fn first(src: &Path, out: &Path) -> Result<(), Diagnostic> {
    let seeded = ply_machine::load::load_seeded(&src.join(ROOT), producer::KnownRows::default())
        .map_err(|err| match err.diagnostics.first() {
            Some(d) => unbuilt(format!(
                "it does not check:\n{}",
                d.clone().placed(&err.sources)
            )),
            None => unbuilt("it does not check, and nothing said why".to_string()),
        })?;
    let kept = seeded
        .front
        .ok_or_else(|| unbuilt("its front end's answer does not encode".to_string()))?;
    let answer = ply_eval::codec::decode(&kept).map_err(unbuilt)?;
    let at = ply_eval::decode::At::new("a kept front", &answer);
    let files = at
        .field("files")
        .and_then(|files| {
            files.items(|file| {
                Ok(ply_machine::payload::record(vec![
                    ("path", Value::str(file.field("path")?.str()?)),
                    ("name", Value::str(file.field("name")?.str()?)),
                    ("text", Value::bytes(file.field("text")?.str()?.as_bytes())),
                ]))
            })
        })
        .map_err(|e| unbuilt(format!("its front end's answer does not read: {e}")))?;
    let dump = at
        .field("dump")
        .map_err(|e| unbuilt(format!("its front end's answer does not read: {e}")))?
        .value();
    producer::ensure_default();
    let loaded = &seeded.loaded;
    let produced = ply_codegen::Unit::over_front(
        &loaded.front,
        ply_machine::support::module_texts(&loaded.check, &loaded.sources),
    )
    .and_then(|unit| {
        let names: Vec<&str> = unit.compiled().iter().map(String::as_str).collect();
        unit.produce(&names)
    })
    .map_err(|e| unbuilt(format!("its unit could not be produced: {e:#}")))?;
    let bytes = runnable::encode(ENTRY, &Value::list(files), dump, produced.text.as_bytes())
        .map_err(unbuilt)?;
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| unbuilt(format!("`{}` could not be made: {e}", parent.display())))?;
    }
    ply_eval::files::write_atomically(out, &bytes).map_err(|e| {
        unbuilt(format!(
            "it could not be written to `{}`: {e}",
            out.display()
        ))
    })
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
