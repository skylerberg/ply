//! What a `simulate` region schedules, and the trail every region of one entry point writes into.

use crate::cont::{Continuation, Delimiter};
use crate::explore::{Interleaving, Step, Verdict};
use crate::sched::{Scheduler, StepRecord};
use crate::sim::{Access, Domain, Seed, Stream};
use crate::value::Value;

use ply_span::{Diagnostic, Span, Symbol};

pub struct StepSite {
    pub definition: Option<Symbol>,
    pub span: Span,
}

/// A task to start: its body, and the spawning frame's delimiters its stack is installed over.
pub struct Spawned {
    pub body: Value,
    pub over: Vec<Delimiter>,
}

pub type MachineScheduler = Scheduler<Continuation, Spawned>;

pub struct Trail {
    seed: Seed,
    sched: Stream,
    /// The choices actually made, which extend past the seed's path.
    choices: Vec<u16>,
    steps: Vec<StepRecord>,
    /// Parallel to `steps`, appended as each step ends.
    sites: Vec<StepSite>,
    /// Where the running step *first* touched something a task can share.
    pending: Option<StepSite>,
    drawn: u64,
    virtual_time: i64,
    /// The live region's span, for a step whose site never closed because the run failed in it.
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

    pub fn leave(&mut self, virtual_time: i64, drawn: u64) {
        self.virtual_time = virtual_time;
        self.drawn = drawn;
    }

    pub fn point(&self) -> usize {
        self.choices.len()
    }

    /// The choice the seed's path fixes at the next scheduling point, if any.
    pub fn pinned(&self) -> Option<u16> {
        self.seed.choice(self.choices.len())
    }

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

    /// `Seed::at(root, choices[..j].to_vec())` replays this run's first `j` steps exactly.
    pub fn choices(&self) -> &[u16] {
        &self.choices
    }

    pub fn record_access(&mut self, access: Access) {
        if crate::sched::is_scheduler_bookkeeping(&access) {
            return;
        }
        if let Some(step) = self.steps.last_mut() {
            step.accesses.insert(access);
        }
    }

    pub fn has_site(&self) -> bool {
        self.pending.is_some()
    }

    /// The first site noted for a step wins.
    pub fn note_site(&mut self, site: StepSite) {
        self.pending.get_or_insert(site);
    }

    pub fn end_step(&mut self, fallback: Span) {
        let site = self.pending.take().unwrap_or(StepSite {
            definition: None,
            span: fallback,
        });
        self.sites.push(site);
    }

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

#[derive(Clone)]
pub struct Record {
    pub steps: Vec<Step>,
    pub virtual_time: i64,
}

impl Record {
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
