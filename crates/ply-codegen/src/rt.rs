//! The helpers compiled code calls into and the context an entry runs in. A helper that takes an
//! argument owns it, one that reads leaves its count alone, and every answer is the caller's.

use crate::heap::{
    self, CLOSURE_CAPTURES, CLOSURE_CODE, Heap, KIND_BRIDGE, KIND_BYTES, KIND_CLOSURE, KIND_CTOR,
    KIND_LIST, KIND_MAP, KIND_RECORD, KIND_STR, Layouts, Word, bridged, bytes_of, is_unique, obj,
    set_word, str_of, word_at,
};
use crate::list;
use crate::map;
use crate::stack::{Stack, switch};
use ply_eval::arena::{Owner, RegionId, Slot};
use ply_eval::builtins::{cell_in_update, no_such_cell};
use ply_eval::region::StepSite;
use ply_eval::sim::Access;
use ply_eval::{
    BinOp, Builtin, Closure, ClosureKind, Diagnostic, EffectAtom, Mode, Resource, Span, Symbol,
    Value, codes, values_equal,
};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering::{Acquire, Release};
use std::sync::{Arc, Mutex, MutexGuard};

/// A lock nothing poisons: a panic while one is held is already the end of the run.
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A compiled function: `extern "C" fn(ctx, args) -> handle`.
pub type Entry = unsafe extern "C" fn(*mut Ctx, *const i64) -> i64;

/// A unit's tables, which every thread running the unit reads: what a `parallel` branch may change
/// is behind an atomic or a lock.
pub struct Tables {
    /// The constant pool as values.
    pub consts: Vec<Value>,
    /// The same constants as immortal words, which a literal answers.
    pub const_words: Vec<Word>,
    pub layouts: Layouts,
    /// Every field name a compiled body reads, so a field access is an index.
    pub fields: Vec<Symbol>,
    /// Every builtin a compiled body may call.
    pub builtins: Vec<Builtin>,
    /// Every compiled function's finalized address by index, for building native closures.
    pub functions: Vec<usize>,
    /// Each pure nullary function's memoized answer by index, as an immortal word; `0` is none.
    pub memo: Box<[AtomicI64]>,
    /// Owns the constant pool's and the memo's objects for as long as the unit lives.
    pub immortals: Mutex<Heap>,
    /// The 256 one-byte values, each made immortal when first asked for; `0` is not yet.
    pub bytes: [AtomicI64; 256],
    /// Per constructor index, a nullary one's immortal singleton, or `0`.
    pub nullaries: Vec<Word>,
    pub empty_list: Word,
    pub empty_map: Word,
    /// Memo words and their converted values, both ways, so a tree crosses the seam unrebuilt.
    pub memo_values: Mutex<HashMap<Word, Value>>,
    pub memo_words: Mutex<HashMap<Identity, Word>>,
    /// Answers of roots called with only memo words, up to [`CALL_MEMO_LIMIT`].
    pub calls: Mutex<HashMap<(Symbol, Vec<Word>), Word>>,
    /// Every root, sorted by `id`: what a body's stored site names.
    pub roots: Vec<Root>,
}

/// A root a body's sites name by `id`, the `root_id` of `name`.
pub struct Root {
    pub id: u64,
    pub name: Symbol,
    /// Its definition's span in the text the unit runs over: a site is an offset from its start.
    /// Never cached: definitions move.
    pub span: Span,
}

/// How many calls of roots over memo words a unit remembers.
pub const CALL_MEMO_LIMIT: usize = 64;

/// How many of an answer's direct parts get identities of their own.
const PARTS_LIMIT: usize = 64;

/// A handed-out value's allocation, plus a list's window onto it: an identity without a walk.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Identity {
    Record(usize),
    Str(usize),
    Bytes(usize),
    List(usize, usize, usize, usize),
}

fn identity(v: &Value) -> Option<Identity> {
    Some(match v {
        Value::Record(fields) => Identity::Record(Arc::as_ptr(fields) as usize),
        Value::Str(s) => Identity::Str(Arc::as_ptr(s) as *const u8 as usize),
        Value::Bytes(b) => Identity::Bytes(Arc::as_ptr(b) as *const u8 as usize),
        Value::List(items) => {
            let (tail, root, len, start) = items.identity();
            Identity::List(tail, root, len, start)
        }
        _ => return None,
    })
}

impl Tables {
    /// The memo's word for the pure nullary function at `index`, if it has one.
    pub fn memoized(&self, index: usize) -> Option<Word> {
        let w = self.memo.get(index)?.load(Acquire);
        (w != 0).then_some(w)
    }

    /// Remembers `w` as pure nullary function `index`'s answer, copied into the immortal heap. Two
    /// threads memoizing at once keep one copy each and answer the same value.
    pub fn memoize(&self, index: usize, w: Word) -> Word {
        let kept = lock(&self.immortals).adopt(w);
        if let Some(slot) = self.memo.get(index) {
            slot.store(kept, Release);
        }
        kept
    }

    /// The value a memo word was converted to before, if it was.
    pub fn memo_value(&self, w: Word) -> Option<Value> {
        lock(&self.memo_values).get(&w).cloned()
    }

    /// The memo word a value came from; `memo_values` holds the allocations, so ids are not reused.
    pub fn memo_word(&self, v: &Value) -> Option<Word> {
        let id = identity(v)?;
        lock(&self.memo_words).get(&id).copied()
    }

    /// Keeps `v` for memo word `w` and maps its direct parts to their words, since a body that
    /// takes a memo value apart hands those parts back in.
    pub fn remember(&self, w: Word, v: &Value) {
        // Replacing the value would free allocations its recorded identities still name.
        let mut values = lock(&self.memo_values);
        if values.contains_key(&w) {
            return;
        }
        values.insert(w, v.clone());
        let mut words = lock(&self.memo_words);
        if let Some(id) = identity(v) {
            words.insert(id, w);
        }
        // A scalar answer is its own word, with no parts to remember.
        if heap::is_imm(w) || w == 0 {
            return;
        }
        let o = obj(w);
        let parts: Vec<(Word, &Value)> = match v {
            Value::Record(fields) => fields
                .iter()
                .enumerate()
                .map(|(i, (_, part))| (unsafe { word_at(o, i) }, part))
                .collect(),
            Value::Ctor { args, .. } => args
                .iter()
                .enumerate()
                .map(|(i, part)| (unsafe { word_at(o, i) }, part))
                .collect(),
            Value::List(items) if items.len() <= PARTS_LIMIT => items
                .iter()
                .enumerate()
                .map(|(i, part)| (list::get(o, i), part))
                .collect(),
            _ => Vec::new(),
        };
        for (part_word, part) in parts.into_iter().take(PARTS_LIMIT) {
            if let Some(id) = identity(part) {
                words.insert(id, part_word);
            }
        }
    }

    /// The remembered answer of `root` over exactly these memo words, if it has one.
    pub fn memo_call(&self, root: &Symbol, words: &[Word]) -> Option<Value> {
        let kept = *lock(&self.calls).get(&(root.clone(), words.to_vec()))?;
        self.memo_value(kept)
    }

    /// Remembers `out` for `root` over these memo words; `None` once the bound is reached.
    pub fn memoize_call(&self, root: &Symbol, words: &[Word], out: Word) -> Option<Word> {
        let mut calls = lock(&self.calls);
        if calls.len() >= CALL_MEMO_LIMIT {
            return None;
        }
        let kept = lock(&self.immortals).adopt(out);
        calls.insert((root.clone(), words.to_vec()), kept);
        Some(kept)
    }

    /// The immortal `Bytes` holding just `b`.
    pub fn byte(&self, b: u8) -> Word {
        let slot = &self.bytes[b as usize];
        let cached = slot.load(Acquire);
        if cached != 0 {
            return cached;
        }
        let mut immortals = lock(&self.immortals);
        // Another thread may have made it while this one waited.
        let cached = slot.load(Acquire);
        if cached != 0 {
            return cached;
        }
        let w = immortals.bytes(&[b]);
        heap::mark_immortal(w);
        slot.store(w, Release);
        w
    }

    /// Whether the constant pool holds a value that must not outlive the call that made it.
    pub fn retains_a_handle(&self) -> Option<&'static str> {
        self.consts.iter().find_map(holds_a_handle)
    }
}

/// The same question of one value, to its leaves.
pub(crate) fn holds_a_handle(value: &Value) -> Option<&'static str> {
    match value {
        Value::Secret(_) => Some("a Secret"),
        Value::Cell(_) => Some("a Cell"),
        Value::Task(_) => Some("a Task"),
        Value::Closure(_) => Some("a Closure"),
        Value::List(items) => items.iter().find_map(holds_a_handle),
        Value::Map(entries) => entries
            .iter()
            .find_map(|(k, v)| holds_a_handle(k).or_else(|| holds_a_handle(v))),
        Value::Record(fields) => fields.values().find_map(holds_a_handle),
        Value::Ctor { args, .. } => args.iter().find_map(holds_a_handle),
        Value::Int(_)
        | Value::Fixed(_)
        | Value::Bool(_)
        | Value::Float(_)
        | Value::Decimal(_)
        | Value::Str(_)
        | Value::Bytes(_)
        | Value::Unit => None,
    }
}

/// `Ctx::failed` when the fuel ran out.
pub const FAILED_OUT_OF_FUEL: i64 = 2;
/// A call that had to grow the stack could not be given one.
pub const FAILED_OUT_OF_STACK: i64 = 3;
/// A clause answered without resuming: `Ctx::unwind` carries its value to its `handle`.
pub const FAILED_UNWIND: i64 = 4;
/// The harness stopped the entry at its wall clock, which is not a verdict on the program.
pub const FAILED_ABANDONED: i64 = 5;
/// The entry spent its step budget without finishing.
pub const FAILED_OUT_OF_STEPS: i64 = 6;
/// A raise of `abort.raise`: `Ctx::aborting` names the `handle` whose clause answers it.
pub const FAILED_ABORT: i64 = 7;

/// An installed handler: pushed by a `handle` site, searched innermost-out by a `perform`.
pub struct HandlerFrame {
    clauses: Vec<FrameClause>,
    /// The `return` clause's closure, or zero.
    ret: Word,
    /// A `simulate` region's clause-less frame, answering `task`, `clock`, `random` and `sim`.
    simulate: bool,
    /// For a `handle` resuming off the tail: the detached body whose own stack this frame bottoms.
    detached: Option<usize>,
    /// How deep its stack's regions stood when this frame went on: an unwind caught at this
    /// `handle` closes that stack's back to here, since the body it abandons never reaches the
    /// closes below its jump.
    regions: usize,
}

impl HandlerFrame {
    /// The closure of the clause for `abort.raise`, taken out of the frame, which keeps the rest.
    pub(crate) fn take_abort_clause(&mut self) -> Word {
        let at = self
            .clauses
            .iter()
            .position(FrameClause::answers_abort)
            .expect("a raise is bound for a frame with a clause for it");
        self.clauses.swap_remove(at).closure
    }

    fn simulate(regions: usize) -> HandlerFrame {
        HandlerFrame {
            clauses: Vec::new(),
            ret: 0,
            simulate: true,
            detached: None,
            regions,
        }
    }

    /// The bottom of a detached body's own stack, which holds no region yet.
    pub(crate) fn detached(clauses: Vec<FrameClause>, id: usize) -> HandlerFrame {
        HandlerFrame {
            clauses,
            ret: 0,
            simulate: false,
            detached: Some(id),
            regions: 0,
        }
    }
}

/// One stack's handler frames, chained to the stack it was entered from, which a `perform`
/// searches next. A depth names a frame within one stack.
pub(crate) struct Frames {
    pub(crate) list: Vec<HandlerFrame>,
    pub(crate) parent: Option<usize>,
    /// The detached body this stack is the own stack of; its frame at the bottom of `list` is
    /// hidden while one of its clauses runs, so it cannot say.
    pub(crate) body: Option<usize>,
    /// For a task's stack, the stack its region was entered from, where the task's body was
    /// written; a production task's `parent` is the scheduler loop's, which does not lead there.
    pub(crate) entered_from: Option<usize>,
}

impl Frames {
    pub(crate) fn under(parent: Option<usize>) -> Frames {
        Frames {
            list: Vec::new(),
            parent,
            body: None,
            entered_from: None,
        }
    }
}

/// A raise on its way to the `handle` that answers it: that frame's stack and depth, the message
/// its clause is given, and what the entry fails with should the frame be gone.
pub(crate) struct Aborting {
    pub(crate) stack: usize,
    pub(crate) depth: usize,
    pub(crate) message: String,
    pub(crate) diagnostic: Diagnostic,
}

/// One clause, under program-wide effect and resource names.
pub(crate) struct FrameClause {
    effect: Symbol,
    resource: Option<Symbol>,
    op: Symbol,
    closure: Word,
    /// 0: never resumes; 1: resumes in tail position; 2: elsewhere (only in a detached frame).
    resumes: u8,
    /// `[*t]`: the clause's first slot after the parameters is the label the call site named.
    binds_label: bool,
}

impl FrameClause {
    fn answers_abort(&self) -> bool {
        self.effect.as_str() == "abort" && self.op.as_str() == "raise"
    }

    fn answers(&self, effect: &Symbol, op: &Symbol, resource: Option<&Symbol>) -> bool {
        self.effect == *effect
            && self.op == *op
            && match (&self.resource, resource) {
                (None, _) => true,
                // A clause written `[*]` answers every label of the operation it names.
                (Some(mine), _) if mine.as_str() == "*" => true,
                (Some(mine), Some(theirs)) => mine == theirs,
                (Some(_), None) => false,
            }
    }
}

/// The frames a captured continuation holds, each closure held once more for the copy.
pub(crate) fn clone_frames(list: &[HandlerFrame]) -> Vec<HandlerFrame> {
    list.iter()
        .map(|f| {
            for cl in &f.clauses {
                heap::inc(cl.closure);
            }
            if f.ret != 0 {
                heap::inc(f.ret);
            }
            HandlerFrame {
                clauses: f
                    .clauses
                    .iter()
                    .map(|cl| FrameClause {
                        effect: cl.effect.clone(),
                        resource: cl.resource.clone(),
                        op: cl.op.clone(),
                        closure: cl.closure,
                        resumes: cl.resumes,
                        binds_label: cl.binds_label,
                    })
                    .collect(),
                ret: f.ret,
                simulate: f.simulate,
                detached: f.detached,
                regions: f.regions,
            }
        })
        .collect()
}

/// A spawned task's copies of the handlers around its spawn, at the bottom of its own stack; none
/// may name a detached body, whose clause would capture the spawner's stack.
pub(crate) fn inherit_frames(list: &[HandlerFrame]) -> Vec<HandlerFrame> {
    clone_frames(list)
        .into_iter()
        .map(|f| {
            // The `handle` a copy stands for may be over before the task raises, so a raise the task
            // does not answer itself leaves for the region's own surroundings instead.
            let (raises, clauses): (Vec<FrameClause>, Vec<FrameClause>) =
                f.clauses.into_iter().partition(FrameClause::answers_abort);
            for cl in raises {
                heap::dec(cl.closure);
            }
            HandlerFrame {
                clauses,
                detached: None,
                regions: 0,
                ..f
            }
        })
        .collect()
}

pub(crate) fn drop_frame(f: HandlerFrame) {
    for c in f.clauses {
        heap::dec(c.closure);
    }
    if f.ret != 0 {
        heap::dec(f.ret);
    }
}

/// A heap word a cell holds; clone and drop are counts, so the arena drops before the heap.
pub struct Held(pub Word);

impl Held {
    fn into_word(self) -> Word {
        let w = self.0;
        std::mem::forget(self);
        w
    }
}

impl Clone for Held {
    fn clone(&self) -> Held {
        heap::inc(self.0);
        Held(self.0)
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        heap::dec(self.0);
    }
}

impl Default for Held {
    fn default() -> Held {
        Held(heap::unit())
    }
}

impl std::fmt::Debug for Held {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Held({:#x})", self.0)
    }
}

/// Entries begun in this process, every context's: contexts share a unit's memo, so an entry's
/// number must be unique across them, not just within one.
static ENTRIES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[repr(C)]
pub struct Ctx {
    pub failed: i64,
    /// Nested native calls still allowed.
    pub fuel: i64,
    /// The lowest address a compiled frame may begin at: C frames can overflow within the fuel.
    pub stack_floor: usize,
    /// The site the body stored before a fallible call, as bytes from its root's start; the root
    /// is `-1` until one is.
    pub site_root: i64,
    pub site_start: i64,
    pub site_end: i64,
    /// Calls this entry has made: the prologue counts one, and so does every pass of a loop.
    pub ticks: i64,
    /// The tick at which compiled code calls [`rt_tick`] back; `i64::MAX` when neither the budget
    /// nor the clock bounds this entry, so nothing calls back at all.
    pub next_tick: i64,
    /// The calls this entry may make; 0 is no bound.
    step_budget: i64,
    /// Stacks this entry has been given beyond the one it started on, so growing is observable.
    pub grown: u64,
    /// When the running entry's time budget is spent, if it has one.
    deadline: Option<std::time::Instant>,
    time_budget_ms: u64,
    /// The cells, holding heap words: declared before the heap, so their counts go back first.
    /// Each stack in `stacks` owns the regions it opens, under the index that names it.
    pub(crate) cells: ply_eval::TaskRegions<Held>,
    /// The arena's `(total depth, live)` when the running entry began, for [`Ctx::cells_balanced`].
    cells_baseline: (usize, usize),
    pub heap: Heap,
    /// Objects the last entry allocated, kept because [`Ctx::end`] clears the heap's count.
    last_entry: usize,
    pub tables: Arc<Tables>,
    /// Why the last entry failed.
    pub diagnostic: Option<Diagnostic>,
    /// One per stack run in this entry, the entry's own first; `current` is the one running.
    pub(crate) stacks: Vec<Frames>,
    /// Indices of `stacks` a finished production task gave back, which nothing names any longer.
    free_stacks: Vec<usize>,
    pub(crate) current: usize,
    /// Every atom a compiled `perform` performed since the entry began, for the machine's trace.
    pub performed: Vec<EffectAtom>,
    /// The regions live in this entry, innermost last; the checker forbids nesting, so at most one.
    pub sims: Vec<crate::simulate::Simulation>,
    /// The detached bodies this entry has opened, named by index from their frames and tokens.
    pub(crate) detached: Vec<crate::detached::Detached>,
    pub(crate) starting_detached: Option<usize>,
    /// The running entry's number, which no other entry in the process shares: a continuation
    /// carries it, since its index names a body of that entry alone.
    pub(crate) entry: u64,
    /// The host boundary: what a `perform` nothing on the stack answers reaches.
    pub(crate) binding: Arc<ply_eval::HostBinding>,
    /// The program this context's entries run, whose declarations say how a host operation's
    /// arguments read; `None` for one entered without its program's answer.
    pub(crate) program: Option<&'static ply_eval::Front>,
    pub(crate) runtime: Option<Rc<dyn ply_eval::HostRuntime>>,
    /// What makes a reactor, which a `parallel` branch on another thread needs one of its own of.
    pub(crate) runtime_factory: Option<ply_eval::RuntimeFactory>,
    pub(crate) declared: Option<ply_eval::Footprint>,
    pub(crate) re_executed: bool,
    pub(crate) host_use: ply_eval::host::HostUse,
    pub(crate) host_ops: u64,
    /// The last at-most-once host operation this entry performed, for `E0426`.
    pub(crate) last_linear: Option<crate::host::HostMark>,
    pub(crate) id: ply_eval::host::MachineId,
    /// What the host runtime said when an entry ended, for the machine's teardown warnings.
    pub(crate) teardown: Vec<Diagnostic>,
    /// The seed the driver set, and the step budget, for the regions this entry opens.
    pub(crate) seed: ply_eval::Seed,
    pub(crate) sim_steps: u32,
    pub(crate) trail: ply_eval::region::Trail,
    /// What the entry's regions did, for the search, once the last one has completed.
    pub record: Option<ply_eval::region::Record>,
    pub(crate) entered_sims: u32,
    /// Where an unwind is going and what it carries: the frame's depth and the clause's value.
    pub(crate) unwind: Option<(usize, usize, Word)>,
    pub(crate) aborting: Option<Aborting>,
    /// The value a clause handed to `resume` in tail position, read back when the clause returns.
    resumed: Option<Word>,
    /// The heap and poison site of the entry this one began inside, put back when it ends.
    outer: (*mut Heap, *const i64),
}

impl Ctx {
    pub fn new(tables: Arc<Tables>) -> Ctx {
        let cells = ply_eval::TaskRegions::new();
        let baseline = (cells.total_depth(), cells.live());
        Ctx {
            failed: 0,
            fuel: 0,
            stack_floor: 0,
            site_root: -1,
            site_start: 0,
            site_end: 0,
            ticks: 0,
            next_tick: i64::MAX,
            step_budget: 0,
            grown: 0,
            deadline: None,
            time_budget_ms: 0,
            heap: Heap::new(),
            last_entry: 0,
            tables,
            cells,
            cells_baseline: baseline,
            diagnostic: None,
            stacks: vec![Frames::under(None)],
            free_stacks: Vec::new(),
            current: 0,
            performed: Vec::new(),
            sims: Vec::new(),
            detached: Vec::new(),
            starting_detached: None,
            entry: 0,
            binding: Arc::new(ply_eval::HostBinding::hermetic()),
            program: None,
            runtime: None,
            runtime_factory: None,
            declared: None,
            re_executed: false,
            host_use: ply_eval::host::HostUse::default(),
            host_ops: 0,
            last_linear: None,
            id: ply_eval::host::MachineId::next(),
            teardown: Vec::new(),
            seed: ply_eval::Seed::default(),
            sim_steps: ply_eval::sim::DEFAULT_STEPS,
            trail: ply_eval::region::Trail::new(ply_eval::Seed::default()),
            record: None,
            entered_sims: 0,
            unwind: None,
            aborting: None,
            resumed: None,
            outer: (std::ptr::null_mut(), std::ptr::null()),
        }
    }

    /// A context for one branch of a `parallel` block this entry reached: its unit, host, seed and
    /// what is left of its bounds, and nothing of its own, so it can run on another thread.
    pub(crate) fn branch(&self) -> Ctx {
        let mut b = Ctx::new(Arc::clone(&self.tables));
        b.heap = Heap::branch();
        b.fuel = self.fuel;
        // Zero is no bound, so a budget spent to the last call stays a bound of one.
        b.step_budget = if self.step_budget > 0 {
            (self.step_budget - self.ticks).max(1)
        } else {
            0
        };
        b.deadline = self.deadline;
        b.time_budget_ms = self.time_budget_ms;
        b.arm_tick();
        b.entry = self.entry;
        b.binding = Arc::clone(&self.binding);
        b.runtime_factory = self.runtime_factory.clone();
        b.program = self.program;
        b.declared = self.declared.clone();
        b.re_executed = self.re_executed;
        b.id = self.id;
        b.seed = self.seed.clone();
        b.sim_steps = self.sim_steps;
        b.cells_baseline = b.cell_extent();
        b
    }

    /// Takes back what a finished branch did, in the order the branches are written: its memory,
    /// its calls, and what it performed.
    pub(crate) fn absorb(&mut self, mut branch: Ctx) {
        crate::detached::release_all(&mut branch);
        branch.cells.close_program_regions();
        self.heap.adopt_heap(std::mem::take(&mut branch.heap));
        self.ticks = self.ticks.saturating_add(branch.ticks);
        self.grown += branch.grown;
        self.performed.append(&mut branch.performed);
        self.host_use.absorb(&branch.host_use);
        self.host_ops = self.host_ops.saturating_add(branch.host_ops);
        if branch.last_linear.is_some() {
            self.last_linear = branch.last_linear.take();
        }
        self.teardown.append(&mut branch.teardown);
    }

    /// Whether a `parallel` block's branches may run on other threads: nothing they perform can then
    /// reach a handler this entry holds, a region's scheduler or this thread's host runtime, all of
    /// which live on this thread.
    pub(crate) fn runs_branches_at_once(&self) -> bool {
        // A reactor this thread holds and no factory could make again belongs to this thread alone.
        if !self.sims.is_empty() || (self.runtime.is_some() && self.runtime_factory.is_none()) {
            return false;
        }
        let mut stack = Some(self.current);
        while let Some(s) = stack {
            if !self.stacks[s].list.is_empty() {
                return false;
            }
            stack = self.stacks[s].parent;
        }
        true
    }

    /// The calls left before this entry's budget is spent, or `None` when nothing bounds it.
    pub(crate) fn steps_left(&self) -> Option<i64> {
        (self.step_budget > 0).then(|| self.step_budget - self.ticks)
    }

    /// Fails this entry as one of its `parallel` branches failed.
    pub(crate) fn fail_from_branch(&mut self, code: i64, diagnostic: Option<Diagnostic>) {
        if self.failed == 0 {
            self.failed = code;
        }
        if self.diagnostic.is_none() {
            self.diagnostic = diagnostic;
        }
    }

    /// Fails this entry as running out of its step budget does.
    pub(crate) fn fail_out_of_steps(&mut self) {
        self.ticks = self.step_budget.saturating_add(1);
        self.tick();
    }

    /// Between calls, and only between calls.
    pub fn begin(&mut self, fuel: i64) {
        self.failed = 0;
        self.fuel = fuel;
        self.ticks = 0;
        self.step_budget = step_budget();
        self.grown = 0;
        self.time_budget_ms = time_budget_ms();
        self.deadline = (self.time_budget_ms > 0).then(|| {
            std::time::Instant::now() + std::time::Duration::from_millis(self.time_budget_ms)
        });
        self.arm_tick();
        self.stack_floor = stack_floor();
        self.site_root = -1;
        self.last_linear = None;
        self.diagnostic = None;
        self.stacks.clear();
        self.stacks.push(Frames::under(None));
        self.free_stacks.clear();
        self.current = 0;
        self.performed.clear();
        self.sims.clear();
        self.starting_detached = None;
        self.trail = ply_eval::region::Trail::new(self.seed.clone());
        self.record = None;
        self.entered_sims = 0;
        self.unwind = None;
        self.aborting = None;
        self.resumed = None;
        // Every path out of an entry calls `end`; this catches one that did not, before the
        // detached bodies that pin regions are dropped.
        if self.heap.allocated() != 0 {
            self.end();
        }
        self.detached.clear();
        self.entry = ENTRIES.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        // No cell outlives its entry, so each entry can name its cells as a fresh tier would.
        let renewed = self.cells.renew();
        debug_assert!(
            renewed,
            "an entry began with a region above the floor open or pinned, or a cell live"
        );
        // After the recovery above, which gives back what that entry held.
        self.cells_baseline = self.cell_extent();
        // Another unit's entry may be running further up this thread: it gets its own back at `end`.
        self.outer = (
            heap::swap_current(&mut self.heap),
            heap::poison::swap(&raw const self.site_root),
        );
    }

    /// When compiled code must call back next: the call one past the budget, the end of this
    /// chunk, or never, when the entry is bounded by neither the budget nor a clock.
    fn arm_tick(&mut self) {
        let chunk = self.ticks.saturating_add(TICK_CHUNK);
        self.next_tick = match (self.step_budget > 0, self.deadline.is_some()) {
            (true, _) => self.step_budget.saturating_add(1).min(chunk),
            (false, true) => chunk,
            (false, false) => i64::MAX,
        };
    }

    /// The call counter reached [`Ctx::next_tick`]: charge the budget, read the clock, and say
    /// when to call back. Both bounds are decided here, so a call itself costs one increment and
    /// one compare, and the clock is read once every [`TICK_CHUNK`] calls rather than on any.
    fn tick(&mut self) {
        if self.step_budget > 0 && self.ticks > self.step_budget {
            let budget = self.step_budget;
            let d = Diagnostic::error(
                codes::STEP_BUDGET,
                format!("did not finish within its budget of {budget} calls"),
            )
            .primary(Span::DUMMY, "still running here")
            .note("`--steps N` raises the budget; 0 is no bound");
            self.fail_with(FAILED_OUT_OF_STEPS, d);
            return;
        }
        if let Some(deadline) = self.deadline
            && std::time::Instant::now() > deadline
        {
            let ms = self.time_budget_ms;
            let d = Diagnostic::warning(
                codes::RUN_ABANDONED,
                format!("abandoned after {ms} ms of wall clock"),
            )
            .primary(Span::DUMMY, "still running here")
            .note("the clock says nothing about the program, so this run decided nothing")
            .note("`--timeout MS` sets the clock; 0 is no clock");
            self.fail_with(FAILED_ABANDONED, d);
            return;
        }
        self.arm_tick();
    }

    /// The other end of [`Ctx::begin`]: the entry gives back what it used.
    pub fn end(&mut self) {
        // Only this runs on every exit, so a region a failure or an unwind jumped past, a
        // suspended stack still holds, or a detached body pins, closes here; the cells go back
        // before the heap their words live in.
        crate::detached::release_all(self);
        self.cells.close_program_regions();
        debug_assert!(
            self.cells_balanced(),
            "closing the entry's regions left slots the arena did not reclaim"
        );
        heap::poison::swap(self.outer.1);
        heap::swap_current(self.outer.0);
        self.outer = (std::ptr::null_mut(), std::ptr::null());
        self.last_entry = self.heap.allocated();
        if std::env::var("PLY_C_PHASES").is_ok() {
            eprintln!(
                "entry: {} objects allocated, {} recycled, {}MB of chunks, {} stacks grown",
                self.heap.allocated(),
                self.heap.recycled(),
                self.heap.chunk_bytes() / 1_000_000,
                self.grown
            );
            let by_kind = self.heap.allocated_by_kind();
            let mut kinds: Vec<(usize, usize)> = (0..16).map(|k| (k, by_kind[k])).collect();
            kinds.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
            for (kind, n) in kinds.into_iter().filter(|(_, n)| *n > 0) {
                eprintln!("  allocated: kind {kind}: {n} objects");
            }
            let layouts = &self.tables.layouts;
            for ((kind, key), n) in self.heap.allocated_by_layout().into_iter().take(24) {
                let what = match kind {
                    heap::KIND_CTOR => format!("`{}`", layouts.ctors[key as usize].0),
                    heap::KIND_RECORD => format!(
                        "{{{}}}",
                        layouts
                            .shape_names(key)
                            .iter()
                            .map(|s| s.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    heap::KIND_BYTES => format!("bytes under {key}"),
                    _ => format!("str under {key}"),
                };
                eprintln!("  allocated: {n} of {what}");
            }
            let live = self.heap.live_by_kind();
            if live.iter().map(|(_, n, _)| n).sum::<usize>() > 1000 {
                for (kind, n, bytes) in live {
                    eprintln!(
                        "  live at end: kind {kind}: {n} objects, {}MB",
                        bytes / 1_000_000
                    );
                }
            }
        }
        self.heap.end();
    }

    /// Whether the entry gave back every region it opened and cell slot it took; a cell word in the
    /// answer is refused separately.
    pub fn cells_balanced(&self) -> bool {
        self.cell_extent() == self.cells_baseline
    }

    /// The cell arena's `(regions open on every stack, slots live)`, which an entry has to leave
    /// as it found.
    pub fn cell_extent(&self) -> (usize, usize) {
        (self.cells.total_depth(), self.cells.live())
    }

    /// The running stack, which owns the regions it opens.
    fn owner(&self) -> Owner {
        Owner(self.current)
    }

    /// How deep the running stack's regions stand, for a handler frame that closes back to it.
    pub(crate) fn region_depth(&self) -> usize {
        self.cells.depth(self.owner())
    }

    /// A stack that will not run again gives back the regions it still holds.
    pub(crate) fn release_regions(&mut self, stack: usize) {
        self.cells.close_regions_above(Owner(stack), 0);
    }

    /// The singleton a nullary constructor is.
    pub fn nullary(&self, index: u32) -> Word {
        self.tables.nullaries[index as usize]
    }

    pub fn set_host(
        &mut self,
        binding: Arc<ply_eval::HostBinding>,
        runtime: Option<Rc<dyn ply_eval::HostRuntime>>,
        factory: Option<ply_eval::RuntimeFactory>,
    ) {
        self.binding = binding;
        self.runtime = runtime;
        self.runtime_factory = factory;
    }

    /// The reactor this context waits on, made on first need on a `parallel` branch's thread.
    pub(crate) fn host_runtime(&mut self) -> Option<Rc<dyn ply_eval::HostRuntime>> {
        if self.runtime.is_none()
            && let Some(factory) = &self.runtime_factory
        {
            self.runtime = Some(factory());
        }
        self.runtime.clone()
    }

    pub(crate) fn frames(&mut self) -> &mut Vec<HandlerFrame> {
        &mut self.stacks[self.current].list
    }

    /// A new stack's frames, chained under `parent`; its index names it.
    pub(crate) fn open_stack(&mut self, parent: Option<usize>) -> usize {
        match self.free_stacks.pop() {
            Some(stack) => {
                self.stacks[stack] = Frames::under(parent);
                stack
            }
            None => {
                self.stacks.push(Frames::under(parent));
                self.stacks.len() - 1
            }
        }
    }

    /// Gives back a dead stack's index, its frames and regions already gone; nothing may name it.
    pub(crate) fn close_stack(&mut self, stack: usize) {
        debug_assert!(
            self.stacks[stack].list.is_empty() && self.cells.depth(Owner(stack)) == 0,
            "a stack gave its index back while still holding frames or regions"
        );
        self.free_stacks.push(stack);
    }

    /// The stacks' frame tables this entry holds, given-back ones included: what a leak grows.
    pub fn stack_slots(&self) -> usize {
        self.stacks.len()
    }

    /// The stacks the cell arena keeps a nesting for: what a leak grows.
    pub fn cell_owners(&self) -> usize {
        self.cells.owners()
    }

    pub(crate) fn fail(&mut self, d: Diagnostic) -> i64 {
        self.fail_with(1, d)
    }

    /// Fails as a raise of `abort.raise`, bound for the innermost `handle` in reach with a clause
    /// for it; with none, the entry fails with `d` as [`Ctx::fail`] would.
    pub(crate) fn raise(&mut self, d: Diagnostic, message: String) -> i64 {
        if self.failed != 0 {
            return 0;
        }
        let Some((stack, depth)) = self.abort_handler() else {
            return self.fail(d);
        };
        let diagnostic = self.placed(d);
        self.aborting = Some(Aborting {
            stack,
            depth,
            message,
            diagnostic,
        });
        self.failed = FAILED_ABORT;
        0
    }

    /// Searched as a `perform` searches, so frames hidden while a clause runs are passed over.
    fn abort_handler(&self) -> Option<(usize, usize)> {
        let mut stack = self.current;
        loop {
            let frames = &self.stacks[stack].list;
            if let Some(depth) = frames
                .iter()
                .rposition(|f| f.clauses.iter().any(FrameClause::answers_abort))
            {
                return Some((stack, depth));
            }
            stack = self.stacks[stack].parent?;
        }
    }

    /// How a finished branch failed, a raise bound past it as the failure it carries.
    pub(crate) fn take_branch_failure(&mut self) -> Option<(i64, Option<Diagnostic>)> {
        if self.failed == 0 {
            return None;
        }
        Some(match self.aborting.take() {
            Some(a) if self.failed == FAILED_ABORT => (1, Some(a.diagnostic)),
            _ => (self.failed, self.diagnostic.take()),
        })
    }

    fn fail_with(&mut self, code: i64, d: Diagnostic) -> i64 {
        if self.failed == 0 {
            self.failed = code;
        }
        if self.diagnostic.is_none() {
            self.diagnostic = Some(self.placed(d));
        }
        0
    }

    /// The span the body last stored, or `Span::DUMMY` before any has.
    pub fn site(&self) -> Span {
        self.stored_in(self.stored_root())
    }

    /// [`Ctx::site`] and the definition the body that stored it belongs to.
    pub(crate) fn step_site(&self) -> StepSite {
        let root = self.stored_root();
        StepSite {
            definition: root.map(|r| r.name.clone()),
            span: self.stored_in(root),
        }
    }

    /// Puts `access` into the running step, made where the body last stored a site.
    pub(crate) fn record_access(&mut self, access: Access) {
        let at = self.step_site();
        self.trail.record_access(access, at);
    }

    fn stored_root(&self) -> Option<&Root> {
        let id = u64::try_from(self.site_root).ok()?;
        let roots = &self.tables.roots;
        let at = roots.binary_search_by_key(&id, |r| r.id).ok()?;
        Some(&roots[at])
    }

    /// The stored offsets as a span in `root`'s definition.
    fn stored_in(&self, root: Option<&Root>) -> Span {
        let Some(defined) = root.map(|r| r.span).filter(|s| !s.is_dummy()) else {
            return Span::DUMMY;
        };
        let at = |offset: i64| {
            i64::from(defined.start)
                .checked_add(offset)
                .and_then(|o| u32::try_from(o).ok())
        };
        match (at(self.site_start), at(self.site_end)) {
            (Some(start), Some(end)) => Span::new(defined.source, start, end),
            _ => Span::DUMMY,
        }
    }

    /// `d` anchored at the body's stored site when it names no place of its own.
    fn placed(&self, mut d: Diagnostic) -> Diagnostic {
        let site = self.site();
        if site == Span::DUMMY {
            return d;
        }
        let at = d
            .labels
            .iter()
            .position(|l| l.primary)
            .or_else(|| (!d.labels.is_empty()).then_some(0));
        match at {
            Some(i) if d.labels[i].span == Span::DUMMY => d.labels[i].span = site,
            Some(_) => {}
            None => d = d.primary(site, "here"),
        }
        d
    }

    pub fn take_failure(&mut self) -> Option<Diagnostic> {
        self.diagnostic
            .take()
            .or_else(|| self.aborting.take().map(|a| a.diagnostic))
    }

    /// The value a word denotes, for a builtin or an error message.
    pub(crate) fn value(&self, w: Word) -> Value {
        Heap::to_value(&self.tables.layouts, w)
    }

    pub(crate) fn word(&mut self, v: &Value) -> Word {
        let tables = Arc::clone(&self.tables);
        self.heap.to_word(&tables.layouts, v)
    }

    /// Safe on any word an error path meets: an immediate or `0` reads no memory.
    fn type_name(&self, w: Word) -> &'static str {
        if w == 0 {
            return "no value";
        }
        self.value(w).type_name()
    }
}

/// A runtime failure inside compiled code.
fn error(message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(codes::RUNTIME_ERROR, message.into()).primary(Span::DUMMY, "in compiled code")
}

fn args_of<'a>(ptr: *const i64, n: i64) -> &'a [Word] {
    unsafe { std::slice::from_raw_parts(ptr, n as usize) }
}

/// The arguments a builtin consumes, as the values it reads: each word released once copied.
pub(crate) fn values_taken(ctx: &mut Ctx, args: &[Word]) -> Vec<Value> {
    let mut out = ply_eval::argv::take(args.len());
    for w in args {
        out.push(ctx.value(*w));
        heap::dec(*w);
    }
    out
}

/// Opens a `with cell` region on the running stack; `unique` is the site's proof that no
/// continuation crosses it.
pub unsafe extern "C" fn rt_region(ctx: *mut Ctx, unique: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let kind = match unique {
        0 => ply_eval::RegionKind::Shared,
        _ => ply_eval::RegionKind::Unique,
    };
    let owner = ctx.owner();
    ctx.cells.open(owner, kind).to_bits() as i64
}

/// Closes a region, reclaiming its cells unless something that can still run reaches them: a
/// snapshot of its stack, or a detached body opened inside it. The emitter puts it after the body,
/// on the stack that opened it, so only a body that ran to its end reaches it; an abandoned one is
/// closed by its `handle`, by its stack's release, or by [`Ctx::end`].
pub unsafe extern "C" fn rt_region_close(ctx: *mut Ctx, region: i64) {
    let ctx = unsafe { &mut *ctx };
    ctx.cells.close(RegionId::from_bits(region as u64));
}

/// Allocates a cell in the running stack's innermost region.
pub unsafe extern "C" fn rt_cell(ctx: *mut Ctx, init: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    if !ctx.sims.is_empty() {
        ctx.record_access(Access::Alloc);
    }
    let owner = ctx.owner();
    let slot = ctx
        .cells
        .alloc(owner, Held(init))
        .expect("a `with_cell` allocates in the region its stack just opened");
    ctx.heap.bridge(Value::Cell(slot))
}

/// Perceus's `dup`: the same word, held once more.
pub unsafe extern "C" fn rt_dup(_ctx: *mut Ctx, w: i64) -> i64 {
    heap::inc(w);
    w
}

/// Perceus's `drop`: one holder fewer.
pub unsafe extern "C" fn rt_dec(ctx: *mut Ctx, w: i64) {
    let ctx = unsafe { &mut *ctx };
    ctx.heap.release_last(w);
}

/// Perceus's `reset`, as [`heap::reset`].
pub unsafe extern "C" fn rt_reset(_ctx: *mut Ctx, w: i64) -> i64 {
    heap::reset(w)
}

pub unsafe extern "C" fn rt_box_int(ctx: *mut Ctx, v: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    ctx.heap.boxed_int(v)
}

pub unsafe extern "C" fn rt_unbox_int(ctx: *mut Ctx, w: i64) -> i64 {
    match heap::as_int(w) {
        Some(i) => i,
        None => {
            let ctx = unsafe { &mut *ctx };
            let d = error(format!("an `Int` operation on a {}", ctx.type_name(w)));
            ctx.fail(d)
        }
    }
}

pub unsafe extern "C" fn rt_unbox_bool(ctx: *mut Ctx, w: i64) -> i64 {
    match heap::as_bool(w) {
        Some(b) => i64::from(b),
        None => {
            let ctx = unsafe { &mut *ctx };
            let d = error(format!("a condition of type {}", ctx.type_name(w)));
            ctx.fail(d)
        }
    }
}

/// The operator codes compiled code hands [`rt_binary`]; `emit.ply`'s `binary_code` must match.
const BINOPS: [BinOp; 18] = [
    BinOp::Add,
    BinOp::Sub,
    BinOp::Mul,
    BinOp::Div,
    BinOp::Rem,
    BinOp::Eq,
    BinOp::Ne,
    BinOp::Lt,
    BinOp::Le,
    BinOp::Gt,
    BinOp::Ge,
    BinOp::Concat,
    BinOp::BitAnd,
    BinOp::BitOr,
    BinOp::BitXor,
    BinOp::Shl,
    BinOp::Shr,
    BinOp::Ushr,
];

/// The machine's own negation of a `Float`, a `Decimal` or a width past 32 bits, which compiled
/// code holds as the runtime's own words. Takes it.
pub unsafe extern "C" fn rt_negate(ctx: *mut Ctx, a: i64) -> i64 {
    let c = unsafe { &mut *ctx };
    let vals = values_taken(c, &[a]);
    let answer = match &vals[0] {
        Value::Float(f) => Value::Float(-f),
        Value::Decimal(d) => Value::Decimal(-*d),
        Value::Fixed(f) => match f.checked_neg() {
            Some(n) => Value::Fixed(n),
            None => return c.fail(error("negation overflowed its width")),
        },
        other => match other.as_int(Span::DUMMY, "negation") {
            Ok(i) => match i.checked_neg() {
                Some(n) => Value::Int(n),
                None => return c.fail(error("integer overflow in negation")),
            },
            Err(d) => return c.fail(d),
        },
    };
    c.word(&answer)
}

/// `~` over a word the emitter cannot read as an `Int`: a width past 32 bits. Takes it.
pub unsafe extern "C" fn rt_bitnot(ctx: *mut Ctx, a: i64) -> i64 {
    let c = unsafe { &mut *ctx };
    let vals = values_taken(c, &[a]);
    let answer = match &vals[0] {
        Value::Fixed(f) => Value::Fixed(ply_eval::Fixed::new(f.ty, !f.bits())),
        other => match other.as_int(Span::DUMMY, "`~`") {
            Ok(i) => Value::Int(!i),
            Err(d) => return c.fail(d),
        },
    };
    c.word(&answer)
}

/// The machine's own operator over two words of a `Float`, a `Decimal` or a width past 32 bits.
/// Takes both.
pub unsafe extern "C" fn rt_binary(ctx: *mut Ctx, op: i64, a: i64, b: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let Some(op) = usize::try_from(op)
        .ok()
        .and_then(|i| BINOPS.get(i).copied())
    else {
        return ctx.fail(error("an operator this runtime has no code for"));
    };
    let vals = values_taken(ctx, &[a, b]);
    match ply_eval::strict_binary(
        op,
        &vals[0],
        &vals[1],
        Span::DUMMY,
        Span::DUMMY,
        Span::DUMMY,
    ) {
        Ok(v) => ctx.word(&v),
        Err(d) if matches!(op, BinOp::Div | BinOp::Rem) && is_zero(&vals[1]) => raise_error(ctx, d),
        Err(d) => ctx.fail(d),
    }
}

/// A divisor `/` and `%` raise on rather than overflow.
fn is_zero(v: &Value) -> bool {
    match v {
        Value::Int(n) => *n == 0,
        Value::Fixed(f) => f.raw() == 0,
        Value::Decimal(d) => d.is_zero(),
        _ => false,
    }
}

/// The prologue's refusal: this call would nest past the budget the machine handed the entry.
pub unsafe extern "C" fn rt_no_fuel(ctx: *mut Ctx) {
    let ctx = unsafe { &mut *ctx };
    let d = error("this call would nest past the machine's own bound on nested calls");
    ctx.fail_with(FAILED_OUT_OF_FUEL, d);
}

/// What a growth that could not be given a stack raises: the one bound left is the platform's.
pub unsafe extern "C" fn rt_no_stack(ctx: *mut Ctx) {
    let ctx = unsafe { &mut *ctx };
    let d = error("this call would nest past what the native stack holds");
    ctx.fail_with(FAILED_OUT_OF_STACK, d);
}

/// The handover a grown call runs from: what to enter, with what, where its answer goes, and the
/// stack pointer to come back to. It lives in [`rt_grow`]'s frame, which the new stack outlives.
struct Grown {
    ctx: *mut Ctx,
    entry: Entry,
    args: *const i64,
    answer: i64,
    back: usize,
}

/// The new stack's first and only frame: the call, then back to whoever grew it. Nothing switches
/// into this stack again, so where its own pointer lands is spent with it.
extern "C" fn run_grown(arg: usize) {
    let g = arg as *mut Grown;
    let (ctx, entry, args) = unsafe { ((*g).ctx, (*g).entry, (*g).args) };
    let answer = unsafe { entry(ctx, args) };
    let to = unsafe {
        (*g).answer = answer;
        (*g).back
    };
    let mut spent = 0;
    unsafe { switch(&mut spent, to) };
    std::process::abort();
}

/// The prologue's answer to a frame that would cross the floor: the call runs on a stack of its
/// own, so how deep a program nests is the fuel's answer and never this thread's. `entry` is the
/// callee's own `(ctx, args)` form and `args` its arguments, both still held by the caller's frame.
pub unsafe extern "C" fn rt_grow(ctx: *mut Ctx, entry: i64, args: i64) -> i64 {
    let Some(stack) = Stack::reserve() else {
        unsafe { rt_no_stack(ctx) };
        return 0;
    };
    let mut grown = Grown {
        ctx,
        // SAFETY: the emitter passes the address of a compiled function's own `(ctx, args)` entry.
        entry: unsafe { std::mem::transmute::<usize, Entry>(entry as usize) },
        args: args as usize as *const i64,
        answer: 0,
        back: 0,
    };
    let handover = &raw mut grown;
    let sp = stack.prepare(run_grown, handover as usize);
    let floor = {
        let c = unsafe { &mut *ctx };
        c.grown += 1;
        std::mem::replace(&mut c.stack_floor, stack.floor())
    };
    let from = unsafe { &mut (*handover).back };
    unsafe { switch(from, sp) };
    let c = unsafe { &mut *ctx };
    c.stack_floor = floor;
    unsafe { (*handover).answer }
}

/// The callback the counter reaching [`Ctx::next_tick`] makes: the entry gets more work, or it
/// gets none.
pub unsafe extern "C" fn rt_tick(ctx: *mut Ctx) {
    let ctx = unsafe { &mut *ctx };
    ctx.tick();
}

/// Calls between two callbacks while an entry is still within its bounds.
const TICK_CHUNK: i64 = 4096;

thread_local! {
    /// The calls one entry may make; 0 is no bound.
    static THREAD_STEP_BUDGET: std::cell::Cell<Option<i64>> = const { std::cell::Cell::new(None) };
}

/// Runs `f` with entries on this thread bounded by `steps` rather than by the default budget.
pub fn with_step_budget<R>(steps: i64, f: impl FnOnce() -> R) -> R {
    let before = THREAD_STEP_BUDGET.with(|t| t.replace(Some(steps.max(0))));
    let out = f();
    THREAD_STEP_BUDGET.with(|t| t.set(before));
    out
}

pub fn step_budget() -> i64 {
    THREAD_STEP_BUDGET
        .with(|t| t.get())
        .unwrap_or(ply_eval::DEFAULT_STEP_BUDGET)
}

/// Runs `f` with entries on this thread bounded by neither budget: the compiler's own work is
/// not the program's, so the program's bounds are not its.
pub fn unbounded<R>(f: impl FnOnce() -> R) -> R {
    with_step_budget(0, || with_time_budget(0, f))
}

thread_local! {
    /// The wall clock an entry may take, in milliseconds; 0 is none. It abandons a run rather than
    /// judging it, so only a harness that will say so sets one.
    static THREAD_TIME_BUDGET_MS: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

/// Runs `f` with entries on this thread bounded by `ms` rather than by none.
pub fn with_time_budget<R>(ms: u64, f: impl FnOnce() -> R) -> R {
    let before = THREAD_TIME_BUDGET_MS.with(|t| t.replace(Some(ms)));
    let out = f();
    THREAD_TIME_BUDGET_MS.with(|t| t.set(before));
    out
}

pub fn time_budget_ms() -> u64 {
    THREAD_TIME_BUDGET_MS.with(|t| t.get()).unwrap_or(0)
}

/// Room below the floor for the runtime's frames and the deepest compiled frame itself.
pub(crate) const STACK_MARGIN: usize = 512 * 1024;

/// The floor for this thread, asked of the platform once per thread (it can be a `/proc` read).
pub(crate) fn stack_floor() -> usize {
    thread_local! {
        static FLOOR: usize = stack_floor_of_this_thread();
    }
    FLOOR.with(|f| *f)
}

#[cfg(target_os = "macos")]
fn stack_floor_of_this_thread() -> usize {
    unsafe extern "C" {
        fn pthread_self() -> *mut std::ffi::c_void;
        fn pthread_get_stackaddr_np(thread: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
        fn pthread_get_stacksize_np(thread: *mut std::ffi::c_void) -> usize;
    }
    // The address is the stack's *top*; the region runs down from it.
    let (top, size) = unsafe {
        let me = pthread_self();
        (
            pthread_get_stackaddr_np(me) as usize,
            pthread_get_stacksize_np(me),
        )
    };
    top.saturating_sub(size).saturating_add(STACK_MARGIN)
}

#[cfg(target_os = "linux")]
fn stack_floor_of_this_thread() -> usize {
    // Room for the opaque `pthread_attr_t`, at most 56 bytes on 64-bit libcs.
    #[repr(C, align(8))]
    struct Attr([u8; 64]);
    unsafe extern "C" {
        fn pthread_self() -> usize;
        fn pthread_getattr_np(thread: usize, attr: *mut Attr) -> i32;
        fn pthread_attr_getstack(
            attr: *const Attr,
            addr: *mut *mut std::ffi::c_void,
            size: *mut usize,
        ) -> i32;
        fn pthread_attr_destroy(attr: *mut Attr) -> i32;
    }
    let mut attr = Attr([0; 64]);
    let mut addr: *mut std::ffi::c_void = std::ptr::null_mut();
    let mut size = 0usize;
    let bottom = unsafe {
        if pthread_getattr_np(pthread_self(), &mut attr) != 0 {
            return fallback_floor();
        }
        let ok = pthread_attr_getstack(&attr, &mut addr, &mut size) == 0;
        pthread_attr_destroy(&mut attr);
        if !ok {
            return fallback_floor();
        }
        addr as usize
    };
    bottom.saturating_add(STACK_MARGIN)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn stack_floor_of_this_thread() -> usize {
    fallback_floor()
}

/// Assumes a spawned thread's default stack below this frame: refusing early beats overflowing.
#[cfg(not(target_os = "macos"))]
fn fallback_floor() -> usize {
    let here = 0u8;
    (std::ptr::from_ref(&here) as usize).saturating_sub(1 << 20)
}

/// Whichever of `checked_mul`, `checked_div` and `checked_rem` the operator was.
pub unsafe extern "C" fn rt_arith(ctx: *mut Ctx, op: i64, a: i64, b: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let (result, what) = match op {
        0 => (a.checked_mul(b), "multiplication"),
        1 => (a.checked_div(b), "division"),
        _ => (a.checked_rem(b), "remainder"),
    };
    match result {
        Some(n) => n,
        None => {
            // The machine's own words, so a failure reads the same on either engine.
            if op != 0 && b == 0 {
                raise_error(ctx, error(format!("{what} by zero")))
            } else {
                ctx.fail(error(format!("integer overflow in {what}")))
            }
        }
    }
}

/// A literal: the unit's immortal word for it, built once at load.
pub unsafe extern "C" fn rt_lit(ctx: *mut Ctx, index: i64) -> i64 {
    let ctx = unsafe { &*ctx };
    ctx.tables.const_words[index as usize]
}

/// A `match` whose arms did not cover the value.
pub unsafe extern "C" fn rt_no_match(ctx: *mut Ctx) {
    let ctx = unsafe { &mut *ctx };
    let d = error("no arm of this `match` matched");
    ctx.fail(d);
}

/// A refutable `let` whose pattern did not match the value bound to it.
pub unsafe extern "C" fn rt_let_no_match(ctx: *mut Ctx) {
    let ctx = unsafe { &mut *ctx };
    let d = error("`let` pattern did not match the bound value");
    raise_error(ctx, d);
}

/// `what` is `emit.ply`'s `overflow_code`.
pub unsafe extern "C" fn rt_overflow(ctx: *mut Ctx, what: i64) {
    let ctx = unsafe { &mut *ctx };
    let name = match what {
        0 => "addition",
        1 => "subtraction",
        2 => "negation",
        3 => "multiplication",
        _ => "division",
    };
    let d = error(format!("integer overflow in {name}"));
    ctx.fail(d);
}

/// An `Int` outside the target width; `which` indexes [`ply_eval::INT_TYPES`].
pub unsafe extern "C" fn rt_not_that_width(ctx: *mut Ctx, which: i64, value: i64) {
    let ctx = unsafe { &mut *ctx };
    let t = ply_eval::INT_TYPES[which as usize];
    let d = error(format!(
        "`{}` was given {value}: `{t}` holds {} to {}",
        t.of_int_name(),
        t.min(),
        t.max()
    ));
    raise_error(ctx, d);
}

/// `==` beyond two `Int`s or `Bool`s, deferring to the evaluator's comparison. Reads both.
pub unsafe extern "C" fn rt_equal(ctx: *mut Ctx, a: i64, b: i64) -> i64 {
    if heap::is_imm(a) && heap::is_imm(b) {
        return i64::from(a == b);
    }
    if !heap::is_imm(a) && !heap::is_imm(b) {
        let (ka, kb) = (heap::kind(a), heap::kind(b));
        if ka == kb && (ka == KIND_STR || ka == KIND_BYTES) {
            return i64::from(unsafe { bytes_of(obj(a)) == bytes_of(obj(b)) });
        }
    }
    let ctx = unsafe { &mut *ctx };
    let (l, r) = (ctx.value(a), ctx.value(b));
    match values_equal(&l, &r, Span::DUMMY) {
        Ok(eq) => i64::from(eq),
        Err(d) => ctx.fail(d),
    }
}

/// `++`: two strings or two byte strings append natively, answering the kind they share; anything
/// else raises what `strict_binary` raises. Takes both.
pub unsafe extern "C" fn rt_concat(ctx: *mut Ctx, a: i64, b: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let kind = heap::kind(a);
    if kind == heap::kind(b) && (kind == KIND_STR || kind == KIND_BYTES) {
        let out = ctx.heap.append(a, unsafe { bytes_of(obj(b)) });
        heap::dec(b);
        return out;
    }
    let (l, r) = (ctx.value(a), ctx.value(b));
    heap::dec(a);
    heap::dec(b);
    match ply_eval::strict_binary(BinOp::Concat, &l, &r, Span::DUMMY, Span::DUMMY, Span::DUMMY) {
        Ok(v) => ctx.word(&v),
        Err(d) => ctx.fail(d),
    }
}

/// A builtin over taken arguments, by this unit's index for it.
pub unsafe extern "C" fn rt_builtin(ctx: *mut Ctx, index: i64, args: *const i64, n: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let b = ctx.tables.builtins[index as usize];
    builtin(ctx, b, args_of(args, n))
}

/// Every call of a builtin, whether named or called through a value: natively over words where it
/// can be, else over the values they denote. Takes the arguments.
fn builtin(ctx: &mut Ctx, b: Builtin, args: &[Word]) -> Word {
    if !ctx.sims.is_empty()
        && let Some(access) = crate::simulate::cell_access(ctx, b, args)
    {
        ctx.record_access(access);
    }
    match native_builtin(ctx, b, args) {
        Some(w) => w,
        None => builtin_over_values(ctx, b, args),
    }
}

/// `b` over the values the words denote. Takes the arguments.
fn builtin_over_values(ctx: &mut Ctx, b: Builtin, args: &[Word]) -> Word {
    let values = values_taken(ctx, args);
    let site = ctx.site();
    let panicked = match (b, values.first()) {
        (Builtin::Panic, Some(Value::Str(s))) => Some(s.to_string()),
        _ => None,
    };
    match ply_eval::builtins::call(b, values, site) {
        Ok(v) => ctx.word(&v),
        Err(d) if b.raises() => {
            let message = panicked.unwrap_or_else(|| raised_message(&d));
            ctx.raise(d, message)
        }
        Err(d) => ctx.fail(d),
    }
}

/// A runtime error's message as a clause for `abort.raise` is given it, each value the text names
/// told by its kind: nothing in Rust renders one.
fn raised_message(d: &Diagnostic) -> String {
    d.values
        .iter()
        .enumerate()
        .fold(d.message.clone(), |text, (i, v)| {
            text.replace(&ply_eval::slot(i), v.describe())
        })
}

/// [`Ctx::raise`] for a runtime error that is the program's to answer.
fn raise_error(ctx: &mut Ctx, d: Diagnostic) -> i64 {
    let message = raised_message(&d);
    ctx.raise(d, message)
}

/// `bytes_concat_all` over a list literal's pieces, without building the list. Takes the pieces.
pub unsafe extern "C" fn rt_bytes_join(ctx: *mut Ctx, args: *const i64, n: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let pieces = args_of(args, n);
    if pieces.iter().any(|w| heap::kind(*w) != KIND_BYTES) {
        let xs = ctx.heap.list_from(pieces);
        return builtin_over_values(ctx, Builtin::BytesConcatAll, &[xs]);
    }
    // A unique accumulator grows in place; a fresh buffer per step would be quadratic.
    if let Some((&first, rest)) = pieces.split_first()
        && heap::is_unique(first)
    {
        let mut out = first;
        for w in rest {
            out = ctx.heap.append(out, unsafe { bytes_of(obj(*w)) });
            heap::dec(*w);
        }
        return out;
    }
    let total: usize = pieces
        .iter()
        .map(|w| unsafe { (*obj(*w)).len } as usize)
        .sum();
    if total <= 1 {
        let one = pieces
            .iter()
            .find_map(|w| unsafe { bytes_of(obj(*w)) }.first().copied());
        for w in pieces {
            heap::dec(*w);
        }
        return ctx.heap.bytes(one.as_slice());
    }
    let out = ctx.heap.alloc_bytes(KIND_BYTES, total as u32);
    let mut at = 0;
    for w in pieces {
        let piece = unsafe { bytes_of(obj(*w)) };
        unsafe {
            std::ptr::copy_nonoverlapping(
                piece.as_ptr(),
                heap::bytes_ptr(out).add(at),
                piece.len(),
            );
        }
        at += piece.len();
        heap::dec(*w);
    }
    unsafe { (*out).len = total as u32 };
    out as Word
}

/// The cell a word names, when it is one.
fn cell_of(w: Word) -> Option<Slot> {
    if heap::kind(w) != crate::heap::KIND_BRIDGE {
        return None;
    }
    match unsafe { crate::heap::bridged(obj(w)) } {
        Value::Cell(slot) => Some(*slot),
        _ => None,
    }
}

/// The cell's contents, as a count of its own.
fn cell_read(ctx: &Ctx, slot: Slot, what: &str) -> Result<Word, Diagnostic> {
    let site = ctx.site();
    let arena = ctx.cells.arena();
    if arena.is_taken(slot) {
        return Err(cell_in_update(site, slot, what));
    }
    match arena.get(slot) {
        Some(held) => {
            heap::inc(held.0);
            Ok(held.0)
        }
        None => Err(no_such_cell(site, slot)),
    }
}

/// The builtins answered over native words; `None` answers the call over values instead.
fn native_builtin(ctx: &mut Ctx, which: Builtin, args: &[Word]) -> Option<Word> {
    match (which, args) {
        (Builtin::Len, [xs]) if heap::kind(*xs) == KIND_LIST => {
            let n = unsafe { (*obj(*xs)).len } as i64;
            heap::dec(*xs);
            Some(heap::imm(n))
        }
        (Builtin::Push, [xs, x]) if heap::kind(*xs) == KIND_LIST => {
            Some(ctx.heap.list_push(*xs, *x))
        }
        // Cells hold heap words, so their contents never cross the seam.
        (Builtin::CellGet, [c]) => {
            let slot = cell_of(*c)?;
            let answer = match cell_read(ctx, slot, "cell_get") {
                Ok(w) => w,
                Err(d) => ctx.fail(d),
            };
            heap::dec(*c);
            Some(answer)
        }
        (Builtin::CellSet, [c, v]) => {
            let slot = cell_of(*c)?;
            let site = ctx.site();
            if ctx.cells.arena().is_taken(slot) {
                heap::dec(*v);
                heap::dec(*c);
                return Some(ctx.fail(cell_in_update(site, slot, "cell_set")));
            }
            if heap::reaches_cell(*v, slot) {
                ply_eval::rc::note_cell_cycle(slot, site);
            }
            let stored = ctx.cells.arena_mut().set(slot, Held(*v));
            heap::dec(*c);
            if !stored {
                return Some(ctx.fail(no_such_cell(site, slot)));
            }
            Some(heap::unit())
        }
        (Builtin::CellUpdate, [c, f]) => {
            let slot = cell_of(*c)?;
            let site = ctx.site();
            if ctx.cells.arena().is_taken(slot) {
                heap::dec(*f);
                heap::dec(*c);
                return Some(ctx.fail(cell_in_update(site, slot, "cell_update")));
            }
            let Some(current) = ctx.cells.arena_mut().take(slot) else {
                heap::dec(*f);
                heap::dec(*c);
                return Some(ctx.fail(no_such_cell(site, slot)));
            };
            // A raise caught while the cell is still open must find it as it was, so with a clause
            // for one in reach the contents are held twice, and `f` updates a copy.
            let kept = ctx.abort_handler().is_some().then(|| current.clone());
            let updated = call_value(std::ptr::from_mut(ctx), *f, &[current.into_word()]);
            let held = match kept {
                Some(old) if ctx.failed == FAILED_ABORT => old,
                _ if ctx.failed != 0 => Held::default(),
                _ => Held(updated),
            };
            ctx.cells.arena_mut().put_back(slot, held);
            heap::dec(*c);
            Some(if ctx.failed != 0 { 0 } else { heap::unit() })
        }
        // Only compiled code can enter a closure, so these answer whatever they are given.
        (Builtin::Map, [xs, f]) => Some(unsafe { rt_map(std::ptr::from_mut(ctx), *xs, *f) }),
        (Builtin::Filter, [xs, p]) => Some(unsafe { rt_filter(std::ptr::from_mut(ctx), *xs, *p) }),
        (Builtin::Fold, [xs, init, f]) => {
            Some(unsafe { rt_fold(std::ptr::from_mut(ctx), *xs, *init, *f) })
        }
        (Builtin::MapFold, [m, init, f]) => {
            Some(unsafe { rt_map_fold(std::ptr::from_mut(ctx), *m, *init, *f) })
        }
        (Builtin::Iterate, [seed, budget, f]) => {
            Some(unsafe { rt_iterate(std::ptr::from_mut(ctx), *seed, *budget, *f) })
        }
        (Builtin::MapUpdate, [m, k, f]) => Some(map_update(ctx, *m, *k, *f)),
        (Builtin::BytesPosition, [b, from, p]) => Some(bytes_position(ctx, *b, *from, *p)),
        (Builtin::ListAt, [xs, i]) if heap::kind(*xs) == KIND_LIST => {
            let o = obj(*xs);
            let index = heap::as_int(*i)?;
            let len = list::len(o) as i64;
            let some = ctx.tables.layouts.some?;
            let none = ctx.tables.layouts.none?;
            let answer = if (0..len).contains(&index) {
                let item = list::get(o, index as usize);
                heap::inc(item);
                let c = ctx.heap.alloc(KIND_CTOR, 0, 1, some);
                unsafe { set_word(c, 0, item) };
                c as Word
            } else {
                ctx.nullary(none)
            };
            heap::dec(*xs);
            Some(answer)
        }
        (Builtin::ListSet, [xs, i, v]) if heap::kind(*xs) == KIND_LIST => {
            let index = usize::try_from(heap::as_int(*i)?).ok()?;
            if index >= list::len(obj(*xs)) {
                return None;
            }
            Some(ctx.heap.list_set(*xs, index, *v))
        }
        (Builtin::Range, [lo, hi]) => {
            let (a, b) = (heap::as_int(*lo)?, heap::as_int(*hi)?);
            if b - a > (1 << 20) {
                return None;
            }
            let items: Vec<Word> = (a..b).map(heap::imm).collect();
            Some(ctx.heap.list_from(&items))
        }
        // Anything the value builtins would raise on answers `None` before touching a count.
        (Builtin::BytesLen, [b]) if heap::kind(*b) == KIND_BYTES => {
            let n = unsafe { (*obj(*b)).len } as i64;
            heap::dec(*b);
            Some(heap::imm(n))
        }
        (Builtin::BytesAt, [b, i]) if heap::kind(*b) == KIND_BYTES => {
            let index = usize::try_from(heap::as_int(*i)?).ok()?;
            let byte = *unsafe { bytes_of(obj(*b)) }.get(index)?;
            heap::dec(*b);
            Some(heap::imm(i64::from(byte)))
        }
        (Builtin::BytesU32Le, [b, i]) if heap::kind(*b) == KIND_BYTES => {
            let index = usize::try_from(heap::as_int(*i)?).ok()?;
            let four = unsafe { bytes_of(obj(*b)) }.get(index..index + 4)?;
            let w = u32::from_le_bytes(four.try_into().ok()?);
            heap::dec(*b);
            Some(heap::imm(i64::from(w)))
        }
        (Builtin::BytesSlice, [b, s, e]) if heap::kind(*b) == KIND_BYTES => {
            let bytes = unsafe { bytes_of(obj(*b)) };
            let (start, end) = slice_range(*s, *e, bytes.len())?;
            let out = ctx.heap.bytes(&bytes[start..end]);
            heap::dec(*b);
            Some(out)
        }
        (Builtin::BytesConcat, [a, b])
            if heap::kind(*a) == KIND_BYTES && heap::kind(*b) == KIND_BYTES =>
        {
            let out = ctx.heap.append(*a, unsafe { bytes_of(obj(*b)) });
            heap::dec(*b);
            Some(out)
        }
        (Builtin::BytesConcatAll, [xs]) if heap::kind(*xs) == KIND_LIST => {
            let o = obj(*xs);
            let mut total = 0;
            let mut all_bytes = true;
            list::for_each(o, &mut |w| {
                if heap::kind(w) != KIND_BYTES {
                    all_bytes = false;
                } else {
                    total += unsafe { (*obj(w)).len } as usize;
                }
            });
            if !all_bytes {
                return None;
            }
            // As `rt_bytes_join`: a unique first piece of a unique list grows in place.
            if heap::is_unique(*xs) {
                let items = list::to_vec(o);
                if let Some(&first) = items.first()
                    && heap::is_unique(first)
                {
                    for &w in &items {
                        heap::inc(w);
                    }
                    heap::dec(*xs);
                    let mut out = first;
                    for &w in &items[1..] {
                        out = ctx.heap.append(out, unsafe { bytes_of(obj(w)) });
                        heap::dec(w);
                    }
                    return Some(out);
                }
            }
            if total <= 1 {
                let mut one = None;
                list::for_each(o, &mut |w| {
                    one = one.or(unsafe { bytes_of(obj(w)) }.first().copied());
                });
                heap::dec(*xs);
                return Some(ctx.heap.bytes(one.as_slice()));
            }
            let out = ctx.heap.alloc_bytes(KIND_BYTES, total as u32);
            let mut at = 0;
            list::for_each(o, &mut |w| {
                let piece = unsafe { bytes_of(obj(w)) };
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        piece.as_ptr(),
                        heap::bytes_ptr(out).add(at),
                        piece.len(),
                    );
                }
                at += piece.len();
            });
            unsafe { (*out).len = total as u32 };
            heap::dec(*xs);
            Some(out as Word)
        }
        (Builtin::ByteOfInt, [n]) => {
            let byte = u8::try_from(heap::as_int(*n)?).ok()?;
            Some(ctx.tables.byte(byte))
        }
        (Builtin::BytesOfString, [s]) if heap::kind(*s) == KIND_STR => {
            let out = ctx.heap.bytes(unsafe { bytes_of(obj(*s)) });
            heap::dec(*s);
            Some(out)
        }
        (Builtin::StringOfBytes, [b]) if heap::kind(*b) == KIND_BYTES => {
            let text = std::str::from_utf8(unsafe { bytes_of(obj(*b)) }).ok()?;
            let out = ctx.heap.str(text);
            heap::dec(*b);
            Some(out)
        }
        (Builtin::BytesIndexOf, [hay, needle])
            if heap::kind(*hay) == KIND_BYTES && heap::kind(*needle) == KIND_BYTES =>
        {
            let at = find_bytes(
                unsafe { bytes_of(obj(*hay)) },
                unsafe { bytes_of(obj(*needle)) },
                0,
            );
            let out = position(ctx, at);
            heap::dec(*hay);
            heap::dec(*needle);
            Some(out)
        }
        (Builtin::BytesIndexOfFrom, [hay, needle, from])
            if heap::kind(*hay) == KIND_BYTES && heap::kind(*needle) == KIND_BYTES =>
        {
            let h = unsafe { bytes_of(obj(*hay)) };
            let from = usize::try_from(heap::as_int(*from)?).ok()?;
            if from > h.len() {
                return None;
            }
            let at = find_bytes(h, unsafe { bytes_of(obj(*needle)) }, from);
            let out = position(ctx, at);
            heap::dec(*hay);
            heap::dec(*needle);
            Some(out)
        }
        (Builtin::BytesIndexOfByte, [hay, byte]) if heap::kind(*hay) == KIND_BYTES => {
            let byte = u8::try_from(heap::as_int(*byte)?).ok()?;
            let at = memchr::memchr(byte, unsafe { bytes_of(obj(*hay)) });
            let out = position(ctx, at);
            heap::dec(*hay);
            Some(out)
        }
        (Builtin::BytesStartsWith | Builtin::BytesEndsWith, [a, b])
            if heap::kind(*a) == KIND_BYTES && heap::kind(*b) == KIND_BYTES =>
        {
            let (x, y) = unsafe { (bytes_of(obj(*a)), bytes_of(obj(*b))) };
            let answer = if which == Builtin::BytesStartsWith {
                x.starts_with(y)
            } else {
                x.ends_with(y)
            };
            heap::dec(*a);
            heap::dec(*b);
            Some(heap::bool(answer))
        }
        (Builtin::StringConcat, [a, b])
            if heap::kind(*a) == KIND_STR && heap::kind(*b) == KIND_STR =>
        {
            let out = ctx.heap.append(*a, unsafe { bytes_of(obj(*b)) });
            heap::dec(*b);
            Some(out)
        }
        (Builtin::StringLen, [s]) if heap::kind(*s) == KIND_STR => {
            let n = unsafe { str_of(obj(*s)) }.chars().count() as i64;
            heap::dec(*s);
            Some(heap::imm(n))
        }
        (Builtin::IntToString, [n]) => {
            let text = heap::as_int(*n)?.to_string();
            Some(ctx.heap.str(&text))
        }
        (Builtin::StringStartsWith | Builtin::StringEndsWith | Builtin::StringContains, [a, b])
            if heap::kind(*a) == KIND_STR && heap::kind(*b) == KIND_STR =>
        {
            let (x, y) = unsafe { (str_of(obj(*a)), str_of(obj(*b))) };
            let answer = match which {
                Builtin::StringStartsWith => x.starts_with(y),
                Builtin::StringEndsWith => x.ends_with(y),
                _ => x.contains(y),
            };
            heap::dec(*a);
            heap::dec(*b);
            Some(heap::bool(answer))
        }
        (Builtin::Len, [s]) if heap::kind(*s) == KIND_STR => {
            let n = unsafe { str_of(obj(*s)) }.chars().count() as i64;
            heap::dec(*s);
            Some(heap::imm(n))
        }
        // Within `max` bytes of `from`: the first byte in (or off) the class, or the window's end.
        (Builtin::BytesScan | Builtin::BytesScanUntil, [hay, from, members, max])
            if heap::kind(*hay) == KIND_BYTES && heap::kind(*members) == KIND_BYTES =>
        {
            let h = unsafe { bytes_of(obj(*hay)) };
            let from = usize::try_from(heap::as_int(*from)?).ok()?;
            if from > h.len() {
                return None;
            }
            let max = usize::try_from(heap::as_int(*max)?).ok()?;
            let window = &h[from..h.len().min(from.saturating_add(max))];
            let want = which == Builtin::BytesScanUntil;
            let found = match (want, unsafe { bytes_of(obj(*members)) }) {
                (true, []) => None,
                (true, [a]) => memchr::memchr(*a, window),
                (true, [a, b]) => memchr::memchr2(*a, *b, window),
                (true, [a, b, c]) => memchr::memchr3(*a, *b, *c, window),
                (_, set) => {
                    let mut bits = [0u64; 4];
                    for &b in set {
                        bits[usize::from(b >> 6)] |= 1 << (b & 63);
                    }
                    window
                        .iter()
                        .position(|&b| (bits[usize::from(b >> 6)] >> (b & 63) & 1 == 1) == want)
                }
            };
            let at = match found {
                Some(at) => from + at,
                None => from + window.len(),
            };
            heap::dec(*hay);
            heap::dec(*members);
            Some(heap::imm(at as i64))
        }
        (Builtin::BytesIsUtf8, [b]) if heap::kind(*b) == KIND_BYTES => {
            let ok = std::str::from_utf8(unsafe { bytes_of(obj(*b)) }).is_ok();
            heap::dec(*b);
            Some(heap::bool(ok))
        }
        (Builtin::StringOfBytesLossy, [b]) if heap::kind(*b) == KIND_BYTES => {
            let text = String::from_utf8_lossy(unsafe { bytes_of(obj(*b)) });
            let out = ctx.heap.str(&text);
            heap::dec(*b);
            Some(out)
        }
        (Builtin::BytesSplit, [b, sep])
            if heap::kind(*b) == KIND_BYTES && heap::kind(*sep) == KIND_BYTES =>
        {
            let (x, y) = unsafe { (bytes_of(obj(*b)), bytes_of(obj(*sep))) };
            if y.is_empty() {
                return None;
            }
            let mut pieces = Vec::new();
            let mut at = 0;
            for found in memchr::memmem::find_iter(x, y) {
                pieces.push(ctx.heap.bytes(&x[at..found]));
                at = found + y.len();
            }
            pieces.push(ctx.heap.bytes(&x[at..]));
            let out = list_of(ctx, &pieces);
            heap::dec(*b);
            heap::dec(*sep);
            Some(out)
        }
        (Builtin::StringSlice, [s, start, end]) if heap::kind(*s) == KIND_STR => {
            let text = unsafe { str_of(obj(*s)) };
            let chars = text.chars().count();
            let (from, to) = slice_range(*start, *end, chars)?;
            let (from, to) = (char_offset(text, from), char_offset(text, to));
            let out = ctx.heap.str(&text[from..to]);
            heap::dec(*s);
            Some(out)
        }
        (Builtin::StringSplit, [s, sep])
            if heap::kind(*s) == KIND_STR && heap::kind(*sep) == KIND_STR =>
        {
            let (x, y) = unsafe { (str_of(obj(*s)), str_of(obj(*sep))) };
            if y.is_empty() {
                return None;
            }
            let pieces: Vec<Word> = x.split(y).map(|piece| ctx.heap.str(piece)).collect();
            let out = list_of(ctx, &pieces);
            heap::dec(*s);
            heap::dec(*sep);
            Some(out)
        }
        (Builtin::StringTrim | Builtin::StringLower | Builtin::StringUpper, [s])
            if heap::kind(*s) == KIND_STR =>
        {
            let text = unsafe { str_of(obj(*s)) };
            let out = match which {
                Builtin::StringTrim => ctx.heap.str(text.trim()),
                Builtin::StringLower => ctx.heap.str(&text.to_lowercase()),
                _ => ctx.heap.str(&text.to_uppercase()),
            };
            heap::dec(*s);
            Some(out)
        }
        (Builtin::StringFind, [s, needle])
            if heap::kind(*s) == KIND_STR && heap::kind(*needle) == KIND_STR =>
        {
            let (x, y) = unsafe { (str_of(obj(*s)), str_of(obj(*needle))) };
            // Absent is the value builtins' diagnostic to raise.
            let at = x.find(y)?;
            let n = x[..at].chars().count() as i64;
            heap::dec(*s);
            heap::dec(*needle);
            Some(heap::imm(n))
        }
        (Builtin::Compare | Builtin::CompareValues, [a, b])
            if heap::native_key(*a) && heap::native_key(*b) =>
        {
            let layouts = &ctx.tables.layouts;
            let index = match heap::cmp_words(layouts, *a, *b) {
                std::cmp::Ordering::Less => layouts.less?,
                std::cmp::Ordering::Equal => layouts.equal?,
                std::cmp::Ordering::Greater => layouts.greater?,
            };
            heap::dec(*a);
            heap::dec(*b);
            Some(ctx.nullary(index))
        }
        (Builtin::MapNew, []) => Some(ctx.tables.empty_map),
        (Builtin::MapInsert, [m, k, v]) if heap::kind(*m) == KIND_MAP && heap::native_key(*k) => {
            let tables = Arc::clone(&ctx.tables);
            Some(ctx.heap.map_insert(&tables.layouts, *m, *k, *v))
        }
        (Builtin::MapGet, [m, k]) if heap::kind(*m) == KIND_MAP && heap::native_key(*k) => {
            let some = ctx.tables.layouts.some?;
            let none = ctx.tables.layouts.none?;
            let o = obj(*m);
            let answer = match map::get(&ctx.tables.layouts, o, *k) {
                Some(v) => {
                    heap::inc(v);
                    let c = ctx.heap.alloc(KIND_CTOR, 0, 1, some);
                    unsafe { set_word(c, 0, v) };
                    c as Word
                }
                None => ctx.nullary(none),
            };
            heap::dec(*m);
            heap::dec(*k);
            Some(answer)
        }
        (Builtin::MapContains, [m, k]) if heap::kind(*m) == KIND_MAP && heap::native_key(*k) => {
            let found = map::get(&ctx.tables.layouts, obj(*m), *k).is_some();
            heap::dec(*m);
            heap::dec(*k);
            Some(heap::bool(found))
        }
        (Builtin::MapRemove, [m, k]) if heap::kind(*m) == KIND_MAP && heap::native_key(*k) => {
            let tables = Arc::clone(&ctx.tables);
            let out = ctx.heap.map_remove(&tables.layouts, *m, *k);
            heap::dec(*k);
            Some(out)
        }
        (Builtin::MapLen, [m]) if heap::kind(*m) == KIND_MAP => {
            let n = unsafe { (*obj(*m)).len } as i64;
            heap::dec(*m);
            Some(heap::imm(n))
        }
        (Builtin::MapKeys | Builtin::MapValues, [m]) if heap::kind(*m) == KIND_MAP => {
            let o = obj(*m);
            let keys = which == Builtin::MapKeys;
            let items: Vec<Word> = map::to_vec(o)
                .into_iter()
                .map(|(k, v)| {
                    let w = if keys { k } else { v };
                    heap::inc(w);
                    w
                })
                .collect();
            let out = ctx.heap.list_from(&items);
            heap::dec(*m);
            Some(out)
        }
        (Builtin::MapEntries, [m]) if heap::kind(*m) == KIND_MAP => {
            let o = obj(*m);
            let shape = ctx.tables.layouts.entry_shape();
            let mut items = Vec::with_capacity(map::len(o));
            for (k, v) in map::to_vec(o) {
                heap::inc(k);
                heap::inc(v);
                let e = ctx.heap.alloc(KIND_RECORD, 0, 2, shape);
                unsafe {
                    set_word(e, 0, k);
                    set_word(e, 1, v);
                }
                items.push(e as Word);
            }
            let out = ctx.heap.list_from(&items);
            heap::dec(*m);
            Some(out)
        }
        (Builtin::MapOfEntries, [xs]) if heap::kind(*xs) == KIND_LIST => {
            let tables = Arc::clone(&ctx.tables);
            let entries = list::to_vec(obj(*xs));
            let n = entries.len();
            let (key, value) = (Symbol::new("key"), Symbol::new("value"));
            // Any other entry shape is the value builtins' to raise on.
            let mut pairs = Vec::with_capacity(n);
            for e in entries {
                if heap::kind(e) != KIND_RECORD {
                    return None;
                }
                let shape = unsafe { (*obj(e)).layout };
                let (Some(ka), Some(va)) = (
                    tables.layouts.offset(shape, &key),
                    tables.layouts.offset(shape, &value),
                ) else {
                    return None;
                };
                let (k, v) = unsafe { (word_at(obj(e), ka), word_at(obj(e), va)) };
                if !heap::native_key(k) {
                    return None;
                }
                pairs.push((k, v));
            }
            let mut m = ctx.heap.map_new();
            for (k, v) in pairs {
                heap::inc(k);
                heap::inc(v);
                m = ctx.heap.map_insert(&tables.layouts, m, k, v);
            }
            heap::dec(*xs);
            Some(m)
        }
        (Builtin::MapMerge, [a, bm])
            if heap::kind(*a) == KIND_MAP && heap::kind(*bm) == KIND_MAP =>
        {
            let tables = Arc::clone(&ctx.tables);
            let o = obj(*bm);
            let mut m = *a;
            for (k, v) in map::to_vec(o) {
                heap::inc(k);
                heap::inc(v);
                m = ctx.heap.map_insert(&tables.layouts, m, k, v);
            }
            heap::dec(*bm);
            Some(m)
        }
        _ => None,
    }
}

/// A slicing builtin's half-open range over `len`, never clamped: out of range is `None`.
fn slice_range(start: Word, end: Word, len: usize) -> Option<(usize, usize)> {
    let (start, end) = (heap::as_int(start)?, heap::as_int(end)?);
    if start < 0 || end < start || !usize::try_from(end).is_ok_and(|e| e <= len) {
        return None;
    }
    Some((start as usize, end as usize))
}

/// Where `needle` first occurs in `hay` at or after `from`; an empty needle occurs at `from`.
fn find_bytes(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() {
        return Some(from);
    }
    memchr::memmem::find(&hay[from..], needle).map(|at| from + at)
}

/// The byte offset of the `n`-th character boundary, as the value builtins' `char_offset` finds it.
fn char_offset(s: &str, n: usize) -> usize {
    s.char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(s.len()))
        .nth(n)
        .unwrap_or(s.len())
}

/// A list of `items`, which it takes.
fn list_of(ctx: &mut Ctx, items: &[Word]) -> Word {
    ctx.heap.list_from(items)
}

/// `Some(at)` or `None`, built over values when the unit does not know the prelude's constructors.
fn position(ctx: &mut Ctx, at: Option<usize>) -> Word {
    let layouts = &ctx.tables.layouts;
    match (at, layouts.some, layouts.none) {
        (Some(i), Some(some), _) => {
            let c = ctx.heap.alloc(KIND_CTOR, 0, 1, some);
            unsafe { set_word(c, 0, heap::imm(i as i64)) };
            c as Word
        }
        (None, _, Some(none)) => ctx.nullary(none),
        (Some(i), None, _) => ctx.word(&Value::ctor("Some", vec![Value::Int(i as i64)])),
        (None, _, None) => ctx.word(&Value::ctor("None", Vec::new())),
    }
}

/// `map_update(m, k, f)`: the entry leaves the map while `f` runs, so a map held once lends `f` a
/// value held once. Takes all three.
fn map_update(ctx: &mut Ctx, m: Word, k: Word, f: Word) -> Word {
    let (rest, current) = if heap::kind(m) == KIND_MAP && heap::native_key(k) {
        let tables = Arc::clone(&ctx.tables);
        match map::get(&tables.layouts, obj(m), k) {
            Some(v) => {
                heap::inc(v);
                (ctx.heap.map_remove(&tables.layouts, m, k), Some(v))
            }
            None => (m, None),
        }
    } else {
        let (map, key) = (ctx.value(m), ctx.value(k));
        heap::dec(m);
        match ply_eval::map::take(map, &key, ctx.site()) {
            Ok((map, v)) => (ctx.word(&map), v.map(|v| ctx.word(&v))),
            Err(d) => {
                heap::dec(k);
                heap::dec(f);
                return ctx.fail(d);
            }
        }
    };
    let Some(current) = current else {
        heap::dec(k);
        heap::dec(f);
        return rest;
    };
    let updated = call_value(std::ptr::from_mut(ctx), f, &[current]);
    heap::dec(f);
    if ctx.failed != 0 {
        heap::dec(rest);
        heap::dec(k);
        return 0;
    }
    builtin(ctx, Builtin::MapInsert, &[rest, k, updated])
}

/// `bytes_position(b, from, p)`: the first index at or past `from` whose byte `p` accepts. Takes
/// all three.
fn bytes_position(ctx: &mut Ctx, b: Word, from: Word, p: Word) -> Word {
    let found = first_accepted(ctx, b, from, p);
    heap::dec(b);
    heap::dec(from);
    heap::dec(p);
    match found {
        Some(at) => position(ctx, at),
        None => 0,
    }
}

/// `bytes_position`'s search, or `None` with the context failed. Reads all three.
fn first_accepted(ctx: &mut Ctx, b: Word, from: Word, p: Word) -> Option<Option<usize>> {
    // Only a payload too long for a native object stays bridged.
    let bridged = (heap::kind(b) != KIND_BYTES).then(|| ctx.value(b));
    let bytes: &[u8] = match &bridged {
        None => unsafe { bytes_of(obj(b)) },
        Some(Value::Bytes(bytes)) => bytes,
        Some(other) => {
            let d = error(format!(
                "`bytes_position` needs Bytes, and this is {}",
                other.type_name()
            ));
            ctx.fail(d);
            return None;
        }
    };
    let start = match heap::as_int(from) {
        Some(n) if usize::try_from(n).is_ok_and(|n| n <= bytes.len()) => n as usize,
        Some(n) => {
            let site = ctx.site();
            let d = ply_eval::builtins::start_outside(n, bytes.len(), site, "bytes_position");
            raise_error(ctx, d);
            return None;
        }
        None => {
            let d = error(format!(
                "`bytes_position` needs an Int start, and this is {}",
                ctx.type_name(from)
            ));
            ctx.fail(d);
            return None;
        }
    };
    for (i, &byte) in bytes.iter().enumerate().skip(start) {
        let r = call_value(std::ptr::from_mut(ctx), p, &[heap::imm(i64::from(byte))]);
        if ctx.failed != 0 {
            return None;
        }
        match heap::as_bool(r) {
            Some(true) => return Some(Some(i)),
            Some(false) => {}
            None => {
                let d = error(format!(
                    "the predicate given to `bytes_position` answered {}, not a Bool",
                    ctx.type_name(r)
                ));
                heap::dec(r);
                ctx.fail(d);
                return None;
            }
        }
    }
    Some(None)
}

/// A closure of compiled function `index` over `env`, its leading arguments. Takes the captures.
pub unsafe extern "C" fn rt_closure(
    ctx: *mut Ctx,
    index: i64,
    arity: i64,
    env: *const i64,
    n: i64,
) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let code = ctx.tables.functions[index as usize];
    let env = args_of(env, n);
    let o = ctx.heap.alloc(
        KIND_CLOSURE,
        0,
        (env.len() + CLOSURE_CAPTURES) as u32,
        arity as u32,
    );
    unsafe {
        set_word(o, CLOSURE_CODE, code as Word);
        for (i, w) in env.iter().enumerate() {
            set_word(o, CLOSURE_CAPTURES + i, *w);
        }
    }
    o as Word
}

/// Installs a handler and answers its depth; the clause closures and `ret` are taken until it pops.
pub unsafe extern "C" fn rt_handle_push(
    ctx: *mut Ctx,
    clauses: *const i64,
    n: i64,
    ret: i64,
) -> i64 {
    let c = unsafe { &mut *ctx };
    let clauses = clauses_of(c, clauses, n);
    let regions = c.region_depth();
    let frames = c.frames();
    frames.push(HandlerFrame {
        clauses,
        ret,
        simulate: false,
        detached: None,
        regions,
    });
    (frames.len() - 1) as i64
}

/// A `handle` site's clause table: per clause the effect, resource (negative for none) and op
/// as field-table indices, the closure, its `resumes`, and whether it binds the label.
fn clauses_of(c: &Ctx, clauses: *const i64, n: i64) -> Vec<FrameClause> {
    let words = args_of(clauses, n * 6);
    let name = |i: i64| c.tables.fields[i as usize].clone();
    words
        .chunks(6)
        .map(|w| FrameClause {
            effect: name(w[0]),
            resource: (w[1] >= 0).then(|| name(w[1])),
            op: name(w[2]),
            closure: w[3],
            resumes: w[4] as u8,
            binds_label: w[5] != 0,
        })
        .collect()
}

/// A `handle` whose clause resumes off the tail: `body`, a nullary closure, runs on its own stack.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_handle_detached(
    ctx: *mut Ctx,
    clauses: *const i64,
    n: i64,
    ret: i64,
    body: i64,
) -> i64 {
    let c = unsafe { &mut *ctx };
    let clauses = clauses_of(c, clauses, n);
    unsafe { crate::detached::open(ctx, clauses, ret, body) }
}

/// The `k` a `resume`-binding clause gets: a tail call records the value for `rt_perform`.
pub(crate) unsafe extern "C" fn rt_resume_entry(ctx: *mut Ctx, args: *const i64) -> i64 {
    let c = unsafe { &mut *ctx };
    let v = unsafe { *args.add(1) };
    c.resumed = Some(v);
    v
}

fn hide_above(c: &mut Ctx, stack: usize, depth: usize) -> Vec<(usize, Vec<HandlerFrame>)> {
    let mut hidden = Vec::new();
    let mut s = c.current;
    while s != stack {
        hidden.push((s, std::mem::take(&mut c.stacks[s].list)));
        s = c.stacks[s]
            .parent
            .expect("the handler's stack is in the chain");
    }
    hidden.push((stack, c.stacks[stack].list.split_off(depth)));
    hidden
}

fn restore_hidden(c: &mut Ctx, hidden: Vec<(usize, Vec<HandlerFrame>)>) {
    for (s, frames) in hidden.into_iter().rev() {
        c.stacks[s].list.extend(frames);
    }
}

fn resume_token(c: &mut Ctx, stack: usize, depth: usize) -> Word {
    closure_of(
        c,
        rt_resume_entry as *const () as usize,
        &[((stack << 32) | depth) as i64],
    )
}

/// A unary closure over immediate captures whose entry is a runtime function.
pub(crate) fn closure_of(c: &mut Ctx, entry: usize, captures: &[i64]) -> Word {
    let o = c.heap.alloc(
        KIND_CLOSURE,
        0,
        (captures.len() + CLOSURE_CAPTURES) as u32,
        1,
    );
    unsafe {
        set_word(o, CLOSURE_CODE, entry as Word);
        for (i, capture) in captures.iter().enumerate() {
            set_word(o, CLOSURE_CAPTURES + i, heap::imm(*capture));
        }
    }
    o as Word
}

/// Records the atom; then the innermost frame with a clause answers, with frames from its own
/// up set aside. A `resume`-binding clause that never resumes unwinds to its `handle`.
pub unsafe extern "C" fn rt_perform(
    ctx: *mut Ctx,
    effect: i64,
    op: i64,
    resource: i64,
    mode: i64,
    args: *const i64,
    n: i64,
) -> i64 {
    let c = unsafe { &mut *ctx };
    let effect = c.tables.fields[effect as usize].clone();
    let op = c.tables.fields[op as usize].clone();
    if effect.as_str() == "abort" && op.as_str() == "raise" {
        return raise_from_perform(c, args_of(args, n));
    }
    let resource = (resource >= 0).then(|| c.tables.fields[resource as usize].clone());
    let atom = EffectAtom::operation(
        effect.clone(),
        resource
            .clone()
            .map_or(Resource::Singleton, Resource::Named),
        if mode != 0 { Mode::Write } else { Mode::Read },
        op.clone(),
    );
    // A step conflicts on the mode, and a scheduled draw's access is recorded here alone.
    if !c.sims.is_empty() {
        c.record_access(Access::Atom(atom.mode_atom()));
    }
    c.performed.push(atom);
    let mut found = None;
    let mut inherited_off_tail = false;
    let mut stack = c.current;
    'search: loop {
        for (i, f) in c.stacks[stack].list.iter().enumerate().rev() {
            if f.simulate {
                if effect.as_str() == "sim" && op.as_str() == "seed" {
                    let root = c.seed.root as i64;
                    return c.word(&Value::Int(root));
                }
                if ply_eval::sim::is_scheduled(effect.as_str(), op.as_str()) {
                    return unsafe {
                        crate::simulate::perform(ctx, &effect, &op, args_of(args, n))
                    };
                }
                continue;
            }
            if let Some(cl) = f
                .clauses
                .iter()
                .find(|cl| cl.answers(&effect, &op, resource.as_ref()))
            {
                if cl.resumes == 2 {
                    let Some(id) = f.detached else {
                        inherited_off_tail = true;
                        break 'search;
                    };
                    let closure = cl.closure;
                    let mut bound: Vec<Word> = args_of(args, n).to_vec();
                    if cl.binds_label
                        && let Some(label) = &resource
                    {
                        bound.push(c.word(&Value::Str(Arc::from(label.as_str()))));
                    }
                    // Moved in: a local owning memory across the switch is freed once per restore.
                    return unsafe {
                        crate::detached::stop(ctx, id, closure, &bound, (effect, op, resource))
                    };
                }
                found = Some((stack, i, cl.closure, cl.resumes != 0, cl.binds_label));
                break 'search;
            }
        }
        match c.stacks[stack].parent {
            Some(p) => stack = p,
            None => break,
        }
    }
    if inherited_off_tail {
        let d = Diagnostic::error(
            codes::RUNTIME_ERROR,
            format!(
                "`{effect}.{op}` was performed in a task, and its handler resumes off the tail"
            ),
        )
        .primary(c.site(), "performed here")
        .note(
            "the clause would capture the continuation of the body the task was spawned in, which \
             is not the task's own: handle the operation inside the task, or resume on the tail",
        );
        return c.fail(d);
    }
    let Some((stack, depth, closure, resumes, binds_label)) = found else {
        // Inside an open production region a `task` op is the scheduler's; outside, the host's.
        if effect.as_str() == "task"
            && ply_eval::sim::TASK_OPS.contains(&op.as_str())
            && c.sims.last().is_some_and(|sim| sim.is_production())
        {
            return unsafe { crate::simulate::perform(ctx, &effect, &op, args_of(args, n)) };
        }
        return unsafe {
            crate::host::perform(ctx, &effect, &op, resource.as_ref(), args_of(args, n))
        };
    };
    let mut call_args: Vec<Word> = args_of(args, n).to_vec();
    // A clause written `[*t]` reads the label its call site named, before the continuation does.
    if binds_label && let Some(label) = &resource {
        call_args.push(c.word(&Value::Str(Arc::from(label.as_str()))));
    }
    // The clause runs outside its handler: that frame and all above it are hidden until it returns.
    let hidden = hide_above(c, stack, depth);
    if resumes {
        let k = resume_token(c, stack, depth);
        call_args.push(k);
        c.resumed = None;
    }
    let r = call_value(ctx, closure, &call_args);
    let c = unsafe { &mut *ctx };
    restore_hidden(c, hidden);
    if c.failed != 0 {
        return 0;
    }
    if !resumes {
        return r;
    }
    match c.resumed.take() {
        Some(v) => v,
        None => {
            c.unwind = Some((stack, depth, r));
            c.failed = FAILED_UNWIND;
            0
        }
    }
}

/// `abort.raise(message)`, which no clause answers where it is performed: the frame whose clause
/// does is unwound to first. Takes the message.
fn raise_from_perform(c: &mut Ctx, args: &[Word]) -> i64 {
    let message = match values_taken(c, args).first() {
        Some(Value::Str(s)) => s.to_string(),
        _ => String::new(),
    };
    let d = Diagnostic::error(codes::RUNTIME_ERROR, format!("panic: {message}"))
        .primary(c.site(), "raised here");
    c.raise(d, message)
}

/// `simulate { body }`: runs the nullary `body` as a region's root task on this stack.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rt_simulate(ctx: *mut Ctx, body: i64) -> i64 {
    let c = unsafe { &mut *ctx };
    if !c.sims.is_empty() {
        heap::dec(body);
        let outer = c.sims.last().map_or(Span::DUMMY, |sim| sim.site);
        let d = ply_eval::err_nested_simulation(c.site(), outer);
        return c.fail(d);
    }
    let id = ply_eval::SimId(c.entered_sims);
    c.entered_sims += 1;
    let site = c.site();
    c.trail.enter(site);
    let stack = c.current;
    let depth = c.frames().len();
    let regions = c.region_depth();
    c.frames().push(HandlerFrame::simulate(regions));
    let sim = crate::simulate::Simulation::new(
        ply_eval::sched::Scheduler::new(id, site).with_step_budget(c.sim_steps),
        site,
        c.seed.root,
        c.trail.drawn(),
        stack,
        c.stack_floor,
        body,
    );
    c.sims.push(sim);
    let r = unsafe { crate::simulate::run(ctx) };
    let c = unsafe { &mut *ctx };
    c.current = stack;
    for f in c.stacks[stack].list.split_off(depth) {
        drop_frame(f);
    }
    crate::simulate::end(c);
    c.record = Some(c.trail.record());
    r
}

/// The `handle` at `depth` is over: pops its frames, catches an unwind to it, applies `return`.
pub unsafe extern "C" fn rt_handle_land(ctx: *mut Ctx, depth: i64, value: i64) -> i64 {
    let c = unsafe { &mut *ctx };
    let depth = depth as usize;
    let stack = c.current;
    let mut popped = c.frames().split_off(depth);
    let mine = if popped.is_empty() {
        None
    } else {
        Some(popped.remove(0))
    };
    for f in popped {
        drop_frame(f);
    }
    if c.failed == FAILED_ABORT
        && let Some(a) = c.aborting.take_if(|a| a.stack == stack && a.depth == depth)
    {
        c.failed = 0;
        let mut f = mine.expect("a raise is bound for a frame still installed");
        let owner = c.owner();
        c.cells.close_regions_above(owner, f.regions);
        let closure = f.take_abort_clause();
        drop_frame(f);
        // The clause runs outside its `handle`, which is over: its value is the `handle`'s.
        let message = c.word(&Value::str(a.message));
        let r = call_value(ctx, closure, &[message]);
        heap::dec(closure);
        return r;
    }
    if c.failed == FAILED_UNWIND
        && let Some((target_stack, target, v)) = c.unwind.take()
    {
        if target_stack == stack && target == depth {
            c.failed = 0;
            if let Some(f) = mine {
                // The body is abandoned where it stood, above the closes the emitter put after
                // the regions it opened on this stack; they go back here so the entry stays
                // balanced.
                let owner = c.owner();
                c.cells.close_regions_above(owner, f.regions);
                drop_frame(f);
            }
            return v;
        }
        c.unwind = Some((target_stack, target, v));
    }
    if c.failed != 0 {
        if let Some(f) = mine {
            drop_frame(f);
        }
        return 0;
    }
    let Some(f) = mine else {
        return value;
    };
    let r = if f.ret != 0 {
        call_value(ctx, f.ret, &[value])
    } else {
        value
    };
    drop_frame(f);
    r
}

/// A builtin used as a value, which a call through it answers as a named call would.
pub unsafe extern "C" fn rt_builtin_value(ctx: *mut Ctx, index: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let b = ctx.tables.builtins[index as usize];
    ctx.heap.bridge(Value::builtin(b))
}

/// The singleton a nullary constructor *is*, and `0` for any other constructor.
pub unsafe extern "C" fn rt_nullary(ctx: *mut Ctx, index: i64) -> i64 {
    unsafe { &*ctx }.nullary(index as u32)
}

/// A constructor named as a value: the singleton a nullary one is, and a function otherwise.
pub unsafe extern "C" fn rt_ctor_value(ctx: *mut Ctx, index: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let (name, arity) = &ctx.tables.layouts.ctors[index as usize];
    if *arity == 0 {
        return ctx.nullary(index as u32);
    }
    let (name, arity) = (name.clone(), *arity);
    ctx.heap.bridge(Value::Closure(Arc::new(Closure {
        name: Some(name.clone()),
        kind: ClosureKind::Ctor { name, arity },
    })))
}

/// One more holder of a shared object, for compiled code, which has no atomics of its own.
pub unsafe extern "C" fn rt_inc_shared(_ctx: *mut Ctx, w: i64) {
    heap::inc_shared(w);
}

/// One holder fewer of a shared object.
pub unsafe extern "C" fn rt_dec_shared(_ctx: *mut Ctx, w: i64) {
    heap::dec_shared(w);
}

/// `parallel { .. }`: the `n` nullary closures at `slots`, each answer written over its closure.
pub unsafe extern "C" fn rt_parallel(ctx: *mut Ctx, slots: i64, n: i64) {
    unsafe { crate::parallel::run(ctx, slots as usize as *mut Word, n as usize) }
}

/// The value of the pure nullary function at `index`, memoized when world-independent.
pub unsafe extern "C" fn rt_constant(ctx: *mut Ctx, index: i64) -> i64 {
    let tables = Arc::clone(&unsafe { &*ctx }.tables);
    if let Some(w) = tables.memoized(index as usize) {
        return w;
    }
    // SAFETY: as in `call_value`; a nullary function never reads the null argument pointer.
    let f: Entry = unsafe { std::mem::transmute::<usize, Entry>(tables.functions[index as usize]) };
    let w = unsafe { f(ctx, std::ptr::null()) };
    let c = unsafe { &mut *ctx };
    if c.failed != 0 {
        return 0;
    }
    // The memo keeps a copy; the entry keeps using its own word.
    if heap::world_independent(w) {
        tables.memoize(index as usize, w);
    }
    w
}

/// A call through a value. Takes the callee and the arguments.
pub unsafe extern "C" fn rt_call(ctx: *mut Ctx, callee: i64, args: *const i64, n: i64) -> i64 {
    let args = args_of(args, n);
    let r = call_value(ctx, callee, args);
    heap::dec(callee);
    r
}

/// Applies `callee` to `args`, or answers 0 with the context failed. Reads the callee, takes the
/// arguments.
pub(crate) fn call_value(ctx: *mut Ctx, callee: Word, args: &[Word]) -> i64 {
    let c = unsafe { &mut *ctx };
    match heap::kind(callee) {
        KIND_CLOSURE => {
            let o = obj(callee);
            let (arity, len) = unsafe { ((*o).layout as usize, (*o).len as usize) };
            if args.len() != arity {
                let d = error(format!(
                    "a compiled function takes {arity} arguments and was given {}",
                    args.len()
                ));
                return c.fail(d);
            }
            // Captures are held once more; the arguments are already taken.
            // Uninitialised on purpose, as zeroing costs every call;
            // exactly `total` words are written and read.
            let mut handles = [const { std::mem::MaybeUninit::<i64>::uninit() }; 64];
            let mut spilled: Vec<i64> = Vec::new();
            let captures = len - CLOSURE_CAPTURES;
            let total = captures + args.len();
            let inline = total <= handles.len();
            let mut push = |w: Word, i: usize| {
                if inline {
                    handles[i].write(w);
                } else {
                    spilled.push(w);
                }
            };
            for i in 0..captures {
                let w = unsafe { word_at(o, CLOSURE_CAPTURES + i) };
                heap::inc(w);
                push(w, i);
            }
            for (i, w) in args.iter().enumerate() {
                push(*w, captures + i);
            }
            let ptr = if inline {
                handles.as_ptr() as *const i64
            } else {
                spilled.as_ptr()
            };
            let code = unsafe { word_at(o, CLOSURE_CODE) } as usize;
            // SAFETY: `code` is a finalized address from `Tables::functions`, which `Bodies`
            // keeps alive as long as this context, with the signature every compiled function has.
            let f: Entry = unsafe { std::mem::transmute::<usize, Entry>(code) };
            unsafe { f(ctx, ptr) }
        }
        KIND_BRIDGE => {
            let value = unsafe { bridged(obj(callee)) };
            let Value::Closure(closure) = value else {
                let d = error(format!(
                    "a call needs a function, and this is {}",
                    value.type_name()
                ));
                return c.fail(d);
            };
            match &closure.kind {
                ClosureKind::Builtin(b) => builtin(c, *b, args),
                ClosureKind::Ctor { name, arity } => {
                    if args.len() != *arity {
                        let d = error(format!(
                            "the constructor `{name}` takes {arity} fields and was given {}",
                            args.len()
                        ));
                        return c.fail(d);
                    }
                    match c.tables.layouts.ctor_index(name) {
                        Some(index) => {
                            let o = c.heap.alloc(KIND_CTOR, 0, args.len() as u32, index);
                            for (i, w) in args.iter().enumerate() {
                                unsafe { set_word(o, i, *w) };
                            }
                            o as Word
                        }
                        None => {
                            let values = values_taken(c, args);
                            c.word(&Value::ctor(name.clone(), values))
                        }
                    }
                }
                ClosureKind::Synth { arity, rule } => {
                    if args.len() != *arity {
                        let d = error(format!(
                            "a generated function takes {arity} arguments and was given {}",
                            args.len()
                        ));
                        return c.fail(d);
                    }
                    let values = values_taken(c, args);
                    match rule.apply(&values) {
                        Ok(v) => c.word(&v),
                        Err(d) => c.fail(d),
                    }
                }
                // `Heap::to_word` rebuilds a native closure as one, so it never crosses as a bridge.
                ClosureKind::Native { .. } | ClosureKind::Continuation { .. } => {
                    let d = error("a compiled closure arrived bridged rather than native");
                    c.fail(d)
                }
            }
        }
        _ => {
            let d = error(format!(
                "a call needs a function, and this is {}",
                c.type_name(callee)
            ));
            c.fail(d)
        }
    }
}

/// A native list, or a failure naming the builtin that needed one.
fn native_list(ctx: &mut Ctx, w: Word, what: &str) -> Option<*mut heap::Obj> {
    if heap::kind(w) == KIND_LIST {
        return Some(obj(w));
    }
    let d = error(format!(
        "`{what}` needs a List, and this is {}",
        ctx.type_name(w)
    ));
    ctx.fail(d);
    None
}

/// `map(xs, f)`: `f` on every element, in order. Takes the list and the function.
pub unsafe extern "C" fn rt_map(ctx: *mut Ctx, list: i64, f: i64) -> i64 {
    let c = unsafe { &mut *ctx };
    let Some(items) = native_list(c, list, "map") else {
        return 0;
    };
    let items = list::to_vec(items);
    let mut out = Vec::with_capacity(items.len());
    for x in items {
        heap::inc(x);
        let r = call_value(ctx, f, &[x]);
        let c = unsafe { &mut *ctx };
        if c.failed != 0 {
            return 0;
        }
        out.push(r);
    }
    let c = unsafe { &mut *ctx };
    let out = c.heap.list_from(&out);
    heap::dec(list);
    heap::dec(f);
    out
}

/// `filter(xs, p)`: the elements `p` answers `true` for. Takes the list and the predicate.
pub unsafe extern "C" fn rt_filter(ctx: *mut Ctx, list: i64, p: i64) -> i64 {
    let c = unsafe { &mut *ctx };
    let Some(items) = native_list(c, list, "filter") else {
        return 0;
    };
    let items = list::to_vec(items);
    let mut kept = Vec::new();
    for x in items {
        heap::inc(x);
        let r = call_value(ctx, p, &[x]);
        let c = unsafe { &mut *ctx };
        if c.failed != 0 {
            return 0;
        }
        match heap::as_bool(r) {
            Some(true) => {
                heap::inc(x);
                kept.push(x);
            }
            Some(false) => {}
            None => {
                let d = error(format!(
                    "the predicate given to `filter` answered {}, not a Bool",
                    c.type_name(r)
                ));
                return c.fail(d);
            }
        }
    }
    let c = unsafe { &mut *ctx };
    let out = c.heap.list_from(&kept);
    heap::dec(list);
    heap::dec(p);
    out
}

/// `fold(xs, init, f)`. Takes all three.
pub unsafe extern "C" fn rt_fold(ctx: *mut Ctx, list: i64, init: i64, f: i64) -> i64 {
    let c = unsafe { &mut *ctx };
    let Some(items) = native_list(c, list, "fold") else {
        return 0;
    };
    // The list is held until the walk ends, so `f` may run whatever it likes over it.
    let mut acc = init;
    let mut failed = false;
    list::for_each(items, &mut |x| {
        if failed {
            return;
        }
        heap::inc(x);
        acc = call_value(ctx, f, &[acc, x]);
        failed = unsafe { (*ctx).failed } != 0;
    });
    if failed {
        return 0;
    }
    heap::dec(list);
    heap::dec(f);
    acc
}

/// `map_fold(m, init, f)` in ascending key order over a snapshot of the entries. Takes all three.
pub unsafe extern "C" fn rt_map_fold(ctx: *mut Ctx, map: i64, init: i64, f: i64) -> i64 {
    let c = unsafe { &mut *ctx };
    if heap::kind(map) == KIND_MAP {
        let o = obj(map);
        let mut acc = init;
        for (k, v) in map::to_vec(o) {
            heap::inc(k);
            heap::inc(v);
            acc = call_value(ctx, f, &[acc, k, v]);
            if unsafe { &*ctx }.failed != 0 {
                return 0;
            }
        }
        heap::dec(map);
        heap::dec(f);
        return acc;
    }
    let value = (map != 0).then(|| c.value(map));
    let Some(Value::Map(entries)) = &value else {
        let d = error(format!(
            "`map_fold` needs a Map, and this is {}",
            c.type_name(map)
        ));
        return c.fail(d);
    };
    let entries: Vec<(Value, Value)> = entries
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let mut acc = init;
    for (k, v) in &entries {
        let c = unsafe { &mut *ctx };
        let kw = c.word(k);
        let vw = c.word(v);
        acc = call_value(ctx, f, &[acc, kw, vw]);
        let c = unsafe { &mut *ctx };
        if c.failed != 0 {
            return 0;
        }
    }
    heap::dec(map);
    heap::dec(f);
    acc
}

/// `iterate(seed, budget, f)`: `f` until it answers `Stop` or the budget runs out. Takes all three.
pub unsafe extern "C" fn rt_iterate(ctx: *mut Ctx, seed: i64, budget: i64, f: i64) -> i64 {
    let c = unsafe { &mut *ctx };
    let budget = match heap::as_int(budget) {
        Some(n) if n >= 1 => n,
        Some(n) => {
            let d = error(format!(
                "`iterate` needs a budget of at least 1, and this is {n}"
            ));
            return raise_error(c, d);
        }
        None => {
            let d = error(format!(
                "`iterate` needs an Int budget, and this is {}",
                c.type_name(budget)
            ));
            return c.fail(d);
        }
    };
    let stop = c.tables.layouts.stop;
    let go = c.tables.layouts.go;
    let mut state = seed;
    let mut left = budget;
    loop {
        if left <= 0 {
            let c = unsafe { &mut *ctx };
            let d = error(format!(
                "`iterate` did not stop within its budget of {budget}"
            ));
            return raise_error(c, d);
        }
        left -= 1;
        let r = call_value(ctx, f, &[state]);
        let c = unsafe { &mut *ctx };
        if c.failed != 0 {
            return 0;
        }
        // The answer gives up its payload so the threaded state stays uniquely held.
        let tag = if heap::kind(r) == KIND_CTOR {
            Some(unsafe { ((*obj(r)).layout, (*obj(r)).len) })
        } else {
            None
        };
        match tag {
            Some((index, 1)) if Some(index) == go || Some(index) == stop => {
                let payload = unsafe { word_at(obj(r), 0) };
                unsafe { set_word(obj(r), 0, heap::unit()) };
                heap::dec(r);
                if Some(index) == stop {
                    heap::dec(f);
                    return payload;
                }
                state = payload;
            }
            _ => {
                let d = error(format!(
                    "the step given to `iterate` answered {}, not `Continue` or `Stop`",
                    c.type_name(r)
                ));
                return c.fail(d);
            }
        }
    }
}

/// A fused `iterate`'s failure: `what` 0 a budget under one, 1 the budget spent, 2 a bad step
/// answer `n`, which it takes.
pub unsafe extern "C" fn rt_iterate_bad(ctx: *mut Ctx, what: i64, n: i64) {
    let ctx = unsafe { &mut *ctx };
    match what {
        0 => raise_error(
            ctx,
            error(format!(
                "`iterate` needs a budget of at least 1, and this is {n}"
            )),
        ),
        1 => raise_error(
            ctx,
            error(format!("`iterate` did not stop within its budget of {n}")),
        ),
        _ => {
            let d = error(format!(
                "the step given to `iterate` answered {}, not `Continue` or `Stop`",
                ctx.type_name(n)
            ));
            heap::dec(n);
            ctx.fail(d)
        }
    };
}

/// A shift count outside the word; `which` indexes [`ply_eval::INT_TYPES`], or is `-1` for `Int`.
pub unsafe extern "C" fn rt_shift_count(ctx: *mut Ctx, n: i64, which: i64) {
    let ctx = unsafe { &mut *ctx };
    let (ty, width) = match usize::try_from(which)
        .ok()
        .and_then(|i| ply_eval::INT_TYPES.get(i))
    {
        Some(t) => (t.name(), i64::from(t.bits())),
        None => ("Int", 64),
    };
    // The machine says this on a label at the shift; compiled code has no span for one.
    let d = error("shift count out of range")
        .note(format!("{n} is not in 0..={}", width - 1))
        .note(format!(
            "a `{ty}` is {width} bits, so no other count names a shift of it"
        ));
    ctx.fail(d);
}

/// An applied constructor. Takes the arguments.
pub unsafe extern "C" fn rt_ctor(ctx: *mut Ctx, index: i64, args: *const i64, n: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    if n == 0 {
        return ctx.nullary(index as u32);
    }
    let args = args_of(args, n);
    let o = ctx
        .heap
        .alloc(KIND_CTOR, flat_over(args), n as u32, index as u32);
    for (i, w) in args.iter().enumerate() {
        unsafe { set_word(o, i, *w) };
    }
    o as Word
}

/// A record literal: the fields in the shape's own sorted order. Takes the fields.
pub unsafe extern "C" fn rt_record(ctx: *mut Ctx, shape: i64, args: *const i64, n: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let args = args_of(args, n);
    let o = ctx
        .heap
        .alloc(KIND_RECORD, flat_over(args), n as u32, shape as u32);
    for (i, w) in args.iter().enumerate() {
        unsafe { set_word(o, i, *w) };
    }
    o as Word
}

/// One field of a record by name. `own`: 0 reads the base and holds the field once more; 2 moves
/// the field out of a unique base that stays; 1 and 3 also take and release the base.
pub unsafe extern "C" fn rt_field(ctx: *mut Ctx, base: i64, index: i64, own: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    if heap::kind(base) != KIND_RECORD {
        let d = error(format!(
            "a field access needs a record, and this is {}",
            ctx.type_name(base)
        ));
        return ctx.fail(d);
    }
    let o = obj(base);
    let shape = unsafe { (*o).layout };
    let Some(at) = ctx
        .tables
        .layouts
        .offset_by_index(shape, index as usize, &ctx.tables.fields)
    else {
        let d = error(format!(
            "this record has no field `{}`",
            ctx.tables.fields[index as usize]
        ));
        return ctx.fail(d);
    };
    let w = unsafe { word_at(o, at) };
    match own {
        0 => heap::inc(w),
        2 => {
            if is_unique(base) {
                unsafe { set_word(o, at, heap::unit()) };
            } else {
                heap::inc(w);
            }
        }
        _ => {
            if is_unique(base) {
                unsafe { set_word(o, at, heap::unit()) };
            } else {
                heap::inc(w);
            }
            heap::dec(base);
        }
    }
    w
}

/// A list literal. Takes the items.
pub unsafe extern "C" fn rt_list(ctx: *mut Ctx, args: *const i64, n: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    ctx.heap.list_from(args_of(args, n))
}

/// Whether a value is a record a pattern admits: of exactly `len` fields when `exact`. Reads.
pub unsafe extern "C" fn rt_record_fits(_ctx: *mut Ctx, value: i64, len: i64, exact: i64) -> i64 {
    if heap::kind(value) != KIND_RECORD {
        return 0;
    }
    i64::from(exact == 0 || unsafe { (*obj(value)).len } as i64 == len)
}

/// Whether a record holds the field a pattern names; a missing one fails the match. Reads.
pub unsafe extern "C" fn rt_record_has(ctx: *mut Ctx, value: i64, index: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    if heap::kind(value) != KIND_RECORD {
        return 0;
    }
    let shape = unsafe { (*obj(value)).layout };
    i64::from(
        ctx.tables
            .layouts
            .offset_by_index(shape, index as usize, &ctx.tables.fields)
            .is_some(),
    )
}

/// Whether a value is a list of `len` elements when `exact`, else of at least `len`. Reads.
pub unsafe extern "C" fn rt_list_fits(_ctx: *mut Ctx, value: i64, len: i64, exact: i64) -> i64 {
    if heap::kind(value) != KIND_LIST {
        return 0;
    }
    let n = unsafe { (*obj(value)).len } as i64;
    i64::from(if exact != 0 { n == len } else { n >= len })
}

/// One element of a list, once [`rt_list_fits`] has admitted its length: held once more. Reads.
pub unsafe extern "C" fn rt_list_at(ctx: *mut Ctx, value: i64, i: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    if heap::kind(value) != KIND_LIST {
        let d = error("a list pattern bound a value that is not a list");
        return ctx.fail(d);
    }
    let o = obj(value);
    if i < 0 || i >= list::len(o) as i64 {
        let d = error("a list pattern read past the end of the list");
        return ctx.fail(d);
    }
    let w = list::get(o, i as usize);
    heap::inc(w);
    w
}

/// What a `..rest` binds: the list from `from` on, sharing the trie. Reads.
pub unsafe extern "C" fn rt_list_rest(ctx: *mut Ctx, value: i64, from: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    if heap::kind(value) != KIND_LIST {
        let d = error("a list pattern bound a value that is not a list");
        return ctx.fail(d);
    }
    ctx.heap.list_skip(value, from.max(0) as usize)
}

/// Argument `i` of a matched constructor: moved out when `take` and it is unique, else held once
/// more. Reads the constructor.
pub unsafe extern "C" fn rt_ctor_arg(ctx: *mut Ctx, value: i64, i: i64, take: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    if heap::kind(value) != KIND_CTOR {
        let d = error("a constructor pattern bound a value that is not a constructor");
        return ctx.fail(d);
    }
    let o = obj(value);
    if i < 0 || i >= unsafe { (*o).len } as i64 {
        let d = error("a constructor pattern read an argument that is not there");
        return ctx.fail(d);
    }
    let w = unsafe { word_at(o, i as usize) };
    if take != 0 && is_unique(value) {
        unsafe { set_word(o, i as usize, heap::unit()) };
    } else {
        heap::inc(w);
    }
    w
}

/// `map_get` for a `match` that unwraps it at once: the value held once more, or `0` when absent,
/// with no constructor built. Takes the map and the key.
pub unsafe extern "C" fn rt_map_lookup(ctx: *mut Ctx, m: i64, k: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    if heap::kind(m) == KIND_MAP && heap::native_key(k) {
        let found = map::get(&ctx.tables.layouts, obj(m), k);
        if let Some(v) = found {
            heap::inc(v);
        }
        heap::dec(m);
        heap::dec(k);
        return found.unwrap_or(0);
    }
    let answer = builtin_over_values(ctx, Builtin::MapGet, &[m, k]);
    unwrapped(ctx, answer)
}

/// [`heap::FLAT`] when none of `words` holds a count.
fn flat_over(words: &[Word]) -> u8 {
    if words.iter().all(|w| heap::is_imm(*w)) {
        heap::FLAT
    } else {
        0
    }
}

/// A fresh object of `kind` with `len` words, whose fields the compiled caller stores itself.
pub unsafe extern "C" fn rt_alloc(
    ctx: *mut Ctx,
    kind: i64,
    len: i64,
    layout: i64,
    flags: i64,
) -> i64 {
    let ctx = unsafe { &mut *ctx };
    ctx.heap
        .alloc(kind as u8, flags as u8, len as u32, layout as u32) as Word
}

/// The value inside an `Option` answer, held once more, or `0` for `None`; the answer is let go.
fn unwrapped(ctx: &mut Ctx, answer: Word) -> Word {
    if ctx.failed != 0 {
        return 0;
    }
    let o = obj(answer);
    let held = unsafe { (*o).len == 1 && ctx.tables.layouts.some == Some((*o).layout) };
    let v = if held {
        let v = unsafe { word_at(o, 0) };
        heap::inc(v);
        v
    } else {
        0
    };
    heap::dec(answer);
    v
}

pub unsafe extern "C" fn rt_list_index(ctx: *mut Ctx, xs: i64, i: i64) -> i64 {
    builtin(unsafe { &mut *ctx }, Builtin::ListAt, &[xs, i])
}

pub unsafe extern "C" fn rt_list_set(ctx: *mut Ctx, xs: i64, i: i64, v: i64) -> i64 {
    builtin(unsafe { &mut *ctx }, Builtin::ListSet, &[xs, i, v])
}

/// `list_at` for a `match` that unwraps its answer at once, like [`rt_map_lookup`].
pub unsafe extern "C" fn rt_list_lookup(ctx: *mut Ctx, xs: i64, i: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    if heap::kind(xs) == KIND_LIST
        && let Some(index) = heap::as_int(i)
    {
        let o = obj(xs);
        let w = if index >= 0 && (index as usize) < list::len(o) {
            let item = list::get(o, index as usize);
            heap::inc(item);
            item
        } else {
            0
        };
        heap::dec(xs);
        return w;
    }
    let answer = builtin_over_values(ctx, Builtin::ListAt, &[xs, i]);
    unwrapped(ctx, answer)
}

pub unsafe extern "C" fn rt_push(ctx: *mut Ctx, xs: i64, x: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    if heap::kind(xs) == KIND_LIST {
        return ctx.heap.list_push(xs, x);
    }
    builtin(ctx, Builtin::Push, &[xs, x])
}

pub unsafe extern "C" fn rt_map_insert(ctx: *mut Ctx, m: i64, k: i64, v: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    if heap::kind(m) == KIND_MAP && heap::native_key(k) {
        let tables = Arc::clone(&ctx.tables);
        return ctx.heap.map_insert(&tables.layouts, m, k, v);
    }
    builtin(ctx, Builtin::MapInsert, &[m, k, v])
}

pub unsafe extern "C" fn rt_map_contains(ctx: *mut Ctx, m: i64, k: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    if heap::kind(m) == KIND_MAP && heap::native_key(k) {
        let found = map::get(&ctx.tables.layouts, obj(m), k).is_some();
        heap::dec(m);
        heap::dec(k);
        return heap::bool(found);
    }
    builtin(ctx, Builtin::MapContains, &[m, k])
}

pub unsafe extern "C" fn rt_map_get(ctx: *mut Ctx, m: i64, k: i64) -> i64 {
    builtin(unsafe { &mut *ctx }, Builtin::MapGet, &[m, k])
}

pub unsafe extern "C" fn rt_compare(ctx: *mut Ctx, a: i64, b: i64) -> i64 {
    builtin(unsafe { &mut *ctx }, Builtin::Compare, &[a, b])
}

pub unsafe extern "C" fn rt_byte_of_int(ctx: *mut Ctx, n: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    match heap::as_int(n).and_then(|v| u8::try_from(v).ok()) {
        Some(b) => ctx.tables.byte(b),
        None => builtin(ctx, Builtin::ByteOfInt, &[n]),
    }
}

pub unsafe extern "C" fn rt_bytes_scan(
    ctx: *mut Ctx,
    hay: i64,
    from: i64,
    members: i64,
    max: i64,
) -> i64 {
    builtin(
        unsafe { &mut *ctx },
        Builtin::BytesScan,
        &[hay, from, members, max],
    )
}

pub unsafe extern "C" fn rt_bytes_scan_until(
    ctx: *mut Ctx,
    hay: i64,
    from: i64,
    members: i64,
    max: i64,
) -> i64 {
    builtin(
        unsafe { &mut *ctx },
        Builtin::BytesScanUntil,
        &[hay, from, members, max],
    )
}

pub unsafe extern "C" fn rt_bytes_slice(ctx: *mut Ctx, b: i64, s: i64, e: i64) -> i64 {
    builtin(unsafe { &mut *ctx }, Builtin::BytesSlice, &[b, s, e])
}

pub unsafe extern "C" fn rt_bytes_concat(ctx: *mut Ctx, a: i64, b: i64) -> i64 {
    builtin(unsafe { &mut *ctx }, Builtin::BytesConcat, &[a, b])
}
