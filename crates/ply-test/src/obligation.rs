//! Discharging obligations, and the evidence they are filed and read back under.

use ply_eval::{DefHash, Diagnostic, codes};
use ply_prove::{
    CaseReport, Certificate, Discharge, Evidence, Fault, Obligation, ProvePlan, ProveReport, Rule,
};
use ply_store::{
    CachedCases, CachedCertificate, CachedEvidence, CachedObligation, CachedRule, Store,
};
use rayon::prelude::*;
use std::time::Instant;

pub fn to_cached(evidence: &Evidence) -> CachedObligation {
    CachedObligation {
        tier: evidence.tier().as_str().to_string(),
        evidence: match evidence {
            Evidence::Proof(c) => CachedEvidence::Proof(CachedCertificate {
                rules: c.rules.iter().map(to_cached_rule).collect(),
                steps: c.steps,
                guard_satisfiable: c.guard_satisfiable,
                sorts: c.sorts.clone(),
            }),
            Evidence::Cases(c) => CachedEvidence::Cases(CachedCases {
                generated: c.generated,
                kept: c.kept,
                rejected: c.rejected,
                roots: c.roots.clone(),
                instantiations: c.instantiations.clone(),
            }),
        },
    }
}

/// The one place the recorded tier is checked.
pub fn from_cached(entry: &CachedObligation) -> Result<Evidence, String> {
    let evidence = match &entry.evidence {
        CachedEvidence::Proof(c) => Evidence::Proof(Certificate {
            rules: c.rules.iter().map(from_cached_rule).collect(),
            steps: c.steps,
            guard_satisfiable: c.guard_satisfiable,
            sorts: c.sorts.clone(),
        }),
        CachedEvidence::Cases(c) => Evidence::Cases(CaseReport {
            generated: c.generated,
            kept: c.kept,
            rejected: c.rejected,
            roots: c.roots.clone(),
            instantiations: c.instantiations.clone(),
        }),
    };
    let computed = evidence.tier();
    if entry.tier != computed.as_str() {
        return Err(format!(
            "an entry labelled `{}` whose evidence is `{computed}`",
            entry.tier
        ));
    }
    // A certificate that did not establish its guard is `Vacuous`, never readable as a proof.
    if let Evidence::Proof(c) = &evidence
        && !c.guard_satisfiable
    {
        return Err("a proof that did not establish its guard".to_string());
    }
    Ok(evidence)
}

fn to_cached_rule(rule: &Rule) -> CachedRule {
    match rule {
        Rule::GroundEvaluation => CachedRule::GroundEvaluation,
        Rule::ExhaustiveEnumeration { domain, points } => CachedRule::ExhaustiveEnumeration {
            domain: domain.clone(),
            points: *points,
        },
        Rule::LinearArithmetic => CachedRule::LinearArithmetic,
        Rule::Propositional => CachedRule::Propositional,
        Rule::CaseSplit { ty, arms } => CachedRule::CaseSplit {
            ty: ty.clone(),
            arms: *arms,
        },
        Rule::Congruence => CachedRule::Congruence,
        Rule::Injectivity => CachedRule::Injectivity,
        Rule::Unfold { def, depth } => CachedRule::Unfold {
            def: def.clone(),
            depth: *depth,
        },
        Rule::Induction { binder, def } => CachedRule::Induction {
            binder: binder.clone(),
            def: def.clone(),
        },
        Rule::ExhaustiveInterleaving { interleavings } => CachedRule::ExhaustiveInterleaving {
            interleavings: *interleavings,
        },
    }
}

fn from_cached_rule(rule: &CachedRule) -> Rule {
    match rule {
        CachedRule::GroundEvaluation => Rule::GroundEvaluation,
        CachedRule::ExhaustiveEnumeration { domain, points } => Rule::ExhaustiveEnumeration {
            domain: domain.clone(),
            points: *points,
        },
        CachedRule::LinearArithmetic => Rule::LinearArithmetic,
        CachedRule::Propositional => Rule::Propositional,
        CachedRule::CaseSplit { ty, arms } => Rule::CaseSplit {
            ty: ty.clone(),
            arms: *arms,
        },
        CachedRule::Congruence => Rule::Congruence,
        CachedRule::Injectivity => Rule::Injectivity,
        CachedRule::Unfold { def, depth } => Rule::Unfold {
            def: def.clone(),
            depth: *depth,
        },
        CachedRule::Induction { binder, def } => Rule::Induction {
            binder: binder.clone(),
            def: def.clone(),
        },
        CachedRule::ExhaustiveInterleaving { interleavings } => Rule::ExhaustiveInterleaving {
            interleavings: *interleavings,
        },
    }
}

/// What a program decided about the obligations it asked about: which it reports on, which of those
/// the cache could not answer for, and the key each answered one's evidence is read back from. How
/// each is discharged is the obligation's own strategy.
#[derive(Clone, Debug, Default)]
pub struct Choice {
    /// Which obligations the run reports on, by collection index, ascending.
    pub claims: Vec<usize>,
    /// Positions in `claims` the cache could not answer for, in order.
    pub to_discharge: Vec<usize>,
    /// Positions in `claims` the cache answered for, and the key each one's evidence is under.
    pub read: Vec<(usize, DefHash)>,
}

pub trait Discharger: Sync {
    /// Carries out the strategy the obligation holds.
    fn discharge(&self, obligation: &Obligation, plan: &ProvePlan) -> Discharge;

    /// What the entries the discharges made ended with; forgotten once read.
    fn teardown(&self) -> Vec<Diagnostic>;
}

/// A program's decision, carried out up to what the cache answered, so a discharger is built only
/// when one is needed.
pub struct Asked {
    obligations: Vec<Obligation>,
    /// The evidence read back for each position the cache answered for.
    cached: Vec<Option<Evidence>>,
    to_discharge: Vec<usize>,
    plan: ProvePlan,
    started: Instant,
}

impl Asked {
    /// The evidence of every answered obligation is read back under the key the program named.
    pub fn chosen(
        obligations: Vec<Obligation>,
        choice: &Choice,
        store: &Store,
        plan: &ProvePlan,
    ) -> Asked {
        let started = Instant::now();
        let mut cached: Vec<Option<Evidence>> = vec![None; obligations.len()];
        for (at, key) in &choice.read {
            if let Some(slot) = cached.get_mut(*at) {
                *slot = store
                    .obligation(*key)
                    .and_then(|entry| from_cached(&entry).ok());
            }
        }
        Asked {
            obligations,
            cached,
            to_discharge: choice.to_discharge.clone(),
            plan: plan.clone().normalized(),
            started,
        }
    }

    /// Whether the cache left anything to discharge.
    pub fn pending(&self) -> bool {
        !self.to_discharge.is_empty()
    }

    pub fn discharge(self, discharger: &dyn Discharger) -> ProveReport {
        let Asked {
            obligations,
            cached,
            to_discharge,
            plan,
            started,
        } = self;

        let fresh: Vec<(usize, Discharge)> = to_discharge
            .par_iter()
            .filter(|&&index| index < obligations.len())
            .map(|&index| (index, discharger.discharge(&obligations[index], &plan)))
            .collect();

        let mut discharges: Vec<Option<Discharge>> = cached
            .into_iter()
            .map(|evidence| evidence.map(Discharge::Held))
            .collect();
        for (index, discharge) in fresh {
            discharges[index] = Some(discharge);
        }

        let paired: Vec<(Obligation, Discharge)> = obligations
            .into_iter()
            .zip(discharges)
            .map(|(obligation, discharge)| {
                let discharge = discharge.unwrap_or_else(|| unanswered(&obligation));
                (obligation, discharge)
            })
            .collect();

        ProveReport {
            obligations: paired,
            duration: started.elapsed(),
            warnings: discharger.teardown(),
        }
    }
}

/// A claim neither read back nor discharged: the program and the store disagree, which is Ply's.
fn unanswered(obligation: &Obligation) -> Discharge {
    Discharge::Faulted(Fault {
        bindings: Vec::new(),
        diagnostic: Box::new(
            Diagnostic::error(
                codes::INTERNAL_ERROR,
                format!(
                    "no evidence was read back or made for `{}`",
                    obligation.owner
                ),
            )
            .primary(obligation.span, "nothing was discharged for this claim")
            .note(
                "the program chose what the cache answered from the store this run reads; this is \
                 Ply's fault",
            ),
        ),
    })
}
