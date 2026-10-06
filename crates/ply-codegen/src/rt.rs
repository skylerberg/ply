//! The helpers compiled code calls into and the context an entry runs in. A helper that takes an
//! argument owns it, one that reads leaves its count alone, and every answer is the caller's.

use crate::heap::{
    self, CLOSURE_CAPTURES, CLOSURE_CODE, Heap, KIND_ARRAY, KIND_BRIDGE, KIND_BYTES, KIND_CLOSURE,
    KIND_CTOR, KIND_LIST, KIND_MAP, KIND_RECORD, KIND_STR, Layouts, Stated, Word, bridged,
    bytes_of, is_unique, obj, set_word, str_of, word_at,
};
use crate::map;
use crate::stack::{Stack, switch};
use crate::{array, list};
use ply_eval::arena::{Owner, RegionId, Slot};
use ply_eval::builtins::{cell_in_update, hold_released, no_such_cell};
use ply_eval::region::StepSite;
use ply_eval::sched::Shield;
use ply_eval::sim::Access;
use ply_eval::{
    BinOp, Builtin, Closure, ClosureKind, Diagnostic, EffectAtom, Mode, Plain, Resource, Span,
    Symbol, Value, codes, values_equal,
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
    /// What computing each memoized answer cost, in calls and allocations, stored before the
    /// answer is: a later read charges it, so a cost is the same whether or not it was memoized.
    pub memo_costs: Box<[(AtomicI64, AtomicI64)]>,
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
    List(usize),
    Array(usize),
}

fn identity(v: &Value) -> Option<Identity> {
    Some(match v {
        Value::Record(fields) => Identity::Record(Arc::as_ptr(fields) as usize),
        Value::Str(s) => Identity::Str(Arc::as_ptr(s) as *const u8 as usize),
        Value::Bytes(b) => Identity::Bytes(Arc::as_ptr(b) as *const u8 as usize),
        Value::List(items) => Identity::List(items.identity()),
        Value::Array(items) => Identity::Array(Arc::as_ptr(items) as usize),
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

    /// As [`Tables::memoize`], with what computing the answer cost, which every later read charges.
    pub fn memoize_costing(&self, index: usize, w: Word, cost: Cost) -> Word {
        if let Some((steps, allocations)) = self.memo_costs.get(index) {
            steps.store(cost.steps, Release);
            allocations.store(cost.allocations, Release);
        }
        self.memoize(index, w)
    }

    /// What computing the memoized answer at `index` cost.
    pub fn memo_cost(&self, index: usize) -> Cost {
        match self.memo_costs.get(index) {
            Some((steps, allocations)) => Cost {
                steps: steps.load(Acquire),
                allocations: allocations.load(Acquire),
            },
            None => Cost::default(),
        }
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
            Value::Array(items) if items.len() <= PARTS_LIMIT => items
                .iter()
                .enumerate()
                .map(|(i, part)| (unsafe { word_at(o, i) }, part))
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
        Value::Chan(_) => Some("a Chan"),
        Value::Closure(_) => Some("a Closure"),
        Value::List(items) => items.iter().find_map(holds_a_handle),
        Value::Array(items) => items.iter().find_map(holds_a_handle),
        Value::Map(entries) => entries
            .iter()
            .find_map(|(k, v)| holds_a_handle(k).or_else(|| holds_a_handle(v))),
        Value::Record(fields) => fields.values().find_map(holds_a_handle),
        Value::Ctor { args, .. } => args.iter().find_map(holds_a_handle),
        Value::Int(_)
        | Value::Fixed(_)
        | Value::Char(_)
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
/// The task was cancelled: it unwinds to its entry, which reports it cancelled rather than failed.
pub const FAILED_CANCELLED: i64 = 7;
/// A raise: `Ctx::aborting` names the `handle` whose clause answers it.
pub const FAILED_ABORT: i64 = 8;

/// The mode a compiled `perform` of a `raise` operation passes, beside 0 for a read and 1 for a
/// write.
const MODE_RAISE: i64 = 2;

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
    /// The calls its stack had left when this frame went on, which its `handle` lands with: a
    /// frame a failure returns through gives its call back nowhere else.
    fuel: i64,
}

impl HandlerFrame {
    /// The closure of the clause a raise is bound for, taken out of the frame, which keeps the
    /// rest.
    pub(crate) fn take_clause(&mut self, at: usize) -> Word {
        self.clauses.swap_remove(at).closure
    }

    fn simulate(regions: usize, fuel: i64) -> HandlerFrame {
        HandlerFrame {
            clauses: Vec::new(),
            ret: 0,
            simulate: true,
            detached: None,
            regions,
            fuel,
        }
    }

    /// The bottom of a detached body's own stack, which holds no region yet and starts with the
    /// calls its opener had left.
    pub(crate) fn detached(clauses: Vec<FrameClause>, id: usize, fuel: i64) -> HandlerFrame {
        HandlerFrame {
            clauses,
            ret: 0,
            simulate: false,
            detached: Some(id),
            regions: 0,
            fuel,
        }
    }

    /// Puts its stack's count of calls left back where it stood when the frame went on: every
    /// frame above the `handle` is gone by the time it lands.
    pub(crate) fn land(&self, c: &mut Ctx) {
        debug_assert!(
            c.failed != 0 || c.fuel == self.fuel,
            "a `handle` body that returned left {} calls where it found {}",
            c.fuel,
            self.fuel
        );
        c.fuel = self.fuel;
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

/// A raise on its way to the `handle` that answers it: that frame's stack and depth, which of its
/// clauses, what the clause is given, and what the entry fails with should the frame be gone.
pub(crate) struct Aborting {
    pub(crate) stack: usize,
    pub(crate) depth: usize,
    pub(crate) clause: usize,
    /// Held for the clause, which takes them.
    pub(crate) args: Vec<Word>,
    pub(crate) diagnostic: Diagnostic,
}

impl Aborting {
    /// The failure a raise no clause will now answer carries, with what it held let go.
    pub(crate) fn into_failure(self) -> Diagnostic {
        for w in self.args {
            heap::dec(w);
        }
        self.diagnostic
    }
}

/// One clause, under program-wide effect and resource names.
pub(crate) struct FrameClause {
    effect: Symbol,
    resource: Option<Symbol>,
    op: Symbol,
    closure: Word,
    /// 0: never resumes; 1: resumes in tail position; 2: elsewhere (only in a detached frame);
    /// 3: answers a raise, once its `handle`'s body is abandoned.
    resumes: u8,
    /// `[*t]`: the clause's first slot after the parameters is the label the call site named.
    binds_label: bool,
}

/// `abort.raise`, which the runtime itself raises for what fails on a value.
fn is_abort(effect: &str, op: &str) -> bool {
    effect == "abort" && op == "raise"
}

impl FrameClause {
    /// `abort.raise` is a raise whatever its clause is marked: the runtime raises it itself.
    fn raises(&self) -> bool {
        self.resumes == 3 || is_abort(self.effect.as_str(), self.op.as_str())
    }

    fn answers_raise(&self, effect: &str, op: &str) -> bool {
        self.raises() && self.effect.as_str() == effect && self.op.as_str() == op
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
                fuel: f.fuel,
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
                f.clauses.into_iter().partition(FrameClause::raises);
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
pub struct HeldWord(pub Word);

impl HeldWord {
    fn into_word(self) -> Word {
        let w = self.0;
        std::mem::forget(self);
        w
    }
}

impl Clone for HeldWord {
    fn clone(&self) -> HeldWord {
        heap::inc(self.0);
        HeldWord(self.0)
    }
}

impl Drop for HeldWord {
    fn drop(&mut self) {
        heap::dec(self.0);
    }
}

impl Default for HeldWord {
    fn default() -> HeldWord {
        HeldWord(heap::unit())
    }
}

impl std::fmt::Debug for HeldWord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HeldWord({:#x})", self.0)
    }
}

/// Entries begun in this process, every context's: contexts share a unit's memo, so an entry's
/// number must be unique across them, not just within one.
static ENTRIES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[repr(C)]
pub struct Ctx {
    pub failed: i64,
    /// Nested native calls the running stack is still allowed. A call spends one and its return
    /// gives it back; a frame a failure returns through gives nothing back, so whatever runs on
    /// past a failure first puts this where it stood ([`HandlerFrame::land`]). It is the running
    /// stack's own: a switch saves it with the stack it leaves and puts back the other's.
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
    /// Calls and allocations a memoized answer cost when it was computed, charged to the entry that
    /// read it, so `metered` counts the same whether a constant was memoized or not. Not budgeted.
    pub charged: Cost,
    /// Stacks this entry has been given beyond the one it started on, so growing is observable.
    pub grown: u64,
    /// When the running entry's time budget is spent, if it has one.
    deadline: Option<std::time::Instant>,
    time_budget_ms: u64,
    /// The cells, holding heap words: declared before the heap, so their counts go back first.
    /// Each stack in `stacks` owns the regions it opens, under the index that names it.
    pub(crate) cells: ply_eval::TaskRegions<HeldWord>,
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
    pub(crate) program: Option<&'static ply_eval::Analysis>,
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
    /// What that entry's unit answered of its types' keys, put back with them.
    outer_instances: Option<ply_eval::Instances>,
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
            charged: Cost::default(),
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
            outer_instances: None,
        }
    }

    /// This context as what reads its unit's values asks it, while it runs on this thread; `None`
    /// for a unit none of whose types states a `key` or a `show`.
    pub(crate) fn instances(&mut self) -> Option<ply_eval::Instances> {
        self.tables
            .layouts
            .states_any()
            .then(|| ply_eval::Instances {
                unit: std::ptr::from_mut(self).cast(),
                keyed: unit_keys,
                key: key_value,
                shown: shown_value,
                numeric: numeric_value,
                witness: stated_witness,
                of_int: of_int_value,
            })
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
        self.charged = self.charged.plus(branch.charged);
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
        self.charged = Cost::default();
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
        // No cell outlives its entry, so each entry can name its cells as a fresh backend would.
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
        let mine = self.instances();
        self.outer_instances = ply_eval::instances::swap(mine);
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
        ply_eval::instances::swap(self.outer_instances.take());
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
        let Some(at) = self.raise_handler("abort", "raise") else {
            return self.fail(d);
        };
        let message = self.word(&Value::str(message));
        self.bind_raise(at, vec![message], d)
    }

    /// Sends a raise on its way to the clause at `at`, which is handed `args`; `d` is what the
    /// entry fails with should that frame be gone.
    fn bind_raise(&mut self, at: (usize, usize, usize), args: Vec<Word>, d: Diagnostic) -> i64 {
        let (stack, depth, clause) = at;
        let diagnostic = self.placed(d);
        self.aborting = Some(Aborting {
            stack,
            depth,
            clause,
            args,
            diagnostic,
        });
        self.failed = FAILED_ABORT;
        0
    }

    /// The stack, depth and clause that answer a raise of `effect.op`. Searched as a `perform`
    /// searches, so frames hidden while a clause runs are passed over.
    fn raise_handler(&self, effect: &str, op: &str) -> Option<(usize, usize, usize)> {
        let mut stack = self.current;
        loop {
            let found = self.stacks[stack]
                .list
                .iter()
                .enumerate()
                .rev()
                .find_map(|(depth, f)| {
                    let clause = f
                        .clauses
                        .iter()
                        .position(|cl| cl.answers_raise(effect, op))?;
                    Some((stack, depth, clause))
                });
            if found.is_some() {
                return found;
            }
            stack = self.stacks[stack].parent?;
        }
    }

    /// Whether a clause for any raise is in reach.
    fn raise_in_reach(&self) -> bool {
        let mut stack = Some(self.current);
        while let Some(s) = stack {
            let frames = &self.stacks[s];
            if frames
                .list
                .iter()
                .any(|f| f.clauses.iter().any(FrameClause::raises))
            {
                return true;
            }
            stack = frames.parent;
        }
        false
    }

    /// How a finished branch failed, a raise bound past it as the failure it carries.
    pub(crate) fn take_branch_failure(&mut self) -> Option<(i64, Option<Diagnostic>)> {
        if self.failed == 0 {
            return None;
        }
        Some(match self.aborting.take() {
            Some(a) if self.failed == FAILED_ABORT => (1, Some(a.into_failure())),
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
        .alloc(owner, HeldWord(init))
        .expect("a `with_cell` allocates in the region its stack just opened");
    ctx.heap.bridge(Value::Cell(slot))
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
        v @ Value::Ctor { .. } => {
            match numeric_value(
                ctx.cast(),
                ply_eval::instances::NEG,
                std::slice::from_ref(v),
            ) {
                Some(n) => n,
                None => return unsafe { &mut *ctx }.fail(ply_eval::unstated(Span::DUMMY, v)),
            }
        }
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

/// [`switch`] between two computations, each nesting on a stack of its own: returns when something
/// switches back, with [`Ctx::fuel`] as this stack left it, whatever ran meanwhile.
///
/// # Safety
/// As [`switch`], and `ctx` is the context both stacks run under.
pub(crate) unsafe fn switch_keeping(ctx: *mut Ctx, from: *mut usize, to: usize) {
    let fuel = unsafe { (*ctx).fuel };
    unsafe { switch(&mut *from, to) };
    unsafe { (*ctx).fuel = fuel };
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
        Builtin::of_int(t).name(),
        t.min(),
        t.max()
    ));
    raise_error(ctx, d);
}

/// How far the base of the stack a helper runs on may sit from the one the entry's floor was
/// taken from and still be that stack: a guard's width, and never a segment's.
const SAME_STACK: usize = 256 * 1024;

/// A `(ctx, args)` entry the unit states of a type, called with one of its values. Takes `w`;
/// `None` when the call failed, which the context then holds. A value is compared wherever it is
/// read, which may be in frames that grew onto a segment the entry's floor knows nothing of: the
/// call then runs on a stack of its own.
fn call_stated(ctx: *mut Ctx, entry: usize, w: Word) -> Option<Word> {
    call_stated_over(ctx, entry, &[w])
}

/// [`call_stated`] for an entry of any number of words. Takes each.
fn call_stated_over(ctx: *mut Ctx, entry: usize, args: &[Word]) -> Option<Word> {
    let (floor, site) = {
        let c = unsafe { &*ctx };
        (c.stack_floor, (c.site_root, c.site_start, c.site_end))
    };
    let on_entry_stack = ply_eval::limit::stack_base()
        .is_some_and(|base| base.abs_diff(floor.saturating_sub(STACK_MARGIN)) <= SAME_STACK);
    let out = if on_entry_stack {
        // SAFETY: `entry` is a taken root's address, which the unit keeps as long as its tables.
        let f: Entry = unsafe { std::mem::transmute::<usize, Entry>(entry) };
        unsafe { f(ctx, args.as_ptr()) }
    } else {
        unsafe { rt_grow(ctx, entry as i64, args.as_ptr() as i64) }
    };
    let c = unsafe { &mut *ctx };
    if c.failed != 0 {
        return None;
    }
    // What reads the value fails where the body that asked stored its site, not inside this call.
    (c.site_root, c.site_start, c.site_end) = site;
    Some(out)
}

/// The context of the unit running on this thread, which a reader holding only its layouts asks.
fn running_ctx() -> Option<*mut Ctx> {
    ply_eval::instances::running().map(|i| i.unit.cast::<Ctx>())
}

/// A value read through a function its type states, where the unit running does not hold it.
fn absent(ctx: &mut Ctx, what: &str, ctor: &Symbol) -> i64 {
    let d = Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("a `{ctor}` was read through its type's `{what}`, which this unit does not hold"),
    )
    .primary(Span::DUMMY, "in compiled code")
    .note("this is Ply's fault: a unit that holds a type's values takes what the type states");
    ctx.fail(d)
}

/// The keys two constructors' values are compared by, when their type states one: each the
/// caller's. Reads both.
pub(crate) fn keys_of(layouts: &Layouts, a: Word, b: Word) -> Option<(Word, Word)> {
    if !layouts.states_any() {
        return None;
    }
    let ctor = |w: Word| unsafe { (*obj(w)).layout };
    let (ka, kb) = (layouts.key(ctor(a)), layouts.key(ctor(b)));
    if ka == Stated::No || kb == Stated::No {
        return None;
    }
    let ctx = running_ctx()?;
    let (Stated::By(f), Stated::By(g)) = (ka, kb) else {
        absent(
            unsafe { &mut *ctx },
            "key",
            &layouts.ctors[ctor(a) as usize].0,
        );
        return None;
    };
    heap::inc(a);
    let x = call_stated(ctx, f, a)?;
    heap::inc(b);
    match call_stated(ctx, g, b) {
        Some(y) => Some((x, y)),
        None => {
            heap::dec(x);
            None
        }
    }
}

fn unit_keys(unit: *mut (), ctor: &Symbol) -> bool {
    let layouts = &unsafe { &*unit.cast::<Ctx>() }.tables.layouts;
    layouts
        .ctor_index(ctor)
        .is_some_and(|i| layouts.key(i) != Stated::No)
}

fn stated_value(unit: *mut (), v: &Value, key: bool) -> Option<Value> {
    let ctx = unit.cast::<Ctx>();
    let c = unsafe { &mut *ctx };
    let Value::Ctor { name, .. } = v else {
        return None;
    };
    let index = c.tables.layouts.ctor_index(name)?;
    let (stated, what) = if key {
        (c.tables.layouts.key(index), "key")
    } else {
        (c.tables.layouts.show(index), "show")
    };
    match stated {
        Stated::No => None,
        Stated::Absent => {
            absent(c, what, name);
            None
        }
        Stated::By(entry) => {
            let tables = Arc::clone(&c.tables);
            let w = c.heap.to_word(&tables.layouts, v);
            let out = call_stated(ctx, entry, w)?;
            let answer = Heap::to_value(&tables.layouts, out);
            heap::dec(out);
            Some(answer)
        }
    }
}

fn key_value(unit: *mut (), v: &Value) -> Option<Value> {
    stated_value(unit, v, true)
}

/// One of a `numeric`'s functions, for this constructor, called over `args`.
fn numeric_call(ctx: *mut Ctx, ctor: u32, role: usize, args: &[Value]) -> Option<Value> {
    let c = unsafe { &mut *ctx };
    match c.tables.layouts.numeric(ctor, role) {
        Stated::No => None,
        Stated::Absent => {
            let name = c.tables.layouts.ctors[ctor as usize].0.clone();
            absent(c, "numeric", &name);
            None
        }
        Stated::By(entry) => {
            let tables = Arc::clone(&c.tables);
            let words: Vec<Word> = args
                .iter()
                .map(|v| c.heap.to_word(&tables.layouts, v))
                .collect();
            let out = call_stated_over(ctx, entry, &words)?;
            let answer = Heap::to_value(&tables.layouts, out);
            heap::dec(out);
            Some(answer)
        }
    }
}

fn numeric_value(unit: *mut (), role: usize, args: &[Value]) -> Option<Value> {
    let ctx = unit.cast::<Ctx>();
    let Some(Value::Ctor { name, .. }) = args.first() else {
        return None;
    };
    let ctor = unsafe { &*ctx }.tables.layouts.ctor_index(name)?;
    numeric_call(ctx, ctor, role, args)
}

fn stated_witness(unit: *mut (), ctor: &Symbol) -> Option<i64> {
    let layouts = &unsafe { &*unit.cast::<Ctx>() }.tables.layouts;
    let index = layouts.ctor_index(ctor)?;
    (layouts.numeric(index, ply_eval::instances::OF_INT) != Stated::No)
        .then(|| ply_eval::instances::STATED_WITNESS + i64::from(index))
}

fn of_int_value(unit: *mut (), witness: i64, n: i64) -> Option<Value> {
    let ctor = u32::try_from(witness - ply_eval::instances::STATED_WITNESS).ok()?;
    numeric_call(
        unit.cast::<Ctx>(),
        ctor,
        ply_eval::instances::OF_INT,
        &[Value::Int(n)],
    )
}

fn shown_value(unit: *mut (), v: &Value) -> Option<Value> {
    stated_value(unit, v, false)
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
        // Two values of a keyed type are equal as their keys are.
        if ka == KIND_CTOR && kb == KIND_CTOR && unsafe { &*ctx }.tables.layouts.states_any() {
            let tables = Arc::clone(&unsafe { &*ctx }.tables);
            if let Some((x, y)) = keys_of(&tables.layouts, a, b) {
                let equal = unsafe { rt_equal(ctx, x, y) };
                heap::dec(x);
                heap::dec(y);
                return equal;
            }
        }
    }
    let ctx = unsafe { &mut *ctx };
    let (l, r) = (ctx.value(a), ctx.value(b));
    match values_equal(&l, &r, Span::DUMMY) {
        Ok(eq) => i64::from(eq),
        Err(d) => ctx.fail(d),
    }
}

/// Whether `lo <= v <= hi` as `<=` orders the three, for a range pattern over a width the words do
/// not carry. Reads all three.
pub unsafe extern "C" fn rt_between(ctx: *mut Ctx, v: i64, lo: i64, hi: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let (x, l, h) = (ctx.value(v), ctx.value(lo), ctx.value(hi));
    let le = |a: &Value, b: &Value| {
        ply_eval::strict_binary(BinOp::Le, a, b, Span::DUMMY, Span::DUMMY, Span::DUMMY)
    };
    match le(&l, &x).and_then(|low| Ok((low, le(&x, &h)?))) {
        Ok(both) => i64::from(matches!(both, (Value::Bool(true), Value::Bool(true)))),
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

macro_rules! builtin_helper {
    ($variant:ident 0) => {
        pub unsafe extern "C" fn $variant(ctx: *mut Ctx) -> i64 {
            builtin(unsafe { &mut *ctx }, Builtin::$variant, &[])
        }
    };
    ($variant:ident 1) => {
        pub unsafe extern "C" fn $variant(ctx: *mut Ctx, a: i64) -> i64 {
            builtin(unsafe { &mut *ctx }, Builtin::$variant, &[a])
        }
    };
    ($variant:ident 2) => {
        pub unsafe extern "C" fn $variant(ctx: *mut Ctx, a: i64, b: i64) -> i64 {
            builtin(unsafe { &mut *ctx }, Builtin::$variant, &[a, b])
        }
    };
    ($variant:ident 3) => {
        pub unsafe extern "C" fn $variant(ctx: *mut Ctx, a: i64, b: i64, c: i64) -> i64 {
            builtin(unsafe { &mut *ctx }, Builtin::$variant, &[a, b, c])
        }
    };
    ($variant:ident 4) => {
        pub unsafe extern "C" fn $variant(ctx: *mut Ctx, a: i64, b: i64, c: i64, d: i64) -> i64 {
            builtin(unsafe { &mut *ctx }, Builtin::$variant, &[a, b, c, d])
        }
    };
    ($variant:ident 5) => {
        pub unsafe extern "C" fn $variant(
            ctx: *mut Ctx,
            a: i64,
            b: i64,
            c: i64,
            d: i64,
            e: i64,
        ) -> i64 {
            builtin(unsafe { &mut *ctx }, Builtin::$variant, &[a, b, c, d, e])
        }
    };
    ($variant:ident 6) => {
        pub unsafe extern "C" fn $variant(
            ctx: *mut Ctx,
            a: i64,
            b: i64,
            c: i64,
            d: i64,
            e: i64,
            f: i64,
        ) -> i64 {
            builtin(unsafe { &mut *ctx }, Builtin::$variant, &[a, b, c, d, e, f])
        }
    };
}

macro_rules! builtin_helpers {
    ($($variant:ident $name:literal $arity:tt;)*) => {
        /// Each builtin as compiled code calls it: its words in, each one the callee's, and its
        /// answer out.
        #[allow(non_snake_case)]
        mod called {
            use super::{Builtin, Ctx, builtin};
            $( builtin_helper!($variant $arity); )*
        }

        fn called(b: Builtin) -> *const () {
            match b {
                $( Builtin::$variant => called::$variant as *const (), )*
            }
        }
    };
}

ply_eval::each_builtin!(builtin_helpers);

/// What a unit binds a builtin's helper to: the builtin's own road past the dispatch where it has
/// one, else its call through [`builtin`].
pub fn builtin_address(b: Builtin) -> *const () {
    match b {
        Builtin::Push => rt_push as *const (),
        Builtin::MapInsert => rt_map_insert as *const (),
        Builtin::MapContains => rt_map_contains as *const (),
        Builtin::ByteOfInt => rt_byte_of_int as *const (),
        Builtin::Map => rt_map as *const (),
        Builtin::Filter => rt_filter as *const (),
        Builtin::Fold => rt_fold as *const (),
        Builtin::MapFold => rt_map_fold as *const (),
        Builtin::Iterate => rt_iterate as *const (),
        _ => called(b),
    }
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
    // An operator over an `integer` parameter raises for a zero divisor, as one written at its
    // type does; its overflow, like theirs, ends the run.
    let divides_by_zero = match (b, values.as_slice()) {
        (Builtin::NumericBinary, [Value::Int(op), _, _, divisor]) => {
            matches!(
                usize::try_from(*op)
                    .ok()
                    .and_then(|i| ply_eval::builtins::NUMERIC_OPS.get(i)),
                Some(BinOp::Div | BinOp::Rem)
            ) && is_zero(divisor)
        }
        _ => false,
    };
    match ply_eval::builtins::call(b, values, site) {
        Ok(v) => ctx.word(&v),
        Err(d) if b.raises() || divides_by_zero => {
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
pub(crate) fn raise_error(ctx: &mut Ctx, d: Diagnostic) -> i64 {
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

/// A witnessed operator over two immediates, when the witness is `Int` or a width compiled code
/// holds as its `Int`: the answer, or `None` for the value path to give, or to raise, instead.
fn narrow_binary(op: i64, w: i64, a: i64, b: i64) -> Option<Word> {
    let range = if w == ply_eval::builtins::INT_WITNESS {
        None
    } else {
        let t = *ply_eval::INT_TYPES.get(usize::try_from(w).ok()?)?;
        if t.bits() >= 64 {
            return None;
        }
        Some((
            i64::try_from(ply_eval::IntTy::min(t)).ok()?,
            i64::try_from(ply_eval::IntTy::max(t)).ok()?,
        ))
    };
    let op = ply_eval::builtins::NUMERIC_OPS.get(usize::try_from(op).ok()?)?;
    let n = match op {
        BinOp::Add => a.checked_add(b)?,
        BinOp::Sub => a.checked_sub(b)?,
        BinOp::Mul => a.checked_mul(b)?,
        BinOp::Div => a.checked_div(b)?,
        BinOp::Rem => a.checked_rem(b)?,
        BinOp::BitAnd => a & b,
        BinOp::BitOr => a | b,
        BinOp::BitXor => a ^ b,
        BinOp::Lt => return Some(heap::bool(a < b)),
        BinOp::Le => return Some(heap::bool(a <= b)),
        BinOp::Gt => return Some(heap::bool(a > b)),
        BinOp::Ge => return Some(heap::bool(a >= b)),
        _ => return None,
    };
    match range {
        Some((lo, hi)) if n < lo || n > hi => None,
        _ => heap::fits_imm(n).then(|| heap::imm(n)),
    }
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

/// What a hold holds, as a count of its own. A hold is a cell of its region, and one let go holds
/// the null word, which no value is.
fn hold_read(ctx: &Ctx, slot: Slot) -> Result<Word, Diagnostic> {
    match ctx.cells.arena().get(slot) {
        Some(held) if held.0 != 0 => {
            heap::inc(held.0);
            Ok(held.0)
        }
        Some(_) => Err(hold_released(ctx.site(), "read")),
        None => Err(no_such_cell(ctx.site(), slot)),
    }
}

/// A hold's contents moved out for its release, leaving the null word: a second release finds it
/// and is refused, so nothing is released twice.
fn hold_take(ctx: &mut Ctx, slot: Slot) -> Result<Word, Diagnostic> {
    let site = ctx.site();
    match ctx.cells.arena().get(slot) {
        Some(held) if held.0 != 0 => {}
        Some(_) => return Err(hold_released(site, "let go")),
        None => return Err(no_such_cell(site, slot)),
    }
    let Some(current) = ctx.cells.arena_mut().take(slot) else {
        return Err(no_such_cell(site, slot));
    };
    ctx.cells.arena_mut().put_back(slot, HeldWord(0));
    Ok(current.into_word())
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
        (Builtin::CellSet | Builtin::HoldPut, [c, v]) => {
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
            let stored = ctx.cells.arena_mut().set(slot, HeldWord(*v));
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
            let kept = ctx.raise_in_reach().then(|| current.clone());
            let updated = call_value(std::ptr::from_mut(ctx), *f, &[current.into_word()]);
            let held = match kept {
                Some(old) if ctx.failed == FAILED_ABORT => old,
                _ if ctx.failed != 0 => HeldWord::default(),
                _ => HeldWord(updated),
            };
            ctx.cells.arena_mut().put_back(slot, held);
            heap::dec(*c);
            Some(if ctx.failed != 0 { 0 } else { heap::unit() })
        }
        (Builtin::HoldGet, [h]) => {
            let slot = cell_of(*h)?;
            let answer = match hold_read(ctx, slot) {
                Ok(w) => w,
                Err(d) => ctx.fail(d),
            };
            heap::dec(*h);
            Some(answer)
        }
        (Builtin::HoldTake, [h]) => {
            let slot = cell_of(*h)?;
            let answer = match hold_take(ctx, slot) {
                Ok(w) => w,
                Err(d) => ctx.fail(d),
            };
            heap::dec(*h);
            Some(answer)
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
        (Builtin::Bracket | Builtin::HoldBracket, [acquire, release, body]) => Some(rt_bracket(
            std::ptr::from_mut(ctx),
            *acquire,
            *release,
            *body,
        )),
        (Builtin::Metered, [f]) => Some(metered(ctx, *f)),
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
        (Builtin::ArrayLen, [xs]) if heap::kind(*xs) == KIND_ARRAY => {
            let n = array::len(obj(*xs)) as i64;
            heap::dec(*xs);
            Some(heap::imm(n))
        }
        (Builtin::ArrayGet, [xs, i]) if heap::kind(*xs) == KIND_ARRAY => {
            let index = usize::try_from(heap::as_int(*i)?).ok()?;
            let item = *array::items(obj(*xs)).get(index)?;
            heap::inc(item);
            heap::dec(*xs);
            Some(item)
        }
        (Builtin::ArrayAt, [xs, i]) if heap::kind(*xs) == KIND_ARRAY => {
            let index = heap::as_int(*i)?;
            let some = ctx.tables.layouts.some?;
            let none = ctx.tables.layouts.none?;
            let held = usize::try_from(index)
                .ok()
                .and_then(|at| array::items(obj(*xs)).get(at).copied());
            let answer = match held {
                Some(item) => {
                    heap::inc(item);
                    let c = ctx.heap.alloc(KIND_CTOR, 0, 1, some);
                    unsafe { set_word(c, 0, item) };
                    c as Word
                }
                None => ctx.nullary(none),
            };
            heap::dec(*xs);
            Some(answer)
        }
        (Builtin::ArraySet, [xs, i, v]) if heap::kind(*xs) == KIND_ARRAY => {
            let index = usize::try_from(heap::as_int(*i)?).ok()?;
            if index >= array::len(obj(*xs)) {
                return None;
            }
            Some(ctx.heap.array_set(*xs, index, *v))
        }
        (Builtin::ArrayNew, [n, x]) => {
            let n = heap::as_int(*n)?;
            if !(0..=ply_eval::builtins::MAX_ARRAY_LEN).contains(&n) {
                return None;
            }
            Some(ctx.heap.array_new(n as usize, *x))
        }
        (Builtin::ArrayOfList, [xs]) if heap::kind(*xs) == KIND_LIST => {
            let items = list::to_vec(obj(*xs));
            for w in &items {
                heap::inc(*w);
            }
            heap::dec(*xs);
            Some(ctx.heap.array_from(&items))
        }
        (Builtin::ArrayToList, [xs]) if heap::kind(*xs) == KIND_ARRAY => {
            let items = array::items(obj(*xs)).to_vec();
            for w in &items {
                heap::inc(*w);
            }
            heap::dec(*xs);
            Some(ctx.heap.list_from(&items))
        }
        (Builtin::NumericBinary, [op, w, x, y])
            if heap::is_imm(*op) && heap::is_imm(*w) && heap::is_imm(*x) && heap::is_imm(*y) =>
        {
            narrow_binary(
                heap::imm_value(*op),
                heap::imm_value(*w),
                heap::imm_value(*x),
                heap::imm_value(*y),
            )
        }
        (Builtin::Min | Builtin::Max, [x, y]) if heap::is_imm(*x) && heap::is_imm(*y) => {
            let (a, b) = (heap::imm_value(*x), heap::imm_value(*y));
            Some(heap::imm(if (a <= b) == (which == Builtin::Min) {
                a
            } else {
                b
            }))
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
            let mut items = Vec::with_capacity(map::len(o));
            for (k, v) in map::to_vec(o) {
                items.push(entry(ctx, k, v));
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
            for (k, v) in &pairs {
                heap::inc(*k);
                heap::inc(*v);
            }
            // Entries in key order, each key once, are the tree's leaves as they stand.
            let ascending = pairs.windows(2).all(|w| {
                heap::cmp_words(&tables.layouts, w[0].0, w[1].0) == std::cmp::Ordering::Less
            });
            let m = if ascending {
                ctx.heap.map_from_sorted(&pairs)
            } else {
                let mut m = ctx.heap.map_new();
                for (k, v) in pairs {
                    m = ctx.heap.map_insert(&tables.layouts, m, k, v);
                }
                m
            };
            heap::dec(*xs);
            Some(m)
        }
        (Builtin::MapMerge, [a, bm])
            if heap::kind(*a) == KIND_MAP && heap::kind(*bm) == KIND_MAP =>
        {
            let tables = Arc::clone(&ctx.tables);
            // The smaller map's entries go into the larger, and `b`'s entry stands at a shared key.
            if map::len(obj(*a)) < map::len(obj(*bm)) {
                let mut m = *bm;
                for (k, v) in map::to_vec(obj(*a)) {
                    if map::get(&tables.layouts, obj(m), k).is_none() {
                        heap::inc(k);
                        heap::inc(v);
                        m = ctx.heap.map_insert(&tables.layouts, m, k, v);
                    }
                }
                heap::dec(*a);
                return Some(m);
            }
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
        (Builtin::MapFirst | Builtin::MapLast, [m]) if heap::kind(*m) == KIND_MAP => {
            let o = obj(*m);
            let index = match which {
                Builtin::MapFirst => Some(0),
                _ => map::len(o).checked_sub(1),
            };
            let answer = entry_option(ctx, index.and_then(|i| map::at(o, i)))?;
            heap::dec(*m);
            Some(answer)
        }
        (
            Builtin::MapFloor | Builtin::MapCeiling | Builtin::MapBelow | Builtin::MapAbove,
            [m, k],
        ) if heap::kind(*m) == KIND_MAP && heap::native_key(*k) => {
            let o = obj(*m);
            let (below, found) = map::locate(&ctx.tables.layouts, o, *k);
            let index = match which {
                Builtin::MapCeiling => Some(below),
                Builtin::MapAbove => Some(below + usize::from(found.is_some())),
                Builtin::MapFloor if found.is_some() => Some(below),
                _ => below.checked_sub(1),
            };
            let answer = entry_option(ctx, index.and_then(|i| map::at(o, i)))?;
            heap::dec(*m);
            heap::dec(*k);
            Some(answer)
        }
        (Builtin::MapPopFirst | Builtin::MapPopLast, [m]) if heap::kind(*m) == KIND_MAP => {
            let (some, none) = (ctx.tables.layouts.some?, ctx.tables.layouts.none?);
            let tables = Arc::clone(&ctx.tables);
            let greatest = which == Builtin::MapPopLast;
            let (rest, taken) = ctx.heap.map_pop(&tables.layouts, *m, greatest);
            let Some((k, v)) = taken else {
                heap::dec(rest);
                return Some(ctx.nullary(none));
            };
            let r = ctx
                .heap
                .alloc(KIND_RECORD, 0, 3, tables.layouts.popped_shape());
            unsafe {
                set_word(r, 0, k);
                set_word(r, 1, rest);
                set_word(r, 2, v);
            }
            Some(wrapped(ctx, some, r as Word))
        }
        (Builtin::MapRange, [m, lo, lo_inclusive, hi, hi_inclusive, limit])
            if heap::kind(*m) == KIND_MAP =>
        {
            let tables = Arc::clone(&ctx.tables);
            let layouts = &tables.layouts;
            let (lo, hi) = (bound_key(layouts, *lo)?, bound_key(layouts, *hi)?);
            let lo_inclusive = heap::as_bool(*lo_inclusive)?;
            let hi_inclusive = heap::as_bool(*hi_inclusive)?;
            let limit = usize::try_from(heap::as_int(*limit)?).unwrap_or(0);
            let o = obj(*m);
            let start = lo.map_or(0, |k| {
                let (below, found) = map::locate(layouts, o, k);
                below + usize::from(found.is_some() && !lo_inclusive)
            });
            let end = hi.map_or(map::len(o), |k| {
                let (below, found) = map::locate(layouts, o, k);
                below + usize::from(found.is_some() && hi_inclusive)
            });
            let most = end.saturating_sub(start).min(limit);
            let mut items = Vec::with_capacity(most);
            map::for_each_from(o, start, most, |k, v| items.push(entry(ctx, k, v)));
            let out = ctx.heap.list_from(&items);
            for w in args {
                heap::dec(*w);
            }
            Some(out)
        }
        (Builtin::MapSplit, [m, k]) if heap::kind(*m) == KIND_MAP && heap::native_key(*k) => {
            let (some, none) = (ctx.tables.layouts.some?, ctx.tables.layouts.none?);
            let tables = Arc::clone(&ctx.tables);
            let (below, at, above) = ctx.heap.map_split(&tables.layouts, *m, *k);
            heap::dec(*k);
            let at = match at {
                Some(v) => wrapped(ctx, some, v),
                None => ctx.nullary(none),
            };
            let r = ctx
                .heap
                .alloc(KIND_RECORD, 0, 3, tables.layouts.split_shape());
            unsafe {
                set_word(r, 0, above);
                set_word(r, 1, at);
                set_word(r, 2, below);
            }
            Some(r as Word)
        }
        _ => None,
    }
}

/// `Some(inner)`, which it takes.
fn wrapped(ctx: &mut Ctx, some: u32, inner: Word) -> Word {
    let c = ctx.heap.alloc(KIND_CTOR, 0, 1, some);
    unsafe { set_word(c, 0, inner) };
    c as Word
}

/// A `{key, value}` entry of a key and a value a map still holds, each held once more.
fn entry(ctx: &mut Ctx, k: Word, v: Word) -> Word {
    heap::inc(k);
    heap::inc(v);
    let e = ctx
        .heap
        .alloc(KIND_RECORD, 0, 2, ctx.tables.layouts.entry_shape());
    unsafe {
        set_word(e, 0, k);
        set_word(e, 1, v);
    }
    e as Word
}

/// `Some` of the entry, or `None`; nothing where the unit holds neither constructor.
fn entry_option(ctx: &mut Ctx, found: Option<(Word, Word)>) -> Option<Word> {
    let (some, none) = (ctx.tables.layouts.some?, ctx.tables.layouts.none?);
    Some(match found {
        Some((k, v)) => {
            let e = entry(ctx, k, v);
            wrapped(ctx, some, e)
        }
        None => ctx.nullary(none),
    })
}

/// The key a bound of `map_range` holds, none for `None`; nothing where the word is neither, or
/// holds a key no native map orders in place.
fn bound_key(layouts: &Layouts, w: Word) -> Option<Option<Word>> {
    if heap::kind(w) != KIND_CTOR {
        return None;
    }
    let o = obj(w);
    let (ctor, held) = unsafe { ((*o).layout, (*o).len) };
    if Some(ctor) == layouts.none && held == 0 {
        return Some(None);
    }
    if Some(ctor) == layouts.some && held == 1 {
        let k = unsafe { word_at(o, 0) };
        return heap::native_key(k).then_some(Some(k));
    }
    None
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
    let fuel = c.fuel;
    let frames = c.frames();
    frames.push(HandlerFrame {
        clauses,
        ret,
        simulate: false,
        detached: None,
        regions,
        fuel,
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
    if mode == MODE_RAISE || is_abort(effect.as_str(), op.as_str()) {
        return raise_from_perform(c, &effect, &op, args_of(args, n));
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

/// A raise, which no clause answers where it is performed: the frame whose clause does is unwound
/// to first, and with none in reach the entry fails. Takes the arguments.
fn raise_from_perform(c: &mut Ctx, effect: &Symbol, op: &Symbol, args: &[Word]) -> i64 {
    let release = |args: &[Word]| args.iter().for_each(|w| heap::dec(*w));
    if c.failed != 0 {
        release(args);
        return 0;
    }
    let at = c.raise_handler(effect.as_str(), op.as_str());
    let d = if is_abort(effect.as_str(), op.as_str()) {
        let message = match args.first().map(|w| c.value(*w)) {
            Some(Value::Str(ref s)) => s.to_string(),
            _ => String::new(),
        };
        Diagnostic::error(codes::RUNTIME_ERROR, format!("panic: {message}"))
    } else if at.is_some() {
        Diagnostic::error(codes::RUNTIME_ERROR, format!("`{effect}.{op}` was raised"))
    } else {
        // Only a raise nothing answers is shown what it carried: one a clause takes is not read.
        let carried: Vec<Plain> = args.iter().map(|w| Plain::shown(&c.value(*w))).collect();
        let slots: Vec<String> = (0..carried.len()).map(ply_eval::slot).collect();
        let with = if slots.is_empty() {
            String::new()
        } else {
            format!(" with {}", slots.join(", "))
        };
        Diagnostic::error(
            codes::RUNTIME_ERROR,
            format!("`{effect}.{op}` was raised{with}, and nothing answers it"),
        )
        .note("a `try` around the call answers it as an `Err`, and a `handle` clause as it likes")
        .showing(carried)
    };
    let d = d.primary(c.site(), "raised here");
    match at {
        Some(at) => c.bind_raise(at, args.to_vec(), d),
        None => {
            release(args);
            c.fail(d)
        }
    }
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
    let fuel = c.fuel;
    c.frames().push(HandlerFrame::simulate(regions, fuel));
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
    if let Some(f) = &mine {
        f.land(c);
    }
    if c.failed == FAILED_ABORT
        && let Some(a) = c.aborting.take_if(|a| a.stack == stack && a.depth == depth)
    {
        c.failed = 0;
        let mut f = mine.expect("a raise is bound for a frame still installed");
        let owner = c.owner();
        c.cells.close_regions_above(owner, f.regions);
        let closure = f.take_clause(a.clause);
        drop_frame(f);
        // The clause runs outside its `handle`, which is over: its value is the `handle`'s.
        let r = call_value(ctx, closure, &a.args);
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

/// What a computation cost: the calls it made and the objects it allocated.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cost {
    pub steps: i64,
    pub allocations: i64,
}

impl Cost {
    fn plus(self, other: Cost) -> Cost {
        Cost {
            steps: self.steps.saturating_add(other.steps),
            allocations: self.allocations.saturating_add(other.allocations),
        }
    }

    fn minus(self, other: Cost) -> Cost {
        Cost {
            steps: self.steps - other.steps,
            allocations: self.allocations - other.allocations,
        }
    }
}

impl Ctx {
    /// What this entry has cost so far, memoized answers charged as if computed again.
    fn cost(&self) -> Cost {
        Cost {
            steps: self.ticks,
            allocations: i64::try_from(self.heap.allocated()).unwrap_or(i64::MAX),
        }
        .plus(self.charged)
    }
}

/// `metered(f)`: `f()` and what it cost, as `{allocations, performs, steps, value}` with each
/// performed atom's count in `performs`, ordered by the atom.
fn metered(ctx: &mut Ctx, f: Word) -> Word {
    let before = ctx.cost();
    let performed = ctx.performed.len();
    let value = call_value(std::ptr::from_mut(ctx), f, &[]);
    heap::dec(f);
    if ctx.failed != 0 {
        return 0;
    }
    let spent = ctx.cost().minus(before);
    let mut counts: std::collections::BTreeMap<String, i64> = std::collections::BTreeMap::new();
    for atom in &ctx.performed[performed..] {
        *counts.entry(atom.to_string()).or_insert(0) += 1;
    }
    let tables = Arc::clone(&ctx.tables);
    let per_atom = tables
        .layouts
        .shape(vec![Symbol::new("atom"), Symbol::new("count")]);
    let items: Vec<Word> = counts
        .into_iter()
        .map(|(atom, count)| {
            let text = ctx.heap.str(&atom);
            let r = ctx.heap.alloc(KIND_RECORD, 0, 2, per_atom);
            unsafe {
                set_word(r, 0, text);
                set_word(r, 1, heap::imm(count));
            }
            r as Word
        })
        .collect();
    let performs = ctx.heap.list_from(&items);
    let shape = tables.layouts.shape(vec![
        Symbol::new("allocations"),
        Symbol::new("performs"),
        Symbol::new("steps"),
        Symbol::new("value"),
    ]);
    let r = ctx.heap.alloc(KIND_RECORD, 0, 4, shape);
    unsafe {
        set_word(r, 0, heap::imm(spent.allocations));
        set_word(r, 1, performs);
        set_word(r, 2, heap::imm(spent.steps));
        set_word(r, 3, value);
    }
    r as Word
}

/// The value of the pure nullary function at `index`, memoized when world-independent.
pub unsafe extern "C" fn rt_constant(ctx: *mut Ctx, index: i64) -> i64 {
    let tables = Arc::clone(&unsafe { &*ctx }.tables);
    if let Some(w) = tables.memoized(index as usize) {
        let c = unsafe { &mut *ctx };
        c.charged = c.charged.plus(tables.memo_cost(index as usize));
        return w;
    }
    let before = unsafe { &*ctx }.cost();
    // SAFETY: as in `call_value`; a nullary function never reads the null argument pointer.
    let f: Entry = unsafe { std::mem::transmute::<usize, Entry>(tables.functions[index as usize]) };
    let w = unsafe { f(ctx, std::ptr::null()) };
    let c = unsafe { &mut *ctx };
    if c.failed != 0 {
        return 0;
    }
    // The memo keeps a copy; the entry keeps using its own word.
    if heap::world_independent(w) {
        tables.memoize_costing(index as usize, w, c.cost().minus(before));
    }
    w
}

/// The value a unit holds as `len` bytes of text at `text`: what a build kept of a `const`
/// definition, read into the unit's own heap, so the entry that reads it is charged nothing.
pub unsafe extern "C" fn rt_baked(ctx: *mut Ctx, text: i64, len: i64) -> i64 {
    let c = unsafe { &mut *ctx };
    let tables = Arc::clone(&c.tables);
    let text = unsafe { std::slice::from_raw_parts(text as *const u8, len as usize) };
    let read = crate::stored::read(
        &tables.layouts,
        &tables.nullaries,
        &mut lock(&tables.immortals),
        text,
    );
    match read {
        Ok(w) => w,
        Err(why) => c.fail(
            Diagnostic::error(
                codes::INTERNAL_ERROR,
                format!("the value a build kept of a `const` definition does not read: {why}"),
            )
            .primary(Span::DUMMY, "in compiled code")
            .note("this is Ply's fault: a unit holds only values this runtime wrote"),
        ),
    }
}

/// `w` as the text [`rt_baked`] reads back, a `Bytes`. Reads `w`.
pub unsafe extern "C" fn rt_stored(ctx: *mut Ctx, w: i64) -> i64 {
    let c = unsafe { &mut *ctx };
    let tables = Arc::clone(&c.tables);
    match crate::stored::text(&tables.layouts, w) {
        Ok(text) if u32::try_from(text.len()).is_ok() => c.heap.bytes(&text),
        Ok(text) => c.fail(error(format!(
            "a value of {} stored bytes has no stored form",
            text.len()
        ))),
        Err(what) => c.fail(error(format!("{what} has no stored form"))),
    }
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

/// `bracket(acquire, release, body)`: what `body` answers for what `acquire` answered, with
/// `release` run on it however `body` ends: by returning, by a raise or a clause that did not
/// resume it unwinding through, or by a cancel. It runs where the bracket stands, with the
/// handlers around it, and a failure in it replaces whatever was unwinding. A runtime failure ends
/// the entry, so nothing more runs then. A cancel takes no answer from `acquire` and no wait from
/// `release`: it lands when that one returns, so what was acquired is released, and wholly.
fn rt_bracket(ctx: *mut Ctx, acquire: Word, release: Word, body: Word) -> Word {
    let entered = crate::simulate::shield(unsafe { &mut *ctx }, Shield::Acquire);
    let held = call_value(ctx, acquire, &[]);
    heap::dec(acquire);
    let c = unsafe { &mut *ctx };
    let cancelled = crate::simulate::unshield(c, entered);
    if c.failed != 0 {
        if cancelled {
            cancel_here(c);
        }
        heap::dec(release);
        heap::dec(body);
        return 0;
    }
    let standing = c.fuel;
    let answer = if cancelled {
        c.failed = FAILED_CANCELLED;
        0
    } else {
        heap::inc(held);
        call_value(ctx, body, &[held])
    };
    heap::dec(body);
    let c = unsafe { &mut *ctx };
    // `release` runs as deep as the bracket stands, however deep the body was when it ended.
    c.fuel = standing;
    let ending = c.failed;
    if ending != 0
        && ending != FAILED_UNWIND
        && ending != FAILED_CANCELLED
        && ending != FAILED_ABORT
    {
        heap::dec(held);
        heap::dec(release);
        return 0;
    }
    let unwinding = c.unwind.take();
    let raising = c.aborting.take();
    c.failed = 0;
    let entered = crate::simulate::shield(c, Shield::Release);
    let released = call_value(ctx, release, &[held]);
    heap::dec(release);
    let c = unsafe { &mut *ctx };
    let cancelled = crate::simulate::unshield(c, entered);
    if c.failed != 0 || cancelled {
        if let Some((_, _, carried)) = unwinding {
            heap::dec(carried);
        }
        if let Some(raise) = raising {
            raise.into_failure();
        }
        heap::dec(answer);
        if c.failed == 0 {
            heap::dec(released);
            c.failed = FAILED_CANCELLED;
        } else if cancelled {
            cancel_here(c);
        }
        return 0;
    }
    heap::dec(released);
    c.failed = ending;
    c.unwind = unwinding;
    c.aborting = raising;
    answer
}

/// A cancel held back until now lands on a task a raise or a clause is unwinding: the task unwinds
/// as cancelled from here, since a handler that answered the other would let it run on.
fn cancel_here(c: &mut Ctx) {
    if c.failed != FAILED_UNWIND && c.failed != FAILED_ABORT {
        return;
    }
    if let Some((_, _, carried)) = c.unwind.take() {
        heap::dec(carried);
    }
    if let Some(raise) = c.aborting.take() {
        raise.into_failure();
    }
    c.failed = FAILED_CANCELLED;
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

/// `array_at` for a `match` that unwraps its answer at once, like [`rt_map_lookup`].
pub unsafe extern "C" fn rt_array_lookup(ctx: *mut Ctx, xs: i64, i: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    if heap::kind(xs) == KIND_ARRAY
        && let Some(index) = heap::as_int(i)
    {
        let held = usize::try_from(index)
            .ok()
            .and_then(|at| array::items(obj(xs)).get(at).copied());
        if let Some(item) = held {
            heap::inc(item);
        }
        heap::dec(xs);
        return held.unwrap_or(0);
    }
    let answer = builtin_over_values(ctx, Builtin::ArrayAt, &[xs, i]);
    unwrapped(ctx, answer)
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

pub unsafe extern "C" fn rt_byte_of_int(ctx: *mut Ctx, n: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    match heap::as_int(n).and_then(|v| u8::try_from(v).ok()) {
        Some(b) => ctx.tables.byte(b),
        None => builtin(ctx, Builtin::ByteOfInt, &[n]),
    }
}
