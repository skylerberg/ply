//! The benchmark harnesses the corpus program (`crates/ply-corpus/ply`) still hands to Rust: the
//! transitional executor's subcommands, each run from the plan the program writes.

pub mod discharge;
pub mod measure;
pub mod payload;
pub mod pg;
pub mod pipeline;
pub mod r4;
pub mod regions;
pub mod rng;
pub mod serve;
pub mod simulate;
pub mod w3;
pub mod w4;
pub mod w5;
pub mod w6;
pub mod w6_run;

use anyhow::Result;
use ply_store::Store;
use std::path::Path;

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
    machine.set_compiled(ply_eval::Provider::attach(unit));
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
        .with_backend(unit)
        .with_search(search)
        .with_hosts(hosting);
    ply_test::run_with(selection, &front.check, &front.hashes, store, &executor)
}
