//! A live `simulate` region, and the trail every region of one entry point writes into.

use crate::code::{Captures, Code};
use crate::cont::{SimId, Stack};
use crate::explore::{Interleaving, Step, Verdict};
use crate::sched::{Scheduler, StepRecord};
use crate::sim::{Access, Domain, Handlers, Seed, Stream};
use crate::value::Value;

use ply_span::{Diagnostic, Span, Symbol};
use std::rc::Rc;

/// Where a step was standing when it ended, which the scheduler does not know and a race report
/// prints.
pub struct StepSite {
    pub definition: Option<Symbol>,
    pub span: Span,
}

pub struct Region {
    pub id: SimId,
    pub sched: Scheduler,
    pub handlers: Handlers,
    /// The stack the region delivers its value onto.
    pub below: Stack,
    /// What the root task evaluates.
    pub body: Option<Code>,
    /// The root body's window size, and the values its free variables were bound to at the
    /// region's entry.
    pub size: u32,
    pub captures: Rc<Captures>,
    pub captured: Rc<[Value]>,
    pub module: usize,
    pub span: Span,
    /// The slot-stack height at the region's entry, which every scheduling turn resets to: a
    /// task's windows live above it and die with the task's turn.
    pub floor: usize,
    /// The entering activation's base, restored when the region delivers its value.
    pub rbase: usize,
}

impl Region {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: SimId,
        root: u64,
        drawn: u64,
        steps: u32,
        below: Stack,
        body: Code,
        size: u32,
        captures: Rc<Captures>,
        captured: Rc<[Value]>,
        module: usize,
        span: Span,
        floor: usize,
        rbase: usize,
    ) -> Region {
        Region {
            id,
            sched: Scheduler::new(id, span).with_step_budget(steps),
            handlers: Handlers::at(root, drawn),
            below,
            body: Some(body),
            size,
            captures,
            captured,
            module,
            span,
            floor,
            rbase,
        }
    }

    /// A region the host binding opened, over a scheduler the caller built and rooted.
    pub fn production(id: SimId, sched: Scheduler, span: Span) -> Region {
        Region {
            id,
            sched,
            handlers: Handlers::at(0, 0),
            below: Stack::new(),
            body: None,
            size: 0,
            captures: crate::code::no_captures(),
            captured: crate::code::no_captured(),
            module: 0,
            span,
            floor: 0,
            rbase: 0,
        }
    }
}

/// One entry point's whole simulated run: the choice sequence it makes, the steps it takes, and
/// where each of them stood.
pub struct Trail {
    seed: Seed,
    sched: Stream,
    /// The choice actually made at each scheduling point — which is *not* the seed's path, because
    /// the path runs out and the stream decides after it.
    choices: Vec<u16>,
    steps: Vec<StepRecord>,
    /// Parallel to `steps`, appended as each step ends.
    sites: Vec<StepSite>,
    /// Where the running step *first* touched something a task can share.
    pending: Option<StepSite>,
    /// Draws the `rand` stream has served.
    drawn: u64,
    /// Virtual time the most recently ended region reached.
    virtual_time: i64,
    /// The region currently live, for a step whose site was never closed because the run failed
    /// inside it.
    fallback: Span,
    entered: bool,
}

impl Trail {
    pub fn new(seed: Seed) -> Trail {
        let root = seed.root;
        Trail {
            seed,
            sched: Stream::new(root, Domain::Sched),
            choices: Vec::new(),
            steps: Vec::new(),
            sites: Vec::new(),
            pending: None,
            drawn: 0,
            virtual_time: 0,
            fallback: Span::DUMMY,
            entered: false,
        }
    }

    pub fn seed(&self) -> &Seed {
        &self.seed
    }

    /// Whether this entry point reached a `simulate` region at all.
    pub fn entered(&self) -> bool {
        self.entered
    }

    /// The `rand` stream's counter, which the next region starts from.
    pub fn drawn(&self) -> u64 {
        self.drawn
    }

    pub fn enter(&mut self, span: Span) {
        self.entered = true;
        self.fallback = span;
    }

    /// A region ended.
    pub fn leave(&mut self, virtual_time: i64, drawn: u64) {
        self.virtual_time = virtual_time;
        self.drawn = drawn;
    }

    /// The next scheduling point's index.
    pub fn point(&self) -> usize {
        self.choices.len()
    }

    /// The choice the seed's path fixes at the next scheduling point, if any.
    pub fn pinned(&self) -> Option<u16> {
        self.seed.choice(self.choices.len())
    }

    /// Draws the next scheduling choice from the `sched` stream.
    pub fn draw(&mut self, options: usize) -> Option<usize> {
        self.sched.below(options as u64).map(|drawn| drawn as usize)
    }

    pub fn push_step(&mut self, step: StepRecord) {
        self.choices.push(step.choice);
        self.steps.push(step);
    }

    pub fn steps(&self) -> &[StepRecord] {
        &self.steps
    }

    /// The realized choice sequence: `Seed::at(root, choices[..j].to_vec())` replays this run's
    /// first `j` steps exactly.
    pub fn choices(&self) -> &[u16] {
        &self.choices
    }

    /// Records what the current step touched.
    pub fn record_access(&mut self, access: Access) {
        if crate::sched::is_scheduler_bookkeeping(&access) {
            return;
        }
        if let Some(step) = self.steps.last_mut() {
            step.accesses.insert(access);
        }
    }

    /// Whether the running step already knows where it is standing.
    pub fn has_site(&self) -> bool {
        self.pending.is_some()
    }

    /// Records where the running step is standing, if it has not already.
    pub fn note_site(&mut self, site: StepSite) {
        self.pending.get_or_insert(site);
    }

    /// Closes the running step's site.
    pub fn end_step(&mut self, fallback: Span) {
        let site = self.pending.take().unwrap_or(StepSite {
            definition: None,
            span: fallback,
        });
        self.sites.push(site);
    }

    /// What this entry point's regions did, as [`crate::explore`] reads it.
    pub fn record(&self) -> Record {
        Record {
            steps: self
                .steps
                .iter()
                .enumerate()
                .map(|(i, step)| {
                    let (definition, span) = match self.sites.get(i) {
                        Some(site) => (site.definition.clone(), site.span),
                        None => (None, self.fallback),
                    };
                    Step::from_record(step, definition, span)
                })
                .collect(),
            virtual_time: self.virtual_time,
        }
    }
}

/// What one entry point's simulated regions did, as [`crate::explore`] reads it.
pub struct Record {
    pub steps: Vec<Step>,
    pub virtual_time: i64,
}

impl Record {
    /// The record, given how the entry point that produced it ended.
    pub fn interleaving(&self, outcome: &Result<(), Diagnostic>) -> Interleaving {
        Interleaving {
            steps: self.steps.clone(),
            verdict: match outcome {
                Ok(()) => Verdict::Passed,
                Err(diagnostic) => Verdict::Failed(diagnostic.clone()),
            },
            virtual_time: self.virtual_time,
        }
    }
}

#[cfg(test)]
mod tests {
    /// The same rule `sim`, `sched` and `explore` are held to, and for the same reason: this module
    /// holds a live region's state, so a hash map named here would put the host's memory layout
    /// into a seeded run's answer just as surely as one named in the scheduler.
    #[test]
    fn this_module_names_no_hash_based_collection_and_reads_no_clock() {
        let source = include_str!("region.rs");
        let body = source
            .split_once("mod tests {")
            .map(|(body, _)| body)
            .unwrap_or(source);
        for banned in [
            "HashMap",
            "HashSet",
            "FxHashMap",
            "FxHashSet",
            "SystemTime",
            "Instant",
            "thread::",
            "rayon",
            "as_ptr",
            "strong_count",
        ] {
            assert!(
                !body.contains(banned),
                "`{banned}` appears in ply_eval::region; a seeded run must be a \
                 function of its definitions and its seed and nothing else"
            );
        }
    }
}
