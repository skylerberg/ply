//! The benchmark harnesses the corpus program (`crates/ply-corpus/ply`) still hands to Rust: the
//! transitional executor's subcommands, each run from the plan the program writes.

pub mod simulate;

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

/// Where the `ply` binary is, given this binary: its sibling.
pub fn ply_binary() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("locating this binary")?;
    let path = exe
        .parent()
        .map(|dir| dir.join("ply"))
        .context("this binary has no parent directory")?;
    if !path.exists() {
        bail!(
            "`{}` does not exist; build it with `cargo build --release -p ply-launcher --bin ply`",
            path.display()
        );
    }
    Ok(path)
}

/// The front end over one caller-written module that imports the standard library: the
/// caller's source alone, with the shipped std pulled as the built-in package rather than
/// inlined, and the sources placed the way the front end places them. Errors raise the way
/// [`ply_codegen::c::producer::checked_front`] raises them.
pub fn checked_front_with_std(
    path: &Path,
    module: &str,
    text: &str,
) -> Result<(ply_eval::Front, ply_eval::SourceMap)> {
    let answered = ply_codegen::c::producer::checked_front_with_std(&[(
        module.to_string(),
        text.to_string(),
    )])?;
    let mut sources = ply_eval::SourceMap::new();
    for (name, text) in &answered.modules {
        let path = if ply_std::is_std(name) {
            ply_std::pseudo_path(name)
        } else {
            path.to_path_buf()
        };
        sources.add(&path, text.clone());
    }
    Ok((answered.front, sources))
}

/// A machine over the program with the default tier attached.
pub fn tier_machine<'a>(
    port: &'a ply_eval::Front,
    sources: &ply_eval::SourceMap,
) -> ply_eval::Machine<'a> {
    ply_codegen::c::producer::ensure_default();
    let texts = ply_machine::support::module_texts(&port.check, sources);
    let unit = ply_codegen::Unit::over_front(port, texts).expect("this host has a C compiler");
    let mut machine = ply_eval::Machine::new(port);
    machine.set_compiled(ply_eval::Provider::attach(unit));
    machine
}
