//! The helpers the machines share: backend selection, the C backend over a load, schema
//! materialisation, and `plural`.

use ply_eval::{Diagnostic, Span, codes};
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;
use std::sync::Arc;

/// How many programs' backends a thread keeps attached for the next ask.
const ATTACHED_PER_THREAD: usize = 4;

/// The backends a thread holds attached, the most recently asked last.
pub type Attached = RefCell<Vec<(Arc<dyn ply_eval::Provider>, Rc<dyn ply_eval::Compiled>)>>;

/// `provider` attached on this thread, through `held`, which keeps the few most recently asked: a
/// thread that runs many programs one after another holds a handful of their units, not all.
pub fn attached_on(
    held: &Attached,
    provider: &Arc<dyn ply_eval::Provider>,
) -> Rc<dyn ply_eval::Compiled> {
    let found = {
        let mut held = held.borrow_mut();
        held.iter()
            .position(|(p, _)| Arc::ptr_eq(p, provider))
            .map(|at| {
                let entry = held.remove(at);
                let c = Rc::clone(&entry.1);
                held.push(entry);
                c
            })
    };
    if let Some(c) = found {
        return c;
    }
    let c = Arc::clone(provider).attach();
    let mut held = held.borrow_mut();
    held.push((Arc::clone(provider), Rc::clone(&c)));
    if held.len() > ATTACHED_PER_THREAD {
        held.remove(0);
    }
    c
}

fn unbuilt(error: impl std::fmt::Display) -> Diagnostic {
    Diagnostic::error(
        codes::BACKEND_UNAVAILABLE,
        format!("the compiled unit could not be loaded: {error:#}"),
    )
    .note(
        "compiled code is the only evaluator, so a run without a backend would be reported green \
         over a seam nothing reached",
    )
}

/// The unit the program compiled for `front` into the library at `path`, loaded here. What the
/// program reaches was settled where its C was produced, so a refusal is this host failing to load
/// it.
pub fn unit_of(
    front: &ply_eval::Analysis,
    path: &std::path::Path,
) -> Result<Arc<dyn ply_eval::Provider>, Diagnostic> {
    ply_codegen::Unit::opened(front, path)
        .map(|unit| unit as Arc<dyn ply_eval::Provider>)
        .map_err(|error| unbuilt(&error))
}

/// A pure nullary definition entered on `provider`'s unit: how a schema function is evaluated.
pub fn enter_constant(
    provider: Option<Arc<dyn ply_eval::Provider>>,
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
