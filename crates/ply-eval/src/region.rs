//! A live `simulate` region, and the trail every region of one entry point
//! writes into.
//!
//! [`crate::sched::Scheduler`] decides which task runs and [`crate::sim::Handlers`]
//! answers `clock` and `random`; this is the state that binds them to one region
//! of one run — the stack the region sits on, the body the root task evaluates,
//! and the record the search reads back.
//!
//! The machine drives it. That split is deliberate: only the machine holds the
//! stack a resumption splices onto and the world every task shares, and neither
//! of those may become the scheduler's, or the scheduler stops being a pure
//! function of its seed.

use crate::code::Code;
use crate::cont::{SimId, Stack};
use crate::env::Env;
use crate::explore::{Interleaving, Step, Verdict};
use crate::sched::{Scheduler, StepRecord};
use crate::sim::{Access, Domain, Handlers, Seed, Stream};

use ply_span::{Diagnostic, Span, Symbol};

/// Where a step was standing when it ended, which the scheduler does not know
/// and a race report prints.
pub struct StepSite {
    pub definition: Option<Symbol>,
    pub span: Span,
}

pub struct Region {
    pub id: SimId,
    pub sched: Scheduler,
    pub handlers: Handlers,
    /// The stack the region delivers its value onto.
    ///
    /// Every task runs on this plus its own delimiter, so the region's own
    /// control is shared by all of them and none of them can reach past it. It
    /// is not fixed at entry: resuming a continuation that carries this region's
    /// delimiter moves the whole region onto whatever stack the resumption
    /// spliced it over, and delivering the value onto the entry stack instead
    /// would silently drop everything the resuming clause still had pending.
    pub below: Stack,
    /// What the root task evaluates. `None` for a region opened lazily at the
    /// host boundary, whose root task is the computation already in progress and
    /// which therefore has no body to enter: its root is resumed rather than
    /// entered, so nothing ever reads this.
    pub body: Option<Code>,
    pub env: Env,
    pub module: usize,
    pub span: Span,
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
        env: Env,
        module: usize,
        span: Span,
    ) -> Region {
        Region {
            id,
            sched: Scheduler::new(id, span).with_step_budget(steps),
            handlers: Handlers::at(root, drawn),
            below,
            body: Some(body),
            env,
            module,
            span,
        }
    }

    /// A region the host binding opened, over a scheduler the caller built and
    /// rooted.
    ///
    /// `below` is empty and not the stack the perform stood on: that stack *is*
    /// the root task, so the region's value is the entry point's value and there
    /// is nothing under it to deliver onto.
    ///
    /// It still carries [`Handlers`], and they are still unreachable:
    /// [`Scheduler::answers`] says a production region answers `task` alone, so
    /// `clock` and `random` never arrive here to be given virtual time.
    pub fn production(id: SimId, sched: Scheduler, span: Span) -> Region {
        Region {
            id,
            sched,
            handlers: Handlers::at(0, 0),
            below: Stack::new(),
            body: None,
            env: Env::empty(),
            module: 0,
            span,
        }
    }
}

/// One entry point's whole simulated run: the choice sequence it makes, the
/// steps it takes, and where each of them stood.
///
/// **One per entry point, not one per region**, and that is the whole of what
/// makes a seed mean one thing. A test may enter several `simulate` regions in
/// sequence — only *nesting* is `E0416`, and an ordinary function call reaches
/// one region twice without any syntax pointing at it. If each region read
/// `Seed::path` from its own point zero and drew from its own `sched` stream,
/// one path would name a different interleaving in each of them, a backtrack
/// point aimed at one region would silently re-aim the others, and the search
/// would derive every conclusion it draws from whichever region happened to be
/// recorded last. Scheduling point *i* is the *i*th of the **run**.
pub struct Trail {
    seed: Seed,
    sched: Stream,
    /// The choice actually made at each scheduling point — which is *not* the
    /// seed's path, because the path runs out and the stream decides after it. A
    /// search that branches from the path instead of from this names an
    /// interleaving nobody ran.
    choices: Vec<u16>,
    steps: Vec<StepRecord>,
    /// Parallel to `steps`, appended as each step ends.
    sites: Vec<StepSite>,
    /// Where the running step *first* touched something a task can share.
    ///
    /// First rather than last, and in the program rather than in the handler
    /// that answered it: a race names the `bank.credit` a task performed, not
    /// the `cell_set` some clause reached for on its behalf, and not the `true`
    /// the function happened to end on. A step's contended access is almost
    /// always the one it opened with, which is why one site per step is enough
    /// and a map from accesses to sites is not.
    pending: Option<StepSite>,
    /// Draws the `rand` stream has served. A second region picks the stream up
    /// where the first left it, because a draw is a draw of the run.
    drawn: u64,
    /// Virtual time the most recently ended region reached. Time is per region —
    /// it is nanoseconds since *that* region was entered — so there is no total
    /// to report and the last one is what an artifact prints.
    virtual_time: i64,
    /// The region currently live, for a step whose site was never closed because
    /// the run failed inside it.
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

    /// Whether this entry point reached a `simulate` region at all. A search
    /// that observed nothing must say so rather than report an interleaving
    /// nobody ran.
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

    /// A region ended. Its clock and its share of the `rand` stream are the
    /// run's from here on.
    pub fn leave(&mut self, virtual_time: i64, drawn: u64) {
        self.virtual_time = virtual_time;
        self.drawn = drawn;
    }

    /// The next scheduling point's index. `path[i]` names the choice here.
    pub fn point(&self) -> usize {
        self.choices.len()
    }

    /// The choice the seed's path fixes at the next scheduling point, if any.
    pub fn pinned(&self) -> Option<u16> {
        self.seed.choice(self.choices.len())
    }

    /// Draws the next scheduling choice from the `sched` stream. `None` only for
    /// an empty enabled set, which is not a scheduling point at all.
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

    /// The realized choice sequence: `Seed::at(root, choices[..j].to_vec())`
    /// replays this run's first `j` steps exactly.
    pub fn choices(&self) -> &[u16] {
        &self.choices
    }

    /// Records what the current step touched. The scheduler's own bookkeeping is
    /// dropped here rather than at the call site, so no caller can put every
    /// step in conflict with every other and report a larger reduction for it.
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

    /// Closes the running step's site. `fallback` is used only by a step that
    /// touched nothing shared, which no race can name anyway.
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
///
/// Every region of the entry point, in the order they ran, over one choice
/// sequence. A record covering one region would leave the search's whole input
/// describing that region alone: the others' choice points would never be
/// branched on and the run would still be reported `exhaustive`, which is a
/// proof of something nobody asked about.
///
/// [`Machine::simulated`] says whether anything ran at all.
///
/// [`Machine::simulated`]: crate::machine::Machine::simulated
pub struct Record {
    pub steps: Vec<Step>,
    pub virtual_time: i64,
}

impl Record {
    /// The record, given how the entry point that produced it ended.
    ///
    /// The verdict is the *run's*, not the region's: a region that completed
    /// inside a test whose later assertion failed is a failing interleaving, and
    /// a search that read it otherwise would report green on a red test.
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
    /// The same rule `sim`, `sched` and `explore` are held to, and for the same
    /// reason: this module holds a live region's state, so a hash map named here
    /// would put the host's memory layout into a seeded run's answer just as
    /// surely as one named in the scheduler.
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
