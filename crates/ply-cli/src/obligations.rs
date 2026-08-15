//! Building the list of claims a run has to discharge.
//!
//! One obligation per `ensures` clause and one per law. A `requires` is **not**
//! one: it is a filter on the domain of the `ensures` clauses beside it, not a
//! contract checked at every call, and a reader of a Ply spec must not read it
//! as "the compiler enforces this".
//!
//! The order is a property of the program — every definition in load order, then
//! every law — so two runs produce one artifact and `--jobs 1` and `--jobs 16`
//! agree byte for byte.

use ply_core::{CheckOutput, LawBinder, Type};
use ply_hash::HashOutput;
use ply_prove::{Frame, Obligation, ObligationKind, frame_of};
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_syntax::ast::{Item, Program, SpecKind};
use std::collections::HashMap;

pub struct Collected {
    pub obligations: Vec<Obligation>,
    /// Trouble that cost the run an obligation. Never empty silently: an
    /// obligation that was not collected is a claim nobody checked, and
    /// reporting a clean run with one missing is exactly the over-claim this
    /// milestone exists to avoid.
    pub warnings: Vec<Diagnostic>,
}

/// A project's own view of a checked program: the definitions and laws declared
/// by the modules that ship with the compiler removed.
///
/// The same rule and the same reason as `ply test`'s (ADR 0012 §1). A project's
/// obligation count and the denominator of its coverage line must not move
/// because the compiler was upgraded, for claims the project did not write and
/// cannot fix; the stdlib's own laws are discharged by the compiler's suite.
/// `--std` keeps them, for someone debugging the stdlib itself.
///
/// `LawInfo::index` is a position in the *unfiltered* list and is what
/// `HashOutput::laws` is keyed by, so it is carried across untouched rather than
/// renumbered.
pub fn project_view(check: &CheckOutput, std: bool) -> std::borrow::Cow<'_, CheckOutput> {
    if std {
        return std::borrow::Cow::Borrowed(check);
    }
    let mut scoped = check.clone();
    scoped.defs.retain(|_, info| !ply_std::is_std(&info.module));
    scoped.laws.retain(|law| !ply_std::is_std(&law.module));
    std::borrow::Cow::Owned(scoped)
}

pub fn collect(program: &Program, check: &CheckOutput, hashes: &HashOutput) -> Collected {
    let mut fns: HashMap<Symbol, &ply_syntax::ast::FnDef> = HashMap::new();
    for module in &program.modules {
        for item in &module.items {
            if let Item::Fn(def) = item {
                fns.insert(module.name.qualify(&def.name.name), def);
            }
        }
    }

    let mut out = Collected {
        obligations: Vec::new(),
        warnings: Vec::new(),
    };

    for (name, info) in &check.defs {
        // A definition carrying only `requires` has no obligation: a
        // precondition is a filter on the domain of the `ensures` clauses beside
        // it, and on its own it claims nothing to discharge.
        if !info.spec.iter().any(|s| s.kind == SpecKind::Ensures) {
            continue;
        }
        let guarded = info.spec.iter().any(|s| s.kind == SpecKind::Requires);
        let keys = hashes.specs.get(name);
        let Some(def) = fns.get(name) else {
            out.warnings.push(unparsed(name, info.span));
            continue;
        };
        let binders = clause_binders(def, info);
        let frame = frame_of(&info.footprint);

        // The key is looked up by the clause's position among **all** of the
        // owner's clauses, because that is what `spec_hash` covers — so
        // reordering a `requires` past an `ensures` re-opens it. What is
        // *reported* is the postcondition's own ordinal, which is what a reader
        // counts.
        for (ordinal, clause) in info
            .spec
            .iter()
            .filter(|c| c.kind == SpecKind::Ensures)
            .enumerate()
        {
            let Some(&key) = keys.and_then(|keys| keys.get(clause.index)) else {
                out.warnings.push(unhashed(name, clause.span));
                continue;
            };
            out.obligations.push(Obligation {
                key,
                owner: name.clone(),
                kind: ObligationKind::Ensures { index: ordinal },
                span: clause.span,
                frame: frame.clone(),
                binders: binders.clone(),
                guarded,
                host: false,
                footprint: clause.footprint.clone(),
            });
        }
    }

    for law in &check.laws {
        let Some(&key) = hashes.laws.get(law.index) else {
            out.warnings.push(unhashed(&law.key, law.span));
            continue;
        };
        out.obligations.push(Obligation {
            key,
            owner: law.key.clone(),
            kind: ObligationKind::Law,
            span: law.span,
            // A law is a claim about the definitions it names rather than about
            // one definition's effects, and its own row is `{}` or `{sim.read}`
            // — a read of an input no program can write. There is nothing it
            // could disturb.
            frame: Frame::Pure,
            binders: law.binders.clone(),
            guarded: law.has_guard,
            host: law.host,
            footprint: law.footprint.clone(),
        });
    }

    out
}

/// The owner's parameters, then `result`.
///
/// The names come from the AST and the types from the inferred scheme, which is
/// the only pairing available: a parameter's declared type is optional in the
/// surface syntax and the checker is what resolved it.
fn clause_binders(def: &ply_syntax::ast::FnDef, info: &ply_core::DefInfo) -> Vec<LawBinder> {
    let (params, ret) = match &info.scheme.ty {
        Type::Fn { params, ret, .. } => (params.as_slice(), (**ret).clone()),
        // A definition with no parameters is still a function of nothing whose
        // `ensures` speaks about its value.
        other => (&[][..], other.clone()),
    };
    let mut binders: Vec<LawBinder> = def
        .params
        .iter()
        .zip(params)
        .map(|(param, ty)| LawBinder {
            name: param.name.name.clone(),
            ty: ty.clone(),
            span: param.span,
        })
        .collect();
    binders.push(LawBinder {
        name: Symbol::new("result"),
        ty: ret,
        span: def.span,
    });
    binders
}

fn unparsed(name: &Symbol, span: Span) -> Diagnostic {
    Diagnostic::warning(
        codes::CACHE_CORRUPT,
        format!("`{name}` carries a specification whose source was not parsed"),
    )
    .primary(span, "its obligations were not collected")
    .note("nothing here claims they hold; run again with `--no-incremental`")
}

fn unhashed(name: &Symbol, span: Span) -> Diagnostic {
    Diagnostic::warning(
        codes::CACHE_CORRUPT,
        format!("no obligation key was produced for `{name}`"),
    )
    .primary(span, "this claim was not discharged")
    .note("nothing here claims it holds")
}
