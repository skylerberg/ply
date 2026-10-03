//! The deterministic scheduler.

use crate::host::{HostBinding, HostRuntime, MachineId, Pending};
use crate::region::SimId;
use crate::region::{StepSite, Trail};
use crate::sim::{Access, Clock, DEFAULT_STEPS, Seed, StepFootprint, TaskId};
use crate::value::Value;
use crate::{Diagnostic, Span, codes};
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::{Rc, Weak};

/// The task a `simulate` region's own body runs as.
pub const ROOT: TaskId = TaskId(0);

pub enum Resumption<K, B> {
    Enter,
    Start {
        body: B,
        span: Span,
    },
    Resume {
        k: K,
        value: Value,
    },
    /// Resume `k` only to unwind it: the task was cancelled and performs nothing more.
    Cancel {
        k: K,
    },
    /// Resume `k` failing with `failure`, as a join of a cancelled task does.
    Raise {
        k: K,
        failure: Diagnostic,
    },
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

/// A `Task` value: its region and its id, counted so a production region can retire it once unheld.
#[derive(Clone)]
pub struct TaskHandle(Rc<Held>);

struct Held {
    /// Every region numbers its own tasks, so an id alone names a task only in its own region.
    region: SimId,
    id: TaskId,
    /// A production region's [`Released`]; dangling for every other handle, which nothing counts.
    released: Weak<Released>,
}

/// The tasks whose last handle went since the scheduler last looked.
type Released = Cell<Vec<TaskId>>;

impl TaskHandle {
    /// A handle to `id` of `region` that no region counts toward retiring the task.
    pub fn unowned(region: SimId, id: TaskId) -> TaskHandle {
        TaskHandle(Rc::new(Held {
            region,
            id,
            released: Weak::new(),
        }))
    }

    pub fn region(&self) -> SimId {
        self.0.region
    }

    pub fn id(&self) -> TaskId {
        self.0.id
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        if let Some(released) = self.released.upgrade() {
            release(&released, self.id);
        }
    }
}

fn release(released: &Released, id: TaskId) {
    let mut ids = released.take();
    ids.push(id);
    released.set(ids);
}

enum Wait {
    Join {
        task: TaskId,
        span: Span,
    },
    /// A join that answers `None` rather than failing when the task is cancelled.
    Await {
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
    Blocked {
        wait: Wait,
        k: K,
    },
    Done(Value),
    Failed,
    /// Stopped by `task.cancel` before it finished: it answers nothing.
    Cancelled,
}

struct Task<K, B> {
    state: TaskState<K, B>,
    origin: Span,
    /// The vector clock, indexed by [`TaskId`]; a production region keeps none.
    stamp: Stamp,
    joiners: Vec<TaskId>,
    /// No handle to it is left, so a production region retires it once it is done.
    unheld: bool,
}

impl<K, B> Task<K, B> {
    fn new(state: TaskState<K, B>, origin: Span, stamp: Stamp) -> Task<K, B> {
        Task {
            state,
            origin,
            stamp,
            joiners: Vec::new(),
            unheld: false,
        }
    }
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
    /// Where it first touched something in `accesses`, else where it gave control back.
    pub site: Option<StepSite>,
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
    /// Only the tasks still kept: a production region retires a finished one nobody can join.
    tasks: BTreeMap<TaskId, Task<K, B>>,
    /// Ascending, the order a seed's choice indexes and the round-robin walks.
    ready: BTreeSet<TaskId>,
    /// Each host token a task waits on, and the task.
    parked: BTreeMap<u64, TaskId>,
    unfinished: usize,
    /// The next spawn's id: ids are never reused, so host state keyed on one never passes on.
    next_id: u64,
    released: Rc<Released>,
    /// The machine whose host state a retired task ends; `None` for a seeded region.
    machine: Option<MachineId>,
    max_steps: u32,
    /// Counted only under [`Policy::Host`]; a seeded region spends the [`Trail`]'s points instead.
    steps: u32,
    policy: Policy,
    /// Where a production region's round-robin looks first: one past the task it ran last.
    resume_from: TaskId,
    current: Option<TaskId>,
    span: Span,
    failure: Option<Diagnostic>,
}

impl<K, B> Scheduler<K, B> {
    pub fn new(region: SimId, span: Span) -> Scheduler<K, B> {
        Scheduler::rooted(region, span, Policy::Seeded, DEFAULT_STEPS, None)
    }

    pub fn production(
        region: SimId,
        span: Span,
        _permit: HostPolicy,
        machine: MachineId,
    ) -> Scheduler<K, B> {
        Scheduler::rooted(region, span, Policy::Host, u32::MAX, Some(machine))
    }

    fn rooted(
        region: SimId,
        span: Span,
        policy: Policy,
        max_steps: u32,
        machine: Option<MachineId>,
    ) -> Scheduler<K, B> {
        Scheduler {
            region,
            tasks: BTreeMap::from([(
                ROOT,
                Task::new(TaskState::Ready(Resumption::Enter), span, vec![0]),
            )]),
            ready: BTreeSet::from([ROOT]),
            parked: BTreeMap::new(),
            unfinished: 1,
            next_id: ROOT.0 + 1,
            released: Rc::new(Cell::new(Vec::new())),
            machine,
            max_steps,
            steps: 0,
            policy,
            resume_from: ROOT,
            current: None,
            span,
            failure: None,
        }
    }

    pub fn rooted_running(mut self) -> Result<Scheduler<K, B>, Diagnostic> {
        if self.steps > 0 || self.current.is_some() || self.next_id > ROOT.0 + 1 {
            return Err(self.internal("a region's root was re-rooted after it had begun"));
        }
        self.ready.remove(&ROOT);
        self.task_mut(ROOT)?.state = TaskState::Running;
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

    /// Every task a seeded region spawned; under the host, only those that can still run or be joined.
    pub fn tasks(&self) -> usize {
        self.tasks.len()
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
            let enabled: Vec<TaskId> = self.ready.iter().copied().collect();
            if !enabled.is_empty() {
                break enabled;
            }
            if let Some(wake) = clock.advance() {
                self.wake(&wake.woken)?;
                continue;
            }
            return if self.unfinished == 0 {
                self.complete()
            } else {
                Err(self.err_deadlock(clock.now(), trail.seed()))
            };
        };

        if trail.point() as u32 >= self.max_steps {
            return Err(self.err_step_budget(trail.seed()));
        }

        let choice = self.choose(trail, &enabled)?;
        let task = enabled[choice];
        let resumption = self.take_ready(task)?;

        self.tick(task);
        trail.push_step(StepRecord {
            region: self.region,
            task,
            enabled,
            choice: choice as u16,
            at: clock.now(),
            accesses: StepFootprint::new(),
            site: None,
            stamp: self.task_mut(task)?.stamp.clone(),
        });
        self.current = Some(task);
        Ok(Turn::Run { task, resumption })
    }

    /// Costs what is ready and what resolved, never what the region has ever run.
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
        self.retire(rt);

        let mut fruitless = 0u32;
        let task = loop {
            if let Some(task) = self.round_robin() {
                break task;
            }
            if self.collect(rt)? {
                fruitless = 0;
                continue;
            }
            if self.unfinished == 0 {
                return self.complete();
            }
            // Under a stop, closing listeners is about to resolve pending `accept`s, so wait.
            if self.parked.is_empty() && !rt.stopping() {
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

        let resumption = self.take_ready(task)?;
        self.resume_from = TaskId(task.0 + 1);
        self.current = Some(task);
        Ok(Turn::Run { task, resumption })
    }

    /// Only a production region parks: the boundary refuses a host operation performed under a
    /// seed before a handler can answer one, so a seeded park is a defect in dispatch.
    pub fn park_on_host(
        &mut self,
        k: K,
        pending: Pending,
        span: Span,
        rt: &dyn HostRuntime,
    ) -> Result<(), Diagnostic> {
        if let Err(d) = self.require(Policy::Host) {
            return Err(d.secondary(span, format!("`{pending}` would have parked this task")));
        }
        let task = self.running()?;
        rt.watch(&pending)?;
        if let Some(other) = self.parked.insert(pending.token, task) {
            return Err(self.internal(format!(
                "{task} parked on `{pending}`, which {other} was already waiting on"
            )));
        }
        self.task_mut(task)?.state = TaskState::Blocked {
            wait: Wait::Host { pending, span },
            k,
        };
        self.current = None;
        Ok(())
    }

    /// Readies each task whose token resolved since the last look; answers whether any had.
    fn collect(&mut self, rt: &dyn HostRuntime) -> Result<bool, Diagnostic> {
        if self.parked.is_empty() {
            return Ok(false);
        }
        // A token no task here waits on was watched by a region of this machine that has ended.
        let mut woken: Vec<(TaskId, Result<Value, Diagnostic>)> = rt
            .resolved()
            .into_iter()
            .filter_map(|(token, answer)| self.parked.remove(&token).map(|task| (task, answer)))
            .collect();
        // Ascending by task, so which failure is reported does not depend on the host's order.
        woken.sort_by_key(|(task, _)| *task);
        let mut answers = Vec::with_capacity(woken.len());
        for (task, answer) in woken {
            let value = answer?;
            let Some(TaskState::Blocked {
                wait: Wait::Host { pending, span },
                ..
            }) = self.tasks.get(&task).map(|t| &t.state)
            else {
                return Err(self.internal(format!(
                    "{task} resolved a host token while not waiting on one"
                )));
            };
            // A parked task's answer bypasses the machine's escape checks, so check it here.
            crate::escape::check(
                &crate::escape::Boundary::HostToken {
                    label: pending.label,
                    token: pending.token,
                },
                &value,
                *span,
            )?;
            answers.push((task, value));
        }
        let woke = !answers.is_empty();
        for (task, value) in answers {
            let k = self.unblock(task)?;
            self.make_ready(task, Resumption::Resume { k, value });
        }
        Ok(woke)
    }

    /// Only between turns: a join names its target by id, read from a handle that may be gone.
    fn retire(&mut self, rt: &dyn HostRuntime) {
        loop {
            let released = self.released.take();
            if released.is_empty() {
                return;
            }
            for id in released {
                let Some(task) = self.tasks.get_mut(&id) else {
                    continue;
                };
                task.unheld = true;
                if !matches!(task.state, TaskState::Done(_)) {
                    continue;
                }
                // Its answer may hold another task's last handle, which the next pass picks up.
                self.tasks.remove(&id);
                if let Some(machine) = self.machine {
                    rt.end_task(machine, id);
                }
            }
        }
    }

    /// The first ready task at or past the one after the last to run, wrapping: round-robin.
    fn round_robin(&self) -> Option<TaskId> {
        self.ready
            .range(self.resume_from..)
            .next()
            .or_else(|| self.ready.first())
            .copied()
    }

    /// Leaves the current task running: the caller must hand the handle to [`Scheduler::suspend`].
    pub fn spawn(&mut self, body: B, span: Span) -> TaskHandle {
        let id = TaskId(self.next_id);
        self.next_id += 1;
        let stamp = match (self.policy, self.current) {
            (Policy::Seeded, Some(parent)) => self
                .tasks
                .get(&parent)
                .map(|t| t.stamp.clone())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        self.tasks.insert(
            id,
            Task::new(
                TaskState::Ready(Resumption::Start { body, span }),
                span,
                stamp,
            ),
        );
        self.ready.insert(id);
        self.unfinished += 1;
        let released = match self.policy {
            Policy::Host => Rc::downgrade(&self.released),
            Policy::Seeded => Weak::new(),
        };
        TaskHandle(Rc::new(Held {
            region: self.region,
            id,
            released,
        }))
    }

    /// At the region's end, the bodies of the tasks it never started, for the caller to release.
    pub fn unstarted(&mut self) -> Vec<B> {
        let mut bodies = Vec::new();
        for (id, task) in self.tasks.iter_mut() {
            match std::mem::replace(&mut task.state, TaskState::Failed) {
                TaskState::Ready(Resumption::Start { body, .. }) => {
                    self.ready.remove(id);
                    bodies.push(body);
                }
                other => task.state = other,
            }
        }
        bodies
    }

    fn tick(&mut self, task: TaskId) {
        let width = self.next_id as usize;
        for t in self.tasks.values_mut() {
            t.stamp.resize(width, 0);
        }
        if let Some(t) = self.tasks.get_mut(&task) {
            t.stamp[task.0 as usize] += 1;
        }
    }

    fn absorb(&mut self, into: TaskId, from: TaskId) {
        if self.policy != Policy::Seeded {
            return;
        }
        let Some(source) = self.tasks.get(&from).map(|t| t.stamp.clone()) else {
            return;
        };
        let Some(target) = self.tasks.get_mut(&into) else {
            return;
        };
        if target.stamp.len() < source.len() {
            target.stamp.resize(source.len(), 0);
        }
        for (slot, seen) in target.stamp.iter_mut().zip(source) {
            *slot = (*slot).max(seen);
        }
    }

    pub fn suspend(&mut self, k: K, value: Value) -> Result<(), Diagnostic> {
        let task = self.running()?;
        self.make_ready(task, Resumption::Resume { k, value });
        self.current = None;
        Ok(())
    }

    pub fn join(&mut self, k: K, target: &TaskHandle, span: Span) -> Result<(), Diagnostic> {
        let task = self.running()?;
        if target.region() != self.region {
            return Err(err_foreign_task(span, target.id()));
        }
        let target = target.id();
        let done = match self.tasks.get(&target).map(|t| &t.state) {
            None => return Err(err_unknown_task(span, target)),
            Some(TaskState::Done(value)) => Some(value.clone()),
            Some(TaskState::Cancelled) => return Err(err_joined_cancelled(span, target)),
            Some(_) => None,
        };
        match done {
            Some(value) => {
                self.absorb(task, target);
                self.make_ready(task, Resumption::Resume { k, value });
            }
            None => {
                self.task_mut(target)?.joiners.push(task);
                self.task_mut(task)?.state = TaskState::Blocked {
                    wait: Wait::Join { task: target, span },
                    k,
                };
            }
        }
        self.current = None;
        Ok(())
    }

    /// A join answering `Some` of what the task answered, or `None` once it is cancelled.
    pub fn await_task(&mut self, k: K, target: &TaskHandle, span: Span) -> Result<(), Diagnostic> {
        let task = self.running()?;
        if target.region() != self.region {
            return Err(err_foreign_task(span, target.id()));
        }
        let target = target.id();
        let settled = match self.tasks.get(&target).map(|t| &t.state) {
            None => return Err(err_unknown_task(span, target)),
            Some(TaskState::Done(value)) => Some(some(value.clone())),
            Some(TaskState::Cancelled) => Some(none()),
            Some(_) => None,
        };
        match settled {
            Some(value) => {
                self.absorb(task, target);
                self.make_ready(task, Resumption::Resume { k, value });
            }
            None => {
                self.task_mut(target)?.joiners.push(task);
                self.task_mut(task)?.state = TaskState::Blocked {
                    wait: Wait::Await { task: target, span },
                    k,
                };
            }
        }
        self.current = None;
        Ok(())
    }

    /// Stops `target` where it stands: whatever it waits on is let go, and when it next runs it
    /// only unwinds. Answers whether it stopped anything, and a body never started, for the caller
    /// to release; a task that already ended, or is already unwinding, answers `false`.
    pub fn cancel(
        &mut self,
        k: K,
        target: &TaskHandle,
        span: Span,
        clock: Option<&mut Clock>,
    ) -> Result<Option<B>, Diagnostic> {
        let task = self.running()?;
        if target.region() != self.region {
            return Err(err_foreign_task(span, target.id()));
        }
        let target = target.id();
        if target == task || target == ROOT {
            return Err(Diagnostic::error(
                codes::RUNTIME_ERROR,
                format!("{target} cannot be cancelled from {task}"),
            )
            .primary(span, "cancelled here")
            .note("a task cancels another one: the region's own body, and a task itself, are not cancelled"));
        }
        let state = match self.tasks.get_mut(&target) {
            None => return Err(err_unknown_task(span, target)),
            Some(t) => std::mem::replace(&mut t.state, TaskState::Cancelled),
        };
        let (stopped, body) = match state {
            TaskState::Ready(Resumption::Start { body, .. }) => {
                self.ready.remove(&target);
                self.settle_cancelled(target)?;
                (true, Some(body))
            }
            TaskState::Ready(Resumption::Resume { k, .. }) => {
                self.task_mut(target)?.state = TaskState::Ready(Resumption::Cancel { k });
                (true, None)
            }
            TaskState::Blocked { wait, k } => {
                match wait {
                    Wait::Timer { .. } => {
                        if let Some(clock) = clock {
                            clock.cancel(target);
                        }
                    }
                    Wait::Join { task: on, .. } | Wait::Await { task: on, .. } => {
                        if let Some(t) = self.tasks.get_mut(&on) {
                            t.joiners.retain(|j| *j != target);
                        }
                    }
                    Wait::Host { pending, .. } => {
                        self.parked.remove(&pending.token);
                    }
                }
                self.make_ready(target, Resumption::Cancel { k });
                (true, None)
            }
            other @ (TaskState::Done(_)
            | TaskState::Failed
            | TaskState::Cancelled
            | TaskState::Ready(Resumption::Cancel { .. } | Resumption::Raise { .. })) => {
                self.task_mut(target)?.state = other;
                (false, None)
            }
            other @ (TaskState::Running | TaskState::Ready(Resumption::Enter)) => {
                self.task_mut(target)?.state = other;
                return Err(self.internal(format!("{target} was cancelled while running")));
            }
        };
        if stopped {
            self.absorb(target, task);
        }
        self.make_ready(
            task,
            Resumption::Resume {
                k,
                value: Value::Bool(stopped),
            },
        );
        self.current = None;
        Ok(body)
    }

    /// The running task finished unwinding after a cancel.
    pub fn finish_cancelled(&mut self) -> Result<(), Diagnostic> {
        let done = self.running()?;
        self.task_mut(done)?.state = TaskState::Cancelled;
        self.settle_cancelled(done)?;
        self.current = None;
        Ok(())
    }

    /// `task` answers nothing: an `await` of it hears `None`, and a `join` of it fails.
    fn settle_cancelled(&mut self, task: TaskId) -> Result<(), Diagnostic> {
        let t = self.task_mut(task)?;
        let joiners = std::mem::take(&mut t.joiners);
        let unheld = t.unheld;
        self.unfinished -= 1;
        for joiner in joiners {
            let resumption = match self.waiting_on(joiner) {
                Some(Wait::Await { .. }) => {
                    let k = self.unblock(joiner)?;
                    Resumption::Resume { k, value: none() }
                }
                Some(Wait::Join { span, .. }) => {
                    let failure = err_joined_cancelled(*span, task);
                    let k = self.unblock(joiner)?;
                    Resumption::Raise { k, failure }
                }
                _ => return Err(self.internal(format!("{joiner} waited on {task} without a join"))),
            };
            self.absorb(joiner, task);
            self.make_ready(joiner, resumption);
        }
        if unheld {
            release(&self.released, task);
        }
        Ok(())
    }

    fn waiting_on(&self, task: TaskId) -> Option<&Wait> {
        match self.tasks.get(&task).map(|t| &t.state) {
            Some(TaskState::Blocked { wait, .. }) => Some(wait),
            _ => None,
        }
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
        let task = self.running()?;
        self.task_mut(task)?.state = TaskState::Blocked {
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
        let done = self.running()?;
        let task = self.task_mut(done)?;
        task.state = TaskState::Done(value.clone());
        let joiners = std::mem::take(&mut task.joiners);
        let unheld = task.unheld;
        self.unfinished -= 1;
        for joiner in joiners {
            let answer = match self.waiting_on(joiner) {
                Some(Wait::Await { .. }) => some(value.clone()),
                _ => value.clone(),
            };
            let k = self.unblock(joiner)?;
            self.absorb(joiner, done);
            self.make_ready(joiner, Resumption::Resume { k, value: answer });
        }
        if unheld {
            release(&self.released, done);
        }
        self.current = None;
        Ok(())
    }

    pub fn fail(&mut self, failure: Diagnostic, seed: &Seed) -> Diagnostic {
        let mut failure = failure;
        if let Some(task) = self.current {
            // The failing task is itself unfinished.
            let live = self.unfinished.saturating_sub(1);
            if self.next_id > ROOT.0 + 1 {
                failure = failure.note(match self.policy {
                    Policy::Seeded => format!(
                        "failed in task {task} of a simulated region, with {live} other task(s) unfinished; replay with seed {seed}"
                    ),
                    Policy::Host => format!(
                        "failed in task {task} of a host-scheduled region, with {live} other task(s) unfinished; a host-backed run is not replayable from a seed"
                    ),
                });
            }
            if let Some(t) = self.tasks.get_mut(&task) {
                t.state = TaskState::Failed;
            }
        }
        self.current = None;
        self.failure = Some(failure.clone());
        failure
    }

    fn complete(&self) -> Result<Turn<K, B>, Diagnostic> {
        match self.tasks.get(&ROOT).map(|t| &t.state) {
            Some(TaskState::Done(value)) => Ok(Turn::Complete(value.clone())),
            _ => Err(self.internal("the region finished without its body returning")),
        }
    }

    fn make_ready(&mut self, task: TaskId, resumption: Resumption<K, B>) {
        if let Some(t) = self.tasks.get_mut(&task) {
            t.state = TaskState::Ready(resumption);
            self.ready.insert(task);
        }
    }

    /// Takes a chosen task off the ready set and marks it running.
    fn take_ready(&mut self, task: TaskId) -> Result<Resumption<K, B>, Diagnostic> {
        self.ready.remove(&task);
        let t = self.task_mut(task)?;
        match std::mem::replace(&mut t.state, TaskState::Running) {
            TaskState::Ready(resumption) => Ok(resumption),
            other => {
                t.state = other;
                Err(self.internal(format!("{task} was chosen but is not enabled")))
            }
        }
    }

    /// A blocked task's continuation, leaving it running until the caller readies it.
    fn unblock(&mut self, task: TaskId) -> Result<K, Diagnostic> {
        let t = self.task_mut(task)?;
        match std::mem::replace(&mut t.state, TaskState::Running) {
            TaskState::Blocked { k, .. } => Ok(k),
            other => {
                t.state = other;
                Err(self.internal(format!("{task} was woken but was not blocked")))
            }
        }
    }

    fn task_mut(&mut self, task: TaskId) -> Result<&mut Task<K, B>, Diagnostic> {
        let (span, policy) = (self.span, self.policy);
        self.tasks.get_mut(&task).ok_or_else(|| {
            out_of_order(span, policy, format!("{task} is not a task of this region"))
        })
    }

    /// Readies timer-fired tasks in ascending order, so the seed, not the host, orders ties.
    fn wake(&mut self, woken: &[TaskId]) -> Result<(), Diagnostic> {
        for id in woken {
            match self.tasks.get(id).map(|t| &t.state) {
                Some(TaskState::Blocked {
                    wait: Wait::Timer { .. },
                    ..
                }) => {}
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
            }
            let k = self.unblock(*id)?;
            self.make_ready(
                *id,
                Resumption::Resume {
                    k,
                    value: Value::Unit,
                },
            );
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

    fn running(&self) -> Result<TaskId, Diagnostic> {
        match self.current {
            Some(task) => Ok(task),
            None => Err(self
                .internal("the scheduler was asked to suspend a task while no task was running")),
        }
    }

    fn err_deadlock(&self, now: i64, seed: &Seed) -> Diagnostic {
        let blocked: Vec<(TaskId, &Wait, Span)> = self.blocked().collect();
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
                Wait::Join { task, span } | Wait::Await { task, span } => {
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
                plural(self.blocked().count(), "task is", "tasks are")
            ),
        )
        .primary(self.span, "no task in this region can make progress");
        for (id, wait, origin) in self.blocked() {
            let (span, message) = match wait {
                Wait::Join { task, span } | Wait::Await { task, span } => {
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

    /// Ascending by id; only a diagnostic walks every task.
    fn blocked(&self) -> impl Iterator<Item = (TaskId, &Wait, Span)> {
        self.tasks.iter().filter_map(|(id, t)| match &t.state {
            TaskState::Blocked { wait, .. } => Some((*id, wait, t.origin)),
            _ => None,
        })
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
        .note("`Scheduler::next` drives a seeded region and the host entry points drive a production one; the two are never interchangeable")
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
        out_of_order(self.span, self.policy, message)
    }
}

fn out_of_order(span: Span, policy: Policy, message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(codes::INTERNAL_ERROR, message).primary(
        span,
        match policy {
            Policy::Seeded => "the simulated scheduler was driven out of order",
            Policy::Host => "the production scheduler was driven out of order",
        },
    )
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
fn err_foreign_task(span: Span, task: TaskId) -> Diagnostic {
    Diagnostic::error(
        codes::TASK_ESCAPES_SCOPE,
        format!("`{task}` is a task of another region"),
    )
    .primary(
        span,
        "this handle was spawned by another region's scheduler",
    )
    .note("every region numbers its own tasks, so here the handle's id names another task, or none")
    .note("join the task inside the region that spawned it")
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

#[cold]
#[inline(never)]
fn err_joined_cancelled(span: Span, task: TaskId) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("`{task}` was cancelled, so it has nothing to join"),
    )
    .primary(span, "joined here")
    .note(
        "`task.await` answers `None` for a cancelled task, where `task.join` has no answer to give",
    )
}

fn some(value: Value) -> Value {
    Value::ctor("Some", vec![value])
}

fn none() -> Value {
    Value::ctor("None", vec![])
}
