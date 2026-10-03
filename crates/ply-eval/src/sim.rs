//! Deterministic simulation: seeds, the steps a run records, and seeded `clock`/`random`.

use crate::{Diagnostic, EffectAtom, Mode, Span, Symbol, codes};
use std::collections::BTreeSet;
use std::fmt;

use crate::arena::Slot;
use crate::semantics::arity_error;
use crate::value::Value;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct Seed {
    pub root: u64,
    pub path: Vec<u16>,
}

impl Seed {
    pub fn at(root: u64, path: Vec<u16>) -> Seed {
        Seed { root, path }
    }

    /// `None` when the stream decides.
    pub fn choice(&self, i: usize) -> Option<u16> {
        self.path.get(i).copied()
    }
}

impl fmt::Display for Seed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.root)?;
        for (i, choice) in self.path.iter().enumerate() {
            f.write_str(if i == 0 { ":" } else { "." })?;
            write!(f, "{choice}")?;
        }
        Ok(())
    }
}

/// Never reused within a region, and wide enough that a server spawning without pause never wraps.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct TaskId(pub u64);

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "@{}", self.0)
    }
}

/// A channel of one region, numbered as [`TaskId`] numbers tasks.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ChanId(pub u64);

impl fmt::Display for ChanId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// The two streams a root expands into.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Domain {
    Sched,
    Rand,
}

impl Domain {
    /// Hashed into every draw, so no root makes the two streams coincide.
    fn tag(self) -> u8 {
        match self {
            Domain::Sched => 0,
            Domain::Rand => 1,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Domain::Sched => "sched",
            Domain::Rand => "rand",
        }
    }
}

const STREAM_DOMAIN: &[u8] = b"ply.sim.stream.1";

/// Counter-mode BLAKE3 rather than a PRNG crate.
#[derive(Clone, Debug)]
pub struct Stream {
    root: u64,
    domain: Domain,
    counter: u64,
}

impl Stream {
    pub fn new(root: u64, domain: Domain) -> Stream {
        Stream::at(root, domain, 0)
    }

    pub fn at(root: u64, domain: Domain, counter: u64) -> Stream {
        Stream {
            root,
            domain,
            counter,
        }
    }

    pub fn next_u64(&mut self) -> u64 {
        let value = Stream::draw(self.root, self.domain, self.counter);
        self.counter += 1;
        value
    }

    /// Uniform over `0..n`: draw until `x < (u64::MAX / n) * n`, answer `x % n`.
    pub fn below(&mut self, n: u64) -> Option<u64> {
        if n == 0 {
            return None;
        }
        let limit = (u64::MAX / n) * n;
        loop {
            let x = self.next_u64();
            if x < limit {
                return Some(x % n);
            }
        }
    }

    pub fn drawn(&self) -> u64 {
        self.counter
    }

    /// Pure, so a replay can ask for draw `i` without serving the earlier ones.
    pub fn draw(root: u64, domain: Domain, counter: u64) -> u64 {
        let mut hasher = blake3::Hasher::new();
        hasher.update(STREAM_DOMAIN);
        hasher.update(&root.to_le_bytes());
        hasher.update(&[domain.tag()]);
        hasher.update(&counter.to_le_bytes());
        let bytes = hasher.finalize();
        u64::from_le_bytes(
            bytes.as_bytes()[..8]
                .try_into()
                .expect("blake3 is 32 bytes"),
        )
    }
}

/// Default scheduling steps per interleaving before the region is [`crate::codes::DEADLOCK`].
pub const DEFAULT_STEPS: u32 = 100_000;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Access {
    Atom(EffectAtom),
    Cell {
        id: Slot,
        mode: Mode,
    },
    /// A `with_cell` took the lowest free slot of the store every stack shares.
    Alloc,
}

impl Access {
    /// Cells conflict when one writes the same slot; allocations always, as the order they run in
    /// decides which slot each takes.
    pub fn conflicts_with(&self, other: &Access) -> bool {
        match (self, other) {
            (Access::Atom(a), Access::Atom(b)) => a.conflicts_with(b),
            (
                Access::Cell { id: a, mode: ma },
                Access::Cell {
                    id: b, mode: mb, ..
                },
            ) => a == b && (*ma == Mode::Write || *mb == Mode::Write),
            (Access::Alloc, Access::Alloc) => true,
            _ => false,
        }
    }
}

impl fmt::Display for Access {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Access::Atom(a) => write!(f, "{a}"),
            Access::Cell { id, mode } => write!(f, "cell.{}[{id}]", mode.as_str()),
            Access::Alloc => f.write_str("cell.alloc"),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct StepFootprint(BTreeSet<Access>);

impl StepFootprint {
    pub fn new() -> StepFootprint {
        StepFootprint::default()
    }

    pub fn from_accesses(accesses: impl IntoIterator<Item = Access>) -> StepFootprint {
        StepFootprint(accesses.into_iter().collect())
    }

    pub fn insert(&mut self, access: Access) {
        self.0.insert(access);
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn accesses(&self) -> impl Iterator<Item = &Access> {
        self.0.iter()
    }

    /// The dependence relation.
    pub fn conflicts_with(&self, other: &StepFootprint) -> bool {
        self.0
            .iter()
            .any(|a| other.0.iter().any(|b| a.conflicts_with(b)))
    }

    pub fn contention(&self, other: &StepFootprint) -> Vec<&Access> {
        self.0
            .iter()
            .filter(|a| other.0.iter().any(|b| a.conflicts_with(b)))
            .collect()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SimType {
    Int,
    Unit,
    /// The prelude's `Instant(Int)`, nanoseconds on the region's clock.
    Instant,
    /// The prelude's `Duration(Int)`, nanoseconds between two instants.
    Duration,
}

impl SimType {
    pub fn holds(self, value: &Value) -> bool {
        match (self, value) {
            (SimType::Int, Value::Int(_)) | (SimType::Unit, Value::Unit) => true,
            (SimType::Instant | SimType::Duration, Value::Ctor { name, args }) => {
                name.as_str() == self.as_str() && matches!(args.as_slice(), [Value::Int(_)])
            }
            _ => false,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            SimType::Int => "Int",
            SimType::Unit => "Unit",
            SimType::Instant => "Instant",
            SimType::Duration => "Duration",
        }
    }
}

/// The nanoseconds an `Instant(n)` or a `Duration(n)` holds.
pub fn nanos_of(value: &Value, span: Span, what: &str) -> Result<i64, Diagnostic> {
    match value {
        Value::Ctor { args, .. } if args.len() == 1 => args[0].as_int(span, what),
        other => Err(crate::value::type_error(
            span,
            what,
            "an `Instant` or a `Duration`",
            other,
        )),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct OpSignature {
    pub effect: &'static str,
    pub op: &'static str,
    pub params: &'static [SimType],
    pub ret: SimType,
}

impl fmt::Display for OpSignature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.effect, self.op)
    }
}

pub const SEEDED_OPS: &[OpSignature] = &[
    OpSignature {
        effect: "clock",
        op: "now",
        params: &[],
        ret: SimType::Instant,
    },
    OpSignature {
        effect: "clock",
        op: "sleep",
        params: &[SimType::Duration],
        ret: SimType::Unit,
    },
    OpSignature {
        effect: "random",
        op: "next",
        params: &[],
        ret: SimType::Int,
    },
    OpSignature {
        effect: "random",
        op: "below",
        params: &[SimType::Int],
        ret: SimType::Int,
    },
];

pub const SEEDED_EFFECTS: &[&str] = &["clock", "random"];

/// `None` for `task.*` or a user's own effect, which this module may not answer.
pub fn signature(effect: &str, op: &str) -> Option<&'static OpSignature> {
    SEEDED_OPS
        .iter()
        .find(|sig| sig.effect == effect && sig.op == op)
}

/// Answered by the scheduler, not [`Handlers`]: they are polymorphic and use scheduler state.
pub const TASK_OPS: &[&str] = &[
    "spawn", "join", "yield", "cancel", "await", "channel", "send", "recv", "close",
];

/// What a cancel writes and every step of the cancelled task reads, so the search sees that
/// cancelling earlier or later is a different run.
pub fn liveness(task: TaskId, mode: Mode) -> Access {
    Access::Atom(EffectAtom {
        effect: Symbol::new("task.alive"),
        resource: crate::footprint::Resource::Named(Symbol::new(format!("@{}", task.0))),
        mode,
        op: None,
    })
}

/// What every operation on a channel writes: which of two goes first decides what each answers.
pub fn channel_access(chan: ChanId) -> Access {
    channel_atom(format!("{chan}"))
}

/// What making a channel writes, as allocating a cell does: the order two run in decides each id.
pub fn channel_made() -> Access {
    channel_atom("new".to_string())
}

fn channel_atom(resource: String) -> Access {
    Access::Atom(EffectAtom {
        effect: Symbol::new("task.chan"),
        resource: crate::footprint::Resource::Named(Symbol::new(resource)),
        mode: Mode::Write,
        op: None,
    })
}

/// Whether a `simulate` region's delimiter answers this operation.
pub fn is_scheduled(effect: &str, op: &str) -> bool {
    match effect {
        "task" => TASK_OPS.contains(&op),
        _ => signature(effect, op).is_some(),
    }
}

/// What `clock.sleep(d)` did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sleep {
    /// `d <= 0`.
    Yield,
    Until(i64),
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Wakeup {
    pub now: i64,
    /// Every task whose deadline was exactly `now`, ascending by id.
    pub woken: Vec<TaskId>,
}

/// Virtual time, in nanoseconds since the region was entered.
#[derive(Clone, Debug, Default)]
pub struct Clock {
    now: i64,
    /// Ordered by `(deadline, task)` so ties wake in task order, never a heap's arbitrary order.
    timers: BTreeSet<(i64, TaskId)>,
}

impl Clock {
    pub fn new() -> Clock {
        Clock::default()
    }

    pub fn now(&self) -> i64 {
        self.now
    }

    pub fn sleep(&mut self, task: TaskId, nanos: i64, span: Span) -> Result<Sleep, Diagnostic> {
        if nanos <= 0 {
            return Ok(Sleep::Yield);
        }
        let Some(deadline) = self.now.checked_add(nanos) else {
            return Err(err_time_overflow(span, self.now, nanos));
        };
        if self.deadline_of(task).is_some() {
            return Err(err_already_sleeping(span, task));
        }
        self.timers.insert((deadline, task));
        Ok(Sleep::Until(deadline))
    }

    pub fn sleeping(&self) -> impl Iterator<Item = (TaskId, i64)> + '_ {
        self.timers.iter().map(|&(deadline, task)| (task, deadline))
    }

    pub fn is_sleeping(&self, task: TaskId) -> bool {
        self.deadline_of(task).is_some()
    }

    pub fn deadline_of(&self, task: TaskId) -> Option<i64> {
        self.timers
            .iter()
            .find(|&&(_, t)| t == task)
            .map(|&(deadline, _)| deadline)
    }

    /// `None` when no timer is pending, which with nothing enabled means the region is stuck.
    pub fn next_deadline(&self) -> Option<i64> {
        self.timers.first().map(|&(deadline, _)| deadline)
    }

    pub fn sleepers(&self) -> usize {
        self.timers.len()
    }

    /// Drops a cancelled sleeper's timer, so time no longer advances on its account.
    pub fn cancel(&mut self, task: TaskId) {
        self.timers.retain(|&(_, t)| t != task);
    }

    pub fn advance(&mut self) -> Option<Wakeup> {
        let deadline = self.next_deadline()?;
        let mut woken = Vec::new();
        while let Some(&entry) = self.timers.first() {
            if entry.0 != deadline {
                break;
            }
            self.timers.remove(&entry);
            woken.push(entry.1);
        }
        self.now = deadline;
        Some(Wakeup {
            now: deadline,
            woken,
        })
    }
}

#[derive(Clone, Debug)]
pub struct Rand {
    stream: Stream,
}

impl Rand {
    pub fn new(root: u64) -> Rand {
        Rand::at(root, 0)
    }

    pub fn at(root: u64, drawn: u64) -> Rand {
        Rand {
            stream: Stream::at(root, Domain::Rand, drawn),
        }
    }

    pub fn next_int(&mut self) -> i64 {
        self.stream.next_u64() as i64
    }

    pub fn below(&mut self, bound: i64, span: Span) -> Result<i64, Diagnostic> {
        let n = bound_of(bound, span)?;
        match self.stream.below(n) {
            // `x < n <= i64::MAX`, so the cast keeps the value.
            Some(x) => Ok(x as i64),
            None => Err(err_bad_bound(span, bound)),
        }
    }

    pub fn drawn(&self) -> u64 {
        self.stream.drawn()
    }
}

#[derive(Clone, Debug)]
pub enum Answer {
    Value(Value),
    Sleeping { deadline: i64 },
}

/// Seeded handlers for the `simulate` effects the scheduler does not answer itself.
#[derive(Clone, Debug)]
pub struct Handlers {
    clock: Clock,
    rand: Rand,
}

impl Handlers {
    pub fn new(root: u64) -> Handlers {
        Handlers::at(root, 0)
    }

    /// Virtual time restarts per region; the `rand` stream carries on across the run.
    pub fn at(root: u64, drawn: u64) -> Handlers {
        Handlers {
            clock: Clock::new(),
            rand: Rand::at(root, drawn),
        }
    }

    pub fn clock(&self) -> &Clock {
        &self.clock
    }

    /// For [`Clock::advance`], which the scheduler may call only with nothing enabled.
    pub fn clock_mut(&mut self) -> &mut Clock {
        &mut self.clock
    }

    pub fn rand(&self) -> &Rand {
        &self.rand
    }

    pub fn dispatch(
        &mut self,
        sig: &OpSignature,
        task: TaskId,
        args: &[Value],
        span: Span,
    ) -> Result<Answer, Diagnostic> {
        if args.len() != sig.params.len() {
            return Err(arity_error(
                span,
                &format!("`{sig}`"),
                sig.params.len(),
                args.len(),
            ));
        }
        match (sig.effect, sig.op) {
            ("clock", "now") => Ok(Answer::Value(Value::ctor(
                "Instant",
                vec![Value::Int(self.clock.now())],
            ))),
            ("clock", "sleep") => {
                let nanos = nanos_of(&args[0], span, "`clock.sleep`")?;
                match self.clock.sleep(task, nanos, span)? {
                    Sleep::Yield => Ok(Answer::Value(Value::Unit)),
                    Sleep::Until(deadline) => Ok(Answer::Sleeping { deadline }),
                }
            }
            ("random", "next") => Ok(Answer::Value(Value::Int(self.rand.next_int()))),
            ("random", "below") => {
                let bound = args[0].as_int(span, "`random.below`")?;
                Ok(Answer::Value(Value::Int(self.rand.below(bound, span)?)))
            }
            _ => Err(err_not_seeded(span, sig)),
        }
    }
}

/// What `random.below(bound)` draws below, or the raise a bound below one is.
pub fn bound_of(bound: i64, span: Span) -> Result<u64, Diagnostic> {
    u64::try_from(bound)
        .ok()
        .filter(|n| *n > 0)
        .ok_or_else(|| err_bad_bound(span, bound))
}

#[cold]
#[inline(never)]
fn err_bad_bound(span: Span, bound: i64) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("`random.below` needs a bound above zero, but got {bound}"),
    )
    .primary(span, "this bound names no value to draw")
    .note("`random.below(n)` answers a value in `0..n`, which is empty for `n <= 0`")
    .note("guard the bound, or use `random.next()` for the whole range")
}

#[cold]
#[inline(never)]
fn err_time_overflow(span: Span, now: i64, nanos: i64) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("this sleep runs past the end of virtual time: {now}ns + {nanos}ns overflows"),
    )
    .primary(span, "this deadline cannot be represented")
    .note("virtual time is nanoseconds since the region was entered, and it is an `Int`")
}

#[cold]
#[inline(never)]
fn err_already_sleeping(span: Span, task: TaskId) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("{task} was resumed while it was still blocked on a timer"),
    )
    .primary(span, "this sleep found an earlier one still pending")
    .note("a sleeping task is not enabled, so the scheduler should not have resumed it")
}

#[cold]
#[inline(never)]
fn err_not_seeded(span: Span, sig: &OpSignature) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("`{sig}` is not an operation the seeded handlers answer"),
    )
    .primary(span, "performed here")
    .note("`sim::signature` is the only source of a signature `dispatch` accepts")
}
