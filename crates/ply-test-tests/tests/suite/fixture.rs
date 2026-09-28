use ply_eval::{Plan, Seed};
use ply_span::{Diagnostic, SourceId};
use ply_store::{Outcome, Store};
use ply_test::{
    Isolation, Reason, Selection, group_by_conflict, is_seeded, parallelism, result_key, seed_key,
    writes_seed_keys,
};
use ply_ty::{CheckOutput, Footprint, HashOutput, ModuleName};
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[track_caller]
pub fn port_front(sources: &[(String, String)], ids: &[SourceId]) -> ply_ty::Front {
    ply_codegen::c::producer::checked_front(sources, ids)
        .unwrap_or_else(|e| panic!("the fixture must typecheck: {e:#}"))
}

/// What the port raises over these modules, for a fixture meant to be refused.
#[track_caller]
pub fn port_diagnostics(sources: &[(String, String)], ids: &[SourceId]) -> Vec<Diagnostic> {
    ply_codegen::c::producer::ensure_default();
    ply_codegen::c::producer::front(sources, ids)
        .unwrap_or_else(|e| panic!("the port answers for the fixture: {e:#}"))
        .diagnostics
}

pub struct Compiled {
    /// What the tier is built over and the executor runs.
    pub port: ply_ty::Front,
    pub check: CheckOutput,
    pub hashes: HashOutput,
    /// Keyed by `m.name.to_string()`; the Ply emitter re-parses these.
    pub texts: HashMap<String, String>,
}

impl Compiled {
    /// One module named `m`.
    #[track_caller]
    pub fn new(src: &str) -> Compiled {
        Compiled::modules(&[("m", src)])
    }

    /// The module with no project root, named `""`; the module name reaches every hash.
    #[track_caller]
    pub fn anonymous(src: &str) -> Compiled {
        Compiled::modules(&[("", src)])
    }

    /// Several modules, each one's `SourceId` its position in `sources`.
    #[track_caller]
    pub fn modules(sources: &[(&str, &str)]) -> Compiled {
        let named: Vec<(String, String)> = sources
            .iter()
            .map(|(name, src)| {
                (
                    ModuleName::from_dotted(name).to_string(),
                    (*src).to_string(),
                )
            })
            .collect();
        let ids: Vec<SourceId> = (0..named.len()).map(|i| SourceId(i as u32)).collect();
        let port = port_front(&named, &ids);
        Compiled {
            check: port.check.clone(),
            hashes: port.hashes.clone(),
            port,
            texts: named.into_iter().collect(),
        }
    }

    pub fn footprints(&self) -> Vec<ply_ty::Footprint> {
        self.check
            .tests
            .iter()
            .map(|t| t.footprint.clone())
            .collect()
    }

    /// Leaks the `&'static` unit. A bare machine holds no evaluator, so every run needs this.
    pub fn tier(&self) -> &'static ply_codegen::Unit {
        ply_codegen::Unit::over_front(&self.port, self.texts.clone())
            .expect("this host has a C compiler")
    }
}

fn test_hash(hashes: &HashOutput, index: usize) -> Option<ply_ty::DefHash> {
    hashes.tests.get(index).copied()
}

/// What a program decides for a run of tests, as a model: for each test, look its result key up in
/// the store, and take a pass as the answer while anything else runs. The decision itself is the
/// program's — `suite.select` in Ply — and this is here so a runner test can state the same choice
/// without a program to ask. `plan` keys seeded tests, so a selection made against one plan says
/// nothing about another.
pub fn select(check: &CheckOutput, hashes: &HashOutput, store: &Store, plan: &Plan) -> Selection {
    let plan = plan.clone().normalized();
    let total = check.tests.len();
    let mut reasons = Vec::with_capacity(total);
    let mut cached = Vec::new();
    let mut to_run = Vec::new();
    let mut narrowed: BTreeMap<usize, Plan> = BTreeMap::new();

    for (index, test) in check.tests.iter().enumerate() {
        let seeded = is_seeded(&test.footprint);
        let hash = test_hash(hashes, index);
        let stored = hash.map(|hash| store.get(result_key(hash, seeded, &plan)));

        // A `random` plan is one claim per root, so a widened root set owes only unanswered roots.
        let owed = match (seeded, hash) {
            (true, Some(hash)) if writes_seed_keys(&plan) => plan
                .roots
                .iter()
                .copied()
                .filter(|&root| {
                    !matches!(
                        store.get(seed_key(hash, &Seed::root(root))),
                        Some(Outcome::Pass)
                    )
                })
                .collect(),
            _ => plan.roots.clone(),
        };

        let reason = if test.nondet {
            Reason::Nondet
        } else {
            match stored {
                None => Reason::Unhashed,
                // Every root already passed on its own, so the widened plan is proved.
                Some(None) if owed.is_empty() => Reason::Cached,
                Some(None) => Reason::New,
                Some(Some(Outcome::Pass)) => Reason::Cached,
                // Never trust a stored failure.
                Some(Some(Outcome::Fail { .. })) => Reason::PreviousFailure,
            }
        };

        match (reason, stored) {
            (Reason::Cached, Some(Some(outcome))) => cached.push((index, outcome)),
            (Reason::Cached, _) => cached.push((index, Outcome::Pass)),
            _ => {
                if owed.len() < plan.roots.len() {
                    narrowed.insert(
                        index,
                        Plan {
                            roots: owed,
                            ..plan.clone()
                        }
                        .normalized(),
                    );
                }
                to_run.push(index)
            }
        }
        reasons.push(reason);
    }

    let footprints: Vec<(usize, Footprint)> = to_run
        .iter()
        .map(|&i| (i, check.tests[i].footprint.clone()))
        .collect();
    let groups = group_by_conflict(&footprints);
    let parallelism = parallelism(
        check.tests.iter().map(|t| &t.footprint),
        &footprints,
        &groups,
    );

    Selection {
        total,
        cached,
        to_run,
        groups,
        reasons,
        isolation: check
            .tests
            .iter()
            .map(|t| Isolation::of(&t.footprint))
            .collect(),
        parallelism,
        plan,
        narrowed,
        out_of_scope: BTreeSet::new(),
    }
}
