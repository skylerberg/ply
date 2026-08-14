//! Selecting, caching and reporting obligations.
//!
//! This is the result cache's rule with one word changed, and the change is the
//! whole operational value of `proved`: **a proof is not a search**. A sampled
//! discharge is a claim about the plan that sampled it, so it is written under
//! `prove_key(key, plan)` and re-runs when the plan widens; a proof is a claim
//! about every input satisfying the guard, so it is written under the bare
//! obligation key and a wider search never re-examines it.
//!
//! Getting that wrong in one direction re-discharges forever, which costs time.
//! Getting it wrong in the other caches a proof of something that is no longer
//! true — a claim wearing a certificate — which is the one defect this milestone
//! cannot ship. So every ambiguity here resolves toward re-discharging:
//!
//! - a `Cases` found under the bare key is **refused**, because only a proof
//!   belongs there and reading it would let `--prove-cases 10` satisfy a run
//!   that asked for a thousand;
//! - a `Proof` found under a plan key is **refused**, because a proof written
//!   there was written by something that did not believe the rule above;
//! - an entry whose recorded tier disagrees with the tier its evidence computes
//!   to is **refused**, because a cached `proved` and a cached `property` are
//!   different claims and a file that cannot keep them apart is not evidence of
//!   either.
//!
//! Each refusal is a warning and a re-discharge, never a silent read.

use ply_core::CheckOutput;
use ply_hash::{DefHash, HashOutput};
use ply_prove::key::{prove_key, result_key};
use ply_prove::{
    CaseReport, Certificate, Coverage, Discharge, Evidence, Obligation, ObligationKind, ProvePlan,
    ProveReport, Rule, Tier,
};
use ply_span::{Diagnostic, Symbol, codes};
use ply_store::{
    CachedCases, CachedCertificate, CachedEvidence, CachedObligation, CachedRule, ReviewRecord,
    Store,
};
use rayon::prelude::*;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

/// What each law hashes to, and which definitions it speaks about.
///
/// **Directly**, never transitively: a law over `credited` and `debited` covers
/// those two, and taking the closure would let one law over one hub definition
/// claim the whole program — the shape every coverage metric fails in.
#[derive(Clone, Debug, Default)]
pub struct Laws {
    targets: BTreeMap<Symbol, BTreeSet<Symbol>>,
    hashes: BTreeMap<Symbol, DefHash>,
    /// The law as a *sentence*, for the review baseline only. A law's `hashes`
    /// entry substitutes the hash of every definition it names, so it moves
    /// whenever any of them is re-implemented — which is right for re-opening
    /// the obligation and wrong for asking whether the claim was rewritten.
    texts: BTreeMap<Symbol, DefHash>,
}

impl Laws {
    /// Every law the program declares, with the definitions it names.
    ///
    /// A law with no entry in [`HashOutput::deps`] contributes **nothing**: it
    /// covers no definition and constrains no spec. That is the direction to
    /// fail in — an under-counted coverage number asks for more review than is
    /// strictly owed, where an over-counted one invites a reader to stop.
    pub fn of(check: &CheckOutput, hashes: &HashOutput) -> Laws {
        let mut laws = Laws::default();
        for law in &check.laws {
            let Some(&hash) = hashes.laws.get(law.index) else {
                continue;
            };
            let text = hashes.law_texts.get(law.index).copied().unwrap_or(hash);
            let targets = hashes
                .deps
                .get(&law.key)
                .into_iter()
                .flatten()
                .filter(|name| check.defs.contains_key(*name))
                .cloned();
            laws.insert(law.key.clone(), hash, text, targets);
        }
        laws
    }

    pub fn insert(
        &mut self,
        key: Symbol,
        hash: DefHash,
        text: DefHash,
        targets: impl IntoIterator<Item = Symbol>,
    ) {
        self.targets.entry(key.clone()).or_default().extend(targets);
        self.hashes.insert(key.clone(), hash);
        self.texts.insert(key, text);
    }

    pub fn targets(&self, law: &Symbol) -> Option<&BTreeSet<Symbol>> {
        self.targets.get(law)
    }

    /// The *sentence* of every law naming this definition, so that rewriting a
    /// law reads as a spec change on everything it constrains — and
    /// re-implementing one of those definitions does not.
    pub fn naming(&self, name: &Symbol) -> Vec<DefHash> {
        self.targets
            .iter()
            .filter(|(_, targets)| targets.contains(name))
            .filter_map(|(law, _)| self.texts.get(law).copied())
            .collect()
    }

    pub fn len(&self) -> usize {
        self.hashes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.hashes.is_empty()
    }
}

// --- The cache --------------------------------------------------------------

/// Why an obligation was or was not attempted.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Reason {
    /// Nothing is recorded under any key this run may read.
    New,
    /// A proof, read under the bare key. Valid under every plan, so widening the
    /// search does not re-examine it — the only thing in this system a wider
    /// search costs nothing.
    Proved,
    /// A sampled discharge, read under this exact plan's key.
    Sampled,
    /// The cache was not consulted: `--no-cache`.
    Uncached,
    /// Something was recorded under a key it does not belong under, or under a
    /// label its evidence does not support. Attempted again.
    Refused,
}

impl Reason {
    /// Whether the answer came from the cache rather than from work.
    pub fn hit(self) -> bool {
        matches!(self, Reason::Proved | Reason::Sampled)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Reason::New => "new",
            Reason::Proved => "cached proof",
            Reason::Sampled => "cached sample",
            Reason::Uncached => "uncached",
            Reason::Refused => "cache refused",
        }
    }
}

/// What the cache had to say about one obligation.
pub struct Answer {
    pub reason: Reason,
    pub evidence: Option<Evidence>,
    /// A refusal, reported rather than swallowed: a cache that quietly declines
    /// to answer looks exactly like a prover that is slow for no reason.
    pub warning: Option<Diagnostic>,
}

impl Answer {
    fn miss(reason: Reason) -> Answer {
        Answer {
            reason,
            evidence: None,
            warning: None,
        }
    }

    fn refused(key: DefHash, why: &str) -> Answer {
        Answer {
            reason: Reason::Refused,
            evidence: None,
            warning: Some(
                Diagnostic::warning(
                    codes::CACHE_CORRUPT,
                    format!(
                        "the obligation cache holds {why} for `{}`; it was discarded",
                        key.short()
                    ),
                )
                .note("the obligation was discharged again, so no claim rests on it")
                .note("run `ply cache clear` if it happens again"),
            ),
        }
    }
}

/// What the cache holds for one obligation, under the two keys ADR 0007 §4.3
/// allows.
///
/// The bare key is asked first, because a proof answers for every plan and a
/// sample only answers for this one.
pub fn lookup(store: &Store, key: DefHash, plan: &ProvePlan) -> Answer {
    if let Some(entry) = store.obligation(key) {
        return match from_cached(entry) {
            Ok(evidence @ Evidence::Proof(_)) => Answer {
                reason: Reason::Proved,
                evidence: Some(evidence),
                warning: None,
            },
            Ok(Evidence::Cases(_)) => {
                Answer::refused(key, "a sampled discharge under the bare key")
            }
            Err(mismatch) => Answer::refused(key, &mismatch),
        };
    }

    let Some(entry) = store.obligation(prove_key(key, plan)) else {
        return Answer::miss(Reason::New);
    };
    match from_cached(entry) {
        Ok(evidence @ Evidence::Cases(_)) => Answer {
            reason: Reason::Sampled,
            evidence: Some(evidence),
            warning: None,
        },
        Ok(Evidence::Proof(_)) => Answer::refused(key, "a proof under a plan key"),
        Err(mismatch) => Answer::refused(key, &mismatch),
    }
}

/// Writes a discharge, at the key its tier belongs under.
///
/// Returns whether anything was written. A refutation, a vacuity and a gap are
/// never written — the first two are errors that re-run until they go green,
/// exactly as a failing test does, and the third is not a result.
pub fn record(store: &mut Store, key: DefHash, discharge: &Discharge, plan: &ProvePlan) -> bool {
    let Discharge::Held(evidence) = discharge else {
        return false;
    };
    let tier = evidence.tier();
    let at = result_key(key, Some(tier), plan);
    // `result_key` already decides this. Checked again rather than trusted,
    // because the cost of the check is a comparison and the cost of it being
    // wrong is a sampled claim that a wider run reads as a proof.
    if tier != Tier::Proved && at == key {
        return false;
    }
    store.put_obligation(at, to_cached(evidence));
    true
}

/// The cache's vocabulary, from the prover's.
///
/// Free functions rather than `From` impls: both types are foreign to this
/// crate, which is exactly the point — `ply-store` does not depend on
/// `ply-prove`, so the translation lives in the crate that depends on both. Both
/// directions `match` with no wildcard arm, so a prover that grows a rule stops
/// compiling here before it can write one nothing can read back.
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

/// The prover's vocabulary, from the cache's — and the one place the recorded
/// tier is checked.
///
/// The tier that comes back is always the one the *evidence* computes to. The
/// recorded label is only ever compared against it, so a file claiming `proved`
/// over a case report is refused rather than believed.
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
    // A certificate that did not establish its guard has a domain it cannot
    // vouch for, so it is the `Vacuous` path rather than a hold — and nothing
    // may read one back as a proof.
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
        CachedRule::ExhaustiveInterleaving { interleavings } => Rule::ExhaustiveInterleaving {
            interleavings: *interleavings,
        },
    }
}

// --- Selection and discharge ------------------------------------------------

/// What the cache answered for each obligation, parallel to the obligation list.
pub struct Selection {
    pub reasons: Vec<Reason>,
    pub cached: Vec<Option<Evidence>>,
    /// Indices the cache could not answer for, in order.
    pub to_discharge: Vec<usize>,
    pub warnings: Vec<Diagnostic>,
}

impl Selection {
    pub fn hits(&self) -> usize {
        self.reasons.iter().filter(|r| r.hit()).count()
    }
}

pub fn select(
    obligations: &[Obligation],
    store: &Store,
    plan: &ProvePlan,
    use_cache: bool,
) -> Selection {
    let mut selection = Selection {
        reasons: Vec::with_capacity(obligations.len()),
        cached: Vec::with_capacity(obligations.len()),
        to_discharge: Vec::new(),
        warnings: Vec::new(),
    };
    for (index, obligation) in obligations.iter().enumerate() {
        let answer = if use_cache {
            lookup(store, obligation.key, plan)
        } else {
            Answer::miss(Reason::Uncached)
        };
        if let Some(warning) = answer.warning {
            selection.warnings.push(warning);
        }
        if answer.evidence.is_none() {
            selection.to_discharge.push(index);
        }
        selection.reasons.push(answer.reason);
        selection.cached.push(answer.evidence);
    }
    selection
}

/// What actually decides an obligation.
///
/// A seam rather than a call, so that this crate holds the cache rule and the
/// prover holds the fragment, and neither can quietly acquire the other's
/// responsibility. `Sync` because every obligation is pure — that is ADR 0007
/// §2's consequence, and it is why there is no conflict graph here.
pub trait Discharger: Sync {
    fn discharge(&self, obligation: &Obligation, plan: &ProvePlan) -> Discharge;
}

/// A discharged run: the report, and where each answer came from.
pub struct Proved {
    pub report: ProveReport,
    /// Parallel to [`ProveReport::obligations`] — every obligation gets exactly
    /// one answer, from the cache or from work.
    pub reasons: Vec<Reason>,
    pub warnings: Vec<Diagnostic>,
}

/// Selects, discharges what the cache could not answer, records what may be
/// recorded, and assembles the report.
///
/// The obligation order is the caller's, and it is preserved end to end: it is a
/// property of the program — file order, then source order — which is what makes
/// two runs one artifact. Discharging runs on whatever pool the caller
/// installed, which is safe without a conflict graph because a spec expression's
/// row is empty: no two obligations contend, so they are all in group 0 for any
/// number of them. That falls out of the purity rule rather than being arranged.
pub fn prove(
    obligations: Vec<Obligation>,
    check: &CheckOutput,
    laws: &Laws,
    store: &mut Store,
    plan: &ProvePlan,
    use_cache: bool,
    discharger: &dyn Discharger,
) -> Proved {
    let started = Instant::now();
    let plan = plan.clone().normalized();
    let selection = select(&obligations, store, &plan, use_cache);
    let mut warnings = selection.warnings;

    let fresh: Vec<(usize, Discharge)> = selection
        .to_discharge
        .par_iter()
        .map(|&index| (index, discharger.discharge(&obligations[index], &plan)))
        .collect();

    let mut discharges: Vec<Option<Discharge>> = selection
        .cached
        .into_iter()
        .map(|evidence| evidence.map(Discharge::Held))
        .collect();
    for (index, discharge) in fresh {
        if use_cache {
            record(store, obligations[index].key, &discharge, &plan);
        }
        discharges[index] = Some(discharge);
    }

    // Every index either came from the cache or was discharged, so no `None`
    // survives. It is filled in rather than dropped anyway: an obligation
    // missing from the report is a claim nobody checked and nobody was told
    // about, where a gap claims nothing and is counted.
    let paired: Vec<(Obligation, Discharge)> = obligations
        .into_iter()
        .zip(discharges)
        .map(|(obligation, discharge)| {
            let discharge = discharge.unwrap_or_else(|| {
                Discharge::Unattempted(ply_prove::Gap::UnhandledEffect(
                    obligation.footprint.clone(),
                ))
            });
            (obligation, discharge)
        })
        .collect();

    warnings.extend(store.take_warnings());
    let coverage = coverage(check, laws, &paired);
    Proved {
        report: ProveReport {
            obligations: paired,
            coverage,
            plan,
            cached: selection.reasons.iter().filter(|r| r.hit()).count(),
            duration: started.elapsed(),
        },
        reasons: selection.reasons,
        warnings,
    }
}

/// A [`Discharger`] that decides nothing.
///
/// Every obligation comes back `Unattempted`, which is the only outcome that
/// claims nothing at all: no tier, no coverage, no cache entry. A run driven by
/// this reports the obligation inventory and the review surface honestly and
/// asserts nothing about whether any claim holds.
///
/// It carries the obligation's own footprint rather than inventing one, so the
/// gap describes the obligation rather than a reason nobody asked about.
pub struct Undecided;

impl Discharger for Undecided {
    fn discharge(&self, obligation: &Obligation, _plan: &ProvePlan) -> Discharge {
        Discharge::Unattempted(ply_prove::Gap::UnhandledEffect(
            obligation.footprint.clone(),
        ))
    }
}

// --- Coverage ---------------------------------------------------------------

/// The review surface: how much of the program a reader still has to read line
/// by line.
///
/// A definition is covered iff it carries at least one `ensures` whose
/// obligation **holds**, or is named directly by a law whose obligation holds.
/// Three things that deliberately do not count:
///
/// - a `requires` alone. A precondition restricts a domain; it makes no claim
///   about behaviour, and this function never sees one — a `requires` is not an
///   obligation at all.
/// - a refuted, vacuous or unattempted obligation. Counting one would make the
///   number go up exactly when the system got less trustworthy.
/// - transitive reachability. Only what a law names.
pub fn coverage(check: &CheckOutput, laws: &Laws, results: &[(Obligation, Discharge)]) -> Coverage {
    let mut strongest: BTreeMap<Symbol, Tier> = BTreeMap::new();
    let mut by_tier: BTreeMap<Tier, usize> = BTreeMap::new();

    for (obligation, discharge) in results {
        let Some(tier) = discharge.tier() else {
            continue;
        };
        let covered: Vec<Symbol> = match obligation.kind {
            ObligationKind::Ensures { .. } => vec![obligation.owner.clone()],
            ObligationKind::Law => laws
                .targets(&obligation.owner)
                .map(|names| names.iter().cloned().collect())
                .unwrap_or_default(),
        };
        for name in covered {
            if !check.defs.contains_key(&name) {
                continue;
            }
            strongest
                .entry(name)
                .and_modify(|best| *best = (*best).max(tier))
                .or_insert(tier);
        }
    }

    for tier in strongest.values() {
        *by_tier.entry(*tier).or_insert(0) += 1;
    }

    let mut uncovered: Vec<Symbol> = check
        .defs
        .keys()
        .filter(|name| !strongest.contains_key(*name))
        .cloned()
        .collect();
    uncovered.sort();

    Coverage {
        definitions: check.defs.len(),
        covered: strongest.len(),
        uncovered,
        by_tier,
    }
}

/// How many definitions carry at least one obligation at all, whatever became of
/// it.
///
/// Reported beside [`Coverage::covered`] rather than folded into it: "carries an
/// obligation" and "carries an obligation the machine could establish" are
/// different numbers, and a reader shown only the first would read a gap as
/// evidence.
pub fn specified(check: &CheckOutput, laws: &Laws, obligations: &[Obligation]) -> usize {
    let mut named: BTreeSet<Symbol> = BTreeSet::new();
    for obligation in obligations {
        match obligation.kind {
            ObligationKind::Ensures { .. } => {
                named.insert(obligation.owner.clone());
            }
            ObligationKind::Law => {
                if let Some(targets) = laws.targets(&obligation.owner) {
                    named.extend(targets.iter().cloned());
                }
            }
        }
    }
    named
        .iter()
        .filter(|name| check.defs.contains_key(*name))
        .count()
}

// --- Review -----------------------------------------------------------------

/// Whether something moved since the baseline a human accepted.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Moved {
    Unchanged,
    Changed,
    /// No baseline exists: this definition has never been accepted, or its name
    /// moved and took its baseline with it. Reported as unreviewed rather than
    /// as unchanged, which costs one re-read and never a false "nothing to see".
    Never,
}

impl Moved {
    pub fn as_str(self) -> &'static str {
        match self {
            Moved::Unchanged => "unchanged",
            Moved::Changed => "changed",
            Moved::Never => "never reviewed",
        }
    }
}

/// One definition, as `ply review --changed` reports it.
#[derive(Clone, Debug)]
pub struct Reviewed {
    pub name: Symbol,
    pub implementation: Moved,
    pub spec: Moved,
    /// Indices into [`ProveReport::obligations`], in report order.
    pub obligations: Vec<usize>,
    /// How many of them the machine actually established.
    pub holding: usize,
}

impl Reviewed {
    /// Whether anything about this definition is a claim **the machine
    /// established**. The row of ADR 0007 §9.2's table this is `false` for is the
    /// one where review still costs what it costs today.
    ///
    /// Carrying an obligation is not enough, and the difference is the whole
    /// point of the tier contract: ADR 0007 §5.5 says an `Unattempted`
    /// obligation "does **not** count toward coverage — a definition whose only
    /// obligation is undischargeable is a definition a reviewer still has to
    /// read". Every row of §9.2's table that tells a reviewer to stop reading is
    /// derived from *"the claim is fixed and still holds"*, and a gap is neither
    /// half of that sentence.
    pub fn specified(&self) -> bool {
        self.holding > 0
    }

    /// Whether a spec is attached at all, whatever became of it. Separate from
    /// [`Reviewed::specified`] so a definition that carries a claim nobody could
    /// discharge is not reported as carrying none.
    pub fn claimed(&self) -> bool {
        !self.obligations.is_empty()
    }

    pub fn changed(&self) -> bool {
        self.implementation != Moved::Unchanged || self.spec != Moved::Unchanged
    }
}

/// What a review run found. Every count here is over **definitions**, so a
/// reader is never comparing one denominator against another.
#[derive(Clone, Debug, Default)]
pub struct ReviewReport {
    pub definitions: usize,
    /// Definitions with a baseline at all.
    pub reviewed: usize,
    /// Only the definitions that moved, in program order.
    pub changed: Vec<Reviewed>,
    pub coverage: Coverage,
    /// Obligations attached to a changed definition that did **not** hold.
    pub broken: usize,
    /// How many of those were never discharged at all, rather than refuted or
    /// found vacuous. Counted apart so the headline can say "no longer holds"
    /// only of a claim that was once established — a gap never held, and saying
    /// it stopped holding is a claim about a check nobody ran.
    pub undischarged: usize,
    pub duration: Duration,
}

impl ReviewReport {
    /// Changed definitions carrying at least one obligation that holds.
    pub fn specified(&self) -> usize {
        self.changed.iter().filter(|r| r.specified()).count()
    }

    /// Changed definitions carrying none. Exactly the surface where review still
    /// costs what it costs today.
    pub fn unspecified(&self) -> usize {
        self.changed.len() - self.specified()
    }

    /// The one sentence this command must not get wrong.
    ///
    /// "No **specified** behaviour changed" is a true claim; "nothing changed"
    /// is a false one, because an unspecified behaviour change is invisible to
    /// this tool by construction. The unspecified count is in the same sentence
    /// rather than further down the page, so the limit is visible at the point
    /// of use and cannot be read past.
    pub fn headline(&self) -> String {
        if self.changed.is_empty() {
            return "no definition changed since the last accepted review".to_string();
        }
        let changed = self.changed.len();
        let unspecified = self.unspecified();
        let claim = if self.broken > 0 && self.broken == self.undischarged {
            // Never discharged is not "no longer holds": nothing established it
            // in the first place, and saying it stopped holding would report a
            // check nobody ran.
            format!(
                "{} obligation{} on a changed definition {} not discharged, so nothing here was established",
                self.broken,
                if self.broken == 1 { "" } else { "s" },
                if self.broken == 1 { "was" } else { "were" }
            )
        } else if self.broken > 0 {
            format!(
                "{} obligation{} on a changed definition no longer hold",
                self.broken,
                if self.broken == 1 { "" } else { "s" }
            )
        } else if self.specified() == 0 {
            "no specified behaviour changed".to_string()
        } else {
            format!(
                "no specified behaviour changed: every obligation on the {} specified of them still holds",
                self.specified()
            )
        };
        if unspecified == 0 {
            return claim;
        }
        let count = if unspecified == changed {
            format!("all {changed} of them")
        } else {
            format!("{unspecified} of {changed}")
        };
        format!(
            "{claim} · {count} carry no obligation that holds, so this run says nothing about \
             whether their behaviour changed"
        )
    }
}

/// Which definitions moved, whether their claims moved with them, and what
/// became of those claims.
///
/// The baseline is what a **human** accepted, not what a machine last ran: a run
/// that went green reviewed nothing.
pub fn review(
    check: &CheckOutput,
    hashes: &HashOutput,
    laws: &Laws,
    store: &Store,
    report: &ProveReport,
) -> ReviewReport {
    let started = Instant::now();
    let mut owed: BTreeMap<Symbol, Vec<usize>> = BTreeMap::new();
    for (index, (obligation, _)) in report.obligations.iter().enumerate() {
        for name in owners(obligation, laws) {
            owed.entry(name).or_default().push(index);
        }
    }

    let mut out = ReviewReport {
        definitions: check.defs.len(),
        coverage: report.coverage.clone(),
        duration: Duration::ZERO,
        ..ReviewReport::default()
    };

    for name in check.defs.keys() {
        let Some(&def_hash) = hashes.defs.get(name) else {
            continue;
        };
        let current = ReviewRecord::new(def_hash, spec_hashes(name, hashes, laws));
        let baseline = store.review_record(name);
        if baseline.is_some() {
            out.reviewed += 1;
        }
        let (implementation, spec) = match baseline {
            None => (Moved::Never, Moved::Never),
            Some(record) => (
                moved(record.def_hash == current.def_hash),
                moved(record.specs == current.specs),
            ),
        };
        let obligations = owed.get(name).cloned().unwrap_or_default();
        let holding = obligations
            .iter()
            .filter(|&&i| report.obligations[i].1.holds())
            .count();
        let reviewed = Reviewed {
            name: name.clone(),
            implementation,
            spec,
            obligations,
            holding,
        };
        if reviewed.changed() {
            for &index in &reviewed.obligations {
                match &report.obligations[index].1 {
                    Discharge::Held(_) => {}
                    Discharge::Unattempted(_) => {
                        out.broken += 1;
                        out.undischarged += 1;
                    }
                    Discharge::Refuted(_) | Discharge::Vacuous(_) => out.broken += 1,
                }
            }
            out.changed.push(reviewed);
        }
    }
    out.duration = started.elapsed();
    out
}

/// Records the current state of every definition as accepted.
///
/// Keyed by program-wide name, which is the only key an edit that moves a hash
/// does not move. Renaming a definition therefore loses its baseline: it is
/// reported as newly unreviewed, which is one re-read and never a false
/// "unchanged".
pub fn accept(check: &CheckOutput, hashes: &HashOutput, laws: &Laws, store: &mut Store) -> usize {
    let mut accepted = 0;
    for name in check.defs.keys() {
        let Some(&def_hash) = hashes.defs.get(name) else {
            continue;
        };
        store.put_review_record(
            name.clone(),
            ReviewRecord::new(def_hash, spec_hashes(name, hashes, laws)),
        );
        accepted += 1;
    }
    accepted
}

/// Every claim *about* one definition: its own clause keys, and the hash of
/// every law that names it directly.
///
/// A law belongs here because it is part of the specification of the definitions
/// it speaks about. Leaving it out would let a law be rewritten while every
/// definition it constrains reported "spec unchanged", which is the one row of
/// ADR 0007 §9.2's table that tells a reviewer to stop reading.
fn spec_hashes(name: &Symbol, hashes: &HashOutput, laws: &Laws) -> Vec<DefHash> {
    let mut out: Vec<DefHash> = hashes.spec_texts.get(name).cloned().unwrap_or_default();
    out.extend(laws.naming(name));
    out
}

fn owners(obligation: &Obligation, laws: &Laws) -> Vec<Symbol> {
    match obligation.kind {
        ObligationKind::Ensures { .. } => vec![obligation.owner.clone()],
        ObligationKind::Law => laws
            .targets(&obligation.owner)
            .map(|names| names.iter().cloned().collect())
            .unwrap_or_default(),
    }
}

fn moved(same: bool) -> Moved {
    if same {
        Moved::Unchanged
    } else {
        Moved::Changed
    }
}
