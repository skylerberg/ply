//! The helpers the machines share: backend selection, the compiled tier over a load, schema
//! materialisation, and `plural`.

use ply_eval::{Diagnostic, SourceMap, Span, codes};
use std::collections::BTreeSet;

fn unbuilt(error: impl std::fmt::Display) -> Diagnostic {
    Diagnostic::error(
        codes::BACKEND_UNAVAILABLE,
        format!("the C backend could not be built: {error:#}"),
    )
    .note(
        "compiled code is the only evaluator, so a run without a backend would be reported green \
         over a seam nothing reached",
    )
    .note("this tier shells out to `cc`; `PLY_CC` names another compiler")
}

/// Fixes the emitted C tier's toolchain before anything compiles; not part of the cache key,
/// because both profiles must answer identically.
pub fn select_profile(flag: &str) -> Result<(), Diagnostic> {
    let Some(profile) = ply_codegen::Profile::parse(flag) else {
        return Err(Diagnostic::error(
            codes::BACKEND_UNAVAILABLE,
            format!("`--profile {flag}` is not a profile"),
        )
        .note(
            "`development` compiles fast for code that runs slowly enough; `release` compiles \
             slowly for code that runs fast",
        )
        .note(
            "the two are required to answer identically, so this decides what a run costs and \
             not what it means",
        ));
    };
    ply_codegen::select_profile(profile);
    Ok(())
}

pub fn module_texts(
    check: &ply_eval::CheckOutput,
    sources: &SourceMap,
) -> std::collections::HashMap<String, String> {
    check
        .modules
        .values()
        .filter_map(|m| {
            sources
                .get(m.source)
                .map(|f| (m.name.to_string(), f.text.to_string()))
        })
        .collect()
}

/// The unit over the whole program, its laws' and clauses' roots included.
pub fn prover_backend(
    loaded: &crate::load::Loaded,
) -> Result<&'static dyn ply_eval::Provider, Diagnostic> {
    ply_codegen::c::producer::ensure_default();
    build_backend_over(&loaded.front, module_texts(&loaded.check, &loaded.sources))
}

/// Every command that loaded a program uses this, so an invocation runs one front end.
pub fn build_backend_over(
    front: &ply_eval::Front,
    texts: std::collections::HashMap<String, String>,
) -> Result<&'static dyn ply_eval::Provider, Diagnostic> {
    ply_codegen::c::producer::ensure_default();
    // A refused definition is already a diagnostic about the program; anything else is this
    // host failing to make a backend at all.
    ply_codegen::Unit::over_front(front, texts)
        .map(|unit| unit as &'static dyn ply_eval::Provider)
        .map_err(|error| match ply_codegen::c::refused_in(&error) {
            Some(refusals) => refusals.diagnostic().clone(),
            None => unbuilt(&error),
        })
}

/// A pure nullary definition entered on `provider`'s unit: how a schema function is evaluated.
pub fn enter_constant(
    provider: Option<&'static dyn ply_eval::Provider>,
    name: &str,
) -> Result<ply_eval::Value, Diagnostic> {
    let name = ply_eval::Symbol::new(name);
    let entered = match provider {
        Some(provider) => provider.attach().enter_whole(&name, &[], 10_000),
        None => ply_eval::Entered::Declined,
    };
    match entered {
        ply_eval::Entered::Answered(value) => Ok(value),
        ply_eval::Entered::Raised(raised) => Err(raised),
        ply_eval::Entered::Declined => Err(ply_eval::err_not_compiled(&name, Span::DUMMY)),
    }
}

pub fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        return word.to_string();
    }
    match word.strip_suffix('y') {
        Some(stem) if !stem.ends_with(['a', 'e', 'i', 'o', 'u']) => format!("{stem}ies"),
        _ => format!("{word}s"),
    }
}

/// A lazy read and a later flush can each report the same unreadable file.
pub fn once_each(warnings: Vec<Diagnostic>) -> Vec<Diagnostic> {
    let mut seen: BTreeSet<(&'static str, String)> = BTreeSet::new();
    warnings
        .into_iter()
        .filter(|d| seen.insert((d.code, d.message.clone())))
        .collect()
}
