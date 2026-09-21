//! Building the list of claims a run has to discharge.

use ply_prove::{Frame, Obligation, ObligationKind, frame_of};
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_ty::HashOutput;
use ply_ty::SpecKind;
use ply_ty::{CheckOutput, DefWritten, Front, LawBinder, Type};

pub struct Collected {
    pub obligations: Vec<Obligation>,
    /// Trouble that cost the run an obligation.
    pub warnings: Vec<Diagnostic>,
}

/// The checked program without the shipped modules' definitions and laws, unless `std`.
pub fn project_view(check: &CheckOutput, std: bool) -> std::borrow::Cow<'_, CheckOutput> {
    if std {
        return std::borrow::Cow::Borrowed(check);
    }
    let mut scoped = check.clone();
    scoped
        .defs
        .retain(|_, info| !crate::shipped::is_shipped(&info.module));
    scoped
        .laws
        .retain(|law| !crate::shipped::is_shipped(&law.module));
    std::borrow::Cow::Owned(scoped)
}

/// `front` for each definition's parameters as written; `check` for which claims to collect.
pub fn collect(front: &Front, check: &CheckOutput, hashes: &HashOutput) -> Collected {
    let mut out = Collected {
        obligations: Vec::new(),
        warnings: Vec::new(),
    };

    for (name, info) in &check.defs {
        // A `requires` alone claims nothing to discharge.
        if !info.spec.iter().any(|s| s.kind == SpecKind::Ensures) {
            continue;
        }
        let guarded = info.spec.iter().any(|s| s.kind == SpecKind::Requires);
        let keys = hashes.specs.get(name);
        let Some(written) = front.defs_written.get(name) else {
            out.warnings.push(unwritten(name, info.span));
            continue;
        };
        let binders = clause_binders(written, info);
        let frame = frame_of(&info.footprint);

        // Keyed by position among all the owner's clauses, as `spec_hash` covers them.
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
            // A law's own row is `{}` or `{sim.read}`, a read no program can write.
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
fn clause_binders(written: &DefWritten, info: &ply_ty::DefInfo) -> Vec<LawBinder> {
    let (params, ret) = match &info.scheme.ty {
        Type::Fn { params, ret, .. } => (params.as_slice(), (**ret).clone()),
        other => (&[][..], other.clone()),
    };
    let mut binders: Vec<LawBinder> = written
        .params
        .iter()
        .zip(params)
        .map(|(param, ty)| LawBinder {
            name: param.name.clone(),
            ty: ty.clone(),
            span: param.span,
        })
        .collect();
    binders.push(LawBinder {
        name: Symbol::new("result"),
        ty: ret,
        span: info.span,
    });
    binders
}

fn unwritten(name: &Symbol, span: Span) -> Diagnostic {
    Diagnostic::warning(
        codes::CACHE_CORRUPT,
        format!("`{name}` carries a specification with no record of its parameters"),
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
