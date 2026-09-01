//! Selecting, caching and reporting obligations.

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
#[derive(Clone, Debug, Default)]
pub struct Laws {
    targets: BTreeMap<Symbol, BTreeSet<Symbol>>,
    hashes: BTreeMap<Symbol, DefHash>,
    /// The law as a *sentence*, for the review baseline only.
    texts: BTreeMap<Symbol, DefHash>,
}

impl Laws {
    /// Every law the program declares, with the definitions it names.
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

    /// The *sentence* of every law naming this definition, so that rewriting a law reads as a spec
    /// change on everything it constrains — and re-implementing one of those definitions does not.
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
    /// A proof, read under the bare key.
    Proved,
    /// A sampled discharge, read under this exact plan's key.
    Sampled,
    /// The cache was not consulted: `--no-cache`.
    Uncached,
    /// Something was recorded under a key it does not belong under, or under a label its evidence
    /// does not support.
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
    /// A refusal, reported rather than swallowed: a cache that quietly declines to answer looks
    /// exactly like a prover that is slow for no reason.
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

/// What the cache holds for one obligation, under the two keys the obligation cache rule allows.
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
pub fn record(store: &mut Store, key: DefHash, discharge: &Discharge, plan: &ProvePlan) -> bool {
    let Discharge::Held(evidence) = discharge else {
        return false;
    };
    let tier = evidence.tier();
    let at = result_key(key, Some(tier), plan);
    // `result_key` already decides this.
    if tier != Tier::Proved && at == key {
        return false;
    }
    store.put_obligation(at, to_cached(evidence));
    true
}

/// The cache's vocabulary, from the prover's.
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

/// The prover's vocabulary, from the cache's — and the one place the recorded tier is checked.
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
    // A certificate that did not establish its guard has a domain it cannot vouch for, so it is the
    // `Vacuous` path rather than a hold — and nothing may read one back as a proof.
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
        // A `law/host` is never read from the cache, in either direction and for the same reason a
        // host-backed test is not: a green verdict that reached a real database is a claim about
        // that database at that moment, and replaying it as a hit would report a discharge nothing
        // performed.
        let answer = if use_cache && !obligation.host {
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
pub trait Discharger: Sync {
    fn discharge(&self, obligation: &Obligation, plan: &ProvePlan) -> Discharge;
}

/// A discharged run: the report, and where each answer came from.
pub struct Proved {
    pub report: ProveReport,
    /// Parallel to [`ProveReport::obligations`] — every obligation gets exactly one answer, from
    /// the cache or from work.
    pub reasons: Vec<Reason>,
    pub warnings: Vec<Diagnostic>,
}

/// Selects, discharges what the cache could not answer, records what may be recorded, and assembles
/// the report.
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
        if use_cache && !obligations[index].host {
            record(store, obligations[index].key, &discharge, &plan);
        }
        discharges[index] = Some(discharge);
    }

    // Every index either came from the cache or was discharged, so no `None` survives.
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
pub struct Undecided;

impl Discharger for Undecided {
    fn discharge(&self, obligation: &Obligation, _plan: &ProvePlan) -> Discharge {
        Discharge::Unattempted(ply_prove::Gap::UnhandledEffect(
            obligation.footprint.clone(),
        ))
    }
}

// --- Coverage ---------------------------------------------------------------

/// The review surface: how much of the program a reader still has to read line by line.
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

/// How many definitions carry at least one obligation at all, whatever became of it.
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
    /// No baseline exists: this definition has never been accepted, or its name moved and took its
    /// baseline with it.
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
    /// Whether anything about this definition is a claim **the machine established**.
    pub fn specified(&self) -> bool {
        self.holding > 0
    }

    /// Whether a spec is attached at all, whatever became of it.
    pub fn claimed(&self) -> bool {
        !self.obligations.is_empty()
    }

    pub fn changed(&self) -> bool {
        self.implementation != Moved::Unchanged || self.spec != Moved::Unchanged
    }
}

/// What a review run found.
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
    /// How many of those were never discharged at all, rather than refuted or found vacuous.
    pub undischarged: usize,
    pub duration: Duration,
}

impl ReviewReport {
    /// Changed definitions carrying at least one obligation that holds.
    pub fn specified(&self) -> usize {
        self.changed.iter().filter(|r| r.specified()).count()
    }

    /// Changed definitions carrying none.
    pub fn unspecified(&self) -> usize {
        self.changed.len() - self.specified()
    }

    /// The one sentence this command must not get wrong.
    pub fn headline(&self) -> String {
        if self.changed.is_empty() {
            return "no definition changed since the last accepted review".to_string();
        }
        let changed = self.changed.len();
        let unspecified = self.unspecified();
        let claim = if self.broken > 0 && self.broken == self.undischarged {
            // Never discharged is not "no longer holds": nothing established it in the first place,
            // and saying it stopped holding would report a check nobody ran.
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

/// Which definitions moved, whether their claims moved with them, and what became of those claims.
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

/// Every claim *about* one definition: its own clause keys, and the hash of every law that names it
/// directly.
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
