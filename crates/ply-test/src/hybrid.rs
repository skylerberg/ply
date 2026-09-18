//! Building and running one mixed definition graph.

use crate::bisect::{DefKey, Delta, Hybrid, Trial, Unresolved};
use crate::key::{Engine, result_key};
use crate::schedule::is_seeded;
use crate::sim::seed_run;
use ply_eval::{Plan, Provider, Seed};
use ply_hash::body::{BodySet, StoredBody, reconstruct_relinked};
use ply_hash::{DefHash, HashOutput};
use ply_span::{Diagnostic, Symbol};
use ply_store::{Outcome, Store};
use std::collections::{BTreeMap, BTreeSet};
use std::panic::{AssertUnwindSafe, catch_unwind};

/// What makes two failures the same failure.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Signature {
    pub code: &'static str,
    pub message: String,
}

impl Signature {
    pub fn of(diagnostic: &Diagnostic) -> Signature {
        Signature {
            code: diagnostic.code,
            message: diagnostic.message.clone(),
        }
    }
}

/// Every definition either configuration knows, with its baseline (`before`) and current hash.
pub struct Mixture {
    before: BTreeMap<DefKey, DefHash>,
    after: BTreeMap<DefKey, DefHash>,
}

impl Mixture {
    pub fn new() -> Mixture {
        Mixture {
            before: BTreeMap::new(),
            after: BTreeMap::new(),
        }
    }

    pub fn baseline(&mut self, key: DefKey, hash: DefHash) {
        self.before.insert(key, hash);
    }

    pub fn current(&mut self, key: DefKey, hash: DefHash) {
        self.after.insert(key, hash);
    }

    fn keys(&self) -> BTreeSet<&DefKey> {
        self.before.keys().chain(self.after.keys()).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.before.is_empty() && self.after.is_empty()
    }
}

impl Default for Mixture {
    fn default() -> Mixture {
        Mixture::new()
    }
}

pub fn mixture_for(hashes: &HashOutput, key: &Symbol, baseline: &crate::Baseline) -> Mixture {
    let mut mixture = Mixture::new();
    for (key, hash) in baseline
        .closure
        .iter()
        .map(|(n, h)| (DefKey::value(n.clone()), *h))
        .chain(
            baseline
                .decls
                .iter()
                .map(|(n, h)| (DefKey::decl(n.clone()), *h)),
        )
    {
        mixture.baseline(key, hash);
    }
    for name in hashes.closure.get(key).into_iter().flatten() {
        if let Some(hash) = hashes.defs.get(name) {
            mixture.current(DefKey::value(name.clone()), *hash);
        }
        if let Some(hash) = hashes.decls.get(name) {
            mixture.current(DefKey::decl(name.clone()), *hash);
        }
    }
    mixture
}

pub struct BodyHybrid<'a> {
    store: &'a Store,
    /// Consulted before the store: a definition this run introduced is not stored until the flush.
    fresh: &'a BodySet,
    mixture: Mixture,
    test: StoredBody,
    signature: Signature,
    /// Hybrid test hashes that went green.
    proved: Vec<DefHash>,
    /// Pinned to the interleaving the failure happened in.
    plan: Plan,
}

impl<'a> BodyHybrid<'a> {
    pub fn new(
        store: &'a Store,
        fresh: &'a BodySet,
        mixture: Mixture,
        test: StoredBody,
        signature: Signature,
    ) -> BodyHybrid<'a> {
        BodyHybrid {
            store,
            fresh,
            mixture,
            test,
            signature,
            proved: Vec::new(),
            plan: Plan::once(Seed::default()),
        }
    }

    pub fn at_seed(mut self, seed: &Seed) -> BodyHybrid<'a> {
        self.plan = Plan::once(seed.clone());
        self
    }

    /// Found by the hash the run published for it, not by position.
    pub fn test_body(bodies: &BodySet, published: DefHash) -> Option<StoredBody> {
        bodies
            .tests()
            .iter()
            .find(|body| body.key() == Some(published))
            .cloned()
    }

    pub fn take_proved(&mut self) -> Vec<DefHash> {
        std::mem::take(&mut self.proved)
    }

    fn body_of(&self, hash: DefHash) -> Option<StoredBody> {
        if let Some(body) = self.fresh.get(hash) {
            return Some(body.clone());
        }
        self.store.body(hash)?.stored()
    }

    /// Each definition's chosen version, and the redirection pointing every reference at it.
    fn choose(&self, flipped: &BTreeSet<DefKey>) -> Result<Chosen, Unresolved> {
        let mut hashes = Vec::new();
        let mut relink: BTreeMap<DefHash, DefHash> = BTreeMap::new();
        for key in self.mixture.keys() {
            let picked = if flipped.contains(key) {
                self.mixture.after.get(key)
            } else {
                self.mixture.before.get(key)
            };
            let Some(&picked) = picked else {
                continue;
            };
            hashes.push(picked);
            for era in [self.mixture.before.get(key), self.mixture.after.get(key)] {
                let Some(&from) = era else { continue };
                match relink.insert(from, picked) {
                    Some(other) if other != picked => return Err(Unresolved::DoesNotCheck),
                    _ => {}
                }
            }
        }
        hashes.sort();
        hashes.dedup();
        Ok(Chosen { hashes, relink })
    }
}

struct Chosen {
    hashes: Vec<DefHash>,
    relink: BTreeMap<DefHash, DefHash>,
}

impl Hybrid for BodyHybrid<'_> {
    fn trial(&mut self, delta: &Delta, flipped: &[usize]) -> Trial {
        let wanted: BTreeSet<DefKey> = delta.flipped_keys(flipped).into_iter().collect();
        let chosen = match self.choose(&wanted) {
            Ok(chosen) => chosen,
            Err(why) => return Trial::unresolved(why),
        };

        let mut bodies = BodySet::default();
        for hash in &chosen.hashes {
            match self.body_of(*hash) {
                Some(body) => bodies.insert(*hash, body),
                None => return Trial::unresolved(Unresolved::MissingBody),
            }
        }
        bodies.push_test(self.test.clone());

        let Ok(mut rebuilt) = reconstruct_relinked(&bodies, &chosen.relink) else {
            return Trial::unresolved(Unresolved::MissingBody);
        };
        let Ok(resolved) = ply_syntax::resolve(&mut rebuilt.program) else {
            return Trial::unresolved(Unresolved::DoesNotCheck);
        };
        // A mixture has no source text: it is printed once, and that text is checked and built.
        let printed = ply_syntax::print::program(&rebuilt.program);
        // Fresh ids: reconstructed modules all carry `Span::DUMMY.source` and would share one.
        let ids: Vec<ply_span::SourceId> = (0..printed.len())
            .map(|i| ply_span::SourceId(i as u32))
            .collect();
        let Ok(front) = ply_codegen::c::producer::checked_front(&printed, &ids) else {
            return Trial::unresolved(Unresolved::DoesNotCheck);
        };
        let check = &front.check;
        let rehashed = &front.hashes;
        let Some(index) = rebuilt
            .test_keys
            .first()
            .and_then(|key| check.tests.iter().position(|t| &t.key == key))
        else {
            return Trial::unresolved(Unresolved::DoesNotCheck);
        };

        // The hybrid's test hash covers its whole closure, so a `Pass` is about this mixture.
        let seeded = is_seeded(&check.tests[index].footprint);
        let hash = rehashed
            .tests
            .first()
            // The evaluator's namespace whatever the run installed: the trial runs the evaluator.
            .map(|hash| result_key(*hash, seeded, &self.plan, &Engine::Evaluator));
        if let Some(hash) = hash
            && matches!(self.store.get(hash), Some(Outcome::Pass))
        {
            return Trial::passes().from_cache();
        }

        let plan = self.plan.clone();
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            // Hermetic always: a search asks this up to `Budget::max_trials` times.
            let texts: std::collections::HashMap<String, String> =
                printed.iter().cloned().collect();
            let mut machine = ply_eval::Machine::new(&rebuilt.program, &resolved, check);
            let unit = ply_codegen::Unit::over_front(&rebuilt.program, &front, texts)
                .expect("this host has a C compiler");
            let spec = ply_eval::BackendSpec {
                kind: ply_eval::BackendKind::C,
                ..Default::default()
            };
            machine.set_compiled(unit.attach(&spec));
            seed_run(&mut machine, &plan.seeds()[0], plan.steps);
            machine.eval_test(index)
        }));
        match outcome {
            Ok(Ok(())) => {
                if let Some(hash) = hash {
                    self.proved.push(hash);
                }
                Trial::passes()
            }
            Ok(Err(d)) if Signature::of(&d) == self.signature => Trial::fails(),
            Ok(Err(_)) => Trial::unresolved(Unresolved::DifferentFailure),
            // A panic inside a mixture says nothing about the user's program.
            Err(_) => Trial::unresolved(Unresolved::DifferentFailure),
        }
    }
}

/// Whether a mixture can be built at all; `false` means `no_bodies` rather than a bisection.
pub fn bodies_available(store: &Store, fresh: &BodySet, mixture: &Mixture) -> bool {
    mixture
        .before
        .values()
        .chain(mixture.after.values())
        .all(|hash| fresh.contains(*hash) || store.has_body(*hash))
}
