//! Obligations, and the tiers they are discharged at.

// `Value` pins `Arc` for its shared payloads, so none of them can ever be `Send`; that is
// `ply-eval`'s design and not something a value built here can change.
#![allow(clippy::arc_with_non_send_sync)]

#[cfg(test)]
mod numerics;

pub mod concurrency;
pub mod domain;
pub mod key;
pub mod property;
pub mod prove;
pub mod shrink;

use ply_core::{Footprint, LawBinder, Resource, Type};
use ply_eval::{Plan, Race, Seed};
use ply_hash::DefHash;
use ply_span::{Diagnostic, Span, Symbol};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::time::Duration;

/// Kept cases below which a run has concrete evidence and no coverage claim.
pub const MIN_PROPERTY_CASES: u32 = 25;

/// How deep a **non-recursive** call may be inlined by the prover.
pub const UNFOLD_DEPTH: u32 = 3;

/// The largest finite domain the prover will enumerate exhaustively.
pub const ENUMERATION_BOUND: u64 = 4096;

/// Past this generation depth only constructors with no recursive field are drawn, so generating a
/// value of a recursive type terminates.
pub const GEN_DEPTH: u32 = 4;

pub const DEFAULT_CASES: u32 = 200;
pub const DEFAULT_PROVE_BUDGET: u32 = 10_000;
pub const DEFAULT_SHRINK_BUDGET: u32 = 500;

/// The strength of the argument behind a held obligation.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// Concrete cases, and **no** coverage claim.
    Example,
    /// Randomized cases, the count reported, shrinking on failure.
    Property,
    /// A static argument covering **every** input satisfying the guard.
    Proved,
}

impl Tier {
    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Example => "example",
            Tier::Property => "property",
            Tier::Proved => "proved",
        }
    }
}

impl fmt::Display for Tier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An inference rule the prover is allowed to use.
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Rule {
    /// A closed pure Boolean term evaluated to `true`.
    GroundEvaluation,
    /// Every point of a finite domain of at most [`ENUMERATION_BOUND`] points evaluated.
    ExhaustiveEnumeration {
        domain: Symbol,
        points: u64,
    },
    /// `+`, `-`, unary `-`, multiplication by a literal, and the six comparisons.
    LinearArithmetic,
    /// `&&`, `||`, `!`, `if` at `Bool`, by case split.
    Propositional,
    /// A split on a scrutinee's outermost constructor.
    CaseSplit {
        ty: Symbol,
        arms: u32,
    },
    Congruence,
    /// `C(x̄) == C(ȳ) ⟺ x̄ == ȳ`, and `C(..) != D(..)` for `C ≠ D`.
    Injectivity,
    /// A **non-recursive** definition inlined.
    Unfold {
        def: Symbol,
        depth: u32,
    },
    /// The one certificate rule that comes from execution rather than from a static argument: M7's
    /// footprint-guided search emptied its frontier, so every interleaving ran.
    ExhaustiveInterleaving {
        interleavings: u32,
    },
}

impl Rule {
    /// Whether this rule's evidence came from running the program rather than from reasoning about
    /// it.
    pub fn is_execution(&self) -> bool {
        matches!(
            self,
            Rule::GroundEvaluation
                | Rule::ExhaustiveEnumeration { .. }
                | Rule::ExhaustiveInterleaving { .. }
        )
    }
}

/// Why an obligation is `proved`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct Certificate {
    /// In application order.
    pub rules: Vec<Rule>,
    pub steps: u32,
    /// The guard was shown to admit at least one value.
    pub guard_satisfiable: bool,
    /// Type variables the proof left as uninterpreted sorts, so the claim is genuinely polymorphic
    /// rather than a claim about one instantiation.
    pub sorts: Vec<Symbol>,
}

/// What a sampled run did.
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct CaseReport {
    pub generated: u32,
    /// Candidates that satisfied the guard and were evaluated.
    pub kept: u32,
    /// Candidates the guard rejected.
    pub rejected: u32,
    pub roots: Vec<u64>,
    /// Type variables monomorphised for generation, e.g. `a := Int`.
    pub instantiations: Vec<(Symbol, Type)>,
}

/// What is behind a held obligation.
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "snake_case", tag = "evidence")]
pub enum Evidence {
    Proof(Certificate),
    Cases(CaseReport),
}

impl Evidence {
    pub fn tier(&self) -> Tier {
        match self {
            Evidence::Proof(_) => Tier::Proved,
            Evidence::Cases(c) if c.kept >= MIN_PROPERTY_CASES => Tier::Property,
            Evidence::Cases(_) => Tier::Example,
        }
    }
}

/// One binding of a counterexample, rendered.
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct Binding {
    pub name: Symbol,
    pub ty: Type,
    pub rendered: String,
}

/// A falsifying input, after shrinking.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Counterexample {
    /// The shrunk bindings.
    pub bindings: Vec<Binding>,
    pub original: Vec<Binding>,
    pub shrinks: u32,
    pub root: u64,
    pub case: u32,
    /// For a concurrency law: the two steps whose reordering flipped it.
    pub race: Option<Race>,
    pub sim_seed: Option<Seed>,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum VacuityKind {
    /// The prover showed the guard unsatisfiable within the fragment.
    ProvedUnsatisfiable,
    /// A full case budget kept nothing.
    NoCaseKept { generated: u32 },
}

/// The guard admitted no values, so the obligation is trivially valid and says nothing.
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct Vacuity {
    pub guard: Span,
    pub kind: VacuityKind,
}

/// Why the system could not decide an obligation at any tier.
#[derive(Clone, Debug)]
pub enum Gap {
    /// Checking an `ensures` means calling the definition, and its footprint needs a handler
    /// nothing supplies.
    UnhandledEffect(Footprint),
    /// A parameter of a type the generator cannot inhabit.
    Ungeneratable { param: Symbol, ty: Type },
    /// Evaluating a case raised: a runtime error, a division by zero, the recursion limit.
    Raised {
        bindings: Vec<Binding>,
        diagnostic: Diagnostic,
    },
    /// The guard kept none of a full case budget, and the guard **does** admit a value — one is
    /// carried here, found by evaluating the guard at the points its own literals name.
    GuardNotSampled {
        generated: u32,
        witness: Vec<Binding>,
    },
    /// A `law/host` under a hermetic run.
    ReachesHost(Footprint),
}

/// What became of one obligation.
#[derive(Clone, Debug)]
pub enum Discharge {
    Held(Evidence),
    Refuted(Counterexample),
    Vacuous(Vacuity),
    Unattempted(Gap),
}

impl Discharge {
    /// `None` for everything that is not held.
    pub fn tier(&self) -> Option<Tier> {
        match self {
            Discharge::Held(e) => Some(e.tier()),
            _ => None,
        }
    }

    pub fn holds(&self) -> bool {
        matches!(self, Discharge::Held(_))
    }

    /// Whether this result may be written to the obligation cache at all.
    pub fn is_cacheable(&self) -> bool {
        self.holds()
    }

    /// Whether this result is a claim about **every** plan rather than about the one that produced
    /// it.
    pub fn is_plan_independent(&self) -> bool {
        self.tier() == Some(Tier::Proved)
    }
}

/// What a definition leaves alone, from the footprint the effect system already inferred and
/// already checked as an upper bound.
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "snake_case", tag = "frame")]
pub enum Frame {
    /// The footprint is empty, so the result is a function of the arguments and the `ensures` is a
    /// total specification of the definition.
    Pure,
    /// Every resource outside this set is unchanged.
    Writes(BTreeSet<(Symbol, Resource)>),
}

/// A read changes nothing, so it does not narrow a frame.
pub fn frame_of(footprint: &Footprint) -> Frame {
    let writes: BTreeSet<(Symbol, Resource)> = footprint
        .atoms()
        .filter(|a| a.mode == ply_syntax::ast::Mode::Write)
        .map(|a| (a.effect.clone(), a.resource.clone()))
        .collect();
    if writes.is_empty() {
        Frame::Pure
    } else {
        Frame::Writes(writes)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ObligationKind {
    /// A postcondition.
    Ensures {
        index: usize,
    },
    Law,
}

/// One claim to discharge.
#[derive(Clone, Debug)]
pub struct Obligation {
    /// `spec_hash` for a clause, the law's own `DefHash` for a law.
    pub key: DefHash,
    /// `<module>.<def>` for a clause, `<module>.<label>` for a law.
    pub owner: Symbol,
    pub kind: ObligationKind,
    pub span: Span,
    pub frame: Frame,
    /// The owner's parameters **then** `result` for a clause; the `forall` binders for a law.
    pub binders: Vec<LawBinder>,
    /// Whether a `requires` or a `where` narrows the domain.
    pub guarded: bool,
    /// `law/host`: the body reaches the world, and the law says so in its own declaration.
    pub host: bool,
    /// `{}`, or `{sim.read}` for a concurrency law, or any row at all for a `law/host`.
    pub footprint: Footprint,
}

impl Obligation {
    /// A law whose body reaches a `simulate` region.
    pub fn is_concurrency_law(&self) -> bool {
        matches!(self.kind, ObligationKind::Law) && !self.host && !self.footprint.is_empty()
    }

    /// The binders a run **draws values for**.
    pub fn generated(&self) -> &[LawBinder] {
        match self.kind {
            ObligationKind::Ensures { .. } => &self.binders[..self.binders.len().saturating_sub(1)],
            ObligationKind::Law => &self.binders,
        }
    }

    /// The binder standing for the return value, for the same obligations [`Obligation::generated`]
    /// withholds it from.
    pub fn result_binder(&self) -> Option<&LawBinder> {
        match self.kind {
            ObligationKind::Ensures { .. } => self.binders.last(),
            ObligationKind::Law => None,
        }
    }
}

/// The review surface, reported ahead of the results and never behind a flag.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Coverage {
    pub definitions: usize,
    /// Carries an `ensures` that holds, or is named **directly** by a law that holds.
    pub covered: usize,
    /// Program-wide names, sorted, so two runs produce one artifact.
    pub uncovered: Vec<Symbol>,
    pub by_tier: BTreeMap<Tier, usize>,
}

impl Coverage {
    pub fn uncovered_count(&self) -> usize {
        self.definitions.saturating_sub(self.covered)
    }
}

/// The search an obligation was discharged against.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ProvePlan {
    /// Candidate binder tuples drawn per root.
    pub cases: u32,
    /// Ascending and deduplicated by [`ProvePlan::normalized`].
    pub roots: Vec<u64>,
    /// Static inference steps per obligation.
    pub prove_budget: u32,
    /// Candidate **evaluations**, never seconds: an artifact that varies with machine load cannot
    /// be diffed against yesterday's.
    pub shrink_budget: u32,
    /// The interleaving search a concurrency law is discharged against.
    pub sim: Plan,
}

impl Default for ProvePlan {
    fn default() -> ProvePlan {
        ProvePlan {
            cases: DEFAULT_CASES,
            roots: vec![0],
            prove_budget: DEFAULT_PROVE_BUDGET,
            shrink_budget: DEFAULT_SHRINK_BUDGET,
            sim: Plan::default(),
        }
    }
}

impl ProvePlan {
    /// Ascending, deduplicated roots, so that two spellings of one plan are one cache key.
    pub fn normalized(mut self) -> ProvePlan {
        self.roots.sort_unstable();
        self.roots.dedup();
        self.sim = self.sim.normalized();
        self
    }

    pub fn digest(&self) -> [u8; 32] {
        let plan = self.clone().normalized();
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"ply.prove.plan.1");
        hasher.update(&plan.cases.to_le_bytes());
        hasher.update(&plan.prove_budget.to_le_bytes());
        hasher.update(&(plan.roots.len() as u32).to_le_bytes());
        for root in &plan.roots {
            hasher.update(&root.to_le_bytes());
        }
        hasher.update(&plan.sim.digest());
        *hasher.finalize().as_bytes()
    }
}

/// What `ply prove` produces.
#[derive(Clone, Debug)]
pub struct ProveReport {
    pub obligations: Vec<(Obligation, Discharge)>,
    pub coverage: Coverage,
    pub plan: ProvePlan,
    /// Answered from the cache without being attempted.
    pub cached: usize,
    pub duration: Duration,
}

impl ProveReport {
    pub fn count(&self, tier: Tier) -> usize {
        self.obligations
            .iter()
            .filter(|(_, d)| d.tier() == Some(tier))
            .count()
    }

    pub fn refuted(&self) -> usize {
        self.obligations
            .iter()
            .filter(|(_, d)| matches!(d, Discharge::Refuted(_)))
            .count()
    }

    pub fn vacuous(&self) -> usize {
        self.obligations
            .iter()
            .filter(|(_, d)| matches!(d, Discharge::Vacuous(_)))
            .count()
    }

    pub fn unattempted(&self) -> usize {
        self.obligations
            .iter()
            .filter(|(_, d)| matches!(d, Discharge::Unattempted(_)))
            .count()
    }

    /// Exit `1` on anything the spec got wrong.
    pub fn failed(&self) -> bool {
        self.refuted() > 0 || self.vacuous() > 0
    }
}

/// Whether an exhaustive interleaving search may be reported as a **proof**.
pub fn interleaving_proves(
    plan: &Plan,
    exploration: &ply_eval::Exploration,
    domain_enumerated: bool,
) -> bool {
    plan.mode == ply_eval::SimMode::Dpor
        && exploration.exhaustive
        && !exploration.exhausted
        && exploration.failure.is_none()
        && domain_enumerated
}

#[cfg(test)]
mod tests {
    use super::*;
    use ply_core::{EffectAtom, Resource};
    use ply_eval::{Exploration, SimMode};
    use ply_syntax::ast::Mode;

    fn cases(kept: u32) -> Evidence {
        Evidence::Cases(CaseReport {
            generated: 200,
            kept,
            rejected: 200 - kept,
            roots: vec![0],
            instantiations: Vec::new(),
        })
    }

    fn certificate() -> Certificate {
        Certificate {
            rules: vec![Rule::LinearArithmetic],
            steps: 41,
            guard_satisfiable: true,
            sorts: Vec::new(),
        }
    }

    /// The rule the whole module exists to make unavailable to get wrong: a tier is computed from
    /// the evidence, and only a certificate computes to `Proved`.
    #[test]
    fn only_a_certificate_yields_proved() {
        assert_eq!(Evidence::Proof(certificate()).tier(), Tier::Proved);
        for kept in [0, 1, 24, 25, 200] {
            assert_ne!(cases(kept).tier(), Tier::Proved);
        }
    }

    #[test]
    fn the_kept_count_alone_separates_property_from_example() {
        assert_eq!(cases(MIN_PROPERTY_CASES).tier(), Tier::Property);
        assert_eq!(cases(MIN_PROPERTY_CASES - 1).tier(), Tier::Example);
        assert_eq!(cases(0).tier(), Tier::Example);
    }

    /// `min` on two tiers is "report the weaker", so the ordering has to be the strength ordering
    /// and not the declaration order it happens to share.
    #[test]
    fn tiers_order_by_strength() {
        assert!(Tier::Example < Tier::Property);
        assert!(Tier::Property < Tier::Proved);
        assert_eq!(Tier::Proved.min(Tier::Example), Tier::Example);
    }

    #[test]
    fn nothing_but_a_hold_has_a_tier_or_is_cached() {
        let outcomes = [
            Discharge::Refuted(Counterexample {
                bindings: Vec::new(),
                original: Vec::new(),
                shrinks: 0,
                root: 0,
                case: 0,
                race: None,
                sim_seed: None,
            }),
            Discharge::Vacuous(Vacuity {
                guard: Span::DUMMY,
                kind: VacuityKind::ProvedUnsatisfiable,
            }),
            Discharge::Unattempted(Gap::UnhandledEffect(Footprint::empty())),
        ];
        for outcome in outcomes {
            assert_eq!(outcome.tier(), None);
            assert!(!outcome.holds());
            assert!(!outcome.is_cacheable());
            assert!(!outcome.is_plan_independent());
        }
    }

    /// The asymmetry the whole operational value of `proved` rests on: a proof is valid under every
    /// plan, a sample is a claim about the plan that took it.
    #[test]
    fn only_a_proof_is_plan_independent() {
        assert!(Discharge::Held(Evidence::Proof(certificate())).is_plan_independent());
        assert!(!Discharge::Held(cases(200)).is_plan_independent());
        assert!(!Discharge::Held(cases(3)).is_plan_independent());
    }

    #[test]
    fn a_frame_names_the_writes_and_nothing_else() {
        assert_eq!(frame_of(&Footprint::empty()), Frame::Pure);

        let read = EffectAtom::new("db", Resource::Named("users".into()), Mode::Read);
        assert_eq!(
            frame_of(&Footprint::from_atoms([read.clone()])),
            Frame::Pure,
            "a read changes nothing, so it does not narrow a frame"
        );

        let write = EffectAtom::new("db", Resource::Named("orders".into()), Mode::Write);
        let Frame::Writes(writes) = frame_of(&Footprint::from_atoms([read, write])) else {
            panic!("a write must produce a frame");
        };
        assert_eq!(writes.len(), 1);
        assert!(writes.contains(&("db".into(), Resource::Named("orders".into()))));
    }

    fn exploration(exhaustive: bool, exhausted: bool) -> Exploration {
        Exploration {
            explored: 12,
            exhaustive,
            exhausted,
            naive: None,
            steps: 40,
            virtual_time: 0,
            failure: None,
            race: None,
        }
    }

    #[test]
    fn an_exhaustive_search_proves_only_when_the_value_domain_was_covered_too() {
        let plan = Plan::default();
        assert_eq!(plan.mode, SimMode::Dpor);
        assert!(interleaving_proves(&plan, &exploration(true, false), true));
        // The condition an implementer drops: exhaustive over *schedules* says nothing about the
        // values that were sampled.
        assert!(!interleaving_proves(
            &plan,
            &exploration(true, false),
            false
        ));
    }

    #[test]
    fn a_sampled_or_spent_search_never_proves() {
        let exhaustive = exploration(true, false);
        assert!(!interleaving_proves(&Plan::random(4), &exhaustive, true));
        assert!(!interleaving_proves(
            &Plan::once(Seed::root(7)),
            &exhaustive,
            true
        ));
        assert!(!interleaving_proves(
            &Plan::default(),
            &exploration(false, true),
            true
        ));
    }

    #[test]
    fn a_failing_search_never_proves() {
        let mut failed = exploration(true, false);
        failed.failure = Some(Seed::root(3));
        assert!(!interleaving_proves(&Plan::default(), &failed, true));
    }

    #[test]
    fn a_plan_digest_ignores_the_shrink_budget_and_root_spelling() {
        let base = ProvePlan::default();
        let looser = ProvePlan {
            shrink_budget: base.shrink_budget * 4,
            ..base.clone()
        };
        assert_eq!(base.digest(), looser.digest());

        let respelled = ProvePlan {
            roots: vec![2, 0, 2, 1],
            ..base.clone()
        };
        let sorted = ProvePlan {
            roots: vec![0, 1, 2],
            ..base.clone()
        };
        assert_eq!(respelled.digest(), sorted.digest());
        assert_ne!(base.digest(), sorted.digest());

        let wider = ProvePlan {
            cases: base.cases * 2,
            ..base.clone()
        };
        assert_ne!(base.digest(), wider.digest());

        let deeper = ProvePlan {
            prove_budget: base.prove_budget * 2,
            ..base.clone()
        };
        assert_ne!(base.digest(), deeper.digest());
    }

    #[test]
    fn a_report_fails_on_a_refutation_or_a_vacuity_and_not_on_a_gap() {
        let obligation = Obligation {
            key: DefHash([0; 32]),
            owner: "m.f".into(),
            kind: ObligationKind::Ensures { index: 0 },
            span: Span::DUMMY,
            frame: Frame::Pure,
            binders: Vec::new(),
            guarded: false,
            host: false,
            footprint: Footprint::empty(),
        };
        let report = |discharge| ProveReport {
            obligations: vec![(obligation.clone(), discharge)],
            coverage: Coverage::default(),
            plan: ProvePlan::default(),
            cached: 0,
            duration: Duration::ZERO,
        };
        assert!(
            !report(Discharge::Unattempted(Gap::UnhandledEffect(
                Footprint::empty()
            )))
            .failed()
        );
        assert!(
            report(Discharge::Vacuous(Vacuity {
                guard: Span::DUMMY,
                kind: VacuityKind::NoCaseKept { generated: 200 },
            }))
            .failed()
        );
        assert!(!report(Discharge::Held(cases(200))).failed());
    }
}
