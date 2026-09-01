//! The deterministic scheduler.

use crate::arena::Pin;
use crate::cont::{Continuation, Delimiter, SimId};
use crate::host::{HostBinding, HostRuntime, Pending};
use crate::region::Trail;
use crate::sim::{Access, Clock, DEFAULT_STEPS, Seed, StepFootprint, TaskId};
use crate::value::Value;
use ply_span::{Diagnostic, Span, codes};

/// The task a `simulate` region's own body runs as.
pub const ROOT: TaskId = TaskId(0);

/// What the machine must do to give a task its step.
pub enum Resumption {
    /// Evaluate the region's own body.
    Enter,
    /// Apply this closure to no arguments, under the delimiters that were in scope where it was
    /// spawned.
    Start {
        body: Value,
        over: Vec<Delimiter>,
        span: Span,
    },
    /// Splice this continuation onto the current stack and return `value` into it.
    Resume { k: Continuation, value: Value },
}

/// What [`Scheduler::next`] decided.
pub enum Turn {
    Run {
        task: TaskId,
        resumption: Resumption,
    },
    /// Every task has finished, so the region delivers its body's value.
    Complete(Value),
}

/// Which of the two schedulers a region is.
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

/// Permission to build a [`Policy::Host`] scheduler, and the first of the locks that make real
/// threads unreachable from a hermetic run.
pub struct HostPolicy(());

impl HostPolicy {
    /// `None` when nothing is bound.
    pub fn of(binding: &HostBinding) -> Option<HostPolicy> {
        (!binding.is_hermetic()).then_some(HostPolicy(()))
    }
}

/// Why a task cannot run.
enum Wait {
    Join {
        task: TaskId,
        span: Span,
    },
    /// The timer itself is the [`Clock`]'s; this is the task's side of it, and `until` is carried
    /// so a diagnostic can say what the task is waiting for without asking the clock about a timer
    /// it has already fired.
    Timer {
        until: i64,
        span: Span,
    },
    /// A host operation answered [`HostAnswer::Pending`].
    Host {
        pending: Pending,
        span: Span,
    },
}

enum TaskState {
    /// Enabled: suspended at a scheduling point with the control that continues it already decided.
    Ready(Resumption),
    /// The machine is executing this task's step right now.
    Running,
    Blocked {
        wait: Wait,
        k: Continuation,
    },
    Done(Value),
    /// Raised a diagnostic.
    Failed,
}

struct Task {
    state: TaskState,
    /// The `spawn` that created it, or the region for [`ROOT`].
    origin: Span,
    /// This task's claim on the regions that were open at its `spawn`.
    #[allow(dead_code)]
    pin: Option<Pin>,
}

/// One step of one task, as the search reads it back.
pub struct StepRecord {
    /// Which of the entry point's regions took this step.
    pub region: SimId,
    pub task: TaskId,
    /// Ascending by id.
    pub enabled: Vec<TaskId>,
    pub choice: u16,
    /// Virtual time when the step began.
    pub at: i64,
    /// What the step touched, excluding the scheduler's own bookkeeping.
    pub accesses: StepFootprint,
    /// The acting task's vector clock as of this step.
    pub stamp: Stamp,
}

/// A task's vector clock, indexed by [`TaskId`], as of one step.
pub type Stamp = Vec<u32>;

/// `earlier` happens before `later`: `later`'s task had already observed that step, transitively
/// through spawns and joins, when it ran.
pub fn happens_before(earlier: &Stamp, earlier_task: TaskId, later: &Stamp) -> bool {
    if earlier.is_empty() || later.is_empty() {
        return false;
    }
    let at = earlier_task.0 as usize;
    let mine = earlier.get(at).copied().unwrap_or(0);
    let theirs = later.get(at).copied().unwrap_or(0);
    mine > 0 && theirs >= mine
}

/// Whether an access is the scheduler's own bookkeeping rather than the program's state.
pub fn is_scheduler_bookkeeping(access: &Access) -> bool {
    match access {
        Access::Atom(atom) => matches!(atom.effect.as_str(), "task" | "clock"),
        Access::Cell { .. } | Access::Alloc => false,
    }
}

/// The set of runnable tasks of one region, and the enabledness that decides which of them may be
/// picked.
pub struct Scheduler {
    /// Which of the entry point's regions this is.
    region: SimId,
    /// Indexed by [`TaskId`].
    tasks: Vec<Task>,
    /// One vector clock per task, indexed by [`TaskId`] alongside `tasks`.
    clocks: Vec<Stamp>,
    max_steps: u32,
    /// Steps handed out, which only [`Policy::Host`] counts: a seeded region's budget is spent
    /// against the [`Trail`]'s scheduling points, and one entry point's regions share that count.
    steps: u32,
    policy: Policy,
    /// Where [`Policy::Host`]'s round-robin scan starts.
    resume_from: usize,
    current: Option<TaskId>,
    /// The region, for a diagnostic that is about the region rather than about any one task.
    span: Span,
    /// Set once a task fails.
    failure: Option<Diagnostic>,
}

impl Scheduler {
    /// The seeded scheduler.
    pub fn new(region: SimId, span: Span) -> Scheduler {
        Scheduler::rooted(region, span, Policy::Seeded, DEFAULT_STEPS)
    }

    /// The production scheduler: this same state machine, choosing by real readiness instead of by
    /// a seed.
    pub fn production(region: SimId, span: Span, _permit: HostPolicy) -> Scheduler {
        Scheduler::rooted(region, span, Policy::Host, u32::MAX)
    }

    fn rooted(region: SimId, span: Span, policy: Policy, max_steps: u32) -> Scheduler {
        Scheduler {
            region,
            tasks: vec![Task {
                state: TaskState::Ready(Resumption::Enter),
                origin: span,
                // The root task is the region's own control; it holds no claim of its own, because
                // the region it runs in outlives it.
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

    /// Opens the region with its root task already **running**.
    pub fn rooted_running(mut self) -> Result<Scheduler, Diagnostic> {
        if self.steps > 0 || self.current.is_some() || self.tasks.len() > 1 {
            return Err(self.internal("a region's root was re-rooted after it had begun"));
        }
        self.tasks[ROOT.0 as usize].state = TaskState::Running;
        self.current = Some(ROOT);
        self.steps = 1;
        Ok(self)
    }

    /// Scheduling steps this interleaving may take before it is [`codes::DEADLOCK`].
    pub fn with_step_budget(mut self, steps: u32) -> Scheduler {
        self.max_steps = steps.max(1);
        self
    }

    pub fn policy(&self) -> Policy {
        self.policy
    }

    /// Whether this region contributes to the [`Trail`] the search reads.
    pub fn records_steps(&self) -> bool {
        self.policy == Policy::Seeded
    }

    /// Whether this region's delimiter answers `effect.op`.
    pub fn answers(&self, effect: &str, op: &str) -> bool {
        match self.policy {
            Policy::Seeded => crate::sim::is_scheduled(effect, op),
            Policy::Host => effect == "task" && crate::sim::TASK_OPS.contains(&op),
        }
    }

    /// The task whose step is in progress, if any.
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

    /// Which task runs next, or that the region is over.
    pub fn next(&mut self, clock: &mut Clock, trail: &mut Trail) -> Result<Turn, Diagnostic> {
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

    /// Which task runs next under [`Policy::Host`], waiting on `rt` when none can.
    pub fn next_host(&mut self, rt: &dyn HostRuntime) -> Result<Turn, Diagnostic> {
        self.require(Policy::Host)?;
        if let Some(failure) = &self.failure {
            return Err(failure.clone());
        }
        if let Some(task) = self.current {
            return Err(self.internal(format!(
                "{task} was still running when the scheduler was asked for the next task"
            )));
        }

        // Once per scheduling decision, and this is the only place the machine gives control back
        // while a request is still running.
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
            // A stop turns "nothing can make progress" from a verdict into a wait: the listening
            // sockets are being closed under the run, so an `accept` that was the only outstanding
            // token has already resolved and the tasks below it are about to become ready.
            if !self.waiting_on_host() && !rt.stopping() {
                return Err(self.err_host_deadlock());
            }
            rt.park()?;
            if let Some(expired) = rt.drain_expired() {
                return Err(expired);
            }
            // A park that woke on a stop resolved no token and is not fruitless: it is `park` doing
            // the one thing that lets an idle service observe a signal at all, and counting it
            // would report the runtime as broken for working.
            if !rt.stopping() {
                fruitless += 1;
                if fruitless > FRUITLESS_PARKS {
                    return Err(self.err_park_made_no_progress());
                }
            }
        };

        // `u32::MAX` is the absence of a budget rather than a very large one, so a server that
        // legitimately schedules four billion times is not told it spent a limit nobody set.
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

    /// Blocks the current task on a host token.
    pub fn park_on_host(
        &mut self,
        k: Continuation,
        pending: Pending,
        span: Span,
    ) -> Result<(), Diagnostic> {
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
                // The third route a host answer takes back into the program, and the one the
                // machine's own two checks cannot see: the task parked, so nothing on this path
                // knows which registration minted the token.
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

    /// The next enabled task after the one that ran last, wrapping, and without building the set:
    /// `next_host` asks once per step of a run that may serve a great many connections, and the set
    /// it would build is one it does not record.
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

    /// Creates a task and leaves the current one running, because the handle has to reach the
    /// program before its step can end: the caller builds a value from this id and passes it to
    /// [`Scheduler::suspend`].
    pub fn spawn(
        &mut self,
        body: Value,
        over: Vec<Delimiter>,
        span: Span,
        pin: Option<Pin>,
    ) -> TaskId {
        let id = TaskId(self.tasks.len() as u32);
        self.tasks.push(Task {
            state: TaskState::Ready(Resumption::Start { body, over, span }),
            origin: span,
            pin,
        });
        // Everything the parent had done by the spawn happens before everything the child does, so
        // the child starts from the parent's clock.
        let inherited = match (self.policy, self.current) {
            (Policy::Seeded, Some(parent)) => self.clocks[parent.0 as usize].clone(),
            _ => Vec::new(),
        };
        self.clocks.push(inherited);
        id
    }

    /// Extends every clock to cover a task that did not exist when they were last written, so an
    /// index is never a length check at a call site.
    fn tick(&mut self, task: usize) {
        let width = self.tasks.len();
        for clock in &mut self.clocks {
            clock.resize(width, 0);
        }
        self.clocks[task][task] += 1;
    }

    /// `into` observes everything `from` had observed.
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

    /// Ends the current task's step; it stays enabled and is resumed with `value` whenever the
    /// scheduler picks it again.
    pub fn suspend(&mut self, k: Continuation, value: Value) -> Result<(), Diagnostic> {
        let at = self.running()?;
        self.tasks[at].state = TaskState::Ready(Resumption::Resume { k, value });
        self.current = None;
        Ok(())
    }

    /// Blocks the current task until `target` finishes, or resumes it immediately with `target`'s
    /// value if it already has.
    pub fn join(&mut self, k: Continuation, target: TaskId, span: Span) -> Result<(), Diagnostic> {
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

    /// Blocks the current task until virtual time reaches `deadline`, which the region's [`Clock`]
    /// has already registered a timer for.
    pub fn sleep_until(
        &mut self,
        k: Continuation,
        deadline: i64,
        span: Span,
    ) -> Result<(), Diagnostic> {
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

    /// The current task's body returned.
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

    /// The current task raised `failure`.
    pub fn fail(&mut self, failure: Diagnostic, seed: &Seed) -> Diagnostic {
        let mut failure = failure;
        if let Some(task) = self.current {
            let live = self
                .tasks
                .iter()
                .filter(|t| !matches!(t.state, TaskState::Done(_) | TaskState::Running))
                .count();
            if self.tasks.len() > 1 {
                // A production region has no seed to replay, and a note offering one would be an
                // instruction that does not work.
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

    /// Makes the tasks a timer fired for enabled, in the order the clock reported them — ascending,
    /// so that two tasks waking at one instant race in an order the seed decides rather than one
    /// the host does.
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

    /// The seed's path decides while it lasts and the `sched` stream decides after it, both counted
    /// over the whole entry point rather than over this region.
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
                // Unreachable while the clock still has a timer, since time would have advanced
                // instead.
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

    /// The production form.
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

    /// The host runtime answered [`HostRuntime::park`] repeatedly without any token resolving.
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

    /// The mutual exclusion, at the point a scheduler is driven.
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

    /// Reaching one of these means the machine drove the scheduler in an order the seam forbids.
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

/// Returns from [`HostRuntime::park`] with nothing resolved that this scheduler tolerates before it
/// calls the runtime broken.
const FRUITLESS_PARKS: u32 = 1024;

/// A host operation reached a `simulate` region — the footprint check's `E0425`, caught at the scheduler
/// because that is where it would otherwise take effect.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arena::Slot;
    use crate::cont::{Prompt, Stack};
    use crate::sim::{Answer, Handlers, signature};
    use ply_core::{EffectAtom, Resource};
    use ply_span::Symbol;
    use ply_syntax::ast::Mode;
    use std::rc::Rc;

    /// A continuation is control, and none of the scheduler's decisions look inside one — so a
    /// captured empty segment is a faithful stand-in for a suspended task and lets the state
    /// machine be tested on its own.
    fn suspended() -> Continuation {
        let prompt = Rc::new(Prompt {
            clauses: Rc::new(Vec::new()),
            effects: Rc::new(Vec::new()),
            ret: None,
            clause_captures: Vec::new(),
            ret_captures: Rc::from(Vec::new()),
            module: 0,
            span: Span::DUMMY,
        });
        Stack::new().push_prompt(prompt, 0).capture(1, 0).0
    }

    /// What a task does, in the order it does it.
    #[derive(Clone)]
    enum Act {
        Mark(&'static str),
        Yield,
        /// Spawn the script at this index.
        Spawn(usize),
        Join(u32),
        Sleep(i64),
        /// Serve the `rand` stream without ending the step, so that a run with draws and a run
        /// without have the same step structure and their schedules can be compared point for
        /// point.
        Draw,
        Fail,
    }

    /// A whole program: script 0 is the region's body and every other script is something spawned.
    type Program = Vec<Vec<Act>>;

    /// A scheduler, its clock and the entry point's trail, for the tests that drive the seam by
    /// hand rather than through a program.
    fn solo(root: u64) -> (Scheduler, Clock, Trail) {
        (
            Scheduler::new(SimId(0), Span::DUMMY),
            Clock::new(),
            Trail::new(Seed::root(root)),
        )
    }

    /// A `Turn` holds control, which has no `Debug` and wants none, so an expected refusal is
    /// unwrapped here rather than through `expect_err`.
    fn refused(turn: Result<Turn, Diagnostic>, why: &str) -> Diagnostic {
        match turn {
            Ok(_) => panic!("the scheduler handed out a task: {why}"),
            Err(diagnostic) => diagnostic,
        }
    }

    #[derive(Debug)]
    struct Run {
        /// `(task, mark)` in the order the marks were reached — the observable that distinguishes
        /// one interleaving from another.
        marks: Vec<(u32, &'static str)>,
        /// Virtual time at the end.
        clock: i64,
        choices: Vec<u16>,
        steps: Vec<(u32, Vec<u32>, u16)>,
        /// `(task, stamp)` per step, which is what the search reads to decide whether two steps
        /// could have run in the other order.
        stamps: Vec<(TaskId, Stamp)>,
    }

    fn run(program: &Program, seed: Seed) -> Result<Run, Diagnostic> {
        run_with(program, seed, DEFAULT_STEPS)
    }

    /// Drives the scheduler exactly as the machine's seeded prompt will: a perform ends a step,
    /// `clock` and `random` are answered by [`Handlers`], and what the handler answers decides
    /// whether the task stays enabled.
    fn run_with(program: &Program, seed: Seed, budget: u32) -> Result<Run, Diagnostic> {
        let root = seed.root;
        let mut trail = Trail::new(seed);
        let mut sched = Scheduler::new(SimId(0), Span::DUMMY).with_step_budget(budget);
        let mut handlers = Handlers::new(root);
        let mut marks = Vec::new();
        // Which script each task runs, and how far into it that task has got.
        let mut script: Vec<usize> = vec![0];
        let mut pc: Vec<usize> = vec![0];

        loop {
            match sched.next(handlers.clock_mut(), &mut trail)? {
                Turn::Complete(_) => {
                    return Ok(Run {
                        marks,
                        clock: handlers.clock().now(),
                        choices: trail.choices().to_vec(),
                        steps: trail
                            .steps()
                            .iter()
                            .map(|s| {
                                (
                                    s.task.0,
                                    s.enabled.iter().map(|t| t.0).collect::<Vec<_>>(),
                                    s.choice,
                                )
                            })
                            .collect(),
                        stamps: trail
                            .steps()
                            .iter()
                            .map(|s| (s.task, s.stamp.clone()))
                            .collect(),
                    });
                }
                Turn::Run { task, resumption } => {
                    let at = task.0 as usize;
                    if let Resumption::Start { body, .. } = &resumption {
                        let index = match body {
                            Value::Int(i) => *i as usize,
                            other => panic!("a spawned body is a script index, found {other:?}"),
                        };
                        while script.len() <= at {
                            script.push(0);
                            pc.push(0);
                        }
                        script[at] = index;
                    }
                    // One step: act until something suspends this task.
                    loop {
                        let Some(act) = program[script[at]].get(pc[at]).cloned() else {
                            sched.finish(Value::Int(task.0 as i64))?;
                            break;
                        };
                        pc[at] += 1;
                        match act {
                            Act::Mark(m) => {
                                marks.push((task.0, m));
                                continue;
                            }
                            Act::Draw => {
                                handlers.dispatch(
                                    signature("random", "next").expect("declared"),
                                    task,
                                    &[],
                                    Span::DUMMY,
                                )?;
                                continue;
                            }
                            Act::Yield => sched.suspend(suspended(), Value::Unit)?,
                            Act::Spawn(index) => {
                                let id = sched.spawn(
                                    Value::Int(index as i64),
                                    Vec::new(),
                                    Span::DUMMY,
                                    None,
                                );
                                while script.len() <= id.0 as usize {
                                    script.push(0);
                                    pc.push(0);
                                }
                                sched.suspend(suspended(), Value::Int(id.0 as i64))?;
                            }
                            Act::Join(id) => sched.join(suspended(), TaskId(id), Span::DUMMY)?,
                            Act::Sleep(nanos) => {
                                let answer = handlers.dispatch(
                                    signature("clock", "sleep").expect("declared"),
                                    task,
                                    &[Value::Int(nanos)],
                                    Span::DUMMY,
                                )?;
                                match answer {
                                    Answer::Value(value) => sched.suspend(suspended(), value)?,
                                    Answer::Sleeping { deadline } => {
                                        sched.sleep_until(suspended(), deadline, Span::DUMMY)?
                                    }
                                }
                            }
                            Act::Fail => {
                                return Err(sched.fail(
                                    Diagnostic::error(codes::RUNTIME_ERROR, "the task failed")
                                        .primary(Span::DUMMY, "here"),
                                    trail.seed(),
                                ));
                            }
                        }
                        break;
                    }
                }
            }
        }
    }

    /// Two tasks that each mark twice, spawned and joined by the root.
    fn two_workers() -> Program {
        vec![
            vec![
                Act::Spawn(1),
                Act::Spawn(2),
                Act::Join(1),
                Act::Join(2),
                Act::Mark("joined"),
            ],
            vec![Act::Mark("a1"), Act::Yield, Act::Mark("a2")],
            vec![Act::Mark("b1"), Act::Yield, Act::Mark("b2")],
        ]
    }

    #[test]
    fn a_region_with_no_tasks_delivers_its_bodys_value() {
        let program = vec![vec![Act::Mark("only")]];
        let run = run(&program, Seed::root(1)).expect("no reason to block");
        assert_eq!(run.marks, vec![(0, "only")]);
        assert_eq!(run.choices, vec![0], "the body's own step is a step");
    }

    #[test]
    fn one_seed_produces_one_interleaving_however_often_it_is_run() {
        let program = two_workers();
        let first = run(&program, Seed::root(7)).expect("completes");
        for _ in 0..64 {
            let again = run(&program, Seed::root(7)).expect("completes");
            assert_eq!(again.marks, first.marks);
            assert_eq!(again.choices, first.choices);
            assert_eq!(again.steps, first.steps);
        }
    }

    #[test]
    fn different_seeds_produce_different_interleavings() {
        let program = two_workers();
        let mut seen: Vec<Vec<(u32, &'static str)>> = Vec::new();
        for root in 0..32 {
            let run = run(&program, Seed::root(root)).expect("completes");
            if !seen.contains(&run.marks) {
                seen.push(run.marks);
            }
        }
        assert!(
            seen.len() > 1,
            "32 seeds explored one interleaving, so the seed decides nothing"
        );
    }

    /// Whatever the interleaving, every task runs to completion and the marks of one task keep
    /// their own order.
    #[test]
    fn every_interleaving_runs_every_task_in_its_own_order() {
        let program = two_workers();
        for root in 0..64 {
            let run = run(&program, Seed::root(root)).expect("completes");
            let of = |task: u32| -> Vec<&'static str> {
                run.marks
                    .iter()
                    .filter(|(t, _)| *t == task)
                    .map(|(_, m)| *m)
                    .collect()
            };
            assert_eq!(of(1), vec!["a1", "a2"], "seed {root}");
            assert_eq!(of(2), vec!["b1", "b2"], "seed {root}");
            assert_eq!(run.marks.last(), Some(&(0, "joined")), "seed {root}");
        }
    }

    /// The realized choice sequence — not the seed's path, which runs out — is what names an
    /// interleaving to replay.
    #[test]
    fn the_realized_choice_sequence_replays_the_run_it_came_from() {
        let program = two_workers();
        let free = run(&program, Seed::root(11)).expect("completes");
        let pinned = run(&program, Seed::at(11, free.choices.clone())).expect("completes");
        assert_eq!(pinned.marks, free.marks);
        assert_eq!(pinned.choices, free.choices);
    }

    /// A prefix pins its own steps and the stream decides the rest, which is what makes a backtrack
    /// point a seed rather than a whole schedule.
    #[test]
    fn a_path_prefix_pins_only_the_steps_it_names() {
        let program = two_workers();
        let free = run(&program, Seed::root(3)).expect("completes");
        let prefix: Vec<u16> = free.choices.iter().copied().take(3).collect();
        let branched = run(&program, Seed::at(3, prefix.clone())).expect("completes");
        assert_eq!(&branched.choices[..3], &prefix[..]);
    }

    /// Ply's fault, not the program's: a path that does not index the enabled set means the run was
    /// not a function of the seed.
    #[test]
    fn a_choice_that_does_not_index_the_enabled_set_is_a_divergence() {
        let program = two_workers();
        let err = run(&program, Seed::at(1, vec![9])).expect_err("point 0 has one enabled task");
        assert_eq!(err.code, codes::SIMULATION_DIVERGENCE);
        assert!(err.notes.iter().any(|n| n.contains("scheduling point 0")));
    }

    #[test]
    fn a_task_may_spawn_tasks_of_its_own() {
        let program: Program = vec![
            vec![Act::Spawn(1), Act::Join(1), Act::Mark("root done")],
            vec![
                Act::Spawn(2),
                Act::Spawn(3),
                Act::Mark("spawned two"),
                Act::Join(2),
                Act::Join(3),
                Act::Mark("children done"),
            ],
            vec![Act::Mark("grandchild a")],
            vec![Act::Mark("grandchild b")],
        ];
        for root in 0..16 {
            let run = run(&program, Seed::root(root)).expect("completes");
            let marks: Vec<&'static str> = run.marks.iter().map(|(_, m)| *m).collect();
            assert!(marks.contains(&"grandchild a"), "seed {root}");
            assert!(marks.contains(&"grandchild b"), "seed {root}");
            assert_eq!(marks.last(), Some(&"root done"), "seed {root}");
            let children = marks
                .iter()
                .position(|m| *m == "children done")
                .expect("the child joined its own children");
            for grandchild in ["grandchild a", "grandchild b"] {
                let at = marks.iter().position(|m| *m == grandchild).expect("ran");
                assert!(
                    at < children,
                    "seed {root}: {grandchild} ran after the join"
                );
            }
        }
    }

    /// Structured concurrency: the region does not deliver its value until every task it spawned
    /// has finished, joined or not.
    #[test]
    fn a_task_nobody_joins_still_runs_to_completion() {
        let program: Program = vec![
            vec![Act::Spawn(1), Act::Mark("body done")],
            vec![Act::Yield, Act::Mark("worker done")],
        ];
        for root in 0..16 {
            let run = run(&program, Seed::root(root)).expect("completes");
            assert!(
                run.marks.contains(&(1, "worker done")),
                "seed {root} abandoned an unjoined task"
            );
        }
    }

    /// The two edges a simulated region has, as the search sees them: a child's steps happen before
    /// everything its parent does after the join, and nothing orders two siblings.
    #[test]
    fn a_join_orders_the_child_before_the_parent_and_siblings_against_nobody() {
        let program: Program = vec![
            vec![
                Act::Spawn(1),
                Act::Spawn(2),
                Act::Join(1),
                Act::Join(2),
                Act::Mark("after both joins"),
            ],
            vec![Act::Yield, Act::Mark("a")],
            vec![Act::Yield, Act::Mark("b")],
        ];
        for root in 0..8 {
            let run = run(&program, Seed::root(root)).expect("completes");
            let last = |task: u32| {
                run.stamps
                    .iter()
                    .rposition(|(t, _)| t.0 == task)
                    .expect("every task takes a step")
            };
            let (parent, a, b) = (last(0), last(1), last(2));
            for child in [1u32, 2] {
                let (t, stamp) = &run.stamps[last(child)];
                assert!(
                    happens_before(stamp, *t, &run.stamps[parent].1),
                    "seed {root}: @{child}'s last step is not ordered before the parent's",
                );
            }
            assert!(
                !happens_before(&run.stamps[a].1, TaskId(1), &run.stamps[b].1)
                    && !happens_before(&run.stamps[b].1, TaskId(2), &run.stamps[a].1),
                "seed {root}: two siblings were ordered against each other",
            );
        }
    }

    /// The second join answers immediately from the recorded value: a task that has finished is
    /// joinable for as long as its region lasts.
    #[test]
    fn joining_a_task_that_already_finished_does_not_block() {
        let program: Program = vec![
            vec![
                Act::Spawn(1),
                Act::Join(1),
                Act::Join(1),
                Act::Mark("joined twice"),
            ],
            vec![Act::Mark("worker")],
        ];
        for root in 0..8 {
            let run = run(&program, Seed::root(root)).expect("completes");
            assert!(run.marks.contains(&(0, "joined twice")), "seed {root}");
        }
    }

    /// A hang is never an answer: every task blocked with none runnable is a diagnostic naming the
    /// tasks and what each waits on.
    #[test]
    fn a_join_cycle_is_a_deadlock_naming_both_tasks() {
        let program: Program = vec![vec![Act::Spawn(1), Act::Join(1)], vec![Act::Join(0)]];
        for root in 0..8 {
            let err = run(&program, Seed::root(root)).expect_err("nothing can run");
            assert_eq!(err.code, codes::DEADLOCK);
            assert!(
                err.message.contains("2 tasks are blocked"),
                "{}",
                err.message
            );
            let waits: Vec<&str> = err.labels.iter().map(|l| l.message.as_str()).collect();
            assert!(
                waits.iter().any(|m| m.contains("@0 waits here for @1")),
                "{waits:?}"
            );
            assert!(
                waits.iter().any(|m| m.contains("@1 waits here for @0")),
                "{waits:?}"
            );
            assert!(err.notes.iter().any(|n| n.contains("replay with seed")));
        }
    }

    #[test]
    fn a_task_that_joins_itself_deadlocks_rather_than_hanging() {
        let program: Program = vec![vec![Act::Join(0)]];
        let err = run(&program, Seed::root(0)).expect_err("nothing can run");
        assert_eq!(err.code, codes::DEADLOCK);
        assert!(err.message.contains("1 task is blocked"), "{}", err.message);
        assert!(
            err.labels
                .iter()
                .any(|l| l.message.contains("@0 waits here for @0"))
        );
    }

    /// A livelock is the same class of problem as a deadlock from the program's side, so it is the
    /// same code with a different message.
    #[test]
    fn a_region_that_never_stops_spends_its_step_budget() {
        let mut forever = vec![Act::Yield; 64];
        forever.push(Act::Mark("unreachable"));
        let program: Program = vec![forever];
        let err = run_with(&program, Seed::root(0), 16).expect_err("the budget is spent");
        assert_eq!(err.code, codes::DEADLOCK);
        assert!(
            err.message.contains("16 scheduling steps"),
            "{}",
            err.message
        );
        assert!(err.notes.iter().any(|n| n.contains("step budget")));
    }

    #[test]
    fn a_task_failing_stops_the_region_and_names_the_task() {
        let program: Program = vec![
            vec![Act::Spawn(1), Act::Spawn(2), Act::Join(1), Act::Join(2)],
            vec![Act::Mark("a"), Act::Fail],
            vec![Act::Mark("b"), Act::Yield, Act::Mark("b2")],
        ];
        let err = run(&program, Seed::root(4)).expect_err("a task failed");
        assert_eq!(err.code, codes::RUNTIME_ERROR);
        assert!(
            err.notes.iter().any(|n| n.contains("@1")),
            "the failure does not name the task: {:?}",
            err.notes
        );
        assert!(err.notes.iter().any(|n| n.contains("replay with seed 4")));
    }

    /// A failed region stays failed: a caller that keeps driving the scheduler cannot turn a
    /// failure into a hang or into a second, different answer.
    #[test]
    fn a_failed_region_answers_with_its_failure_forever() {
        let (mut sched, mut clock, mut trail) = solo(0);
        let Turn::Run { .. } = sched
            .next(&mut clock, &mut trail)
            .expect("the root is enabled")
        else {
            panic!("expected the root's step");
        };
        let failure = sched.fail(
            Diagnostic::error(codes::RUNTIME_ERROR, "boom"),
            &Seed::root(0),
        );
        assert_eq!(failure.code, codes::RUNTIME_ERROR);
        for _ in 0..4 {
            let err = refused(sched.next(&mut clock, &mut trail), "the region is over");
            assert_eq!(err.code, codes::RUNTIME_ERROR);
        }
    }

    #[test]
    fn virtual_time_does_not_advance_while_any_task_can_run() {
        let program: Program = vec![
            vec![Act::Spawn(1), Act::Spawn(2), Act::Join(1), Act::Join(2)],
            vec![Act::Sleep(100), Act::Mark("woke")],
            vec![
                Act::Mark("t1"),
                Act::Yield,
                Act::Mark("t2"),
                Act::Yield,
                Act::Mark("t3"),
            ],
        ];
        for root in 0..16 {
            let run = run(&program, Seed::root(root)).expect("completes");
            let woke = run
                .marks
                .iter()
                .position(|m| *m == (1, "woke"))
                .expect("the sleeper woke");
            let last = run
                .marks
                .iter()
                .position(|m| *m == (2, "t3"))
                .expect("the runnable task finished");
            assert!(
                last < woke,
                "seed {root}: a timer fired while work could still run"
            );
            assert_eq!(run.clock, 100, "seed {root}");
        }
    }

    /// The timer-coalescing race that is nearly impossible to hit on a real clock: two tasks waking
    /// at one instant race, and the race is explored.
    #[test]
    fn tasks_sleeping_to_one_deadline_wake_together_and_their_order_is_explored() {
        let program: Program = vec![
            vec![Act::Spawn(1), Act::Spawn(2), Act::Join(1), Act::Join(2)],
            vec![Act::Sleep(50), Act::Mark("a")],
            vec![Act::Sleep(50), Act::Mark("b")],
        ];
        let mut orders: Vec<Vec<u32>> = Vec::new();
        for root in 0..32 {
            let run = run(&program, Seed::root(root)).expect("completes");
            assert_eq!(run.clock, 50, "seed {root}");
            let order: Vec<u32> = run
                .marks
                .iter()
                .filter(|(t, _)| *t != 0)
                .map(|(t, _)| *t)
                .collect();
            if !orders.contains(&order) {
                orders.push(order);
            }
        }
        assert_eq!(
            orders.len(),
            2,
            "the wake order was never explored: {orders:?}"
        );
    }

    #[test]
    fn a_sleep_of_no_duration_is_a_yield_and_moves_no_clock() {
        let program: Program = vec![vec![Act::Sleep(0), Act::Sleep(-5), Act::Mark("through")]];
        let run = run(&program, Seed::root(0)).expect("completes");
        assert_eq!(run.clock, 0);
        assert_eq!(run.marks, vec![(0, "through")]);
    }

    #[test]
    fn consecutive_sleeps_accumulate_virtual_time() {
        let program: Program = vec![vec![Act::Sleep(30), Act::Sleep(12), Act::Mark("done")]];
        let run = run(&program, Seed::root(0)).expect("completes");
        assert_eq!(run.clock, 42);
    }

    #[test]
    fn joining_a_task_this_region_never_created_is_a_scope_error() {
        let (mut sched, mut clock, mut trail) = solo(0);
        let Turn::Run { .. } = sched
            .next(&mut clock, &mut trail)
            .expect("the root is enabled")
        else {
            panic!("expected the root's step");
        };
        let err = sched
            .join(suspended(), TaskId(7), Span::DUMMY)
            .expect_err("no such task");
        assert_eq!(err.code, codes::TASK_ESCAPES_SCOPE);
        assert!(err.message.contains("@7"));
        assert!(!sched.holds(TaskId(7)));
        assert!(sched.holds(ROOT));
    }

    /// Every step records the set it was chosen from, and the choice indexes it.
    #[test]
    fn every_step_records_the_set_its_choice_indexed() {
        let program = two_workers();
        let run = run(&program, Seed::root(13)).expect("completes");
        assert_eq!(run.choices.len(), run.steps.len());
        for (i, (task, enabled, choice)) in run.steps.iter().enumerate() {
            let mut ascending = enabled.clone();
            ascending.sort_unstable();
            assert_eq!(&ascending, enabled, "step {i} reported an unordered set");
            assert_eq!(
                enabled.get(*choice as usize),
                Some(task),
                "step {i}'s choice does not index its enabled set"
            );
            assert_eq!(run.choices[i], *choice);
        }
    }

    fn atom(effect: &str, resource: Option<&str>, mode: Mode) -> Access {
        Access::Atom(EffectAtom::new(
            effect,
            resource
                .map(|r| Resource::Named(Symbol::new(r)))
                .unwrap_or(Resource::Singleton),
            mode,
        ))
    }

    /// The single most expensive mistake available in this milestone is a dependence relation that
    /// is too coarse *or* too fine.
    #[test]
    fn the_schedulers_own_bookkeeping_is_not_an_access_but_a_draw_is() {
        let (mut sched, mut clock, mut trail) = solo(0);
        let Turn::Run { .. } = sched
            .next(&mut clock, &mut trail)
            .expect("the root is enabled")
        else {
            panic!("expected the root's step");
        };
        trail.record_access(atom("task", None, Mode::Write));
        trail.record_access(atom("clock", None, Mode::Read));
        trail.record_access(atom("clock", None, Mode::Write));
        assert_eq!(trail.steps()[0].accesses.len(), 0);

        trail.record_access(atom("random", None, Mode::Write));
        trail.record_access(atom("db", Some("orders"), Mode::Write));
        trail.record_access(Access::Cell {
            id: Slot::new(3, 0),
            mode: Mode::Write,
        });
        assert_eq!(trail.steps()[0].accesses.len(), 3);
    }

    /// Cell accesses are in the relation at cell granularity: two tasks share one world, so a cell
    /// is the main way two of them touch the same state.
    #[test]
    fn two_steps_touching_one_cell_are_dependent() {
        let (mut sched, mut clock, mut trail) = solo(0);
        let Turn::Run { .. } = sched
            .next(&mut clock, &mut trail)
            .expect("the root is enabled")
        else {
            panic!("expected the root's step");
        };
        trail.record_access(Access::Cell {
            id: Slot::new(1, 0),
            mode: Mode::Write,
        });
        sched.suspend(suspended(), Value::Unit).expect("running");
        let Turn::Run { .. } = sched.next(&mut clock, &mut trail).expect("still enabled") else {
            panic!("expected a second step");
        };
        trail.record_access(Access::Cell {
            id: Slot::new(1, 0),
            mode: Mode::Read,
        });
        let steps = trail.steps();
        assert!(steps[0].accesses.conflicts_with(&steps[1].accesses));
        assert!(!steps[0].accesses.conflicts_with(&StepFootprint::new()));
    }

    /// The two domains have their own counters, so a program that draws random numbers gets the
    /// same schedule as one that does not.
    #[test]
    fn drawing_random_numbers_does_not_disturb_the_schedule() {
        let plain = two_workers();
        let drawing: Program = plain
            .iter()
            .map(|script| {
                let mut with_draws = Vec::new();
                for act in script {
                    with_draws.push(Act::Draw);
                    with_draws.push(act.clone());
                }
                with_draws
            })
            .collect();
        for root in 0..16 {
            let a = run(&plain, Seed::root(root)).expect("completes");
            let b = run(&drawing, Seed::root(root)).expect("completes");
            assert_eq!(a.choices, b.choices, "seed {root}");
            assert_eq!(a.marks, b.marks, "seed {root}");
        }
    }

    #[test]
    fn the_scheduler_refuses_to_hand_out_a_second_task_while_one_is_running() {
        let (mut sched, mut clock, mut trail) = solo(0);
        let Turn::Run { .. } = sched
            .next(&mut clock, &mut trail)
            .expect("the root is enabled")
        else {
            panic!("expected the root's step");
        };
        let err = refused(
            sched.next(&mut clock, &mut trail),
            "a task is still running",
        );
        assert_eq!(err.code, codes::INTERNAL_ERROR);
        assert!(err.message.contains("@0"));
    }

    #[test]
    fn suspending_with_nothing_running_is_refused_rather_than_silently_applied() {
        let (mut sched, _clock, _trail) = solo(0);
        let err = sched
            .suspend(suspended(), Value::Unit)
            .expect_err("no task is running");
        assert_eq!(err.code, codes::INTERNAL_ERROR);
    }

    /// A rule about how a type is *used* is a rule nobody enforces; a rule about which types may be
    /// *named* is greppable.
    #[test]
    fn this_module_names_nothing_a_seeded_run_may_not_depend_on() {
        let source = include_str!("sched.rs");
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
            "rand::",
        ] {
            assert!(
                !body.contains(banned),
                "`{banned}` appears in ply_eval::sched; a scheduling decision must be a \
                 function of the seed and nothing else"
            );
        }
    }

    use crate::host::{
        Determinism, HostAnswer, HostBinding, HostHandler, HostOp, HostRegistry, HostRequest,
        HostResource, Linearity,
    };
    use std::sync::Arc;

    /// Enough of a program to bind against, and nothing else: what these tests need from a binding
    /// is only that it is *bound*.
    fn binding() -> HostBinding {
        let source = "nondet effect db { read get[r](k: Int) -> Int }\n\
                      fn lookup(k: Int) -> Int / {db.read[users]} = db.get[users](k)";
        let module = ply_syntax::parse(ply_span::SourceId(0), source).expect("the fixture parses");
        let check = ply_core::check_module(&module).expect("the fixture typechecks");
        let mut registry = HostRegistry::new();
        registry.register(
            HostOp {
                effect: Symbol::new("db"),
                op: Symbol::new("get"),
                resource: HostResource::Only(Resource::Named(Symbol::new("users"))),
                determinism: Determinism::Nondeterministic,
                linearity: Linearity::AtMostOnce,
                blocking: false,
                secrets: false,
                path: "test::handler",
            },
            Arc::new(Never),
        );
        registry.bind(&check).expect("the fixture binds")
    }

    struct Never;

    impl HostHandler for Never {
        fn call(
            &self,
            _: &dyn HostRuntime,
            req: &HostRequest<'_>,
        ) -> Result<HostAnswer, Diagnostic> {
            Err(
                Diagnostic::error(codes::INTERNAL_ERROR, "the test handler was called")
                    .primary(req.span, "here"),
            )
        }
    }

    /// A runtime that owns no token: enough to drive a region whose tasks never wait, and it fails
    /// loudly if one does.
    struct Idle;

    impl HostRuntime for Idle {
        fn poll(&self, _: &Pending) -> Result<Option<Value>, Diagnostic> {
            Ok(None)
        }

        fn park(&self) -> Result<(), Diagnostic> {
            Err(Diagnostic::error(
                codes::INTERNAL_ERROR,
                "the idle runtime was asked to wait",
            ))
        }

        fn block_on(&self, _: Pending) -> Result<Value, Diagnostic> {
            Err(Diagnostic::error(
                codes::INTERNAL_ERROR,
                "the idle runtime was asked to wait",
            ))
        }
    }

    fn production() -> Scheduler {
        let binding = binding();
        let permit = HostPolicy::of(&binding).expect("a bound binding mints a permit");
        Scheduler::production(SimId(0), Span::DUMMY, permit)
    }

    /// `simulate` reaches `Scheduler::new` and nothing else, so a simulated region is seeded by
    /// construction rather than by anyone remembering.
    #[test]
    fn a_simulated_region_is_seeded_by_construction() {
        let (sched, _clock, _trail) = solo(0);
        assert_eq!(sched.policy(), Policy::Seeded);
        assert!(sched.records_steps());
    }

    #[test]
    fn a_hermetic_binding_mints_no_permit() {
        assert!(HostPolicy::of(&HostBinding::hermetic()).is_none());
        assert!(HostPolicy::of(&HostBinding::default()).is_none());
        assert!(HostPolicy::of(&binding()).is_some());
    }

    /// The lock that does not depend on the permit being unforgeable: whichever scheduler a caller
    /// is holding, driving it through the other loop is a diagnostic rather than a different
    /// answer.
    #[test]
    fn each_entry_point_refuses_the_other_policys_region() {
        let (mut seeded, mut clock, mut trail) = solo(0);
        let err = refused(
            seeded.next_host(&Idle),
            "a seeded region has no host runtime to schedule against",
        );
        assert_eq!(err.code, codes::INTERNAL_ERROR);
        assert!(err.message.contains("seeded region"), "{}", err.message);

        let mut host = production();
        let err = refused(
            host.next(&mut clock, &mut trail),
            "a production region has no seed",
        );
        assert_eq!(err.code, codes::INTERNAL_ERROR);
        assert!(err.message.contains("host region"), "{}", err.message);
    }

    /// A seeded region that parked on a real token would stop being a function of its seed while
    /// every assertion in it still passed, so the refusal is at the park rather than only at the
    /// binding.
    #[test]
    fn a_seeded_region_refuses_to_park_a_task_on_a_host_token() {
        let (mut sched, mut clock, mut trail) = solo(0);
        let Turn::Run { .. } = sched
            .next(&mut clock, &mut trail)
            .expect("the root is enabled")
        else {
            panic!("expected the root's step");
        };
        let err = sched
            .park_on_host(
                suspended(),
                Pending {
                    token: 1,
                    label: "read",
                },
                Span::DUMMY,
            )
            .expect_err("a simulated region may not wait on the host");
        assert_eq!(err.code, codes::HOST_IN_SIMULATION);
        assert!(sched.current().is_some(), "the task was parked anyway");
    }

    /// There is no virtual clock to reach the deadline, so parking on one is a hang.
    #[test]
    fn a_production_region_refuses_a_virtual_sleep() {
        let mut sched = production();
        let Turn::Run { .. } = sched.next_host(&Idle).expect("the root is enabled") else {
            panic!("expected the root's step");
        };
        let err = sched
            .sleep_until(suspended(), 100, Span::DUMMY)
            .expect_err("no virtual clock exists here");
        assert_eq!(err.code, codes::INTERNAL_ERROR);
    }

    /// Round-robin, so a task that yields in a loop cannot hold the region.
    #[test]
    fn the_production_scheduler_starves_nobody() {
        let mut sched = production();
        let Turn::Run { task, .. } = sched.next_host(&Idle).expect("the root is enabled") else {
            panic!("expected the root's step");
        };
        assert_eq!(task, ROOT);
        sched.spawn(Value::Unit, Vec::new(), Span::DUMMY, None);
        sched.spawn(Value::Unit, Vec::new(), Span::DUMMY, None);
        sched.suspend(suspended(), Value::Unit).expect("running");

        let mut order = Vec::new();
        for _ in 0..9 {
            let Turn::Run { task, .. } = sched.next_host(&Idle).expect("three are ready") else {
                panic!("expected a step");
            };
            order.push(task.0);
            sched.suspend(suspended(), Value::Unit).expect("running");
        }
        assert_eq!(order, vec![1, 2, 0, 1, 2, 0, 1, 2, 0]);
    }

    /// A lazily-opened region's root is the computation that opened it, so it starts *running*: the
    /// `task.*` that opened the region has not been answered yet, and only the ordinary
    /// spawn/join/suspend path can answer it.
    #[test]
    fn a_lazily_opened_region_roots_on_the_control_that_opened_it() {
        let mut sched = production().rooted_running().expect("nothing has run yet");
        assert_eq!(sched.current(), Some(ROOT));

        // The opening perform is answered through the same path every later one takes, which is
        // what stops `spawn` meaning two things.
        let child = sched.spawn(Value::Unit, Vec::new(), Span::DUMMY, None);
        sched
            .suspend(suspended(), Value::Task(child))
            .expect("the root is running");

        let Turn::Run { task, resumption } = sched.next_host(&Idle).expect("the root is enabled")
        else {
            panic!("expected a step");
        };
        assert_eq!(task, ROOT);
        assert!(
            matches!(resumption, Resumption::Resume { value: Value::Task(id), .. } if id == child),
            "a lazily-opened root resumes with the answer, never evaluates a body it does not have"
        );

        let err = sched
            .rooted_running()
            .err()
            .expect("the region has already begun");
        assert_eq!(err.code, codes::INTERNAL_ERROR);
    }

    /// A production region writes no step, so it can neither fabricate an exploration nor disturb
    /// one a seeded region of the same entry point made.
    #[test]
    fn a_production_region_records_nothing_in_the_trail() {
        let trail = Trail::new(Seed::root(9));
        let mut sched = production();
        assert!(!sched.records_steps());
        for _ in 0..4 {
            let Turn::Run { .. } = sched.next_host(&Idle).expect("the root is enabled") else {
                panic!("expected a step");
            };
            sched.suspend(suspended(), Value::Unit).expect("running");
        }
        assert!(trail.steps().is_empty());
        assert!(trail.choices().is_empty());
        assert!(!trail.entered());
        assert_eq!(trail.point(), 0);
    }
    /// A runtime that is stopping and has nothing outstanding.
    struct Stopping {
        parks: std::cell::Cell<u32>,
        expire_after: u32,
    }

    impl HostRuntime for Stopping {
        fn poll(&self, _: &Pending) -> Result<Option<Value>, Diagnostic> {
            Ok(None)
        }

        fn park(&self) -> Result<(), Diagnostic> {
            self.parks.set(self.parks.get() + 1);
            Ok(())
        }

        fn block_on(&self, _: Pending) -> Result<Value, Diagnostic> {
            Err(Diagnostic::error(codes::INTERNAL_ERROR, "not reached"))
        }

        fn stopping(&self) -> bool {
            true
        }

        fn drain_expired(&self) -> Option<Diagnostic> {
            (self.parks.get() >= self.expire_after)
                .then(|| Diagnostic::warning(codes::DRAIN_INCOMPLETE, "the drain deadline expired"))
        }
    }

    /// Two tasks each waiting for the other: nothing is enabled, nothing is waiting on the host,
    /// and no virtual clock exists to advance.
    fn deadlock(sched: &mut Scheduler) {
        let other = sched.spawn(Value::Unit, Vec::new(), Span::DUMMY, None);
        sched
            .join(suspended(), other, Span::DUMMY)
            .expect("the root is running");
        let Turn::Run { task, .. } = sched.next_host(&Idle).expect("the spawned task is enabled")
        else {
            panic!("expected the spawned task's first step");
        };
        assert_eq!(task, other);
        sched
            .join(suspended(), ROOT, Span::DUMMY)
            .expect("the spawned task is running");
    }

    /// An idle service observes a signal and stops, and the deadlock check does not report `E0414`
    /// on the way.
    #[test]
    fn a_stopping_region_with_nothing_outstanding_drains_rather_than_deadlocking() {
        let mut sched = production();
        let Turn::Run { .. } = sched
            .next_host(&Stopping {
                parks: std::cell::Cell::new(0),
                expire_after: 3,
            })
            .expect("the root is enabled")
        else {
            panic!("expected the root's step");
        };
        // The root blocks on a task that will never finish: nothing is enabled and nothing is
        // waiting on the host, which is `err_host_deadlock`'s exact condition.
        deadlock(&mut sched);

        let runtime = Stopping {
            parks: std::cell::Cell::new(0),
            expire_after: 3,
        };
        let err = refused(
            sched.next_host(&runtime),
            "the drain deadline ends the region",
        );
        assert_eq!(
            err.code,
            codes::DRAIN_INCOMPLETE,
            "a stopping region ends on its deadline, not on `E0414`: {}",
            err.message
        );
        assert!(
            runtime.parks.get() >= 3,
            "the scheduler parked {} times before the deadline, so it never waited",
            runtime.parks.get()
        );
    }

    /// A park that woke on a stop resolved no token and is not fruitless.
    #[test]
    fn a_park_that_woke_on_a_stop_is_not_counted_as_fruitless() {
        let mut sched = production();
        let Turn::Run { .. } = sched
            .next_host(&Stopping {
                parks: std::cell::Cell::new(0),
                expire_after: u32::MAX,
            })
            .expect("the root is enabled")
        else {
            panic!("expected the root's step");
        };
        deadlock(&mut sched);

        let runtime = Stopping {
            parks: std::cell::Cell::new(0),
            expire_after: FRUITLESS_PARKS + 8,
        };
        let err = refused(sched.next_host(&runtime), "the drain deadline ends it");
        assert_eq!(
            err.code,
            codes::DRAIN_INCOMPLETE,
            "{} parks past the fruitless bound reported `{}` instead",
            runtime.parks.get(),
            err.code
        );
        assert!(runtime.parks.get() > FRUITLESS_PARKS);
    }

    /// The same region with no stop in progress *is* a deadlock, so the exemption above is not a
    /// hole: what it turns off is the verdict for a run that is stopping and nothing else.
    #[test]
    fn a_region_that_is_not_stopping_still_deadlocks() {
        let mut sched = production();
        let Turn::Run { .. } = sched.next_host(&Idle).expect("the root is enabled") else {
            panic!("expected the root's step");
        };
        deadlock(&mut sched);
        let err = refused(sched.next_host(&Idle), "nothing can make progress");
        assert_eq!(err.code, codes::DEADLOCK);
    }
}
