//! Building and running one mixed definition graph.
//!
//! ADR 0004's central claim is that the system can *run the question*: a hybrid
//! program — some definitions at their old hashes, the rest at their new ones —
//! is a legitimate program, so "which of my twelve edits broke this" is answered
//! by evaluating rather than by reading. Without this the only reachable verdicts
//! are the ones that need no mixture, and the artifact hands a ranked list back
//! to the reader with no culprit in it.
//!
//! Three things have to line up for a mixture to be a program:
//!
//! - **One body per name, chosen per era.** The choice is by [`DefKey`], not by
//!   name, so a `fn` and a `type` sharing one are swapped independently.
//! - **Relinking.** A body names its referents by hash, so a caller kept at its
//!   baseline still names its callee's *baseline* hash. Every reference is
//!   redirected to whichever version the mixture chose, or flipping a callee
//!   alone would silently measure the baseline and pass.
//! - **The test as it is written now.** The failure being explained is this
//!   test's failure; the old test asserting something else is not evidence about
//!   it.

use crate::bisect::{DefKey, Delta, Hybrid, Trial, Unresolved};
use ply_hash::body::{BodySet, StoredBody, reconstruct_relinked};
use ply_hash::{DefHash, HashOutput};
use ply_span::{Diagnostic, Symbol};
use ply_store::{Outcome, Store};
use std::collections::{BTreeMap, BTreeSet};
use std::panic::{AssertUnwindSafe, catch_unwind};

/// What makes two failures the same failure.
///
/// The code and the message, deliberately without the span: every span in a
/// reconstructed program is `Span::DUMMY`, so comparing spans would answer
/// "different" for every mixture. The message carries the expected/actual pair,
/// which is what stops an `assert_eq` now reporting different numbers from being
/// read as a reproduction of this one.
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

/// Every definition the two configurations know about, and where its body comes
/// from on each side.
pub struct Mixture {
    /// Baseline hash per key. Absent means the definition did not exist then.
    before: BTreeMap<DefKey, DefHash>,
    /// Current hash per key. Absent means it has since been deleted.
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

    /// Everything either side names, which is the set a trial has to decide
    /// about. A key present on only one side is decided by whether it is flipped.
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
    /// This run's own normalized bytes, consulted before the store: a definition
    /// this run introduced has no stored body until the cache is flushed, and a
    /// bisection that could not see the *current* side would have nothing to
    /// flip to.
    fresh: &'a BodySet,
    mixture: Mixture,
    /// The failing test's body as it is written now.
    test: StoredBody,
    signature: Signature,
    /// Hybrid test hashes that went green. A hybrid's test hash covers its whole
    /// closure, so `Pass` under it is true of exactly that configuration — but
    /// writing it needs a mutable store, which the diagnosis path does not hold,
    /// so the caller drains these afterwards.
    proved: Vec<DefHash>,
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
        }
    }

    /// The current test's stored body, found by the hash the run published for
    /// it rather than by position.
    ///
    /// Position would be wrong: the incremental front end hands `ply-test` only
    /// the modules it re-parsed, so `bodies` is indexed over those while the
    /// published test list covers the whole program.
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

    /// Which version of each definition this mixture takes, and the redirection
    /// that makes every reference point at it.
    ///
    /// A source hash mapping to two different targets means two names share one
    /// definition and the mixture wants them at different versions. They are the
    /// same bytes, so nothing can tell the references apart, and the honest
    /// answer is that this mixture is not a program.
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

        let Ok(rebuilt) = reconstruct_relinked(&bodies, &chosen.relink) else {
            return Trial::unresolved(Unresolved::MissingBody);
        };
        let Ok(resolved) = ply_syntax::resolve(&rebuilt.program) else {
            return Trial::unresolved(Unresolved::DoesNotCheck);
        };
        let Ok(rehashed) = ply_hash::hash_program_ast(&rebuilt.program, &resolved) else {
            return Trial::unresolved(Unresolved::DoesNotCheck);
        };

        // The hybrid's own test hash covers its whole closure, so a `Pass`
        // recorded under it is a claim about exactly this configuration. This is
        // what makes `H(∅)` under an unedited test free: it *is* the hash the
        // baseline test passed at.
        let hash = rehashed.tests.first().copied();
        if let Some(hash) = hash
            && matches!(self.store.get(hash), Some(Outcome::Pass))
        {
            return Trial::passes().from_cache();
        }

        let Ok(check) = ply_core::check_program(&rebuilt.program, &resolved) else {
            return Trial::unresolved(Unresolved::DoesNotCheck);
        };
        let Some(index) = rebuilt
            .test_keys
            .first()
            .and_then(|key| check.tests.iter().position(|t| &t.key == key))
        else {
            return Trial::unresolved(Unresolved::DoesNotCheck);
        };

        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let mut interp = ply_eval::Interp::new(&rebuilt.program, &resolved, &check);
            interp.eval_test(index)
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
            // A panic inside a mixture says nothing about the program the user
            // wrote, and must not be reported as either outcome.
            Err(_) => Trial::unresolved(Unresolved::DifferentFailure),
        }
    }
}

/// Whether a mixture can be built at all, which is the difference between
/// `no_bodies` — go and stop pruning — and a bisection that will run.
pub fn bodies_available(store: &Store, fresh: &BodySet, mixture: &Mixture) -> bool {
    mixture
        .before
        .values()
        .chain(mixture.after.values())
        .all(|hash| fresh.contains(*hash) || store.has_body(*hash))
}
