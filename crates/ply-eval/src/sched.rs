//! The deterministic scheduler.

use crate::arena::Pin;
use crate::cont::SimId;
use crate::host::{HostBinding, HostRuntime, Pending};
use crate::region::Trail;
use crate::sim::{Access, Clock, DEFAULT_STEPS, Seed, StepFootprint, TaskId};
use crate::value::Value;
use ply_span::{Diagnostic, Span, codes};

/// The task a `simulate` region's own body runs as.
pub const ROOT: TaskId = TaskId(0);

pub enum Resumption<K, B> {
    Enter,
    Start { body: B, span: Span },
    Resume { k: K, value: Value },
}

pub enum Turn<K, B> {
    Run {
        task: TaskId,
        resumption: Resumption<K, B>,
    },
    Complete(Value),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Policy {
    Seeded,
    Host,
}

impl Policy {
    pub fn as_str(self) -> &'static str {
        match self {
            Policy::Seeded => "seeded",
            Policy::Host => "host",
        }
    }
}

/// Permission to build a [`Policy::Host`] scheduler; unobtainable in a hermetic run.
pub struct HostPolicy(());

impl HostPolicy {
    /// `None` when nothing is bound.
    pub fn of(binding: &HostBinding) -> Option<HostPolicy> {
        (!binding.is_hermetic()).then_some(HostPolicy(()))
    }
}

enum Wait {
    Join {
        task: TaskId,
        span: Span,
    },
    /// The [`Clock`] owns the timer; `until` is kept so a diagnostic can name it after it fires.
    Timer {
        until: i64,
        span: Span,
    },
    Host {
        pending: Pending,
        span: Span,
    },
}

enum TaskState<K, B> {
    Ready(Resumption<K, B>),
    Running,
    Blocked { wait: Wait, k: K },
    Done(Value),
    Failed,
}

struct Task<K, B> {
    state: TaskState<K, B>,
    origin: Span,
    /// This task's claim on the regions that were open at its `spawn`.
    #[allow(dead_code)]
    pin: Option<Pin>,
}

pub struct StepRecord {
    pub region: SimId,
    pub task: TaskId,
    /// Ascending by id.
    pub enabled: Vec<TaskId>,
    pub choice: u16,
    /// Virtual time when the step began.
    pub at: i64,
    /// What the step touched, excluding the scheduler's own bookkeeping.
    pub accesses: StepFootprint,
    pub stamp: Stamp,
}

/// A task's vector clock, indexed by [`TaskId`], as of one step.
pub type Stamp = Vec<u32>;

/// `later`'s task had observed `earlier`'s step, transitively through spawns and joins.
pub fn happens_before(earlier: &Stamp, earlier_task: TaskId, later: &Stamp) -> bool {
    if earlier.is_empty() || later.is_empty() {
        return false;
    }
    let at = earlier_task.0 as usize;
    let mine = earlier.get(at).copied().unwrap_or(0);
    let theirs = later.get(at).copied().unwrap_or(0);
    mine > 0 && theirs >= mine
}

pub fn is_scheduler_bookkeeping(access: &Access) -> bool {
    match access {
        Access::Atom(atom) => matches!(atom.effect.as_str(), "task" | "clock"),
        Access::Cell { .. } | Access::Alloc => false,
    }
}

pub struct Scheduler<K, B> {
    region: SimId,
    /// Indexed by [`TaskId`].
    tasks: Vec<Task<K, B>>,
    /// One vector clock per task, parallel to `tasks`.
    clocks: Vec<Stamp>,
    max_steps: u32,
    /// Counted only under [`Policy::Host`]; a seeded region spends the [`Trail`]'s points instead.
    steps: u32,
    policy: Policy,
    resume_from: usize,
    current: Option<TaskId>,
    span: Span,
    failure: Option<Diagnostic>,
}

impl<K: Clone, B> Scheduler<K, B> {
    pub fn new(region: SimId, span: Span) -> Scheduler<K, B> {
        Scheduler::rooted(region, span, Policy::Seeded, DEFAULT_STEPS)
    }

    pub fn production(region: SimId, span: Span, _permit: HostPolicy) -> Scheduler<K, B> {
        Scheduler::rooted(region, span, Policy::Host, u32::MAX)
    }

    fn rooted(region: SimId, span: Span, policy: Policy, max_steps: u32) -> Scheduler<K, B> {
        Scheduler {
            region,
            tasks: vec![Task {
                state: TaskState::Ready(Resumption::Enter),
                origin: span,
                // The region outlives its root task, so the root needs no claim.
                pin: None,
            }],
            clocks: vec![vec![0]],
            max_steps,
            steps: 0,
            policy,
            resume_from: 0,
            current: None,
            span,
            failure: None,
        }
    }

    pub fn rooted_running(mut self) -> Result<Scheduler<K, B>, Diagnostic> {
        if self.steps > 0 || self.current.is_some() || self.tasks.len() > 1 {
            return Err(self.internal("a region's root was re-rooted after it had begun"));
        }
        self.tasks[ROOT.0 as usize].state = TaskState::Running;
        self.current = Some(ROOT);
        self.steps = 1;
        Ok(self)
    }

    pub fn with_step_budget(mut self, steps: u32) -> Scheduler<K, B> {
        self.max_steps = steps.max(1);
        self
    }

    pub fn policy(&self) -> Policy {
        self.policy
    }

    pub fn records_steps(&self) -> bool {
        self.policy == Policy::Seeded
    }

    pub fn answers(&self, effect: &str, op: &str) -> bool {
        match self.policy {
            Policy::Seeded => crate::sim::is_scheduled(effect, op),
            Policy::Host => effect == "task" && crate::sim::TASK_OPS.contains(&op),
        }
    }

    pub fn current(&self) -> Option<TaskId> {
        self.current
    }

    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }

    pub fn holds(&self, task: TaskId) -> bool {
        (task.0 as usize) < self.tasks.len()
    }

    /// The tasks that have not finished, ascending.
    pub fn unfinished(&self) -> Vec<TaskId> {
        self.tasks
            .iter()
            .enumerate()
            .filter(|(_, t)| !matches!(t.state, TaskState::Done(_) | TaskState::Failed))
            .map(|(i, _)| TaskId(i as u32))
            .collect()
    }

    pub fn next(&mut self, clock: &mut Clock, trail: &mut Trail) -> Result<Turn<K, B>, Diagnostic> {
        self.require(Policy::Seeded)?;
        if let Some(failure) = &self.failure {
            return Err(failure.clone());
        }
        if let Some(task) = self.current {
            return Err(self.internal(format!(
                "{task} was still running when the scheduler was asked for the next task"
            )));
        }

        let enabled = loop {
            let enabled = self.enabled();
            if !enabled.is_empty() {
                break enabled;
            }
            if let Some(wake) = clock.advance() {
                self.wake(&wake.woken)?;
                continue;
            }
            return if self
                .tasks
                .iter()
                .all(|t| matches!(t.state, TaskState::Done(_)))
            {
                match &self.tasks[ROOT.0 as usize].state {
                    TaskState::Done(value) => Ok(Turn::Complete(value.clone())),
                    _ => Err(self.internal("the region finished without its body returning")),
                }
            } else {
                Err(self.err_deadlock(clock.now(), trail.seed()))
            };
        };

        if trail.point() as u32 >= self.max_steps {
            return Err(self.err_step_budget(trail.seed()));
        }

        let choice = self.choose(trail, &enabled)?;
        let task = enabled[choice];
        let at = task.0 as usize;
        let resumption = match std::mem::replace(&mut self.tasks[at].state, TaskState::Running) {
            TaskState::Ready(resumption) => resumption,
            other => {
                self.tasks[at].state = other;
                return Err(self.internal(format!("{task} was chosen but is not enabled")));
            }
        };

        self.tick(at);
        trail.push_step(StepRecord {
            region: self.region,
            task,
            enabled,
            choice: choice as u16,
            at: clock.now(),
            accesses: StepFootprint::new(),
            stamp: self.clocks[at].clone(),
        });
        self.current = Some(task);
        Ok(Turn::Run { task, resumption })
    }

    pub fn next_host(&mut self, rt: &dyn HostRuntime) -> Result<Turn<K, B>, Diagnostic> {
        self.require(Policy::Host)?;
        if let Some(failure) = &self.failure {
            return Err(failure.clone());
        }
        if let Some(task) = self.current {
            return Err(self.internal(format!(
                "{task} was still running when the scheduler was asked for the next task"
            )));
        }

        // Once per decision: the only point control returns to the machine mid-request.
        if let Some(expired) = rt.drain_expired() {
            return Err(expired);
        }

        let mut fruitless = 0u32;
        let task = loop {
            if let Some(task) = self.first_ready() {
                break task;
            }
            if self.collect(rt)? {
                fruitless = 0;
                continue;
            }
            if self
                .tasks
                .iter()
                .all(|t| matches!(t.state, TaskState::Done(_)))
            {
                return match &self.tasks[ROOT.0 as usize].state {
                    TaskState::Done(value) => Ok(Turn::Complete(value.clone())),
                    _ => Err(self.internal("the region finished without its body returning")),
                };
            }
            // Under a stop, closing listeners is about to resolve pending `accept`s, so wait.
            if !self.waiting_on_host() && !rt.stopping() {
                return Err(self.err_host_deadlock());
            }
            rt.park()?;
            if let Some(expired) = rt.drain_expired() {
                return Err(expired);
            }
            // A park woken by a stop is how an idle service sees a signal, so it is not fruitless.
            if !rt.stopping() {
                fruitless += 1;
                if fruitless > FRUITLESS_PARKS {
                    return Err(self.err_park_made_no_progress());
                }
            }
        };

        // `u32::MAX` means no budget, not a very large one.
        if self.max_steps != u32::MAX && self.steps >= self.max_steps {
            return Err(self.err_host_step_budget());
        }
        self.steps = self.steps.saturating_add(1);

        let at = task.0 as usize;
        let resumption = match std::mem::replace(&mut self.tasks[at].state, TaskState::Running) {
            TaskState::Ready(resumption) => resumption,
            other => {
                self.tasks[at].state = other;
                return Err(self.internal(format!("{task} was chosen but is not enabled")));
            }
        };
        self.resume_from = at + 1;
        self.current = Some(task);
        Ok(Turn::Run { task, resumption })
    }

    pub fn park_on_host(&mut self, k: K, pending: Pending, span: Span) -> Result<(), Diagnostic> {
        if self.policy != Policy::Host {
            return Err(err_host_in_simulation(span, &pending, self.span));
        }
        let at = self.running()?;
        self.tasks[at].state = TaskState::Blocked {
            wait: Wait::Host { pending, span },
            k,
        };
        self.current = None;
        Ok(())
    }

    /// Collects every host token that has resolved, ascending by task, and answers whether any had.
    fn collect(&mut self, rt: &dyn HostRuntime) -> Result<bool, Diagnostic> {
        let mut resolved: Vec<(usize, Value)> = Vec::new();
        for (at, task) in self.tasks.iter().enumerate() {
            if let TaskState::Blocked {
                wait: Wait::Host { pending, span },
                ..
            } = &task.state
                && let Some(value) = rt.poll(pending)?
            {
                // A parked task's answer bypasses the machine's escape checks, so check it here.
                crate::escape::check(
                    &crate::escape::Boundary::HostToken {
                        label: pending.label,
                        token: pending.token,
                    },
                    &value,
                    *span,
                )?;
                resolved.push((at, value));
            }
        }
        let woke = !resolved.is_empty();
        for (at, value) in resolved {
            match std::mem::replace(&mut self.tasks[at].state, TaskState::Running) {
                TaskState::Blocked { k, .. } => {
                    self.tasks[at].state = TaskState::Ready(Resumption::Resume { k, value });
                }
                other => {
                    self.tasks[at].state = other;
                    return Err(self.internal(format!(
                        "{} resolved a host token while not waiting on one",
                        TaskId(at as u32)
                    )));
                }
            }
        }
        Ok(woke)
    }

    fn first_ready(&self) -> Option<TaskId> {
        let n = self.tasks.len();
        let start = self.resume_from % n.max(1);
        (0..n)
            .map(|i| (start + i) % n)
            .find(|&at| matches!(self.tasks[at].state, TaskState::Ready(_)))
            .map(|at| TaskId(at as u32))
    }

    fn waiting_on_host(&self) -> bool {
        self.tasks.iter().any(|t| {
            matches!(
                &t.state,
                TaskState::Blocked {
                    wait: Wait::Host { .. },
                    ..
                }
            )
        })
    }

    /// Leaves the current task running: the caller must hand the id to [`Scheduler::suspend`].
    pub fn spawn(&mut self, body: B, span: Span, pin: Option<Pin>) -> TaskId {
        let id = TaskId(self.tasks.len() as u32);
        self.tasks.push(Task {
            state: TaskState::Ready(Resumption::Start { body, span }),
            origin: span,
            pin,
        });
        let inherited = match (self.policy, self.current) {
            (Policy::Seeded, Some(parent)) => self.clocks[parent.0 as usize].clone(),
            _ => Vec::new(),
        };
        self.clocks.push(inherited);
        id
    }

    fn tick(&mut self, task: usize) {
        let width = self.tasks.len();
        for clock in &mut self.clocks {
            clock.resize(width, 0);
        }
        self.clocks[task][task] += 1;
    }

    fn absorb(&mut self, into: usize, from: usize) {
        if self.policy != Policy::Seeded {
            return;
        }
        let source = self.clocks[from].clone();
        let target = &mut self.clocks[into];
        if target.len() < source.len() {
            target.resize(source.len(), 0);
        }
        for (slot, seen) in target.iter_mut().zip(source) {
            *slot = (*slot).max(seen);
        }
    }

    pub fn suspend(&mut self, k: K, value: Value) -> Result<(), Diagnostic> {
        let at = self.running()?;
        self.tasks[at].state = TaskState::Ready(Resumption::Resume { k, value });
        self.current = None;
        Ok(())
    }

    pub fn join(&mut self, k: K, target: TaskId, span: Span) -> Result<(), Diagnostic> {
        let at = self.running()?;
        let Some(task) = self.tasks.get(target.0 as usize) else {
            return Err(err_unknown_task(span, target));
        };
        let already_done = match &task.state {
            TaskState::Done(value) => Some(value.clone()),
            _ => None,
        };
        self.tasks[at].state = match already_done {
            Some(value) => {
                self.absorb(at, target.0 as usize);
                TaskState::Ready(Resumption::Resume { k, value })
            }
            None => TaskState::Blocked {
                wait: Wait::Join { task: target, span },
                k,
            },
        };
        self.current = None;
        Ok(())
    }

    /// Blocks until virtual time reaches `deadline`; the [`Clock`] already holds the timer.
    pub fn sleep_until(&mut self, k: K, deadline: i64, span: Span) -> Result<(), Diagnostic> {
        if self.policy != Policy::Seeded {
            return Err(Diagnostic::error(
                codes::INTERNAL_ERROR,
                "`clock.sleep` was answered by a production region, which has no virtual clock",
            )
            .primary(span, "performed here")
            .secondary(self.span, "this region schedules against the host runtime")
            .note("under `--host` a sleep is a host operation answering `Pending`, not a timer this scheduler owns"));
        }
        let at = self.running()?;
        self.tasks[at].state = TaskState::Blocked {
            wait: Wait::Timer {
                until: deadline,
                span,
            },
            k,
        };
        self.current = None;
        Ok(())
    }

    pub fn finish(&mut self, value: Value) -> Result<(), Diagnostic> {
        let at = self.running()?;
        let done = TaskId(at as u32);
        self.tasks[at].state = TaskState::Done(value.clone());
        for i in self.joiners_of(done) {
            let TaskState::Blocked { k, .. } = &self.tasks[i].state else {
                continue;
            };
            let k = k.clone();
            self.absorb(i, at);
            self.tasks[i].state = TaskState::Ready(Resumption::Resume {
                k,
                value: value.clone(),
            });
        }
        self.current = None;
        Ok(())
    }

    pub fn fail(&mut self, failure: Diagnostic, seed: &Seed) -> Diagnostic {
        let mut failure = failure;
        if let Some(task) = self.current {
            let live = self
                .tasks
                .iter()
                .filter(|t| !matches!(t.state, TaskState::Done(_) | TaskState::Running))
                .count();
            if self.tasks.len() > 1 {
                failure = failure.note(match self.policy {
                    Policy::Seeded => format!(
                        "failed in task {task} of a simulated region, with {live} other task(s) unfinished; replay with seed {seed}"
                    ),
                    Policy::Host => format!(
                        "failed in task {task} of a host-scheduled region, with {live} other task(s) unfinished; a host-backed run is not replayable from a seed"
                    ),
                });
            }
            self.tasks[task.0 as usize].state = TaskState::Failed;
        }
        self.current = None;
        self.failure = Some(failure.clone());
        failure
    }

    /// Ascending by id, which is the order `path[i]` indexes.
    fn enabled(&self) -> Vec<TaskId> {
        self.tasks
            .iter()
            .enumerate()
            .filter(|(_, t)| matches!(t.state, TaskState::Ready(_)))
            .map(|(i, _)| TaskId(i as u32))
            .collect()
    }

    fn joiners_of(&self, done: TaskId) -> Vec<usize> {
        self.tasks
            .iter()
            .enumerate()
            .filter(|(_, t)| {
                matches!(&t.state, TaskState::Blocked { wait: Wait::Join { task, .. }, .. } if *task == done)
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// Readies timer-fired tasks in ascending order, so the seed, not the host, orders ties.
    fn wake(&mut self, woken: &[TaskId]) -> Result<(), Diagnostic> {
        for id in woken {
            let at = id.0 as usize;
            let k = match self.tasks.get(at).map(|t| &t.state) {
                Some(TaskState::Blocked {
                    wait: Wait::Timer { .. },
                    k,
                }) => k.clone(),
                Some(_) => {
                    return Err(self.internal(format!(
                        "a timer fired for {id}, which was not waiting on one"
                    )));
                }
                None => {
                    return Err(
                        self.internal(format!("a timer fired for {id}, which is not a task"))
                    );
                }
            };
            self.tasks[at].state = TaskState::Ready(Resumption::Resume {
                k,
                value: Value::Unit,
            });
        }
        Ok(())
    }

    /// The seed's path decides while it lasts, then the `sched` stream; both count per entry point.
    fn choose(&self, trail: &mut Trail, enabled: &[TaskId]) -> Result<usize, Diagnostic> {
        let point = trail.point();
        match trail.pinned() {
            Some(choice) => {
                let choice = usize::from(choice);
                if choice >= enabled.len() {
                    return Err(self.err_divergence(point, choice, enabled.len(), trail.seed()));
                }
                Ok(choice)
            }
            None => match trail.draw(enabled.len()) {
                Some(drawn) => Ok(drawn),
                None => Err(self.internal("a scheduling point had no enabled task to choose")),
            },
        }
    }

    fn running(&self) -> Result<usize, Diagnostic> {
        match self.current {
            Some(task) => Ok(task.0 as usize),
            None => Err(self
                .internal("the scheduler was asked to suspend a task while no task was running")),
        }
    }

    fn err_deadlock(&self, now: i64, seed: &Seed) -> Diagnostic {
        let blocked: Vec<(TaskId, &Wait, Span)> = self
            .tasks
            .iter()
            .enumerate()
            .filter_map(|(i, t)| match &t.state {
                TaskState::Blocked { wait, .. } => Some((TaskId(i as u32), wait, t.origin)),
                _ => None,
            })
            .collect();
        let mut diagnostic = Diagnostic::error(
            codes::DEADLOCK,
            format!(
                "this simulated region deadlocked: {} blocked and none runnable",
                plural(blocked.len(), "task is", "tasks are")
            ),
        )
        .primary(self.span, "no task in this region can make progress");
        for (id, wait, origin) in &blocked {
            let (span, message) = match wait {
                Wait::Join { task, span } => {
                    (*span, format!("{id} waits here for {task} to finish"))
                }
                // Unreachable while the clock has a timer: time would have advanced instead.
                Wait::Timer { until, span } => (
                    *span,
                    format!("{id} sleeps here until {until}ns, and it is {now}ns"),
                ),
                // A seeded region never parks on one: `park_on_host` refuses.
                Wait::Host { pending, span } => (
                    *span,
                    format!("{id} waits here on host operation {pending}"),
                ),
            };
            diagnostic =
                diagnostic.secondary(if span.is_dummy() { *origin } else { span }, message);
        }
        diagnostic
            .note("a `simulate` region ends when its last task ends, so a task that never finishes stops the region rather than being abandoned")
            .note("break the wait cycle, or make the task being waited on finish")
            .note(format!("replay with seed {seed}"))
    }

    fn err_host_deadlock(&self) -> Diagnostic {
        let mut diagnostic = Diagnostic::error(
            codes::DEADLOCK,
            format!(
                "this host-scheduled region deadlocked: {} blocked, none runnable and none waiting on the host",
                plural(self.blocked_count(), "task is", "tasks are")
            ),
        )
        .primary(self.span, "no task in this region can make progress");
        for (id, wait, origin) in self.blocked() {
            let (span, message) = match wait {
                Wait::Join { task, span } => {
                    (*span, format!("{id} waits here for {task} to finish"))
                }
                Wait::Timer { until, span } => (*span, format!("{id} sleeps here until {until}ns")),
                Wait::Host { pending, span } => (
                    *span,
                    format!("{id} waits here on host operation {pending}"),
                ),
            };
            diagnostic = diagnostic.secondary(if span.is_dummy() { origin } else { span }, message);
        }
        diagnostic
            .note("a region ends when its last task ends, so a task that never finishes stops the region rather than being abandoned")
            .note("break the wait cycle, or make the task being waited on finish")
    }

    fn err_park_made_no_progress(&self) -> Diagnostic {
        Diagnostic::error(
            codes::INTERNAL_ERROR,
            format!(
                "the host runtime returned from `park` {FRUITLESS_PARKS} times without resolving a token"
            ),
        )
        .primary(self.span, "every task in this region is waiting on the host")
        .note("`HostRuntime::park` must block until at least one outstanding token resolves")
        .note("this is a defect in the host runtime rather than in the program: spinning here would burn a core and report nothing")
    }

    fn err_host_step_budget(&self) -> Diagnostic {
        Diagnostic::error(
            codes::DEADLOCK,
            format!(
                "this host-scheduled region took {} scheduling steps without finishing",
                self.max_steps
            ),
        )
        .primary(self.span, "no task in this region ever stopped running")
        .note("a production region is unbounded by default; this budget was set by the caller")
    }

    fn blocked(&self) -> impl Iterator<Item = (TaskId, &Wait, Span)> {
        self.tasks
            .iter()
            .enumerate()
            .filter_map(|(i, t)| match &t.state {
                TaskState::Blocked { wait, .. } => Some((TaskId(i as u32), wait, t.origin)),
                _ => None,
            })
    }

    fn blocked_count(&self) -> usize {
        self.blocked().count()
    }

    fn require(&self, wanted: Policy) -> Result<(), Diagnostic> {
        if self.policy == wanted {
            return Ok(());
        }
        Err(Diagnostic::error(
            codes::INTERNAL_ERROR,
            format!(
                "a {} region was driven by the {} scheduler's entry point",
                self.policy.as_str(),
                wanted.as_str()
            ),
        )
        .primary(self.span, "this region was opened with the other policy")
        .note("`Scheduler::next` drives a seeded region and `Scheduler::next_host` drives a production one; the two are never interchangeable")
        .note("a seeded region that took real readiness for an answer would stop being a function of its seed"))
    }

    fn err_step_budget(&self, seed: &Seed) -> Diagnostic {
        Diagnostic::error(
            codes::DEADLOCK,
            format!(
                "this simulated region took {} scheduling steps without finishing",
                self.max_steps
            ),
        )
        .primary(self.span, "no task in this region ever stopped running")
        .note("this is the per-interleaving step budget; a region that legitimately needs more steps raises the simulation plan's `steps`")
        .note("more often it is a task that loops without ever finishing, which a real scheduler would spin on forever")
        .note(format!("replay with seed {seed}"))
    }

    fn err_divergence(
        &self,
        point: usize,
        choice: usize,
        enabled: usize,
        seed: &Seed,
    ) -> Diagnostic {
        Diagnostic::error(
            codes::SIMULATION_DIVERGENCE,
            "replaying this seed did not reproduce the schedule it recorded",
        )
        .primary(self.span, "this region scheduled differently on replay")
        .note(format!(
            "at scheduling point {point} the seed names enabled task {choice}, and {} enabled",
            plural(enabled, "task was", "tasks were")
        ))
        .note("this is a defect in Ply's simulation rather than in the program under test: a run must be a function of its definitions and its seed")
        .note(format!("the seed replayed was {seed}"))
    }

    fn internal(&self, message: impl Into<String>) -> Diagnostic {
        Diagnostic::error(codes::INTERNAL_ERROR, message).primary(
            self.span,
            match self.policy {
                Policy::Seeded => "the simulated scheduler was driven out of order",
                Policy::Host => "the production scheduler was driven out of order",
            },
        )
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

pub const FRUITLESS_PARKS: u32 = 1024;

#[cold]
#[inline(never)]
fn err_host_in_simulation(span: Span, pending: &Pending, region: Span) -> Diagnostic {
    Diagnostic::error(
        codes::HOST_IN_SIMULATION,
        format!("a host operation — {pending} — was answered inside a `simulate` region"),
    )
    .primary(span, "this perform reached the host boundary")
    .secondary(region, "the region it was performed in")
    .note("a simulated region is replayed whole per interleaving, so a host operation inside one is performed once per schedule explored")
    .note("handle the operation inside the region with a test double, or move the region out from under the host binding")
}

#[cold]
#[inline(never)]
fn err_unknown_task(span: Span, task: TaskId) -> Diagnostic {
    Diagnostic::error(
        codes::TASK_ESCAPES_SCOPE,
        format!("`{task}` names no task in this simulated region"),
    )
    .primary(span, "this handle outlived the region that created it")
    .note("a `Task` is a key into its region's scheduler, and the scheduler ends with the region")
    .note("join the task inside the `simulate` region that spawned it")
}
