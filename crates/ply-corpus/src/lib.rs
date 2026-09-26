//! A synthetic but realistic Ply project, at whatever scale a benchmark needs, and a harness that
//! says which compiler phase the time went to.

pub mod bench;
pub mod build;
pub mod cmd;
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

/// The default tier; a bare machine has no front end and declines everything.
pub(crate) fn tier_spec() -> ply_eval::BackendSpec {
    ply_eval::BackendSpec {
        kind: ply_eval::BackendKind::C,
    }
}

/// The front end over one caller-written module that imports the standard library: the
/// caller's source alone, with the shipped std pulled as the built-in package rather than
/// inlined, and the sources placed the way the front end places them. Errors raise the way
/// [`ply_codegen::c::producer::checked_front`] raises them.
pub fn checked_front_with_std(
    path: &Path,
    module: &str,
    text: &str,
) -> Result<(ply_ty::Front, ply_span::SourceMap)> {
    let answered = ply_codegen::c::producer::checked_front_with_std(&[(
        module.to_string(),
        text.to_string(),
    )])?;
    let mut sources = ply_span::SourceMap::new();
    for (name, text) in &answered.modules {
        let path = if ply_std::is_reserved(name) {
            ply_std::pseudo_path(&ply_ty::ModuleName::from_dotted(name))
        } else {
            path.to_path_buf()
        };
        sources.add(&path, text.clone());
    }
    Ok((answered.front, sources))
}

/// A machine over the program with the default tier attached.
pub fn tier_machine<'a>(
    port: &'a ply_ty::Front,
    sources: &ply_span::SourceMap,
) -> ply_eval::Machine<'a> {
    ply_codegen::c::producer::ensure_default();
    let texts = ply_machine::support::module_texts(&port.check, sources);
    let unit = ply_codegen::Unit::over_front(port, texts).expect("this host has a C compiler");
    let mut machine = ply_eval::Machine::new(port);
    machine.set_compiled(ply_eval::Provider::attach(unit, &tier_spec()));
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
    let texts = ply_machine::support::module_texts(&front.check, &front.sources);
    let unit =
        ply_codegen::Unit::over_front(&front.port, texts).expect("this host has a C compiler");
    let executor = ply_test::InterpExecutor::new(&front.port)
        .with_backend(unit, tier_spec())
        .with_search(search)
        .with_hosts(hosting);
    ply_test::run_with(selection, &front.check, &front.hashes, store, &executor)
}

#[derive(Clone, Debug)]
pub struct Verified {
    pub definitions: usize,
    pub tests: usize,
    pub passed: usize,
    pub failed: usize,
    pub groups: usize,
    pub largest_group: usize,
    /// Tests whose footprint carries `sim.read`, so their result depends on a seed.
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
