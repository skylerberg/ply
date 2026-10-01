//! The trail every region of one entry point writes into.

use crate::explore::{Interleaving, Step, Verdict};
use crate::sched::StepRecord;
use crate::sim::{Access, Domain, Seed, Stream};

use crate::{Diagnostic, Span, Symbol};

/// A place in the program, and the definition it lies in.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StepSite {
    pub definition: Option<Symbol>,
    pub span: Span,
}

pub struct Trail {
    seed: Seed,
    sched: Stream,
    /// The choices actually made, which extend past the seed's path.
    choices: Vec<u16>,
    steps: Vec<StepRecord>,
    drawn: u64,
    virtual_time: i64,
    /// The live region's span, for a step the run failed in before it placed itself.
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

    /// `at` is where the access is made: a step is placed at its first.
    pub fn record_access(&mut self, access: Access, at: StepSite) {
        if crate::sched::is_scheduler_bookkeeping(&access) {
            return;
        }
        if let Some(step) = self.steps.last_mut() {
            step.accesses.insert(access);
            step.site.get_or_insert(at);
        }
    }

    /// A step that touched nothing a task can share is placed where it gave control back.
    pub fn end_step(&mut self, yielded: StepSite) {
        if let Some(step) = self.steps.last_mut() {
            step.site.get_or_insert(yielded);
        }
    }

    pub fn record(&self) -> Record {
        Record {
            steps: self
                .steps
                .iter()
                .map(|step| Step::from_record(step, self.fallback))
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
