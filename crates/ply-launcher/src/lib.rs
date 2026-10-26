//! The launcher: enter the program this executable was linked with, with the command line, the
//! shipped modules beside it, and its environment lent.
//!
//! This is the whole of what stands between the operating system and the program `ply` is: the
//! big-stack thread, the definition the pack says the program enters, the `cwd` and `shipped`
//! roots, the process and environment bindings, and the exit code.

use ply_compiled::entry::{self, Ended, Grants, Opened};
use ply_core::{Diagnostic, codes};
use std::path::{Path, PathBuf};

/// Where the pack carries what a launcher reads of the linked program: the definition it enters
/// and each `fn` that one reaches with its hash.
pub const FRONT: &str = "program/front";

/// The line a front of this launcher's own format opens with, which the program's builder writes.
const FRONT_FORMAT: &[u8] = b"ply entry 1\n";

/// What a launched program is: the definition it enters, the stage its shipped modules are laid
/// out under, and the binary's version for the environment it may ask about.
pub struct Program {
    pub entry: String,
    /// Each `fn` the entry reaches and its hash, a `name hash` line each in name order.
    pub definitions: String,
    /// The stage's identity: the shipped modules land under it, beside the C cache.
    pub stage: String,
    pub version: String,
}

/// The program this executable was linked with, as its pack names it.
pub fn linked_program(version: &str) -> Result<Program, Diagnostic> {
    let pack = ply_pack::installed();
    let (Some(bytes), Some(digest)) = (pack.bytes(FRONT), pack.digest_of(FRONT)) else {
        return Err(unlinked(format!("its pack carries no `{FRONT}`")));
    };
    let (entry, definitions) = match bytes.strip_prefix(FRONT_FORMAT) {
        Some(rest) => {
            let text = std::str::from_utf8(rest)
                .map_err(|_| unlinked(format!("`{FRONT}` is not UTF-8")))?;
            let (entry, definitions) = text.split_once('\n').unwrap_or((text, ""));
            (entry.to_string(), definitions.to_string())
        }
        None => released_front(bytes)?,
    };
    let stage: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
    Ok(Program {
        entry,
        definitions,
        stage: format!("native-{stage}"),
        version: version.to_string(),
    })
}

/// The entry and definitions of the front a release's `ply bootstrap` packs a builder with: a
/// `front.Linked` in the value codec, of which nothing else is read.
fn released_front(bytes: &[u8]) -> Result<(String, String), Diagnostic> {
    use ply_core::Value;
    let value = ply_core::codec::decode(bytes)
        .map_err(|why| unlinked(format!("`{FRONT}` does not decode: {why}")))?;
    fn field<'v>(v: &'v Value, name: &str) -> Option<&'v Value> {
        match v {
            Value::Record(fields) => fields
                .iter()
                .find(|(k, _)| k.as_str() == name)
                .map(|(_, v)| v),
            _ => None,
        }
    }
    let text = |v: Option<&Value>| match v {
        Some(Value::Str(s)) => Some(s.to_string()),
        Some(Value::Bytes(b)) => String::from_utf8(b.to_vec()).ok(),
        _ => None,
    };
    let entry = text(field(&value, "entry").and_then(|e| field(e, "name")));
    let definitions = text(field(&value, "definitions"));
    match (entry, definitions) {
        (Some(entry), Some(definitions)) => Ok((entry, definitions)),
        _ => Err(unlinked(format!(
            "`{FRONT}` names no entry and definitions"
        ))),
    }
}

#[cold]
fn unlinked(why: String) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("this executable's program cannot be entered: {why}"),
    )
    .primary(
        ply_core::Span::DUMMY,
        "this is Ply's fault, not the program's",
    )
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
            "`{}` carries no pack: a native program is linked with one, as `ply bootstrap` links `ply`",
            binary.display()
        )),
    }
}

/// What the `ply` program is lent: every family; its roots, `cwd` at `root`, `shipped` at the
/// shipped modules and `abs` at the filesystem's root (the program is the tool, so a path that
/// leaves the working directory is addressed under `abs`); its command line; the `git` on `PATH`
/// for its `process.spawn[git]`, which fetches a git dependency, and is unbound where none can be
/// started (the program reads that with `process.bound[git]`); and the PEM files `PLY_TRUST` names,
/// separated as `PATH` separates directories, which its own `net.connect_tls` accepts beside the
/// built-in roots, as `--trust` gives a program a command runs (one that does not load is `E0430`
/// before the program runs).
pub fn grants(root: &Path, shipped: &Path, argv: &[String]) -> Grants {
    let path = |p: &Path| p.display().to_string();
    let mut grants = Grants::default()
        .with("fs", &["cwd", &path(root)])
        .with("fs", &["shipped", &path(shipped)])
        .with("fs", &["abs", "/"]);
    for family in ply_privileged::policy::names() {
        grants = grants.with("allow", &[family]);
    }
    let git = std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join("git"))
            .find(|program| {
                program.is_file()
                    && ply_host::process::Executables::new()
                        .bind("git", program, ply_core::Span::DUMMY)
                        .is_ok()
            })
    });
    if let Some(git) = git {
        grants = grants.with("exec", &["git", &path(&git)]);
    }
    if let Some(value) = std::env::var_os("PLY_TRUST") {
        for trusted in std::env::split_paths(&value).filter(|p| !p.as_os_str().is_empty()) {
            grants = grants.with("trust", &[&path(&trusted)]);
        }
    }
    let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
    grants.with("argv", &argv)
}

/// Not `.ply` files, so the program's own listing passes over them.
const STAMPS: &str = "stamps";

const DEFINITIONS: &str = "definitions";

const DIGESTS: &str = "digests";

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
        stamp(&["ply.stamp.evaluator.1", ply_compiled::c::semantics_digest()]),
        stamp(&["ply.stamp.runtime.4", RUNTIME_SOURCES])
    )
}

/// The directory a stage keeps its shipped modules in, named for all it holds: the stamps, the
/// definitions, and every shipped file by `files`, their digest. The program a stage is named for
/// reaches only some of them, so a `ply` rebuilt over an edit to another lays out its own.
pub fn shipped_dir_name(stamps: &str, definitions: &str, files: &ply_pack::Digest) -> String {
    let mut laid = blake3::Hasher::new();
    laid.update(stamps.as_bytes());
    laid.update(&[0]);
    laid.update(definitions.as_bytes());
    laid.update(&[0]);
    laid.update(files);
    format!("shipped-{}", &laid.finalize().to_hex()[..16])
}

/// The shipped modules as files, `<dotted name>.ply` each, the data files they embed below them by
/// name, and the stamps, the definitions and every shipped file's digest beside them. Each file
/// lands by a rename and the marker last, so a run that finds the marker finds it whole, and reads
/// no file of the pack for it.
pub fn shipped_modules(stage: &str, definitions: &str) -> Result<PathBuf, Diagnostic> {
    let stamps = stamps();
    let dir = ply_compiled::c::stage::stage_dir(stage).join(shipped_dir_name(
        &stamps,
        definitions,
        &ply_privileged::shipped_modules::digest(),
    ));
    if dir.join(SHIPPED_MARKER).exists() {
        ply_compiled::c::sweep::used(&ply_compiled::c::stage::stage_dir(stage));
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
            ply_core::Span::DUMMY,
            "this is Ply's fault, not the program's",
        )
    })?;
    Ok(dir)
}

fn lay_out(dir: &Path, stamps: &str, definitions: &str) -> std::io::Result<()> {
    use ply_core::files::write_atomically;
    use ply_privileged::shipped_modules;
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
    write_atomically(&dir.join(DIGESTS), shipped_modules::digests().as_bytes())?;
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
            shipped: &ply_privileged::shipped::module_digest,
            program: ply_privileged::shipped::program_digest(),
        };
        let _ = std::fs::write(&file, ply_host::observe::report(&recorder, &binary));
    });
    let at_exit = std::sync::Arc::clone(&write);
    ply_host::observe::reports_with(Box::new(move || at_exit()));
    Some(write)
}

/// How a launched program ended: the exit code it asked for, or what stopped it, as it is printed;
/// and what its host runtime warned of as it ended.
pub struct Launched {
    pub code: Result<i32, String>,
    pub warnings: Vec<Diagnostic>,
}

impl Launched {
    fn refused(d: Diagnostic) -> Launched {
        Launched {
            code: Err(d.to_string()),
            warnings: Vec::new(),
        }
    }
}

/// Enter the program on the command's own big-stack thread, lent `lent` beside the families the
/// runtime serves itself and the launcher's own environment.
pub fn run(
    program: Program,
    root: &Path,
    argv: Vec<String>,
    mut lent: Vec<ply_privileged::hosts::LentOp>,
    count: Option<crate::count::Asked>,
) -> Launched {
    ply_compiled::c::sweep::once();
    ply_privileged::binder::install();
    let definitions = program.definitions;
    let shipped = match shipped_modules(&program.stage, &definitions) {
        Ok(shipped) => shipped,
        Err(refused) => return Launched::refused(refused),
    };
    ply_host::observe::laid_out(&shipped, ply_compiled::c::kept_dirs());
    let reporting = reporting();
    ply_privileged::shipped::stamp(stamps());
    ply_privileged::shipped::entered(&program.entry, definitions);
    let grants = grants(root, &shipped, &argv);
    lent.extend(crate::env::registrations(&program.version));
    let entry = program.entry;
    // The program is the tool's own work rather than a program under test, so the budgets a run
    // gives a program are not its. The entry runs on a thread of its own, with the stack a front
    // end's recursion wants.
    let work = std::thread::Builder::new()
        .name("ply".to_string())
        .stack_size(ply_compiled::rt::PROGRAM_STACK)
        .spawn(move || {
            let launch = || ply_compiled::rt::unbounded(|| entered(&entry, &grants, lent));
            match count {
                Some(asked) => {
                    let (launched, counted, sites) =
                        crate::count::window_sampled(launch, asked.every());
                    // The entry's answer stands: the run happened, and a count that could not
                    // be written is reported rather than replacing what the program did.
                    if let Err(e) = crate::count::write(&asked.path, counted, &sites, asked.every())
                    {
                        eprintln!(
                            "the allocation count could not be written to {}: {e}",
                            asked.path.display()
                        );
                    }
                    launched
                }
                None => launch(),
            }
        });
    match work {
        Ok(thread) => match thread.join() {
            Ok(launched) => {
                if let Some(write) = reporting {
                    write();
                }
                launched
            }
            Err(panic) => std::panic::resume_unwind(panic),
        },
        Err(e) => Launched::refused(
            Diagnostic::error(
                codes::INTERNAL_ERROR,
                format!("the program could not be started on a thread of its own: {e}"),
            )
            .primary(ply_core::Span::DUMMY, "this is Ply's fault"),
        ),
    }
}

/// The linked program's `entry` entered with nothing, under the binding `grants` and `lent` make.
fn entered(entry: &str, grants: &Grants, lent: Vec<ply_privileged::hosts::LentOp>) -> Launched {
    let opened = match Opened::linked() {
        Ok(opened) => opened,
        Err(why) => {
            return Launched::refused(Diagnostic::error(
                codes::BACKEND_UNAVAILABLE,
                format!("the program linked into this executable could not be bound: {why}"),
            ));
        }
    };
    let hosts = match ply_privileged::hosts::Hosts::granted(grants, lent) {
        Ok(hosts) => hosts,
        Err(diagnostics) => {
            return Launched::refused(diagnostics.into_iter().next().unwrap_or_else(|| {
                Diagnostic::error(
                    codes::INTERNAL_ERROR,
                    "the program's hosts could not be bound, and nothing said why",
                )
            }));
        }
    };
    let run = entry::Run::unbounded(ply_core::DEFAULT_MAX_CALLS as i64);
    let outcome = entry::enter(
        &opened,
        entry,
        entry::no_arguments(),
        &run,
        Some(&hosts),
        None,
    );
    // Ends the children still running, flushes the sink and puts the terminal back.
    let _ = hosts.torn_down();
    let code = match (hosts.requested_exit(), outcome.ended) {
        (Some(code), _) => Ok(code),
        (None, Ended::Answered(_)) => Ok(0),
        (None, Ended::Failed(d)) => Err(placed(&d, &opened)),
        (None, Ended::Panicked(message)) => Err(format!(
            "Error[{}]: the program panicked: {message}\n  = this is Ply's fault, not the program's",
            codes::INTERNAL_ERROR
        )),
        (None, Ended::NoRoot) => Err(format!(
            "Error[{}]: this executable's program is entered at no `{entry}`",
            codes::INTERNAL_ERROR
        )),
        (None, Ended::Unanswerable(why)) => Err(format!(
            "Error[{}]: `{entry}` answered, and {why}",
            codes::INTERNAL_ERROR
        )),
    };
    Launched {
        code,
        warnings: outcome.warnings,
    }
}

/// `d` as it is printed, with the definition its primary label's site lies in: the launcher holds
/// no source to place it in.
fn placed(d: &Diagnostic, opened: &Opened) -> String {
    let site = d.primary_span().and_then(|span| opened.root_named(span));
    match site {
        Some(root) => format!("{d}\n  = in `{root}`"),
        None => d.to_string(),
    }
}

pub mod code;
pub mod count;
pub mod env;
