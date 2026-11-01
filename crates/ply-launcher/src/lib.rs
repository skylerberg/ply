use ply_compiled::entry::{self, Ended, Grants, Opened};
use ply_core::{Diagnostic, codes};
use std::path::{Path, PathBuf};

pub const FRONT: &str = "program/front";

const ENTRY_HEADER: &[u8] = b"ply entry 1\n";

pub struct Program {
    pub entry: String,
    pub definitions: String,
    pub stage: String,
}

pub fn linked_program() -> Result<Program, Diagnostic> {
    let pack = ply_pack::installed();
    let (Some(bytes), Some(digest)) = (pack.bytes(FRONT), pack.digest_of(FRONT)) else {
        return Err(link_failure(format!("its pack carries no `{FRONT}`")));
    };
    let Some(rest) = bytes.strip_prefix(ENTRY_HEADER) else {
        return Err(link_failure(format!(
            "`{FRONT}` is not a front this launcher reads"
        )));
    };
    let text =
        std::str::from_utf8(rest).map_err(|_| link_failure(format!("`{FRONT}` is not UTF-8")))?;
    let (entry, definitions) = text.split_once('\n').unwrap_or((text, ""));
    let stage: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
    Ok(Program {
        entry: entry.to_string(),
        definitions: definitions.to_string(),
        stage: format!("native-{stage}"),
    })
}

#[cold]
fn link_failure(reason: String) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("this executable's program cannot be entered: {reason}"),
    )
    .primary(
        ply_core::Span::DUMMY,
        "this is Ply's fault, not the program's",
    )
}

const SHIPPED_MARKER: &str = "SHIPPED.ok";

pub fn install_binary_pack() -> Result<(), String> {
    let binary =
        std::env::current_exe().map_err(|e| format!("this binary cannot be found: {e}"))?;
    match ply_pack::Pack::of_binary(&binary)? {
        Some(pack) => {
            ply_pack::install(pack);
            Ok(())
        }
        None => Err(format!(
            "`{}` carries no pack: a native program is linked with one, as a bootstrap links `ply`",
            binary.display()
        )),
    }
}

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

const STAMPS: &str = "stamps";

const DEFINITIONS: &str = "definitions";

const DIGESTS: &str = "digests";

const RUNTIME_SOURCES: &str = env!("PLY_RUNTIME_SOURCES");

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

pub fn shipped_dir_name(stamps: &str, definitions: &str, files: &ply_pack::Digest) -> String {
    let mut laid = blake3::Hasher::new();
    laid.update(stamps.as_bytes());
    laid.update(&[0]);
    laid.update(definitions.as_bytes());
    laid.update(&[0]);
    laid.update(files);
    format!("shipped-{}", &laid.finalize().to_hex()[..16])
}

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

pub struct RunOutcome {
    pub code: Result<i32, String>,
    pub warnings: Vec<Diagnostic>,
}

impl RunOutcome {
    fn rejected(d: Diagnostic) -> RunOutcome {
        RunOutcome {
            code: Err(d.to_string()),
            warnings: Vec::new(),
        }
    }
}

pub fn run(program: Program, root: &Path, argv: Vec<String>) -> RunOutcome {
    ply_compiled::c::sweep::once();
    ply_privileged::binder::install();
    let definitions = program.definitions;
    let shipped = match shipped_modules(&program.stage, &definitions) {
        Ok(shipped) => shipped,
        Err(rejected) => return RunOutcome::rejected(rejected),
    };
    ply_host::observe::set_shipped_and_cache_dirs(&shipped, ply_compiled::c::cache_dirs());
    let reporting = reporting();
    ply_privileged::shipped::stamp(stamps());
    ply_privileged::shipped::entered(&program.entry, definitions);
    let grants = grants(root, &shipped, &argv);
    let entry = program.entry;
    let work = std::thread::Builder::new()
        .name("ply".to_string())
        .stack_size(ply_compiled::rt::PROGRAM_STACK)
        .spawn(move || entered(&entry, &grants));
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
        Err(e) => RunOutcome::rejected(
            Diagnostic::error(
                codes::INTERNAL_ERROR,
                format!("the program could not be started on a thread of its own: {e}"),
            )
            .primary(ply_core::Span::DUMMY, "this is Ply's fault"),
        ),
    }
}

fn entered(entry: &str, grants: &Grants) -> RunOutcome {
    let opened = match Opened::linked() {
        Ok(opened) => opened,
        Err(reason) => {
            return RunOutcome::rejected(Diagnostic::error(
                codes::BACKEND_UNAVAILABLE,
                format!("the program linked into this executable could not be bound: {reason}"),
            ));
        }
    };
    let hosts = match ply_privileged::hosts::Hosts::granted(grants) {
        Ok(hosts) => hosts,
        Err(diagnostics) => {
            return RunOutcome::rejected(diagnostics.into_iter().next().unwrap_or_else(|| {
                Diagnostic::error(
                    codes::INTERNAL_ERROR,
                    "the program's hosts could not be bound, and nothing said why",
                )
            }));
        }
    };
    let run = entry::Run::unbounded();
    let outcome = entry::enter(
        &opened,
        entry,
        entry::NO_ARGUMENTS.to_vec(),
        &run,
        Some(&hosts),
        None,
    );
    let _ = hosts.shutdown_reports();
    let code = match (hosts.requested_exit(), outcome.ended) {
        (Some(code), _) => Ok(code),
        (None, Ended::Returned(_)) => Ok(0),
        (None, Ended::Failed(d)) => Err(placed(&d, &opened)),
        (None, Ended::Panicked(message)) => Err(format!(
            "Error[{}]: the program panicked: {message}\n  = this is Ply's fault, not the program's",
            codes::INTERNAL_ERROR
        )),
        (None, Ended::NoRoot) => Err(format!(
            "Error[{}]: this executable's program is entered at no `{entry}`",
            codes::INTERNAL_ERROR
        )),
        (None, Ended::NotReturnable(reason)) => Err(format!(
            "Error[{}]: `{entry}` returned, and {reason}",
            codes::INTERNAL_ERROR
        )),
    };
    RunOutcome {
        code,
        warnings: outcome.warnings,
    }
}

fn placed(d: &Diagnostic, opened: &Opened) -> String {
    let site = d.primary_span().and_then(|span| opened.root_named(span));
    match site {
        Some(root) => format!("{d}\n  = in `{root}`"),
        None => d.to_string(),
    }
}

pub mod code;
