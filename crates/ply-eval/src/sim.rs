//! Deterministic simulation: the seed, the plan, the dependence relation, and the seeded handlers
//! for `clock` and `random`.

use ply_core::ty::{EffectAtom, Resource, Type};
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_syntax::ast::Mode;
use std::collections::BTreeSet;
use std::fmt;

use crate::arena::Slot;
use crate::interp::arity_error;
use crate::value::Value;

/// The repro artifact.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct Seed {
    pub root: u64,
    pub path: Vec<u16>,
}

impl Seed {
    pub fn root(root: u64) -> Seed {
        Seed {
            root,
            path: Vec::new(),
        }
    }

    pub fn at(root: u64, path: Vec<u16>) -> Seed {
        Seed { root, path }
    }

    pub fn is_root(&self) -> bool {
        self.path.is_empty()
    }

    /// `"7"` or `"7:3.0.2"`.
    pub fn parse(s: &str) -> Option<Seed> {
        let (root, rest) = match s.split_once(':') {
            Some((root, rest)) => (root, Some(rest)),
            None => (s, None),
        };
        let root = parse_u64(root)?;
        let path = match rest {
            None => Vec::new(),
            // `7:` is not `7`: an empty path segment is a typo, not a root.
            Some("") => return None,
            Some(rest) => rest
                .split('.')
                .map(|part| parse_u64(part).and_then(|n| u16::try_from(n).ok()))
                .collect::<Option<Vec<u16>>>()?,
        };
        Some(Seed { root, path })
    }

    /// Canonical bytes for a cache key, which is a different job from the text form: it must be
    /// unambiguous rather than readable, so the path is length prefixed.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(12 + 2 * self.path.len());
        out.extend_from_slice(&self.root.to_le_bytes());
        out.extend_from_slice(&(self.path.len() as u32).to_le_bytes());
        for choice in &self.path {
            out.extend_from_slice(&choice.to_le_bytes());
        }
        out
    }

    /// The seed naming the interleaving that agrees with this one up to scheduling point `at` and
    /// takes `choice` there.
    pub fn branch(&self, at: usize, choice: u16) -> Seed {
        let mut path: Vec<u16> = self.path.iter().copied().take(at).collect();
        path.resize(at, 0);
        path.push(choice);
        Seed {
            root: self.root,
            path,
        }
    }

    /// The choice fixed at scheduling point `i`, or `None` when the stream decides.
    pub fn choice(&self, i: usize) -> Option<u16> {
        self.path.get(i).copied()
    }
}

/// Decimal, or `0x`-prefixed hexadecimal.
fn parse_u64(s: &str) -> Option<u64> {
    match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(hex) if !hex.is_empty() => u64::from_str_radix(hex, 16).ok(),
        Some(_) => None,
        None if s.is_empty() => None,
        None => s.parse().ok(),
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

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct TaskId(pub u32);

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "@{}", self.0)
    }
}

/// The two streams a root expands into.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Domain {
    /// Which enabled task to resume.
    Sched,
    /// `random.next` and `random.below`.
    Rand,
}

impl Domain {
    /// Written into every draw, so the two streams cannot be made to coincide by any choice of
    /// root.
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

    /// A stream that has already served `counter` draws.
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

    /// Uniform over `0..n`, by rejection: with `limit = (u64::MAX / n) * n`, draw until `x <
    /// limit`, answer `x % n`.
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

    /// How many draws this stream has served.
    pub fn drawn(&self) -> u64 {
        self.counter
    }

    /// Pure, so a caller replaying a recorded run can ask for draw *i* without having served the
    /// ones before it.
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

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SimMode {
    /// One interleaving, the one the seed names.
    Once,
    /// One interleaving per root.
    Random,
    /// The search of ADR 0006 §6.2.
    #[default]
    Dpor,
}

impl SimMode {
    pub fn as_str(self) -> &'static str {
        match self {
            SimMode::Once => "once",
            SimMode::Random => "random",
            SimMode::Dpor => "dpor",
        }
    }

    pub fn parse(s: &str) -> Option<SimMode> {
        match s {
            "once" => Some(SimMode::Once),
            "random" => Some(SimMode::Random),
            "dpor" => Some(SimMode::Dpor),
            _ => None,
        }
    }

    /// Whether a root's exploration decomposes into independent per-seed claims.
    pub fn caches_per_seed(self) -> bool {
        matches!(self, SimMode::Random)
    }
}

/// The default interleavings explored per root under [`SimMode::Dpor`].
pub const DEFAULT_BUDGET: u32 = 256;

/// The default scheduling steps one interleaving may take before the region is
/// [`ply_span::codes::DEADLOCK`].
pub const DEFAULT_STEPS: u32 = 100_000;

/// Under `random`, one root is a sample of one.
pub const DEFAULT_RANDOM_ROOTS: u32 = 64;

/// What a run searches.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Plan {
    pub mode: SimMode,
    /// Ascending and deduplicated by [`Plan::normalized`].
    pub roots: Vec<u64>,
    /// Interleavings per root.
    pub budget: u32,
    /// Scheduling steps per interleaving.
    pub steps: u32,
    /// The fixed path under [`SimMode::Once`], so that `--seed 7:3.0.2` names one interleaving
    /// rather than one root.
    pub path: Vec<u16>,
}

impl Default for Plan {
    fn default() -> Plan {
        Plan {
            mode: SimMode::Dpor,
            roots: vec![0],
            budget: DEFAULT_BUDGET,
            steps: DEFAULT_STEPS,
            path: Vec::new(),
        }
    }
}

impl Plan {
    /// The replay plan: exactly this interleaving, explored no further.
    pub fn once(seed: Seed) -> Plan {
        Plan {
            mode: SimMode::Once,
            roots: vec![seed.root],
            budget: 1,
            steps: DEFAULT_STEPS,
            path: seed.path,
        }
    }

    pub fn random(roots: u32) -> Plan {
        Plan {
            mode: SimMode::Random,
            roots: (0..u64::from(roots)).collect(),
            budget: 1,
            steps: DEFAULT_STEPS,
            path: Vec::new(),
        }
    }

    /// Ascending, deduplicated roots, and no path outside [`SimMode::Once`].
    pub fn normalized(mut self) -> Plan {
        self.roots.sort_unstable();
        self.roots.dedup();
        if self.roots.is_empty() {
            self.roots.push(0);
        }
        self.budget = self.budget.max(1);
        self.steps = self.steps.max(1);
        if self.mode != SimMode::Once {
            self.path.clear();
        }
        self
    }

    /// The seeds this plan starts from, in the order it explores them.
    pub fn seeds(&self) -> Vec<Seed> {
        self.roots
            .iter()
            .map(|&root| Seed::at(root, self.path.clone()))
            .collect()
    }

    /// Whether running this plan can drive the entry point more than once.
    pub fn re_executes(&self) -> bool {
        let plan = self.clone().normalized();
        let per_root = match plan.mode {
            SimMode::Once | SimMode::Random => 1,
            SimMode::Dpor => u64::from(plan.budget),
        };
        plan.roots.len() as u64 * per_root > 1
    }

    /// Covers every field, length-prefixed, so no two plans can serialize alike and the digest
    /// never depends on the struct's field order at a call site.
    pub fn digest(&self) -> [u8; 32] {
        let plan = self.clone().normalized();
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"ply.sim.plan.1");
        hasher.update(&[match plan.mode {
            SimMode::Once => 0,
            SimMode::Random => 1,
            SimMode::Dpor => 2,
        }]);
        hasher.update(&plan.budget.to_le_bytes());
        hasher.update(&plan.steps.to_le_bytes());
        hasher.update(&(plan.roots.len() as u32).to_le_bytes());
        for root in &plan.roots {
            hasher.update(&root.to_le_bytes());
        }
        hasher.update(&(plan.path.len() as u32).to_le_bytes());
        for choice in &plan.path {
            hasher.update(&choice.to_le_bytes());
        }
        *hasher.finalize().as_bytes()
    }
}

/// One access a step made.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Access {
    Atom(EffectAtom),
    Cell {
        id: Slot,
        mode: Mode,
    },
    /// A `with_cell` took the next slot from the arena's bump pointer.
    Alloc,
}

impl Access {
    /// Two atoms contend by the relation the whole language is built on; two cells iff they are the
    /// same location and one writes; two allocations always, since they take ids from one counter.
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

    pub fn is_write(&self) -> bool {
        match self {
            Access::Atom(a) => a.mode == Mode::Write,
            Access::Cell { mode, .. } => *mode == Mode::Write,
            Access::Alloc => true,
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

/// What one step of one task touched.
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

    /// The atoms and cells common to both, as a diagnostic renders them.
    pub fn contention(&self, other: &StepFootprint) -> Vec<&Access> {
        self.0
            .iter()
            .filter(|a| other.0.iter().any(|b| a.conflicts_with(b)))
            .collect()
    }
}

/// One end of a race, as the failure artifact prints it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RaceSite {
    pub task: TaskId,
    /// The definition the step was inside.
    pub definition: Option<Symbol>,
    /// The contended access, rendered.
    pub access: String,
    pub span: Span,
}

/// The two steps whose reordering flipped a passing interleaving to a failing one, and the
/// scheduling point where the search flipped them.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Race {
    pub left: RaceSite,
    pub right: RaceSite,
    pub at: u32,
}

/// What an unpruned search would have explored.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Naive {
    pub explored: u32,
    pub bounded: bool,
}

impl fmt::Display for Naive {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.bounded {
            write!(f, ">= {}", self.explored)
        } else {
            write!(f, "{}", self.explored)
        }
    }
}

/// What one entry point's search did.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Exploration {
    pub explored: u32,
    /// The frontier emptied within the budget: **every** interleaving ran, up to an equivalence
    /// that provably preserves outcomes.
    pub exhaustive: bool,
    /// The budget was spent.
    pub exhausted: bool,
    /// `--measure-reduction` only.
    pub naive: Option<Naive>,
    pub steps: u64,
    /// Nanoseconds of virtual time the last interleaving consumed.
    pub virtual_time: i64,
    /// The seed of the interleaving that failed.
    pub failure: Option<Seed>,
    pub race: Option<Race>,
}

impl Exploration {
    /// How many times over an unpruned search would have run.
    pub fn reduction(&self) -> Option<f64> {
        let naive = self.naive?;
        (self.explored > 0).then(|| f64::from(naive.explored) / f64::from(self.explored))
    }

    /// Whether this run's green verdict may be written to the result cache.
    pub fn is_cacheable(&self) -> bool {
        self.failure.is_none() && !self.exhausted
    }
}

/// The types the seeded operations speak in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SimTy {
    Int,
    Unit,
}

impl SimTy {
    /// The declared type this stands for.
    pub fn ply(self) -> Type {
        match self {
            SimTy::Int => Type::int(),
            SimTy::Unit => Type::unit(),
        }
    }

    pub fn holds(self, value: &Value) -> bool {
        matches!(
            (self, value),
            (SimTy::Int, Value::Int(_)) | (SimTy::Unit, Value::Unit)
        )
    }

    pub fn as_str(self) -> &'static str {
        match self {
            SimTy::Int => "Int",
            SimTy::Unit => "Unit",
        }
    }
}

/// One operation the seeded handlers answer, with the signature it answers it at.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct OpSignature {
    pub effect: &'static str,
    pub op: &'static str,
    pub mode: Mode,
    pub params: &'static [SimTy],
    pub ret: SimTy,
}

impl OpSignature {
    /// The atom a perform of this operation contributes to the row.
    pub fn atom(&self) -> EffectAtom {
        EffectAtom::new(self.effect, Resource::Singleton, self.mode)
    }

    /// What this operation contributes to the access set of the step it ends.
    pub fn step_access(&self) -> Option<Access> {
        (self.effect == "random").then(|| Access::Atom(self.atom()))
    }
}

impl fmt::Display for OpSignature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.effect, self.op)
    }
}

/// The clause set [`Handlers`] installs, which is ADR 0006 §1.1's declaration of `clock` and
/// `random` and nothing besides.
pub const SEEDED_OPS: &[OpSignature] = &[
    OpSignature {
        effect: "clock",
        op: "now",
        mode: Mode::Read,
        params: &[],
        ret: SimTy::Int,
    },
    OpSignature {
        effect: "clock",
        op: "sleep",
        mode: Mode::Write,
        params: &[SimTy::Int],
        ret: SimTy::Unit,
    },
    OpSignature {
        effect: "random",
        op: "next",
        mode: Mode::Write,
        params: &[],
        ret: SimTy::Int,
    },
    OpSignature {
        effect: "random",
        op: "below",
        mode: Mode::Write,
        params: &[SimTy::Int],
        ret: SimTy::Int,
    },
];

/// The effects [`Handlers`] discharges.
pub const SEEDED_EFFECTS: &[&str] = &["clock", "random"];

/// The signature of a seeded operation, or `None` when the operation is not one of them — which for
/// the scheduler means `task.*` or a user's own effect, neither of which this module may answer.
pub fn signature(effect: &str, op: &str) -> Option<&'static OpSignature> {
    SEEDED_OPS
        .iter()
        .find(|sig| sig.effect == effect && sig.op == op)
}

/// The three `task` operations, which the scheduler answers rather than [`Handlers`]: their
/// signature is polymorphic and their state is the scheduler's own.
pub const TASK_OPS: &[&str] = &["spawn", "join", "yield"];

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
    /// The task is blocked until virtual time reaches this deadline.
    Until(i64),
}

/// The tasks a timer made enabled, and the time it happened at.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Wake {
    pub now: i64,
    /// Every task whose deadline was exactly `now`, ascending by id.
    pub woken: Vec<TaskId>,
}

/// Virtual time, in nanoseconds since the region was entered.
#[derive(Clone, Debug, Default)]
pub struct Clock {
    now: i64,
    /// Ascending by `(deadline, task)`, so ties come back in task order rather than in whatever
    /// order a heap resolves them — an ordering the host must not get a vote in.
    timers: BTreeSet<(i64, TaskId)>,
}

impl Clock {
    /// Time starts at zero.
    pub fn new() -> Clock {
        Clock::default()
    }

    /// `clock.now()`.
    pub fn now(&self) -> i64 {
        self.now
    }

    /// `clock.sleep(nanos)`.
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

    /// Every task waiting on a timer, ascending by `(deadline, task)`.
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

    /// When the next timer can fire, or `None` when none can — which, with nothing enabled, is the
    /// region being stuck.
    pub fn next_deadline(&self) -> Option<i64> {
        self.timers.first().map(|&(deadline, _)| deadline)
    }

    pub fn sleepers(&self) -> usize {
        self.timers.len()
    }

    /// Jump to the earliest deadline and wake every task waiting on it.
    pub fn advance(&mut self) -> Option<Wake> {
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
        Some(Wake {
            now: deadline,
            woken,
        })
    }
}

/// The seeded `random` handler: one stream per region, drawn from the root.
#[derive(Clone, Debug)]
pub struct Rand {
    stream: Stream,
}

impl Rand {
    pub fn new(root: u64) -> Rand {
        Rand::at(root, 0)
    }

    /// Picks the `rand` stream up where an earlier region of the same entry point left it.
    pub fn at(root: u64, drawn: u64) -> Rand {
        Rand {
            stream: Stream::at(root, Domain::Rand, drawn),
        }
    }

    /// `random.next()`.
    pub fn next_int(&mut self) -> i64 {
        self.stream.next_u64() as i64
    }

    /// `random.below(bound)`, by the rejection rule [`Stream::below`] specifies.
    pub fn below(&mut self, bound: i64, span: Span) -> Result<i64, Diagnostic> {
        let n = u64::try_from(bound).ok().filter(|n| *n > 0);
        match n.and_then(|n| self.stream.below(n)) {
            // `x < n <= i64::MAX`, so the cast keeps the value.
            Some(x) => Ok(x as i64),
            None => Err(err_bad_bound(span, bound)),
        }
    }

    /// How many draws this region has served.
    pub fn drawn(&self) -> u64 {
        self.stream.drawn()
    }
}

/// What a seeded handler answers a perform with.
#[derive(Clone, Debug)]
pub enum Answer {
    /// Resume the performing task with this value.
    Value(Value),
    /// The task is blocked until virtual time reaches `deadline`.
    Sleeping { deadline: i64 },
}

/// The seeded clause set for the effects `simulate` handles and the scheduler does not implement
/// itself.
#[derive(Clone, Debug)]
pub struct Handlers {
    clock: Clock,
    rand: Rand,
}

impl Handlers {
    pub fn new(root: u64) -> Handlers {
        Handlers::at(root, 0)
    }

    /// Virtual time restarts per region — it is time since *this* region was entered — while the
    /// `rand` stream carries on, because a draw is a draw of the run.
    pub fn at(root: u64, drawn: u64) -> Handlers {
        Handlers {
            clock: Clock::new(),
            rand: Rand::at(root, drawn),
        }
    }

    pub fn clock(&self) -> &Clock {
        &self.clock
    }

    /// The scheduler needs this to call [`Clock::advance`], which it may do only with nothing
    /// enabled.
    pub fn clock_mut(&mut self) -> &mut Clock {
        &mut self.clock
    }

    pub fn rand(&self) -> &Rand {
        &self.rand
    }

    /// Answer one `clock.*` or `random.*` perform on behalf of `task`.
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
            ("clock", "now") => Ok(Answer::Value(Value::Int(self.clock.now()))),
            ("clock", "sleep") => {
                let nanos = args[0].as_int(span, "`clock.sleep`")?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seed_round_trips_through_its_text_form() {
        for text in ["0", "7", "18446744073709551615", "7:3", "0:1.0.2"] {
            let seed = Seed::parse(text).expect("parses");
            assert_eq!(seed.to_string(), text, "{text} did not round-trip");
        }
    }

    #[test]
    fn hexadecimal_roots_parse_and_print_as_decimal() {
        assert_eq!(Seed::parse("0xff"), Some(Seed::root(255)));
        assert_eq!(Seed::parse("0xFF:1"), Some(Seed::at(255, vec![1])));
    }

    /// A seed that parses loosely replays something other than what failed.
    #[test]
    fn everything_else_is_rejected() {
        for text in [
            "", "-1", "7:", ":3", "7:a", "7.3", "0x", "1_000", " 7", "7 ",
        ] {
            assert_eq!(Seed::parse(text), None, "`{text}` should not parse");
        }
    }

    #[test]
    fn canonical_bytes_distinguish_a_path_from_a_longer_root() {
        // `7:1` and `7` differ, and no length prefix ambiguity can make the path of one seed look
        // like the root of another.
        assert_ne!(Seed::root(7).to_bytes(), Seed::at(7, vec![1]).to_bytes());
        assert_ne!(
            Seed::at(7, vec![1, 0]).to_bytes(),
            Seed::at(7, vec![1]).to_bytes()
        );
    }

    #[test]
    fn branching_keeps_the_prefix_and_forgets_the_suffix() {
        let seed = Seed::at(3, vec![1, 2, 3, 4]);
        assert_eq!(seed.branch(2, 9), Seed::at(3, vec![1, 2, 9]));
        assert_eq!(seed.branch(0, 5), Seed::at(3, vec![5]));
        // Branching past the recorded path pads rather than panicking: a search may reach a
        // scheduling point the prefix never named, and the choice has to land at that index.
        let branched = seed.branch(6, 1);
        assert_eq!(branched.choice(6), Some(1));
        assert_eq!(branched.path.len(), 7);
    }

    #[test]
    fn the_two_streams_are_independent() {
        let sched: Vec<u64> = (0..8).map(|i| Stream::draw(11, Domain::Sched, i)).collect();
        let rand: Vec<u64> = (0..8).map(|i| Stream::draw(11, Domain::Rand, i)).collect();
        assert_ne!(sched, rand);
        // Serving the rand stream must not disturb the sched stream, which is what stops a new
        // `random.next()` call from shifting the interleaving.
        let mut a = Stream::new(11, Domain::Sched);
        let mut r = Stream::new(11, Domain::Rand);
        let mut b = Stream::new(11, Domain::Sched);
        for expected in &sched {
            assert_eq!(a.next_u64(), *expected);
            r.next_u64();
            assert_eq!(b.next_u64(), *expected);
        }
    }

    #[test]
    fn a_draw_is_a_function_of_root_domain_and_counter_only() {
        let mut s = Stream::new(42, Domain::Sched);
        for i in 0..16 {
            assert_eq!(s.next_u64(), Stream::draw(42, Domain::Sched, i));
        }
        assert_eq!(s.drawn(), 16);
    }

    #[test]
    fn below_is_in_range_and_refuses_a_zero_bound() {
        let mut s = Stream::new(5, Domain::Rand);
        assert_eq!(s.below(0), None);
        assert_eq!(s.below(1), Some(0));
        for _ in 0..1000 {
            assert!(s.below(7).expect("nonzero bound") < 7);
        }
    }

    /// Not a statistical test — a distribution check that fails one run in a thousand is exactly
    /// the flake this project exists to delete.
    #[test]
    fn below_rejects_only_above_the_limit() {
        let n = 3u64;
        let limit = (u64::MAX / n) * n;
        let mut counter = 0u64;
        let mut expected = None;
        while expected.is_none() {
            let x = Stream::draw(9, Domain::Rand, counter);
            counter += 1;
            if x < limit {
                expected = Some(x % n);
            }
        }
        let mut s = Stream::new(9, Domain::Rand);
        assert_eq!(s.below(n), expected);
        assert_eq!(s.drawn(), counter);
    }

    fn atom(effect: &str, resource: Option<&str>, mode: Mode) -> EffectAtom {
        use ply_core::Resource;
        EffectAtom::new(
            effect,
            resource
                .map(|r| Resource::Named(Symbol::new(r)))
                .unwrap_or(Resource::Singleton),
            mode,
        )
    }

    #[test]
    fn two_reads_of_one_resource_commute() {
        let a = StepFootprint::from_accesses([Access::Atom(atom("db", Some("u"), Mode::Read))]);
        let b = StepFootprint::from_accesses([Access::Atom(atom("db", Some("u"), Mode::Read))]);
        assert!(!a.conflicts_with(&b));
    }

    #[test]
    fn a_write_does_not_commute_with_a_read_of_the_same_resource() {
        let r = StepFootprint::from_accesses([Access::Atom(atom("db", Some("u"), Mode::Read))]);
        let w = StepFootprint::from_accesses([Access::Atom(atom("db", Some("u"), Mode::Write))]);
        assert!(r.conflicts_with(&w));
        assert!(w.conflicts_with(&r));
        assert_eq!(r.contention(&w).len(), 1);
    }

    /// The relation is at cell granularity, not label granularity: two cells allocated under one
    /// `with_cell[users]` are two locations.
    #[test]
    fn two_cells_are_two_locations_whatever_they_were_labelled() {
        let one = StepFootprint::from_accesses([Access::Cell {
            id: Slot::new(1, 0),
            mode: Mode::Write,
        }]);
        let two = StepFootprint::from_accesses([Access::Cell {
            id: Slot::new(2, 0),
            mode: Mode::Write,
        }]);
        let also_one = StepFootprint::from_accesses([Access::Cell {
            id: Slot::new(1, 0),
            mode: Mode::Read,
        }]);
        assert!(!one.conflicts_with(&two));
        assert!(one.conflicts_with(&also_one));
    }

    /// The mistake ADR 0006 §6.1 exists to prevent: two tasks share one world, so a `cell` access
    /// is part of the dependence relation.
    #[test]
    fn cell_accesses_are_in_the_relation() {
        let write = StepFootprint::from_accesses([Access::Cell {
            id: Slot::new(7, 0),
            mode: Mode::Write,
        }]);
        assert!(write.conflicts_with(&write));
        assert!(!write.is_empty());
    }

    #[test]
    fn an_atom_and_a_cell_name_disjoint_state() {
        let atoms =
            StepFootprint::from_accesses([Access::Atom(atom("cell", Some("u"), Mode::Write))]);
        let cells = StepFootprint::from_accesses([Access::Cell {
            id: Slot::new(0, 0),
            mode: Mode::Write,
        }]);
        assert!(!atoms.conflicts_with(&cells));
    }

    #[test]
    fn the_empty_step_commutes_with_everything() {
        let empty = StepFootprint::new();
        let w = StepFootprint::from_accesses([Access::Atom(atom("db", Some("u"), Mode::Write))]);
        assert!(!empty.conflicts_with(&w));
        assert!(!w.conflicts_with(&empty));
    }

    #[test]
    fn a_plan_digest_ignores_the_order_roots_were_written_in() {
        let a = Plan {
            roots: vec![3, 1, 1, 2],
            ..Plan::default()
        };
        let b = Plan {
            roots: vec![1, 2, 3],
            ..Plan::default()
        };
        assert_eq!(a.digest(), b.digest());
    }

    #[test]
    fn every_plan_field_reaches_the_digest() {
        let base = Plan::default();
        let variants = [
            Plan {
                mode: SimMode::Random,
                ..base.clone()
            },
            Plan {
                roots: vec![0, 1],
                ..base.clone()
            },
            Plan {
                budget: base.budget + 1,
                ..base.clone()
            },
            Plan {
                steps: base.steps + 1,
                ..base.clone()
            },
            Plan::once(Seed::at(0, vec![1])),
            Plan::once(Seed::at(0, vec![2])),
        ];
        let mut seen = vec![base.digest()];
        for plan in variants {
            let digest = plan.digest();
            assert!(
                !seen.contains(&digest),
                "{plan:?} collided with an earlier plan"
            );
            seen.push(digest);
        }
    }

    #[test]
    fn normalization_drops_a_path_outside_once() {
        let plan = Plan {
            mode: SimMode::Dpor,
            path: vec![1, 2],
            ..Plan::default()
        }
        .normalized();
        assert!(plan.path.is_empty());
        assert_eq!(Plan::once(Seed::at(4, vec![1])).normalized().path, vec![1]);
    }

    #[test]
    fn a_once_plan_names_exactly_the_seed_it_replays() {
        let seed = Seed::at(9, vec![0, 3]);
        assert_eq!(Plan::once(seed.clone()).seeds(), vec![seed]);
    }

    /// An exhausted search proved nothing about the interleavings it did not reach, so its green
    /// verdict may not be cached — the first green `det` test in the language that is not
    /// cacheable, and correctly so.
    #[test]
    fn an_exhausted_search_is_not_cacheable() {
        let exhausted = Exploration {
            explored: 256,
            exhausted: true,
            ..Exploration::default()
        };
        assert!(!exhausted.is_cacheable());

        let complete = Exploration {
            explored: 12,
            exhaustive: true,
            ..Exploration::default()
        };
        assert!(complete.is_cacheable());

        let failed = Exploration {
            explored: 4,
            exhaustive: true,
            failure: Some(Seed::root(0)),
            ..Exploration::default()
        };
        assert!(!failed.is_cacheable());
    }

    #[test]
    fn a_bounded_naive_count_renders_as_a_lower_bound() {
        assert_eq!(
            Naive {
                explored: 4096,
                bounded: true
            }
            .to_string(),
            ">= 4096"
        );
        assert_eq!(
            Naive {
                explored: 720,
                bounded: false
            }
            .to_string(),
            "720"
        );
    }

    #[test]
    fn reduction_is_none_until_it_is_measured() {
        let mut e = Exploration {
            explored: 12,
            ..Exploration::default()
        };
        assert_eq!(e.reduction(), None);
        e.naive = Some(Naive {
            explored: 720,
            bounded: false,
        });
        assert_eq!(e.reduction(), Some(60.0));
    }

    fn span() -> Span {
        Span::new(ply_span::SourceId(0), 12, 20)
    }

    fn sig(effect: &str, op: &str) -> &'static OpSignature {
        signature(effect, op).expect("a seeded operation")
    }

    /// Every answer a region delivered, rendered.
    fn transcript(answers: &[Answer]) -> Vec<String> {
        answers
            .iter()
            .map(|answer| match answer {
                Answer::Value(v) => v.render(),
                Answer::Sleeping { deadline } => format!("sleeping until {deadline}"),
            })
            .collect()
    }

    fn run(root: u64, script: &[(&str, &str, Vec<Value>)]) -> (Vec<Answer>, i64, u64) {
        let mut handlers = Handlers::new(root);
        let answers: Vec<Answer> = script
            .iter()
            .enumerate()
            .map(|(i, (effect, op, args))| {
                handlers
                    .dispatch(sig(effect, op), TaskId(i as u32), args, span())
                    .expect("a well-typed request")
            })
            .collect();
        let now = handlers.clock().now();
        let drawn = handlers.rand().drawn();
        (answers, now, drawn)
    }

    fn script() -> Vec<(&'static str, &'static str, Vec<Value>)> {
        vec![
            ("clock", "now", vec![]),
            ("random", "next", vec![]),
            ("random", "below", vec![Value::Int(6)]),
            ("clock", "sleep", vec![Value::Int(0)]),
            ("random", "next", vec![]),
            ("clock", "now", vec![]),
            ("clock", "sleep", vec![Value::Int(500)]),
        ]
    }

    #[test]
    fn one_seed_answers_one_sequence() {
        let (first, now, drawn) = run(7, &script());
        let (again, now_again, drawn_again) = run(7, &script());
        assert_eq!(transcript(&first), transcript(&again));
        assert_eq!((now, drawn), (now_again, drawn_again));
        assert_eq!(drawn, 3, "one draw per `random` request, and no others");
    }

    #[test]
    fn another_seed_answers_another_sequence() {
        let (a, _, _) = run(7, &script());
        let (b, _, _) = run(8, &script());
        assert_ne!(transcript(&a), transcript(&b));
    }

    /// There is no `clock` stream.
    #[test]
    fn the_clock_is_not_drawn_from_the_seed() {
        let clock_of = |root| {
            let mut handlers = Handlers::new(root);
            let mut times = Vec::new();
            for nanos in [0, 40, 0] {
                let answer = handlers
                    .dispatch(
                        sig("clock", "sleep"),
                        TaskId(0),
                        &[Value::Int(nanos)],
                        span(),
                    )
                    .expect("a well-typed sleep");
                times.push(transcript(&[answer]).remove(0));
                handlers.clock_mut().advance();
            }
            (times, handlers.clock().now())
        };
        assert_eq!(clock_of(1), clock_of(999_999));
    }

    /// The whole of "virtual time advances only via the scheduler": `now()` observes, `sleep`
    /// schedules, a draw is a draw, and none of them moves it.
    #[test]
    fn nothing_a_task_performs_moves_virtual_time() {
        let mut handlers = Handlers::new(3);
        for (effect, op, args) in script() {
            handlers
                .dispatch(sig(effect, op), TaskId(0), &args, span())
                .expect("a well-typed request");
            assert_eq!(handlers.clock().now(), 0, "`{effect}.{op}` moved the clock");
        }
        let wake = handlers.clock_mut().advance().expect("a pending timer");
        assert_eq!(wake.now, 500);
        assert_eq!(handlers.clock().now(), 500);
    }

    #[test]
    fn sleeping_for_no_time_is_a_yield() {
        let mut clock = Clock::new();
        for nanos in [0, -1, i64::MIN] {
            let slept = clock.sleep(TaskId(0), nanos, span()).expect("a yield");
            assert_eq!(slept, Sleep::Yield);
        }
        assert_eq!(clock.sleepers(), 0);
        assert_eq!(clock.next_deadline(), None);
    }

    /// Thirty simulated seconds are a jump, not thirty seconds: nothing here waits on anything, so
    /// the only cost of a long sleep is the arithmetic.
    #[test]
    fn a_long_sleep_advances_exactly_that_far() {
        let mut clock = Clock::new();
        let slept = clock
            .sleep(TaskId(1), 30_000_000_000, span())
            .expect("a valid sleep");
        assert_eq!(slept, Sleep::Until(30_000_000_000));
        assert_eq!(clock.now(), 0);
        let wake = clock.advance().expect("a pending timer");
        assert_eq!(wake.now, 30_000_000_000);
        assert_eq!(wake.woken, vec![TaskId(1)]);
        assert_eq!(clock.now(), 30_000_000_000);
        assert!(!clock.is_sleeping(TaskId(1)));
    }

    /// Sleeps are relative to the time the sleeping task observed, so a task that sleeps twice for
    /// 40ns wakes at 80ns rather than at 40ns again.
    #[test]
    fn a_deadline_is_measured_from_the_time_the_task_saw() {
        let mut clock = Clock::new();
        clock.sleep(TaskId(0), 40, span()).expect("a valid sleep");
        clock.advance();
        let slept = clock.sleep(TaskId(0), 40, span()).expect("a valid sleep");
        assert_eq!(slept, Sleep::Until(80));
        assert_eq!(clock.advance().map(|w| w.now), Some(80));
    }

    #[test]
    fn tasks_sharing_a_deadline_wake_together_and_in_task_order() {
        let mut clock = Clock::new();
        for task in [TaskId(3), TaskId(1), TaskId(2)] {
            clock.sleep(task, 10, span()).expect("a valid sleep");
        }
        clock.sleep(TaskId(4), 20, span()).expect("a valid sleep");
        let wake = clock.advance().expect("a pending timer");
        assert_eq!(wake.now, 10);
        assert_eq!(wake.woken, vec![TaskId(1), TaskId(2), TaskId(3)]);
        assert_eq!(clock.deadline_of(TaskId(4)), Some(20));
    }

    /// A timeout is `clock.sleep` racing something else, and the earliest deadline is the only one
    /// that can fire.
    #[test]
    fn a_timeout_fires_at_its_deadline_and_never_before_a_nearer_one() {
        let mut clock = Clock::new();
        let timeout = TaskId(0);
        let work = TaskId(1);
        clock
            .sleep(timeout, 5_000_000_000, span())
            .expect("a valid sleep");
        for _ in 0..3 {
            clock
                .sleep(work, 100_000_000, span())
                .expect("a valid sleep");
            let wake = clock.advance().expect("a pending timer");
            assert_eq!(wake.woken, vec![work], "the timeout fired early");
        }
        assert_eq!(clock.now(), 300_000_000);
        let wake = clock.advance().expect("the timeout is still pending");
        assert_eq!(wake.now, 5_000_000_000);
        assert_eq!(wake.woken, vec![timeout]);
    }

    /// With nothing enabled and no timer pending the region is stuck, which the scheduler reports
    /// as `E0414`.
    #[test]
    fn no_timer_means_no_advance() {
        let mut clock = Clock::new();
        assert_eq!(clock.advance(), None);
        clock.sleep(TaskId(0), 5, span()).expect("a valid sleep");
        assert!(clock.advance().is_some());
        assert_eq!(clock.advance(), None);
        assert_eq!(clock.now(), 5, "a refused advance left time alone");
    }

    #[test]
    fn a_sleep_past_the_end_of_virtual_time_is_a_diagnostic() {
        let mut clock = Clock::new();
        clock
            .sleep(TaskId(0), i64::MAX, span())
            .expect("a valid sleep");
        clock.advance();
        let err = clock
            .sleep(TaskId(0), i64::MAX, span())
            .expect_err("the deadline overflows");
        assert_eq!(err.code, codes::RUNTIME_ERROR);
        assert_eq!(err.labels[0].span, span());
    }

    #[test]
    fn a_bound_of_zero_or_less_is_a_runtime_error_with_a_real_span() {
        let mut rand = Rand::new(2);
        for bound in [0, -1, i64::MIN] {
            let err = rand.below(bound, span()).expect_err("an empty range");
            assert_eq!(err.code, codes::RUNTIME_ERROR);
            assert_eq!(err.labels[0].span, span());
        }
        assert_eq!(rand.drawn(), 0, "a refused bound drew nothing");
        assert!((0..6).contains(&rand.below(6, span()).expect("a valid bound")));
    }

    #[test]
    fn a_draw_uses_the_rand_stream_and_the_whole_range_of_an_int() {
        let mut rand = Rand::new(4);
        let mut stream = Stream::new(4, Domain::Rand);
        for _ in 0..64 {
            assert_eq!(rand.next_int(), stream.next_u64() as i64);
        }
        let mut negatives = 0;
        for _ in 0..64 {
            if rand.next_int() < 0 {
                negatives += 1;
            }
        }
        assert!(negatives > 0, "`random.next` answers the whole of `Int`");
    }

    /// Excluding the terminating scheduler op is what leaves a reduction to measure; keeping
    /// `random.write` is what stops the search from calling two draws commutative when the program
    /// can tell them apart.
    #[test]
    fn only_a_draw_is_an_access_of_the_step_it_ends() {
        for sig in SEEDED_OPS {
            let access = sig.step_access();
            match sig.effect {
                "clock" => assert!(access.is_none(), "`{sig}` is scheduler bookkeeping"),
                _ => assert_eq!(access, Some(Access::Atom(sig.atom()))),
            }
        }
        let draw = StepFootprint::from_accesses(sig("random", "next").step_access());
        assert!(draw.conflicts_with(&draw));
    }

    #[test]
    fn the_table_names_each_operation_once_and_answers_all_of_them() {
        let mut seen: Vec<(&str, &str)> = Vec::new();
        for sig in SEEDED_OPS {
            assert!(
                !seen.contains(&(sig.effect, sig.op)),
                "`{sig}` is in the table twice"
            );
            seen.push((sig.effect, sig.op));
            assert!(SEEDED_EFFECTS.contains(&sig.effect));

            let args: Vec<Value> = sig.params.iter().map(|_| Value::Int(1)).collect();
            let answer = Handlers::new(0)
                .dispatch(sig, TaskId(0), &args, span())
                .expect("the table's own arguments are well typed");
            match answer {
                // A woken sleeper resumes with `clock.sleep`'s declared return.
                Answer::Sleeping { .. } => assert_eq!(sig.ret, SimTy::Unit),
                Answer::Value(v) => assert!(
                    sig.ret.holds(&v),
                    "`{sig}` promises {} and answered {}",
                    sig.ret.as_str(),
                    v.render()
                ),
            }
        }
        assert_eq!(
            signature("task", "spawn"),
            None,
            "`task` is the scheduler's"
        );
        assert_eq!(signature("clock", "tick"), None);
    }

    #[test]
    fn a_miscounted_argument_list_is_a_diagnostic_rather_than_a_panic() {
        let err = Handlers::new(0)
            .dispatch(sig("clock", "sleep"), TaskId(0), &[], span())
            .expect_err("`clock.sleep` takes one argument");
        assert_eq!(err.code, codes::ARITY_MISMATCH);

        let err = Handlers::new(0)
            .dispatch(sig("random", "below"), TaskId(0), &[Value::Unit], span())
            .expect_err("`random.below` takes an `Int`");
        assert_eq!(err.code, codes::RUNTIME_ERROR);
    }

    /// A sleeping task is not enabled, so the scheduler cannot resume it into a second sleep.
    #[test]
    fn a_task_cannot_hold_two_timers() {
        let mut clock = Clock::new();
        clock.sleep(TaskId(0), 5, span()).expect("a valid sleep");
        let err = clock
            .sleep(TaskId(0), 5, span())
            .expect_err("already sleeping");
        assert_eq!(err.code, codes::INTERNAL_ERROR);
        assert_eq!(clock.sleepers(), 1);
    }

    /// A rule about how a type is *used* is a rule nobody enforces; a rule about which types may be
    /// *named* is greppable.
    #[test]
    fn this_module_names_no_hash_based_collection_and_reads_no_clock() {
        let source = include_str!("sim.rs");
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
                "`{banned}` appears in ply_eval::sim; a seeded run must be a \
                 function of its definitions and its seed and nothing else"
            );
        }
    }
}
