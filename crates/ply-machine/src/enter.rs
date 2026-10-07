//! Entering a program its runnable holds: its front end's answer read back over the sources it
//! carries, its unit's C compiled, its hosts bound from what the caller lends, and its entry called.

use crate::runnable::Runnable;
use ply_eval::{Analysis, Diagnostic, Ended, SourceMap, Span, Symbol, codes};
use std::path::{Path, PathBuf};

/// What a caller lends an entered program: the roots it may reach, the programs its
/// `process.spawn` labels may start, and the host operations only this entry may perform. What is
/// not lent here, the program cannot reach at all.
#[derive(Default)]
pub struct Binds {
    pub roots: Vec<ply_host::fs::RootSpec>,
    pub executables: ply_host::process::Executables,
    pub lent: Vec<crate::hosts::LentOp>,
    /// Certificates its `net.connect_tls` accepts beside the built-in roots, as `--trust` names them.
    pub trust: Vec<PathBuf>,
}

/// The code an entry answers with when it asked for none.
pub const EXIT_OK: i32 = 0;

/// A program opened to be entered.
pub struct Opened {
    pub sources: SourceMap,
    pub front: Analysis,
    /// The name the entry point answers to in this program.
    pub entry: Symbol,
}

/// A runnable opened, with the C it is entered on.
pub struct OpenedRunnable {
    pub opened: Opened,
    unit: String,
}

/// The program a runnable holds: its front end's answer read back over its own sources, rooted at
/// `root`, taking the runnable rather than copying out of it.
pub fn opened_runnable(runnable: Runnable, root: &Path) -> Result<OpenedRunnable, Diagnostic> {
    let Runnable { entry, front, unit } = runnable;
    let loaded =
        crate::driver::load_over_analysis_taken(root.to_path_buf(), front).map_err(|err| {
            err.diagnostics.into_iter().next().unwrap_or_else(|| {
                Diagnostic::error(
                    codes::INTERNAL_ERROR,
                    "the runnable's front end did not read, and nothing said why",
                )
            })
        })?;
    Ok(OpenedRunnable {
        opened: Opened {
            sources: loaded.sources,
            front: std::sync::Arc::try_unwrap(loaded.front).unwrap_or_else(|f| (*f).clone()),
            entry: Symbol::new(&entry),
        },
        unit,
    })
}

/// One entry into the program a runnable holds, on the unit's C it carries, compiled here, with no
/// line of its own on either stream: the program's output is the whole of what a caller sees. The
/// answer is the code `process.exit` asked for, else `0` for a value returned and the diagnostic
/// for a raise; what the entry ended with is the caller's to report.
pub fn enter_runnable(program: OpenedRunnable, argv: Vec<String>, binds: Binds) -> Ended<i32> {
    let OpenedRunnable { opened, unit } = program;
    let provider = ply_codegen::Unit::handed(&opened.front, unit)
        .map(|unit| unit as &'static dyn ply_eval::Provider)
        .map_err(|e| {
            Diagnostic::error(
                codes::BACKEND_UNAVAILABLE,
                format!("the program's unit could not be compiled: {e:#}"),
            )
        });
    entered_with(provider, &opened, argv, binds)
}

fn entered_with(
    provider: Result<&'static dyn ply_eval::Provider, Diagnostic>,
    opened: &Opened,
    argv: Vec<String>,
    binds: Binds,
) -> Ended<i32> {
    let Binds {
        roots,
        executables,
        lent,
        trust,
    } = binds;
    let declared = opened
        .front
        .check
        .defs
        .get(&opened.entry)
        .map(|d| d.footprint.clone());
    let provider = match provider {
        Ok(provider) => provider,
        Err(refused) => return Ended::refused(refused),
    };
    let process = ply_host::process::ProcessHost::new(
        argv,
        ply_host::process::OutputSink::Real {
            out: ply_host::process::Stream::Out,
        },
    )
    .executing(executables);
    let hosts = match crate::hosts::Hosts::open_built(
        &opened.front.check,
        &crate::options::TlsOptions {
            tls: Vec::new(),
            trust,
            mtls: Vec::new(),
        },
        &roots,
        process,
        lent,
    ) {
        Ok(hosts) => hosts,
        Err(diagnostics) => return Ended::refused(bind_failed(&diagnostics)),
    };
    let span = opened
        .front
        .check
        .defs
        .get(&opened.entry)
        .map(|d| d.span)
        .unwrap_or(Span::DUMMY);
    let ended = evaluate(opened, span, &hosts, declared.as_ref(), provider);
    let _ = crate::drive::teardown(&hosts);
    let requested = hosts.requested_exit();
    ended.map(|answer| match requested {
        Some(code) => Ok(code),
        None => answer.map(|_| EXIT_OK),
    })
}

/// What each of `roots` answers on `unit`, the C emitted over `front`, compiled here: each called
/// alone with no arguments, nothing lent and under `steps` calls, on a thread of its own as every
/// machine runs. A root's bytes are the text a build keeps a `const` definition's value as through
/// its stored root; one that raised, ran past its calls, answered anything else or is not in the
/// unit answers none.
pub fn stored_values(
    front: crate::driver::LoadedAnalysis,
    unit: String,
    roots: Vec<String>,
    steps: i64,
) -> Result<Vec<Option<Vec<u8>>>, Diagnostic> {
    let entered = std::thread::Builder::new()
        .name("ply constants".to_string())
        .stack_size(crate::STACK)
        .spawn(move || {
            let loaded = crate::driver::load_over_analysis_taken(PathBuf::from("."), front)
                .map_err(|err| {
                    err.diagnostics.into_iter().next().unwrap_or_else(|| {
                        Diagnostic::error(
                            codes::INTERNAL_ERROR,
                            "the front end's answer did not read, and nothing said why",
                        )
                    })
                })?;
            let unit = ply_codegen::Unit::handed(&loaded.front, unit).map_err(|e| {
                Diagnostic::error(
                    codes::BACKEND_UNAVAILABLE,
                    format!("the unit could not be compiled: {e:#}"),
                )
            })?;
            let mut machine =
                ply_eval::Machine::new(&loaded.front, ply_eval::Provider::attach(unit))?;
            Ok(roots
                .iter()
                .map(|root| {
                    ply_eval::rc::reset();
                    let (answer, _) = ply_codegen::rt::with_step_budget(steps, || {
                        machine.call(root, Vec::new(), Span::DUMMY)
                    })
                    .into_parts();
                    answer.ok().and_then(|value| match &value {
                        ply_eval::Value::Bytes(bytes) => Some(bytes.to_vec()),
                        _ => None,
                    })
                })
                .collect())
        })
        .map_err(|e| {
            Diagnostic::error(
                codes::INTERNAL_ERROR,
                format!("the constants could not be entered on a thread of their own: {e}"),
            )
        })?
        .join();
    entered.unwrap_or_else(|panic| std::panic::resume_unwind(panic))
}

fn bind_failed(diagnostics: &[Diagnostic]) -> Diagnostic {
    diagnostics.first().cloned().unwrap_or_else(|| {
        Diagnostic::error(
            codes::INTERNAL_ERROR,
            "the program's hosts could not be bound, and nothing said why",
        )
    })
}

fn evaluate(
    opened: &Opened,
    span: Span,
    hosts: &crate::hosts::Hosts,
    declared: Option<&ply_eval::Footprint>,
    provider: &'static dyn ply_eval::Provider,
) -> Ended<ply_eval::Value> {
    let mut machine = match ply_eval::Machine::new(&opened.front, provider.attach()) {
        Ok(machine) => machine,
        Err(refused) => return Ended::refused(refused),
    };
    machine.set_host_binding(hosts.binding());
    if let Some(runtime) = hosts.runtime_factory() {
        machine.set_host_runtime(runtime);
    }
    if let Some(declared) = declared {
        machine.set_declared_footprint(declared.clone());
    }
    machine.set_seed(ply_eval::Seed::default(), ply_eval::sim::DEFAULT_STEPS);
    machine.call(opened.entry.as_str(), Vec::new(), span)
}
