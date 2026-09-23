//! The helpers the machines share: backend selection, the compiled tier over a load, schema
//! materialisation, the worker pool, and `plural`.

use ply_span::{Diagnostic, SourceMap, Span, codes};
use std::collections::BTreeSet;

/// The worker pool's frames recurse per node on the native stack.
const WORKER_STACK: usize = 256 << 20;

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

/// `--backend`'s value as a spec, or the diagnostic that refuses it.
pub fn backend_spec(flag: Option<&String>) -> Result<Option<ply_eval::BackendSpec>, Diagnostic> {
    let Some(spec) = flag else {
        return Ok(Some(ply_eval::BackendSpec {
            kind: ply_eval::BackendKind::C,
        }));
    };
    ply_eval::backend::parse(spec)
        .map(Some)
        .map_err(|message| Diagnostic::error(codes::BACKEND_UNAVAILABLE, message))
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

/// Runs `selection` on the compiled tier built from `loaded`'s module source texts.
pub fn run_on_tier(
    loaded: &crate::load::Loaded,
    selection: &ply_test::Selection,
    hosting: ply_test::Hosting<'_>,
    store: &mut ply_store::Store,
) -> ply_test::RunReport {
    ply_codegen::c::producer::ensure_default();
    let texts = module_texts(&loaded.check, &loaded.sources);
    let unit =
        ply_codegen::Unit::over_front(&loaded.front, texts).expect("this host has a C compiler");
    let spec = ply_eval::BackendSpec {
        kind: ply_eval::BackendKind::C,
    };
    let executor = ply_test::InterpExecutor::new(&loaded.front)
        .with_backend(unit, spec)
        .with_search(ply_test::Search::of(selection))
        .with_hosts(hosting);
    ply_test::run_with(selection, &loaded.check, &loaded.hashes, store, &executor)
}

pub fn module_texts(
    check: &ply_ty::CheckOutput,
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
    flag: Option<&String>,
    loaded: &crate::load::Loaded,
) -> Result<Option<(&'static dyn ply_eval::Provider, ply_eval::BackendSpec)>, Diagnostic> {
    ply_codegen::c::producer::ensure_default();
    let Some(spec) = backend_spec(flag)? else {
        return Ok(None);
    };
    let provider = build_backend_over(
        &spec,
        &loaded.front,
        module_texts(&loaded.check, &loaded.sources),
    )?;
    Ok(Some((provider, spec)))
}

/// Every command that loaded a program uses this, so an invocation runs one front end.
pub fn build_backend_over(
    spec: &ply_eval::BackendSpec,
    front: &ply_ty::Front,
    texts: std::collections::HashMap<String, String>,
) -> Result<&'static dyn ply_eval::Provider, Diagnostic> {
    ply_codegen::c::producer::ensure_default();
    match spec.kind {
        // A refused definition is already a diagnostic about the program; anything else is this
        // host failing to make a backend at all.
        ply_eval::BackendKind::C => ply_codegen::Unit::over_front(front, texts)
            .map(|unit| unit as &'static dyn ply_eval::Provider)
            .map_err(|error| match ply_codegen::c::refused_in(&error) {
                Some(refusals) => refusals.diagnostic().clone(),
                None => unbuilt(&error),
            }),
    }
}

pub fn describe_schema(
    hosts: &mut crate::hosts::Hosts,
    constant: &dyn Fn(&str) -> Result<ply_eval::Value, Diagnostic>,
) {
    let Some(name) = hosts.schema_function().map(str::to_string) else {
        return;
    };
    hosts.describe_schema(materialise_schema(&name, constant));
}

pub fn materialise_schema(
    name: &str,
    constant: &dyn Fn(&str) -> Result<ply_eval::Value, Diagnostic>,
) -> Option<crate::db::schema::Shape> {
    constant(name)
        .ok()
        .as_ref()
        .and_then(crate::db::schema::shape_of)
}

/// A pure nullary definition entered on `provider`'s unit: how a schema function is evaluated.
pub fn enter_constant(
    provider: Option<&'static dyn ply_eval::Provider>,
    name: &str,
) -> Result<ply_eval::Value, Diagnostic> {
    let spec = ply_eval::BackendSpec {
        kind: ply_eval::BackendKind::C,
    };
    let name = ply_span::Symbol::new(name);
    let entered = match provider {
        Some(provider) => provider.attach(&spec).enter_whole(&name, &[], 10_000),
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

pub fn build_pool(
    jobs: Option<u32>,
    warnings: &mut Vec<Diagnostic>,
) -> (Option<rayon::ThreadPool>, usize) {
    let requested = jobs.unwrap_or(0) as usize;
    match rayon::ThreadPoolBuilder::new()
        .num_threads(requested)
        .stack_size(WORKER_STACK)
        .build()
    {
        Ok(pool) => {
            let workers = pool.current_num_threads();
            (Some(pool), workers)
        }
        Err(e) => {
            warnings.push(
                Diagnostic::warning(
                    ply_span::codes::RUNTIME_ERROR,
                    format!("could not start {requested} worker threads: {e}"),
                )
                .note("the run continued on the default thread pool"),
            );
            (None, rayon::current_num_threads())
        }
    }
}
