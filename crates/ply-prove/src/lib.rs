//! Obligations, and the tiers they are discharged at.
//!
//! **A tier label is a truth claim.** Every prior milestone could produce a
//! wrong answer; only this one can produce a wrong answer wearing a
//! certificate, and a reviewer told that an obligation is `proved` stops
//! reading — which is the whole point of telling them. So when in doubt, report
//! the weaker tier.
//!
//! That rule is enforced by the shapes in this module rather than by
//! convention. There is no `tier` field anywhere: [`Evidence::tier`] computes
//! one, and the only evidence that computes to [`Tier::Proved`] is a
//! [`Certificate`], which names every inference rule it used. A component that
//! wants to report `proved` has to produce a proof; there is no other spelling.
//!
//! `docs/adr/0007-specs.md` is the specification.

// `Value` pins `Arc` for its shared payloads, so none of them can ever be
// `Send`; that is `ply-eval`'s design and not something a value built here can
// change.
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
///
/// A constant rather than a fraction of [`ProvePlan::cases`], so a run at
/// `--prove-cases 5` can only ever produce [`Tier::Example`]. That is correct:
/// you asked for fewer cases than a property claim needs.
pub const MIN_PROPERTY_CASES: u32 = 25;

/// How deep a **non-recursive** call may be inlined by the prover. A member of a
/// recursive SCC is never unfolded at all, which is the boundary where induction
/// would be needed and is not available.
pub const UNFOLD_DEPTH: u32 = 3;

/// The largest finite domain the prover will enumerate exhaustively. Beyond it
/// the obligation is sampled.
pub const ENUMERATION_BOUND: u64 = 4096;

/// Past this generation depth only constructors with no recursive field are
/// drawn, so generating a value of a recursive type terminates.
pub const GEN_DEPTH: u32 = 4;

pub const DEFAULT_CASES: u32 = 200;
pub const DEFAULT_PROVE_BUDGET: u32 = 10_000;
pub const DEFAULT_SHRINK_BUDGET: u32 = 500;

/// The strength of the argument behind a held obligation.
///
/// `Ord` is the strength order, so `Example < Property < Proved` and reporting
/// the weaker tier is `min`.
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
///
/// A **closed** enum, deliberately: a prover that grows a rule nobody
/// sanctioned stops compiling before it fails the certificate audit. Every
/// variant here is a member of the fragment in ADR 0007 §5.1, plus the one that
/// comes from execution.
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Rule {
    /// A closed pure Boolean term evaluated to `true`. A decision procedure for
    /// a domain of one point, and therefore a proof.
    GroundEvaluation,
    /// Every point of a finite domain of at most [`ENUMERATION_BOUND`] points
    /// evaluated.
    ExhaustiveEnumeration {
        domain: Symbol,
        points: u64,
    },
    /// `+`, `-`, unary `-`, multiplication by a literal, and the six
    /// comparisons. `x * y` with both symbolic, `/` and `%` are **not** in it.
    LinearArithmetic,
    /// `&&`, `||`, `!`, `if` at `Bool`, by case split.
    Propositional,
    /// A split on a scrutinee's outermost constructor. Exhaustive for recursive
    /// types too, because the split is over the constructor set rather than the
    /// value space — which is exactly why it reaches depth 1 and no further.
    CaseSplit {
        ty: Symbol,
        arms: u32,
    },
    Congruence,
    /// `C(x̄) == C(ȳ) ⟺ x̄ == ȳ`, and `C(..) != D(..)` for `C ≠ D`.
    Injectivity,
    /// A **non-recursive** definition inlined. `depth` is at most
    /// [`UNFOLD_DEPTH`].
    Unfold {
        def: Symbol,
        depth: u32,
    },
    /// The one certificate rule that comes from execution rather than from a
    /// static argument: M7's footprint-guided search emptied its frontier, so
    /// every interleaving ran. Distinct from the rest so that an audit can find
    /// every execution-derived proof and re-check it against ADR 0007 §6's five
    /// conditions.
    ExhaustiveInterleaving {
        interleavings: u32,
    },
}

impl Rule {
    /// Whether this rule's evidence came from running the program rather than
    /// from reasoning about it. The audit partitions certificates on it.
    pub fn is_execution(&self) -> bool {
        matches!(
            self,
            Rule::GroundEvaluation
                | Rule::ExhaustiveEnumeration { .. }
                | Rule::ExhaustiveInterleaving { .. }
        )
    }
}

/// Why an obligation is `proved`. Only the prover constructs one, and holding
/// one is the only way to report [`Tier::Proved`].
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct Certificate {
    /// In application order. The audit asserts every entry is a fragment rule.
    pub rules: Vec<Rule>,
    pub steps: u32,
    /// The guard was shown to admit at least one value. Required rather than
    /// optional: `guard ⟹ body` over an empty domain is valid and says nothing,
    /// so a certificate that did not establish it has a domain it cannot vouch
    /// for. Always `true` on a [`Discharge::Held`]; a `false` is the
    /// [`Discharge::Vacuous`] path.
    pub guard_satisfiable: bool,
    /// Type variables the proof left as uninterpreted sorts, so the claim is
    /// genuinely polymorphic rather than a claim about one instantiation.
    pub sorts: Vec<Symbol>,
}

/// What a sampled run did. The tier follows from `kept`, not from an opinion
/// about it.
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct CaseReport {
    pub generated: u32,
    /// Candidates that satisfied the guard and were evaluated.
    pub kept: u32,
    /// Candidates the guard rejected. A large number beside a small `kept` is
    /// why this obligation reports `example`.
    pub rejected: u32,
    pub roots: Vec<u64>,
    /// Type variables monomorphised for generation, e.g. `a := Int`. A sampled
    /// polymorphic law is a claim about the instantiation and says so.
    pub instantiations: Vec<(Symbol, Type)>,
}

/// What is behind a held obligation.
///
/// Two variants, not three: `property` and `example` are the same run reported
/// honestly at two strengths, and making the tier a function of `kept` is what
/// stops the strength from being a separate, forgeable field.
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
///
/// Both halves are reported: a minimal value alone does not say the space was
/// searched, and "shrank from a list of 400 to `[0, 1]` in 11 steps" does.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Counterexample {
    /// The shrunk bindings. Every candidate accepted on the way here still
    /// falsified the obligation **and** still satisfied the guard.
    pub bindings: Vec<Binding>,
    pub original: Vec<Binding>,
    pub shrinks: u32,
    pub root: u64,
    pub case: u32,
    /// For a concurrency law: the two steps whose reordering flipped it.
    /// `Some` only when the search observed the flip — never inferred.
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

/// The guard admitted no values, so the obligation is trivially valid and says
/// nothing. Always a defect in the spec: reporting it `proved` would turn a typo
/// in a guard into a proof of everything.
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct Vacuity {
    pub guard: Span,
    pub kind: VacuityKind,
}

/// Why the system could not decide an obligation at any tier.
///
/// A gap, reported as one. It is not a weak tier, it does not fail the run, and
/// it leaves its definition **uncovered** — a definition whose only obligation
/// is undischargeable is a definition a reviewer still has to read.
#[derive(Clone, Debug)]
pub enum Gap {
    /// Checking an `ensures` means calling the definition, and its footprint
    /// needs a handler nothing supplies. Inventing one would be inventing a
    /// behaviour and then testing against it.
    UnhandledEffect(Footprint),
    /// A parameter of a type the generator cannot inhabit. A gap rather than the
    /// compile error a `forall` binder gets, because forbidding it would forbid
    /// attaching a spec to a higher-order definition.
    Ungeneratable { param: Symbol, ty: Type },
    /// Evaluating a case raised: a runtime error, a division by zero, the
    /// recursion limit. A spec that raises is not false, so this is neither a
    /// refutation nor a hold. The raising input is shrunk all the same.
    Raised {
        bindings: Vec<Binding>,
        diagnostic: Diagnostic,
    },
    /// The guard kept none of a full case budget, and the guard **does** admit a
    /// value — one is carried here, found by evaluating the guard at the points
    /// its own literals name.
    ///
    /// The difference between this and [`Discharge::Vacuous`] is the difference
    /// between "the search missed the domain" and "there is no domain", and only
    /// the second is a defect in the spec. `where x > 1000000 && x < 1000010`
    /// admits nine integers that 200 draws from the whole of `i64` will never
    /// hit; calling that `E0420 the guard admits no value` states something
    /// false about the program and fails the build on it.
    GuardNotSampled {
        generated: u32,
        witness: Vec<Binding>,
    },
    /// A `law/host` under a hermetic run.
    ///
    /// Reported rather than skipped, and never green: a law about a database
    /// that never ran a database, reported as passing, is precisely the "green
    /// result over unexplored space" this project audits for.
    ReachesHost(Footprint),
}

/// What became of one obligation.
/// Deliberately not `Serialize`: what the store persists is an [`Evidence`],
/// which has no variant for a refutation, so a cache that held one would not
/// type-check. `ply-cli` projects the rest into the JSON artifact, exactly as it
/// projects `ply_test::Failure`.
#[derive(Clone, Debug)]
pub enum Discharge {
    Held(Evidence),
    Refuted(Counterexample),
    Vacuous(Vacuity),
    Unattempted(Gap),
}

impl Discharge {
    /// `None` for everything that is not held. There is no tier for a refutation,
    /// a vacuity or a gap, and inventing one would be the whole defect this
    /// module exists to prevent.
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
    /// `Refuted` and `Vacuous` are errors and re-run until they go green;
    /// `Unattempted` is not a result.
    pub fn is_cacheable(&self) -> bool {
        self.holds()
    }

    /// Whether this result is a claim about **every** plan rather than about the
    /// one that produced it. True exactly of a proof, which is why a proved
    /// obligation costs nothing forever and a sampled one is re-run when the
    /// plan widens.
    pub fn is_plan_independent(&self) -> bool {
        self.tier() == Some(Tier::Proved)
    }
}

/// What a definition leaves alone, from the footprint the effect system already
/// inferred and already checked as an upper bound.
///
/// This is the frame condition Dafny and Why3 make users write as a `modifies`
/// clause and then prove. It is not an obligation here: it is evidence in hand
/// before any obligation exists.
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "snake_case", tag = "frame")]
pub enum Frame {
    /// The footprint is empty, so the result is a function of the arguments and
    /// the `ensures` is a total specification of the definition.
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
    /// A postcondition. Each clause is its own obligation, so a definition whose
    /// first is proved and whose second is sampled is told both.
    Ensures {
        index: usize,
    },
    Law,
}

/// One claim to discharge.
///
/// A `requires` clause is **not** one: it is a filter on the domain of the
/// `ensures` clauses beside it, not a contract checked at every call.
#[derive(Clone, Debug)]
pub struct Obligation {
    /// `spec_hash` for a clause, the law's own `DefHash` for a law. Covers the
    /// owner's hash, and therefore the owner's whole closure.
    pub key: DefHash,
    /// `<module>.<def>` for a clause, `<module>.<label>` for a law.
    pub owner: Symbol,
    pub kind: ObligationKind,
    pub span: Span,
    pub frame: Frame,
    /// The owner's parameters **then** `result` for a clause; the `forall`
    /// binders for a law. Empty means a ground claim, decided by evaluating it.
    ///
    /// `result` is last and is part of this list because the prover needs a
    /// symbolic constant for it. A sampled run does not draw one — see
    /// [`Obligation::generated`].
    pub binders: Vec<LawBinder>,
    /// Whether a `requires` or a `where` narrows the domain.
    pub guarded: bool,
    /// `law/host`: the body reaches the world, and the law says so in its own
    /// declaration.
    ///
    /// Three things follow and each is enforced separately: it can never be
    /// `proved` (the static tier and the finite enumeration are both skipped,
    /// because either would be a claim about every value rather than about the
    /// ones that ran), it is never read from or written to the obligation cache,
    /// and under a hermetic run it is [`Gap::ReachesHost`] rather than green.
    pub host: bool,
    /// `{}`, or `{sim.read}` for a concurrency law, or any row at all for a
    /// `law/host`.
    pub footprint: Footprint,
}

impl Obligation {
    /// A law whose body reaches a `simulate` region. Discharged by execution,
    /// and the only place a `proved` in this milestone does not come from a
    /// static argument.
    pub fn is_concurrency_law(&self) -> bool {
        matches!(self.kind, ObligationKind::Law) && !self.host && !self.footprint.is_empty()
    }

    /// The binders a run **draws values for**.
    ///
    /// Every binder for a law; the parameters but not `result` for an
    /// `ensures`, because `result` is what calling the definition produces. A
    /// run that generated one would be checking a postcondition against a value
    /// the definition never returned, which is not a claim about the definition
    /// at all.
    pub fn generated(&self) -> &[LawBinder] {
        match self.kind {
            ObligationKind::Ensures { .. } => &self.binders[..self.binders.len().saturating_sub(1)],
            ObligationKind::Law => &self.binders,
        }
    }

    /// The binder standing for the return value, for the same obligations
    /// [`Obligation::generated`] withholds it from.
    pub fn result_binder(&self) -> Option<&LawBinder> {
        match self.kind {
            ObligationKind::Ensures { .. } => self.binders.last(),
            ObligationKind::Law => None,
        }
    }
}

/// The review surface, reported ahead of the results and never behind a flag.
///
/// The count of definitions carrying no obligation is exactly where review still
/// costs what it costs today. `uncovered` is a list of names rather than only a
/// number because a number is something to feel bad about and a list is
/// something to work through.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Coverage {
    pub definitions: usize,
    /// Carries an `ensures` that holds, or is named **directly** by a law that
    /// holds. `requires` alone does not cover; a refuted, vacuous or unattempted
    /// obligation covers nothing; reachability is not coverage.
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

/// The search an obligation was discharged against. Part of the cache key of
/// everything weaker than a proof.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ProvePlan {
    /// Candidate binder tuples drawn per root.
    pub cases: u32,
    /// Ascending and deduplicated by [`ProvePlan::normalized`].
    pub roots: Vec<u64>,
    /// Static inference steps per obligation. A spent budget is inconclusive,
    /// which reports `property` — never `proved` and never `refuted`.
    pub prove_budget: u32,
    /// Candidate **evaluations**, never seconds: an artifact that varies with
    /// machine load cannot be diffed against yesterday's. Deliberately absent
    /// from [`ProvePlan::digest`] — it can only change a counterexample's
    /// minimality, and failures are never cached.
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
    /// Ascending, deduplicated roots, so that two spellings of one plan are one
    /// cache key.
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

/// What `ply prove` produces. The order of `obligations` is a property of the
/// program — file order, then source order — so two runs produce one artifact.
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

    /// Exit `1` on anything the spec got wrong. An `Unattempted` obligation is a
    /// reported gap and not a failure — making it one would mean a spec could
    /// never be attached to an effectful definition.
    pub fn failed(&self) -> bool {
        self.refuted() > 0 || self.vacuous() > 0
    }
}

/// Whether an exhaustive interleaving search may be reported as a **proof**.
///
/// All five of ADR 0007 §6's conditions, in one place, because the fifth is the
/// one an implementer drops and dropping it is this milestone's worst available
/// defect: `exhaustive: true` is a claim about schedules, and a law over
/// `n: Int` ranges over 2⁶⁴ values. The two coverage claims are independent.
///
/// **Five conditions are not enough, and this signature cannot express the
/// sixth.** An [`Exploration`] over a body that entered no `simulate` region
/// reports `exhaustive: true` — the frontier it emptied was never filled — and
/// every condition below passes on it. Whether a region ran is a fact about the
/// machine rather than about the exploration, so it cannot be read here.
/// [`concurrency::discharge`] is the only correct caller: it asks the machine,
/// and `concurrency::a_search_that_reached_no_region_is_exhaustive_over_nothing`
/// is the test that this function alone would have said yes.
///
/// [`Exploration`]: ply_eval::Exploration
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

    /// The rule the whole module exists to make unavailable to get wrong: a
    /// tier is computed from the evidence, and only a certificate computes to
    /// `Proved`.
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

    /// `min` on two tiers is "report the weaker", so the ordering has to be the
    /// strength ordering and not the declaration order it happens to share.
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

    /// The asymmetry the whole operational value of `proved` rests on: a proof
    /// is valid under every plan, a sample is a claim about the plan that took
    /// it.
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
        // The condition an implementer drops: exhaustive over *schedules* says
        // nothing about the values that were sampled.
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
