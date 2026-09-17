//! A synthetic but realistic Ply project, at whatever scale a benchmark needs, and a harness that
//! says which compiler phase the time went to.

pub mod bench;
pub mod build;
pub mod discharge;
pub mod emit;
pub mod measure;
pub mod model;
pub mod payload;
pub mod pipeline;
pub mod r4;
pub mod regions;
pub mod rng;
pub mod serve;
pub mod simulate;
pub mod spec;
pub mod w3;
pub mod w4;
pub mod w5;
pub mod w6;
pub mod w6_run;
pub mod write;

pub use spec::CorpusSpec;

use anyhow::{Result, bail};
use ply_eval::Plan;
use ply_store::Store;
use std::path::Path;

/// The honest default tier: what every run here evaluates on under tier-only (ADR 0048), since a
/// bare machine has no front end and declines everything.
pub(crate) fn honest() -> ply_eval::BackendSpec {
    ply_eval::BackendSpec {
        kind: ply_eval::BackendKind::C,
        ..Default::default()
    }
}

/// The port's check over these very texts (ADR 0052 §1).
///
/// The harnesses here need a checked program in order to measure what it *does*; what checking
/// costs is `pipeline.rs`'s subject, and that one keeps the Rust chain because timing its phases
/// is the whole of what it reports.
///
/// `texts` is in the program's module order, because the protocol writes a span's module as its
/// position in this very list.
pub fn port_front(texts: &[(String, String)], ids: &[ply_span::SourceId]) -> Result<ply_ty::Front> {
    ply_codegen::c::producer::ensure_default();
    let front = ply_codegen::c::producer::front(texts, ids)?;
    if let Some(d) = front
        .diagnostics
        .iter()
        .find(|d| d.severity == ply_span::Severity::Error)
    {
        bail!("the port refuses this program: {} [{}]", d.message, d.code);
    }
    Ok(front)
}

/// A machine over the program with the default tier attached, produced from the module texts
/// `sources` holds.
pub fn tier_machine<'a>(
    program: &'a ply_syntax::ast::Program,
    resolved: &'a ply_syntax::resolve::Resolved,
    port: &'a ply_ty::Front,
    sources: &ply_span::SourceMap,
) -> ply_eval::Machine<'a> {
    ply_codegen::c::producer::ensure_default();
    let texts = ply_cli::commands::common::module_texts(program, sources);
    let unit = ply_codegen::Unit::over_front(program, resolved, port, texts)
        .expect("this host has a C compiler");
    let mut machine = ply_eval::Machine::new(program, resolved, &port.check);
    machine.set_compiled(ply_eval::Provider::attach(unit, &honest()));
    machine
}

/// The selected tests run on the default tier.
pub fn run_on_tier(
    front: &pipeline::Front,
    selection: &ply_test::Selection,
    store: &mut Store,
    search: ply_test::Search,
    hosting: ply_test::Hosting<'_>,
) -> ply_test::RunReport {
    ply_codegen::c::producer::ensure_default();
    let texts = ply_cli::commands::common::module_texts(&front.program, &front.sources);
    let unit = ply_codegen::Unit::over_front(&front.program, &front.resolved, &front.port, texts)
        .expect("this host has a C compiler");
    let executor = ply_test::InterpExecutor::new(&front.program, &front.resolved, &front.check)
        .with_backend(unit, honest())
        .with_search(search)
        .with_hosts(hosting);
    ply_test::run_with(selection, &front.check, &front.hashes, store, &executor)
}

/// Runs `f` on a thread with a stack deep enough for the tier's longest legal recursion.
///
/// The compiled tier recurses on the native C stack (ADR 0048), where the interpreter recursed on
/// the heap, and the language's call limit is `ply_eval::limit::DEFAULT_MAX_CALLS`. A benchmark
/// that drives a loop written as tail recursion that deep needs the room the CLI's own worker pool
/// gives it (`ply-cli`'s `WORKER_STACK`); a 2 MiB `cargo test` thread overflows first and the tier
/// raises its recursion limit early. This is the harness's equivalent of that pool.
pub fn on_deep_stack<R: Send>(f: impl FnOnce() -> R + Send) -> R {
    const DEEP_STACK: usize = 256 << 20;
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .stack_size(DEEP_STACK)
            .spawn_scoped(scope, f)
            .expect("a deep-stack thread")
            .join()
            .unwrap_or_else(|_| {
                std::panic::resume_unwind(Box::new("the deep-stack thread panicked"))
            })
    })
}

#[derive(Clone, Debug)]
pub struct Verified {
    pub definitions: usize,
    pub tests: usize,
    pub passed: usize,
    pub failed: usize,
    pub groups: usize,
    pub largest_group: usize,
    /// Tests whose footprint carries `sim.read` — the ones whose result is a function of the
    /// definition set *and* a seed.
    pub seeded: usize,
}

/// Compiles and runs a corpus with the real crates.
pub fn verify(root: &Path) -> Result<Verified> {
    let front = pipeline::front(root)?;
    let mut store = Store::open(root)?;
    store.clear()?;

    let selection = ply_test::select(
        &front.check,
        &front.hashes,
        &store,
        &Plan::default(),
        &ply_test::Engine::Evaluator,
    );
    let report = run_on_tier(
        &front,
        &selection,
        &mut store,
        ply_test::Search::of(&selection),
        ply_test::Hosting::hermetic(),
    );

    if report.failed > 0 {
        let shown: Vec<String> = report
            .failures
            .iter()
            .take(3)
            .map(|f| format!("{}: {}", f.key, f.diagnostic.message))
            .collect();
        bail!(
            "{} of {} generated tests failed — the reference evaluator disagrees with `ply-eval`:\n  {}",
            report.failed,
            selection.total,
            shown.join("\n  ")
        );
    }

    Ok(Verified {
        definitions: front.check.defs.len(),
        tests: front.check.tests.len(),
        passed: report.passed,
        failed: report.failed,
        groups: selection.groups.len(),
        largest_group: selection.groups.iter().map(|g| g.len()).max().unwrap_or(0),
        seeded: front
            .check
            .tests
            .iter()
            .filter(|t| ply_test::is_seeded(&t.footprint))
            .count(),
    })
}
