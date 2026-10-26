//! The production scheduler: a region a `task` operation outside `simulate` opens, scheduling its
//! tasks round-robin against the host runtime. It holds what its tasks pass each other, the
//! payloads `P` a task answers and a channel carries, without reading them, and answers each
//! operation with a [`Reply`] the region writes as the operation's answer.

use crate::host::{HostBinding, HostRuntime, MachineId, Pending};
use crate::sim::{ChanId, SimId, TaskId};
use crate::value::Value;
use crate::{Diagnostic, Span, codes};
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::rc::{Rc, Weak};

/// The task a region's root runs as.
pub const ROOT: TaskId = TaskId(0);

/// What an operation a task performed is answered.
#[derive(Debug, PartialEq)]
pub enum Reply<P> {
    /// What the scheduler, a region's handlers or the host answer themselves: a handle, a flag, a
    /// reading, nothing.
    Value(Value),
    /// What a joined task answered.
    Item(P),
    /// `Some` of what an await or a receive took; `None` for a cancelled task or a channel closed
    /// and empty.
    Maybe(Option<P>),
    /// `Some` of the arm a select went on, by its place, beside what it received or sent, which is
    /// `None` where the arm's channel was closed; `None` where no arm could go.
    Chosen(Option<(usize, Option<P>)>),
}

pub enum Resumption<K, B, P> {
    Enter,
    Start {
        body: B,
        span: Span,
    },
    Resume {
        k: K,
        value: Reply<P>,
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

/// What `task.channel(capacity)` may hold, or the raise a negative capacity is.
pub fn capacity_of(capacity: i64, span: Span) -> Result<usize, Diagnostic> {
    usize::try_from(capacity).map_err(|_| {
        Diagnostic::error(
            codes::RUNTIME_ERROR,
            format!("a channel cannot hold {capacity} values"),
        )
        .primary(span, "made here")
        .note("a capacity is how many sent values may wait for a receiver; `0` is a rendezvous")
    })
}

pub enum Turn<K, B, P> {
    Run {
        task: TaskId,
        resumption: Resumption<K, B, P>,
    },
    /// What the region's own body answered.
    Complete(P),
}

/// Permission to build a production scheduler; unobtainable in a run bound to no host.
pub struct HostPolicy(());

impl HostPolicy {
    /// `None` when nothing is bound.
    pub fn of(binding: &HostBinding) -> Option<HostPolicy> {
        binding.is_granted().then_some(HostPolicy(()))
    }
}

/// A `Task` value: its region and its id, counted so a production region can retire it once unheld.
#[derive(Clone)]
pub struct TaskHandle(Rc<HeldTask>);

struct HeldTask {
    /// Every region numbers its own tasks, so an id alone names a task only in its own region.
    region: SimId,
    id: TaskId,
    /// Its region's [`Released`]; dangling for an [`TaskHandle::unowned`] one, which nothing counts.
    released: Weak<Released>,
}

/// The tasks whose last handle went since the scheduler last looked.
type Released = Cell<Vec<TaskId>>;

impl TaskHandle {
    /// A handle to `id` of `region` that no region counts toward retiring the task.
    pub fn unowned(region: SimId, id: TaskId) -> TaskHandle {
        TaskHandle(Rc::new(HeldTask {
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

impl Drop for HeldTask {
    fn drop(&mut self) {
        if let Some(released) = self.released.upgrade() {
            release(&released, self.id);
        }
    }
}

/// A `Chan` value: its region and its id, since every region numbers its own channels.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct ChanHandle {
    pub region: SimId,
    pub id: ChanId,
}

/// A bounded queue between tasks. A blocked sender's value waits in its [`Wait::Send`].
struct Channel<P> {
    capacity: usize,
    /// Sent and not yet received.
    queue: VecDeque<P>,
    senders: VecDeque<TaskId>,
    receivers: VecDeque<TaskId>,
    closed: bool,
}

/// How a send that does not wait went: sent or refused by a closed channel, or the value back.
enum Sent<P> {
    Went(bool),
    Waits(P),
}

fn release(released: &Released, id: TaskId) {
    let mut ids = released.take();
    ids.push(id);
    released.set(ids);
}

/// One arm of a `task.select`: a receive from `chan`, or a send of `send` to it.
struct Arm<P> {
    chan: ChanId,
    send: Option<P>,
}

/// What `task.cancel` did beside answering.
#[derive(Debug, PartialEq, Eq)]
pub struct Stopped<B> {
    /// Whether it stopped the task, which is what it answered.
    pub stopped: bool,
    /// The body of a task that never started, for the caller to release.
    pub unstarted: Option<B>,
}

/// A `bracket`'s `acquire` or its `release`: where a cancel does not take a task's answer from it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shield {
    /// An operation that answers keeps its answer and a wait is let go; the cancel lands when the
    /// `acquire` returns, so what it took is released.
    Acquire,
    /// Nothing is let go; the cancel lands when the `release` returns.
    Release,
}

enum Wait<P> {
    Join {
        task: TaskId,
        span: Span,
    },
    /// A join that answers `None` rather than failing when the task is cancelled.
    Await {
        task: TaskId,
        span: Span,
    },
    /// Until a reading of the host's clock.
    Timer {
        until: i64,
        span: Span,
    },
    Host {
        pending: Pending,
        span: Span,
    },
    /// A send to a full channel, holding what it sends until a receive makes room.
    Send {
        chan: ChanId,
        value: P,
        span: Span,
    },
    Recv {
        chan: ChanId,
        span: Span,
    },
    /// A select none of whose arms could go, waiting on every arm's channel at once.
    Select {
        arms: Vec<Arm<P>>,
        span: Span,
    },
}

enum TaskState<K, B, P> {
    Ready(Resumption<K, B, P>),
    Running,
    Blocked {
        wait: Wait<P>,
        k: K,
    },
    Done(P),
    Failed,
    /// Stopped by `task.cancel` before it finished: it answers nothing.
    Cancelled,
}

struct Task<K, B, P> {
    state: TaskState<K, B, P>,
    origin: Span,
    joiners: Vec<TaskId>,
    /// No handle to it is left, so the region retires it once it is done.
    unheld: bool,
    /// A cancel reached it: a later one changes nothing, and every step it still takes reads it.
    cancelled: bool,
    /// A cancel is held back while it stands in a bracket's `acquire` or `release`.
    stopping: bool,
    /// The `acquire`s and the `release`s it stands in.
    acquiring: u32,
    releasing: u32,
}

impl<K, B, P> Task<K, B, P> {
    fn new(state: TaskState<K, B, P>, origin: Span) -> Task<K, B, P> {
        Task {
            state,
            origin,
            joiners: Vec::new(),
            unheld: false,
            cancelled: false,
            stopping: false,
            acquiring: 0,
            releasing: 0,
        }
    }
}

/// The scheduler of one region: `K` how a suspended task is resumed, `B` a task's body before it
/// starts, and `P` what a task answers and a channel carries, which it clones for each task that
/// hears it and drops when the region lets it go.
pub struct Scheduler<K, B, P> {
    region: SimId,
    /// Only the tasks still kept: a finished one nobody can join is retired.
    tasks: BTreeMap<TaskId, Task<K, B, P>>,
    /// Ascending, the order the round-robin walks.
    ready: BTreeSet<TaskId>,
    /// Each host token a task waits on, and the task.
    parked: BTreeMap<u64, TaskId>,
    /// The sleepers, each beside the reading of the host's clock that wakes it.
    timers: BTreeSet<(i64, TaskId)>,
    channels: BTreeMap<ChanId, Channel<P>>,
    unfinished: usize,
    /// The next spawn's id: ids are never reused, so host state keyed on one never passes on.
    next_id: u64,
    /// The next channel's id. One past a channel not in `channels` is spent: closed and empty.
    next_chan: u64,
    released: Rc<Released>,
    /// The machine whose host state a retired task ends.
    machine: MachineId,
    /// No budget where `u32::MAX`, the default.
    max_steps: u32,
    steps: u32,
    /// Where the round-robin looks first: one past the task it ran last.
    resume_from: TaskId,
    current: Option<TaskId>,
    span: Span,
    failure: Option<Diagnostic>,
}

impl<K, B, P: Clone> Scheduler<K, B, P> {
    pub fn production(
        region: SimId,
        span: Span,
        _permit: HostPolicy,
        machine: MachineId,
    ) -> Scheduler<K, B, P> {
        Scheduler {
            region,
            tasks: BTreeMap::from([(ROOT, Task::new(TaskState::Ready(Resumption::Enter), span))]),
            ready: BTreeSet::from([ROOT]),
            parked: BTreeMap::new(),
            timers: BTreeSet::new(),
            channels: BTreeMap::new(),
            unfinished: 1,
            next_id: ROOT.0 + 1,
            next_chan: 0,
            released: Rc::new(Cell::new(Vec::new())),
            machine,
            max_steps: u32::MAX,
            steps: 0,
            resume_from: ROOT,
            current: None,
            span,
            failure: None,
        }
    }

    pub fn rooted_running(mut self) -> Result<Scheduler<K, B, P>, Diagnostic> {
        if self.steps > 0 || self.current.is_some() || self.next_id > ROOT.0 + 1 {
            return Err(self.internal("a region's root was re-rooted after it had begun"));
        }
        self.ready.remove(&ROOT);
        self.task_mut(ROOT)?.state = TaskState::Running;
        self.current = Some(ROOT);
        self.steps = 1;
        Ok(self)
    }

    pub fn with_step_budget(mut self, steps: u32) -> Scheduler<K, B, P> {
        self.max_steps = steps.max(1);
        self
    }

    pub fn current(&self) -> Option<TaskId> {
        self.current
    }

    /// The tasks that can still run or be joined.
    pub fn tasks(&self) -> usize {
        self.tasks.len()
    }

    /// Costs what is ready and what resolved, never what the region has ever run.
    pub fn next_host(&mut self, rt: &dyn HostRuntime) -> Result<Turn<K, B, P>, Diagnostic> {
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
            // Ahead of the ready, so a task that never waits starves no sleeper.
            self.wake_due(rt)?;
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
            let Some(&(deadline, _)) = self.timers.first() else {
                // Under a stop, closing listeners is about to resolve pending `accept`s, so wait.
                if self.parked.is_empty() && !rt.stopping() {
                    return Err(self.err_host_deadlock());
                }
                rt.park()?;
                if let Some(expired) = rt.drain_expired() {
                    return Err(expired);
                }
                // A park woken by a stop is how an idle service sees a signal, so it is not
                // fruitless.
                if !rt.stopping() {
                    fruitless += 1;
                    if fruitless > FRUITLESS_PARKS {
                        return Err(self.err_park_made_no_progress());
                    }
                }
                continue;
            };
            // Bounded by the deadline, so a return that woke nothing cannot go on for ever.
            rt.park_until(deadline)?;
            if let Some(expired) = rt.drain_expired() {
                return Err(expired);
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

    /// Parks the running task until the host resolves `pending`, the region's other tasks running
    /// meanwhile.
    pub fn park_on_host(
        &mut self,
        k: K,
        pending: Pending,
        span: Span,
        rt: &dyn HostRuntime,
    ) -> Result<(), Diagnostic> {
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

    /// Parks the running task until the host's clock has run `nanos` on, the region's other tasks
    /// running meanwhile. No span or less is a yield.
    pub fn sleep_on_host(
        &mut self,
        k: K,
        nanos: i64,
        span: Span,
        rt: &dyn HostRuntime,
    ) -> Result<(), Diagnostic> {
        if nanos <= 0 {
            return self.suspend(k, Value::Unit);
        }
        let task = self.running()?;
        let until = rt
            .now()
            .map_err(|d| d.primary(span, "slept here"))?
            .saturating_add(nanos);
        self.timers.insert((until, task));
        self.task_mut(task)?.state = TaskState::Blocked {
            wait: Wait::Timer { until, span },
            k,
        };
        self.current = None;
        Ok(())
    }

    /// Readies each sleeper whose deadline the host's clock has reached, earliest first.
    fn wake_due(&mut self, rt: &dyn HostRuntime) -> Result<(), Diagnostic> {
        if self.timers.is_empty() {
            return Ok(());
        }
        let now = rt.now()?;
        let mut due = Vec::new();
        while let Some(&(until, task)) = self.timers.first() {
            if until > now {
                break;
            }
            self.timers.remove(&(until, task));
            due.push(task);
        }
        self.wake(&due)
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
            self.make_ready(
                task,
                Resumption::Resume {
                    k,
                    value: Reply::Value(value),
                },
            );
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
                rt.end_task(self.machine, id);
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
        self.tasks.insert(
            id,
            Task::new(TaskState::Ready(Resumption::Start { body, span }), span),
        );
        self.ready.insert(id);
        self.unfinished += 1;
        TaskHandle(Rc::new(HeldTask {
            region: self.region,
            id,
            released: Rc::downgrade(&self.released),
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

    /// Readies the running task with `value`, which the region answered it itself, and gives
    /// control back.
    pub fn suspend(&mut self, k: K, value: Value) -> Result<(), Diagnostic> {
        let task = self.running()?;
        self.answer(task, k, Reply::Value(value))
    }

    pub fn join(&mut self, k: K, target: &TaskHandle, span: Span) -> Result<(), Diagnostic> {
        let task = self.running()?;
        if target.region() != self.region {
            return Err(err_foreign_task(span, target.id()));
        }
        let target = target.id();
        let settled = match self.tasks.get(&target).map(|t| &t.state) {
            None => return Err(err_unknown_task(span, target)),
            Some(TaskState::Done(value)) => Some(Ok(value.clone())),
            Some(TaskState::Cancelled) => Some(Err(err_joined_cancelled(span, target))),
            Some(_) => None,
        };
        match settled {
            Some(answer) => {
                let resumption = match answer {
                    Ok(value) => Resumption::Resume {
                        k,
                        value: Reply::Item(value),
                    },
                    // A raise in the joiner, as it is when the task is cancelled under its join.
                    Err(failure) => Resumption::Raise { k, failure },
                };
                self.make_ready(task, resumption);
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
            Some(TaskState::Done(value)) => Some(Reply::Maybe(Some(value.clone()))),
            Some(TaskState::Cancelled) => Some(Reply::Maybe(None)),
            Some(_) => None,
        };
        match settled {
            Some(value) => self.make_ready(task, Resumption::Resume { k, value }),
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
    /// only unwinds. A task standing in a bracket's `acquire` keeps an answer it was already
    /// given, and one standing in a `release` keeps its wait too; the cancel lands when that
    /// returns. Answers whether it stopped anything; a task that already ended, or that a cancel
    /// already reached, answers `false`.
    pub fn cancel(
        &mut self,
        k: K,
        target: &TaskHandle,
        span: Span,
    ) -> Result<Stopped<B>, Diagnostic> {
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
        let Some(t) = self.tasks.get_mut(&target) else {
            return Err(err_unknown_task(span, target));
        };
        let ended = matches!(
            t.state,
            TaskState::Done(_) | TaskState::Failed | TaskState::Cancelled
        );
        let stops = !ended && !t.cancelled;
        let mut stopped = Stopped {
            stopped: stops,
            unstarted: None,
        };
        if stops {
            t.cancelled = true;
            let (acquiring, releasing) = (t.acquiring > 0, t.releasing > 0);
            match std::mem::replace(&mut t.state, TaskState::Cancelled) {
                TaskState::Ready(Resumption::Start { body, .. }) => {
                    stopped.unstarted = Some(body);
                    self.ready.remove(&target);
                    self.settle_cancelled(target)?;
                }
                answered @ TaskState::Ready(
                    Resumption::Resume { .. } | Resumption::Raise { .. },
                ) if acquiring || releasing => {
                    t.state = answered;
                    t.stopping = true;
                }
                TaskState::Ready(Resumption::Resume { k, .. } | Resumption::Raise { k, .. }) => {
                    t.state = TaskState::Ready(Resumption::Cancel { k });
                }
                waiting @ TaskState::Blocked { .. } if releasing => {
                    t.state = waiting;
                    t.stopping = true;
                }
                TaskState::Blocked { wait, k } => {
                    self.let_go(target, wait);
                    self.make_ready(target, Resumption::Cancel { k });
                }
                other => {
                    t.state = other;
                    return Err(self.internal(format!("{target} was cancelled while running")));
                }
            }
        }
        self.make_ready(
            task,
            Resumption::Resume {
                k,
                value: Reply::Value(Value::Bool(stops)),
            },
        );
        self.current = None;
        Ok(stopped)
    }

    /// Takes `target` off whatever `wait` had it waiting on.
    fn let_go(&mut self, target: TaskId, wait: Wait<P>) {
        match wait {
            Wait::Timer { until, .. } => {
                self.timers.remove(&(until, target));
            }
            Wait::Join { task: on, .. } | Wait::Await { task: on, .. } => {
                if let Some(t) = self.tasks.get_mut(&on) {
                    t.joiners.retain(|j| *j != target);
                }
            }
            Wait::Host { pending, .. } => {
                self.parked.remove(&pending.token);
            }
            Wait::Send { chan, .. } | Wait::Recv { chan, .. } => self.unwait(target, [chan]),
            Wait::Select { arms, .. } => self.unwait(target, arms.iter().map(|arm| arm.chan)),
        }
    }

    /// Takes `task` out of the queues of each of `chans`.
    fn unwait(&mut self, task: TaskId, chans: impl IntoIterator<Item = ChanId>) {
        for chan in chans {
            if let Some(ch) = self.channels.get_mut(&chan) {
                ch.senders.retain(|t| *t != task);
                ch.receivers.retain(|t| *t != task);
            }
        }
    }

    /// The running task enters a bracket's `acquire` or `release`.
    pub fn shield(&mut self, shield: Shield) -> Option<TaskId> {
        let task = self.current?;
        let t = self.tasks.get_mut(&task)?;
        match shield {
            Shield::Acquire => t.acquiring += 1,
            Shield::Release => t.releasing += 1,
        }
        Some(task)
    }

    /// `task` leaves what [`Scheduler::shield`] entered. Answers whether a cancel was held back
    /// and lands now: the task is to unwind from here.
    pub fn unshield(&mut self, task: TaskId, shield: Shield) -> bool {
        let Some(t) = self.tasks.get_mut(&task) else {
            return false;
        };
        match shield {
            Shield::Acquire => t.acquiring = t.acquiring.saturating_sub(1),
            Shield::Release => t.releasing = t.releasing.saturating_sub(1),
        }
        let lands = t.stopping && t.acquiring == 0 && t.releasing == 0;
        if lands {
            t.stopping = false;
        }
        lands
    }

    /// Lets go of a wait `task` just began with a cancel held back for the `acquire` it stands
    /// in: there an operation that answers keeps its answer, and a wait does not start.
    pub fn refuse_wait(&mut self, task: TaskId) {
        let Some(t) = self.tasks.get_mut(&task) else {
            return;
        };
        if !t.stopping || t.releasing > 0 || !matches!(t.state, TaskState::Blocked { .. }) {
            return;
        }
        t.stopping = false;
        match std::mem::replace(&mut t.state, TaskState::Cancelled) {
            TaskState::Blocked { wait, k } => {
                self.let_go(task, wait);
                self.make_ready(task, Resumption::Cancel { k });
            }
            other => t.state = other,
        }
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
                    Resumption::Resume {
                        k,
                        value: Reply::Maybe(None),
                    }
                }
                Some(Wait::Join { span, .. }) => {
                    let failure = err_joined_cancelled(*span, task);
                    let k = self.unblock(joiner)?;
                    Resumption::Raise { k, failure }
                }
                _ => return Err(self.internal(format!("{joiner} waited on {task} without a join"))),
            };
            self.make_ready(joiner, resumption);
        }
        if unheld {
            release(&self.released, task);
        }
        Ok(())
    }

    /// A channel holding up to `capacity` values no receiver has taken; `0` is a rendezvous.
    pub fn channel(&mut self, k: K, capacity: usize) -> Result<(), Diagnostic> {
        let task = self.running()?;
        let id = ChanId(self.next_chan);
        self.next_chan += 1;
        self.channels.insert(
            id,
            Channel {
                capacity,
                queue: VecDeque::new(),
                senders: VecDeque::new(),
                receivers: VecDeque::new(),
                closed: false,
            },
        );
        let handle = ChanHandle {
            region: self.region,
            id,
        };
        self.answer(task, k, Reply::Value(Value::Chan(handle)))
    }

    /// Hands `value` to the longest-waiting receiver, or queues it while there is room; otherwise
    /// the task waits for a receive to make room. Answers `false`, sending nothing, once closed.
    pub fn send(
        &mut self,
        k: K,
        chan: &ChanHandle,
        value: P,
        span: Span,
    ) -> Result<(), Diagnostic> {
        let task = self.running()?;
        let id = self.channel_of(chan, span)?;
        let value = match self.send_now(id, value)? {
            Sent::Went(sent) => return self.answer(task, k, Reply::Value(Value::Bool(sent))),
            Sent::Waits(value) => value,
        };
        if let Some(ch) = self.channels.get_mut(&id) {
            ch.senders.push_back(task);
        }
        self.task_mut(task)?.state = TaskState::Blocked {
            wait: Wait::Send {
                chan: id,
                value,
                span,
            },
            k,
        };
        self.current = None;
        Ok(())
    }

    /// A send that does not wait: handed to the longest-waiting receiver, queued while there is
    /// room, or refused by a closed channel.
    fn send_now(&mut self, id: ChanId, value: P) -> Result<Sent<P>, Diagnostic> {
        let Some(ch) = self.channels.get_mut(&id) else {
            return Ok(Sent::Went(false));
        };
        if ch.closed {
            return Ok(Sent::Went(false));
        }
        if let Some(receiver) = ch.receivers.pop_front() {
            self.deliver(receiver, id, Some(value))?;
            return Ok(Sent::Went(true));
        }
        if ch.queue.len() < ch.capacity {
            ch.queue.push_back(value);
            return Ok(Sent::Went(true));
        }
        Ok(Sent::Waits(value))
    }

    /// Readies `receiver`, which waited to receive on `chan`, with `value`, or with the close.
    fn deliver(
        &mut self,
        receiver: TaskId,
        chan: ChanId,
        value: Option<P>,
    ) -> Result<(), Diagnostic> {
        let t = self.task_mut(receiver)?;
        let (k, answer) = match std::mem::replace(&mut t.state, TaskState::Running) {
            TaskState::Blocked {
                wait: Wait::Recv { .. },
                k,
            } => (k, Reply::Maybe(value)),
            TaskState::Blocked {
                wait: Wait::Select { arms, .. },
                k,
            } => {
                let at = arms
                    .iter()
                    .position(|arm| arm.chan == chan && arm.send.is_none());
                self.unwait(receiver, arms.iter().map(|arm| arm.chan));
                match at {
                    Some(at) => (k, Reply::Chosen(Some((at, value)))),
                    None => {
                        return Err(self.internal(format!(
                            "{receiver} was taken as a receiver on {chan}, which its select does not receive from"
                        )));
                    }
                }
            }
            other => {
                t.state = other;
                return Err(self.internal(format!(
                    "{receiver} was taken as a receiver but was not receiving"
                )));
            }
        };
        self.make_ready(receiver, Resumption::Resume { k, value: answer });
        Ok(())
    }

    /// Readies `sender`, which waited to send on `chan`, with whether its value went, and
    /// answers the value.
    fn take_sent(&mut self, sender: TaskId, chan: ChanId, sent: bool) -> Result<P, Diagnostic> {
        let t = self.task_mut(sender)?;
        let (k, value, answer) = match std::mem::replace(&mut t.state, TaskState::Running) {
            TaskState::Blocked {
                wait: Wait::Send { value, .. },
                k,
            } => (k, value, Reply::Value(Value::Bool(sent))),
            TaskState::Blocked {
                wait: Wait::Select { mut arms, .. },
                k,
            } => {
                let at = arms
                    .iter()
                    .position(|arm| arm.chan == chan && arm.send.is_some());
                self.unwait(sender, arms.iter().map(|arm| arm.chan));
                match at.and_then(|at| arms[at].send.take().map(|value| (at, value))) {
                    Some((at, value)) => {
                        let answer = Reply::Chosen(Some((at, sent.then(|| value.clone()))));
                        (k, value, answer)
                    }
                    None => {
                        return Err(self.internal(format!(
                            "{sender} was taken as a sender on {chan}, which its select does not send to"
                        )));
                    }
                }
            }
            other => {
                t.state = other;
                return Err(self.internal(format!(
                    "{sender} was taken as a sender but was not sending"
                )));
            }
        };
        self.make_ready(sender, Resumption::Resume { k, value: answer });
        Ok(value)
    }

    /// Takes the oldest value sent, `None` once the channel is closed and empty; otherwise the task
    /// waits for a send.
    pub fn recv(&mut self, k: K, chan: &ChanHandle, span: Span) -> Result<(), Diagnostic> {
        let task = self.running()?;
        let id = self.channel_of(chan, span)?;
        if let Some(taken) = self.receive_now(id)? {
            return self.answer(task, k, Reply::Maybe(taken));
        }
        if let Some(ch) = self.channels.get_mut(&id) {
            ch.receivers.push_back(task);
        }
        self.task_mut(task)?.state = TaskState::Blocked {
            wait: Wait::Recv { chan: id, span },
            k,
        };
        self.current = None;
        Ok(())
    }

    /// A receive that does not wait: `Some` of the oldest value sent, `Some(None)` from a channel
    /// closed and empty, and `None` where it would wait.
    fn receive_now(&mut self, id: ChanId) -> Result<Option<Option<P>>, Diagnostic> {
        enum Taken<P> {
            Queued(P),
            From(TaskId),
            Closed,
            Waits,
        }
        let taken = match self.channels.get_mut(&id) {
            None => Taken::Closed,
            Some(ch) => {
                if let Some(value) = ch.queue.pop_front() {
                    Taken::Queued(value)
                } else if let Some(sender) = ch.senders.pop_front() {
                    Taken::From(sender)
                } else if ch.closed {
                    Taken::Closed
                } else {
                    Taken::Waits
                }
            }
        };
        match taken {
            Taken::Queued(value) => {
                // The room this made takes the longest-waiting sender's value.
                let waiting = self
                    .channels
                    .get_mut(&id)
                    .and_then(|ch| ch.senders.pop_front());
                if let Some(sender) = waiting {
                    let sent = self.take_sent(sender, id, true)?;
                    if let Some(ch) = self.channels.get_mut(&id) {
                        ch.queue.push_back(sent);
                    }
                }
                self.retire_if_spent(id);
                Ok(Some(Some(value)))
            }
            Taken::From(sender) => {
                let sent = self.take_sent(sender, id, true)?;
                Ok(Some(Some(sent)))
            }
            Taken::Closed => Ok(Some(None)),
            Taken::Waits => Ok(None),
        }
    }

    /// Ends sending: each waiting receiver hears `None` and each waiting sender `false`, while
    /// what was queued before stays to be received. Closing a closed channel changes nothing.
    pub fn close(&mut self, k: K, chan: &ChanHandle, span: Span) -> Result<(), Diagnostic> {
        let task = self.running()?;
        let id = self.channel_of(chan, span)?;
        if let Some(ch) = self.channels.get_mut(&id) {
            ch.closed = true;
        }
        // One at a time off the live queues: a select may wait on both sides of one channel.
        while let Some(receiver) = self
            .channels
            .get_mut(&id)
            .and_then(|ch| ch.receivers.pop_front())
        {
            self.deliver(receiver, id, None)?;
        }
        while let Some(sender) = self
            .channels
            .get_mut(&id)
            .and_then(|ch| ch.senders.pop_front())
        {
            self.take_sent(sender, id, false)?;
        }
        self.retire_if_spent(id);
        self.answer(task, k, Reply::Value(Value::Unit))
    }

    /// The first arm, in the order given, that can go without waiting goes, and the task hears
    /// `Some((index, got))`: `got` is `Some` of the value received or sent, and `None` where the
    /// arm's channel was closed. With no arm able to go the task hears `None` at once, or with
    /// `wait` waits on every arm for the first that can.
    pub fn select(
        &mut self,
        k: K,
        arms: Vec<(ChanHandle, Option<P>)>,
        wait: bool,
        span: Span,
    ) -> Result<(), Diagnostic> {
        let task = self.running()?;
        let mut waiting = Vec::with_capacity(arms.len());
        for (chan, send) in arms {
            waiting.push(Arm {
                chan: self.channel_of(&chan, span)?,
                send,
            });
        }
        for (at, arm) in waiting.iter_mut().enumerate() {
            let got = match arm.send.take() {
                None => self.receive_now(arm.chan)?,
                Some(value) => match self.send_now(arm.chan, value.clone())? {
                    Sent::Went(sent) => Some(sent.then_some(value)),
                    Sent::Waits(value) => {
                        arm.send = Some(value);
                        None
                    }
                },
            };
            if let Some(got) = got {
                return self.answer(task, k, Reply::Chosen(Some((at, got))));
            }
        }
        if !wait {
            return self.answer(task, k, Reply::Chosen(None));
        }
        for arm in &waiting {
            if let Some(ch) = self.channels.get_mut(&arm.chan) {
                let queue = if arm.send.is_some() {
                    &mut ch.senders
                } else {
                    &mut ch.receivers
                };
                if !queue.contains(&task) {
                    queue.push_back(task);
                }
            }
        }
        self.task_mut(task)?.state = TaskState::Blocked {
            wait: Wait::Select {
                arms: waiting,
                span,
            },
            k,
        };
        self.current = None;
        Ok(())
    }

    /// The channel a handle names in this region, which may be spent.
    fn channel_of(&self, chan: &ChanHandle, span: Span) -> Result<ChanId, Diagnostic> {
        if chan.region != self.region {
            return Err(err_foreign_channel(span, chan.id));
        }
        if chan.id.0 >= self.next_chan {
            return Err(self.internal(format!("{} is not a channel of this region", chan.id)));
        }
        Ok(chan.id)
    }

    /// A closed channel with nothing queued and nobody waiting answers as a spent one does.
    fn retire_if_spent(&mut self, id: ChanId) {
        if self.channels.get(&id).is_some_and(|ch| {
            ch.closed && ch.queue.is_empty() && ch.senders.is_empty() && ch.receivers.is_empty()
        }) {
            self.channels.remove(&id);
        }
    }

    /// Readies the running task with `value` and gives control back.
    fn answer(&mut self, task: TaskId, k: K, value: Reply<P>) -> Result<(), Diagnostic> {
        self.make_ready(task, Resumption::Resume { k, value });
        self.current = None;
        Ok(())
    }

    fn waiting_on(&self, task: TaskId) -> Option<&Wait<P>> {
        match self.tasks.get(&task).map(|t| &t.state) {
            Some(TaskState::Blocked { wait, .. }) => Some(wait),
            _ => None,
        }
    }

    /// The running task answered `value`: each task waiting on it hears a copy, and it keeps
    /// `value` for those that ask later.
    pub fn finish(&mut self, value: P) -> Result<(), Diagnostic> {
        let done = self.running()?;
        let task = self.task_mut(done)?;
        let joiners = std::mem::take(&mut task.joiners);
        let unheld = task.unheld;
        self.unfinished -= 1;
        for joiner in joiners {
            let answer = match self.waiting_on(joiner) {
                Some(Wait::Await { .. }) => Reply::Maybe(Some(value.clone())),
                _ => Reply::Item(value.clone()),
            };
            let k = self.unblock(joiner)?;
            self.make_ready(joiner, Resumption::Resume { k, value: answer });
        }
        self.task_mut(done)?.state = TaskState::Done(value);
        if unheld {
            release(&self.released, done);
        }
        self.current = None;
        Ok(())
    }

    /// `failure` noted with the task it ends in where the region spawned any: which task, and how
    /// many others it leaves unfinished.
    pub fn noted(&self, failure: Diagnostic) -> Diagnostic {
        match self.current {
            Some(task) if self.next_id > ROOT.0 + 1 => {
                // The failing task is itself unfinished.
                let live = self.unfinished.saturating_sub(1);
                failure.note(format!(
                    "failed in task {task} of a host-scheduled region, with {live} other task(s) unfinished; a host-backed run is not replayable from a seed"
                ))
            }
            _ => failure,
        }
    }

    pub fn fail(&mut self, failure: Diagnostic) -> Diagnostic {
        let failure = self.noted(failure);
        if let Some(t) = self.current.and_then(|task| self.tasks.get_mut(&task)) {
            t.state = TaskState::Failed;
        }
        self.current = None;
        self.failure = Some(failure.clone());
        failure
    }

    fn complete(&self) -> Result<Turn<K, B, P>, Diagnostic> {
        match self.tasks.get(&ROOT).map(|t| &t.state) {
            Some(TaskState::Done(value)) => Ok(Turn::Complete(value.clone())),
            _ => Err(self.internal("the region finished without its body returning")),
        }
    }

    fn make_ready(&mut self, task: TaskId, resumption: Resumption<K, B, P>) {
        if let Some(t) = self.tasks.get_mut(&task) {
            t.state = TaskState::Ready(resumption);
            self.ready.insert(task);
        }
    }

    /// Takes a chosen task off the ready set and marks it running.
    fn take_ready(&mut self, task: TaskId) -> Result<Resumption<K, B, P>, Diagnostic> {
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

    fn task_mut(&mut self, task: TaskId) -> Result<&mut Task<K, B, P>, Diagnostic> {
        let span = self.span;
        self.tasks
            .get_mut(&task)
            .ok_or_else(|| out_of_order(span, format!("{task} is not a task of this region")))
    }

    /// Readies timer-fired tasks in ascending order.
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
                    value: Reply::Value(Value::Unit),
                },
            );
        }
        Ok(())
    }

    fn running(&self) -> Result<TaskId, Diagnostic> {
        match self.current {
            Some(task) => Ok(task),
            None => Err(self
                .internal("the scheduler was asked to suspend a task while no task was running")),
        }
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
            let (span, message) = waiting(id, wait);
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
    fn blocked(&self) -> impl Iterator<Item = (TaskId, &Wait<P>, Span)> {
        self.tasks.iter().filter_map(|(id, t)| match &t.state {
            TaskState::Blocked { wait, .. } => Some((*id, wait, t.origin)),
            _ => None,
        })
    }

    fn internal(&self, message: impl Into<String>) -> Diagnostic {
        out_of_order(self.span, message)
    }
}

/// Where a blocked task waits and on what, for a deadlock's report.
fn waiting<P>(id: TaskId, wait: &Wait<P>) -> (Span, String) {
    match wait {
        Wait::Join { task, span } | Wait::Await { task, span } => {
            (*span, format!("{id} waits here for {task} to finish"))
        }
        Wait::Timer { until, span } => (*span, format!("{id} sleeps here until {until}ns")),
        Wait::Host { pending, span } => (
            *span,
            format!("{id} waits here on host operation {pending}"),
        ),
        Wait::Send { chan, span, .. } => (
            *span,
            format!("{id} waits here to send on channel {chan}, which is full"),
        ),
        Wait::Recv { chan, span } => (
            *span,
            format!("{id} waits here to receive from channel {chan}, which is empty"),
        ),
        Wait::Select { arms, span } => {
            let on: Vec<String> = arms
                .iter()
                .map(|arm| match arm.send {
                    Some(_) => format!("to send on {}", arm.chan),
                    None => format!("to receive from {}", arm.chan),
                })
                .collect();
            (
                *span,
                if on.is_empty() {
                    format!("{id} waits here on a select with no arm")
                } else {
                    format!(
                        "{id} waits here on a select, none of whose arms can go: {}",
                        on.join(", ")
                    )
                },
            )
        }
    }
}

fn out_of_order(span: Span, message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(codes::INTERNAL_ERROR, message)
        .primary(span, "the production scheduler was driven out of order")
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
fn err_foreign_channel(span: Span, chan: ChanId) -> Diagnostic {
    Diagnostic::error(
        codes::TASK_ESCAPES_SCOPE,
        format!("`{chan}` is a channel of another region"),
    )
    .primary(span, "this handle was made by another region's scheduler")
    .note("every region numbers its own channels, so here the handle's id names another channel, or none")
    .note("use a channel inside the region that made it")
}

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
        format!("`{task}` names no task in this region"),
    )
    .primary(span, "this handle outlived the region that created it")
    .note("a `Task` is a key into its region's scheduler, and the scheduler ends with the region")
    .note("join the task inside the region that spawned it")
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
