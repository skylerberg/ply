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
use ply_eval::arena::Slot;
use ply_eval::builtins::{cell_in_update, no_such_cell};
use ply_eval::{Builtin, Closure, ClosureKind, Step, Value, values_equal};
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_ty::{BinOp, EffectAtom, Mode, Resource};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

/// A compiled function: `extern "C" fn(ctx, args) -> handle`.
pub type Entry = unsafe extern "C" fn(*mut Ctx, *const i64) -> i64;

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
    /// Each pure nullary function's memoized answer by index, as an immortal word.
    pub memo: RefCell<Vec<Option<Word>>>,
    /// Owns the constant pool's and the memo's objects for as long as the unit lives.
    pub immortals: RefCell<Heap>,
    /// The 256 one-byte values, each made immortal when first asked for.
    pub bytes: RefCell<[Word; 256]>,
    /// Per constructor index, a nullary one's immortal singleton, or `0`.
    pub nullaries: Vec<Word>,
    pub empty_list: Word,
    pub empty_map: Word,
    /// Memo words and their converted values, both ways, so a tree crosses the seam unrebuilt.
    pub memo_values: RefCell<HashMap<Word, Value>>,
    pub memo_words: RefCell<HashMap<Identity, Word>>,
    /// Answers of roots called with only memo words, up to [`CALL_MEMO_LIMIT`].
    pub calls: RefCell<HashMap<(Symbol, Vec<Word>), Word>>,
    /// Each root's definition span in the text the unit runs over, by `root_id` of its name and
    /// sorted by it: a site is an offset from its start. Never cached: definitions move.
    pub roots: Vec<(u64, Span)>,
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
        self.memo.borrow().get(index).copied().flatten()
    }

    /// Remembers `w` as pure nullary function `index`'s answer, copied into the immortal heap.
    pub fn memoize(&self, index: usize, w: Word) -> Word {
        let kept = self.immortals.borrow_mut().adopt(w);
        let mut memo = self.memo.borrow_mut();
        if memo.len() <= index {
            memo.resize(index + 1, None);
        }
        memo[index] = Some(kept);
        kept
    }

    /// The value a memo word was converted to before, if it was.
    pub fn memo_value(&self, w: Word) -> Option<Value> {
        self.memo_values.borrow().get(&w).cloned()
    }

    /// The memo word a value came from; `memo_values` holds the allocations, so ids are not reused.
    pub fn memo_word(&self, v: &Value) -> Option<Word> {
        let id = identity(v)?;
        self.memo_words.borrow().get(&id).copied()
    }

    /// Keeps `v` for memo word `w` and maps its direct parts to their words, since a body that
    /// takes a memo value apart hands those parts back in.
    pub fn remember(&self, w: Word, v: &Value) {
        // Replacing the value would free allocations its recorded identities still name.
        if self.memo_values.borrow().contains_key(&w) {
            return;
        }
        self.memo_values.borrow_mut().insert(w, v.clone());
        let mut words = self.memo_words.borrow_mut();
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
        let kept = *self.calls.borrow().get(&(root.clone(), words.to_vec()))?;
        self.memo_value(kept)
    }

    /// Remembers `out` for `root` over these memo words; `None` once the bound is reached.
    pub fn memoize_call(&self, root: &Symbol, words: &[Word], out: Word) -> Option<Word> {
        let mut calls = self.calls.borrow_mut();
        if calls.len() >= CALL_MEMO_LIMIT {
            return None;
        }
        let kept = self.immortals.borrow_mut().adopt(out);
        calls.insert((root.clone(), words.to_vec()), kept);
        Some(kept)
    }

    /// The immortal `Bytes` holding just `b`.
    pub fn byte(&self, b: u8) -> Word {
        let cached = self.bytes.borrow()[b as usize];
        if cached != 0 {
            return cached;
        }
        let w = self.immortals.borrow_mut().bytes(&[b]);
        heap::mark_immortal(w);
        self.bytes.borrow_mut()[b as usize] = w;
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
        Value::Continuation(_) => Some("a Continuation"),
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
/// The entry ran past its time budget.
pub const FAILED_OUT_OF_TIME: i64 = 5;

/// An installed handler: pushed by a `handle` site, searched innermost-out by a `perform`.
pub struct HandlerFrame {
    clauses: Vec<FrameClause>,
    /// The `return` clause's closure, or zero.
    ret: Word,
    /// A `simulate` region's clause-less frame, answering `task`, `clock`, `random` and `sim`.
    simulate: bool,
    /// For a `handle` resuming off the tail: the detached body whose own stack this frame bottoms.
    detached: Option<usize>,
}

impl HandlerFrame {
    fn simulate() -> HandlerFrame {
        HandlerFrame {
            clauses: Vec::new(),
            ret: 0,
            simulate: true,
            detached: None,
        }
    }

    pub(crate) fn detached(clauses: Vec<FrameClause>, id: usize) -> HandlerFrame {
        HandlerFrame {
            clauses,
            ret: 0,
            simulate: false,
            detached: Some(id),
        }
    }
}

/// One stack's handler frames, chained to the stack it was entered from, which a `perform`
/// searches next. A depth names a frame within one stack.
pub(crate) struct Frames {
    pub(crate) list: Vec<HandlerFrame>,
    pub(crate) parent: Option<usize>,
}

impl Frames {
    pub(crate) fn under(parent: Option<usize>) -> Frames {
        Frames {
            list: Vec::new(),
            parent,
        }
    }
}

/// One clause, under program-wide effect and resource names.
pub(crate) struct FrameClause {
    effect: Symbol,
    resource: Option<Symbol>,
    op: Symbol,
    closure: Word,
    /// 0: never resumes; 1: resumes in tail position; 2: elsewhere (only in a detached frame).
    resumes: u8,
}

impl FrameClause {
    fn answers(&self, effect: &Symbol, op: &Symbol, resource: Option<&Symbol>) -> bool {
        self.effect == *effect
            && self.op == *op
            && match (&self.resource, resource) {
                (None, _) => true,
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
                    })
                    .collect(),
                ret: f.ret,
                simulate: f.simulate,
                detached: f.detached,
            }
        })
        .collect()
}

/// A spawned task's copies of the handlers around its spawn; none may name a detached body,
/// whose clause would capture the spawner's stack.
pub(crate) fn inherit_frames(list: &[HandlerFrame]) -> Vec<HandlerFrame> {
    clone_frames(list)
        .into_iter()
        .map(|f| HandlerFrame {
            detached: None,
            ..f
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
    /// Loop passes since the entry began; every 4096th samples the clock against the deadline.
    pub ticks: i64,
    /// Stacks this entry has been given beyond the one it started on, so growing is observable.
    pub grown: u64,
    /// When the running entry's time budget is spent, if it has one.
    deadline: Option<std::time::Instant>,
    time_budget_ms: u64,
    /// The cells, holding heap words: declared before the heap, so their counts go back first.
    cells: ply_eval::TaskRegions<Held>,
    /// The arena's `(depth, live)` when the running entry began, for [`Ctx::cells_balanced`].
    cells_baseline: (usize, usize),
    /// The arena `ply_eval::builtins::call` insists on; nothing reaching it uses it.
    scratch: ply_eval::Arena,
    pub heap: Heap,
    /// Objects the last entry allocated, kept because [`Ctx::end`] clears the heap's count.
    last_entry: usize,
    unclosed_entries: u64,
    pub tables: Rc<Tables>,
    /// Why the last entry failed.
    pub diagnostic: Option<Diagnostic>,
    pub builtin_calls: u64,
    /// One per stack run in this entry, the entry's own first; `current` is the one running.
    pub(crate) stacks: Vec<Frames>,
    pub(crate) current: usize,
    /// Every atom a compiled `perform` performed since the entry began, for the machine's trace.
    pub performed: Vec<EffectAtom>,
    /// The regions live in this entry, innermost last; the checker forbids nesting, so at most one.
    pub sims: Vec<crate::simulate::Simulation>,
    /// The detached bodies this entry has opened, named by index from their frames and tokens.
    pub(crate) detached: Vec<crate::detached::Detached>,
    pub(crate) starting_detached: Option<usize>,
    /// The host boundary: what a `perform` nothing on the stack answers reaches.
    pub(crate) binding: Arc<ply_eval::HostBinding>,
    pub(crate) runtime: Option<Rc<dyn ply_eval::HostRuntime>>,
    pub(crate) declared: Option<ply_ty::Footprint>,
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
    /// The value a clause handed to `resume` in tail position, read back when the clause returns.
    resumed: Option<Word>,
}

impl Ctx {
    pub fn new(tables: Rc<Tables>) -> Ctx {
        let cells = ply_eval::TaskRegions::new();
        let baseline = (cells.arena().depth(), cells.arena().live());
        Ctx {
            failed: 0,
            fuel: 0,
            stack_floor: 0,
            site_root: -1,
            site_start: 0,
            site_end: 0,
            ticks: 0,
            grown: 0,
            deadline: None,
            time_budget_ms: 0,
            heap: Heap::new(),
            last_entry: 0,
            unclosed_entries: 0,
            tables,
            cells,
            cells_baseline: baseline,
            scratch: ply_eval::Arena::new(),
            diagnostic: None,
            builtin_calls: 0,
            stacks: vec![Frames::under(None)],
            current: 0,
            performed: Vec::new(),
            sims: Vec::new(),
            detached: Vec::new(),
            starting_detached: None,
            binding: Arc::new(ply_eval::HostBinding::hermetic()),
            runtime: None,
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
            resumed: None,
        }
    }

    /// Between calls, and only between calls.
    pub fn begin(&mut self, fuel: i64) {
        let arena = self.cells.arena();
        self.cells_baseline = (arena.depth(), arena.live());
        self.failed = 0;
        self.fuel = fuel;
        self.ticks = 0;
        self.grown = 0;
        self.time_budget_ms = time_budget_ms();
        self.deadline = (self.time_budget_ms > 0).then(|| {
            std::time::Instant::now() + std::time::Duration::from_millis(self.time_budget_ms)
        });
        self.stack_floor = stack_floor();
        self.site_root = -1;
        self.last_linear = None;
        self.diagnostic = None;
        self.stacks.clear();
        self.stacks.push(Frames::under(None));
        self.current = 0;
        self.performed.clear();
        self.sims.clear();
        self.detached.clear();
        self.starting_detached = None;
        self.trail = ply_eval::region::Trail::new(self.seed.clone());
        self.record = None;
        self.entered_sims = 0;
        self.unwind = None;
        self.resumed = None;
        // Every path out of an entry calls `end`; this catches one that did not.
        if self.heap.allocated() != 0 {
            self.unclosed_entries += 1;
            self.end();
        }
        heap::enter(&mut self.heap);
        heap::poison::enter(&raw const self.site_root);
    }

    /// The other end of [`Ctx::begin`]: the entry gives back what it used.
    pub fn end(&mut self) {
        heap::poison::leave();
        heap::leave();
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

    /// How many objects the entry that just finished allocated.
    pub fn allocated_by_entry(&self) -> usize {
        if self.heap.allocated() == 0 {
            self.last_entry
        } else {
            self.heap.allocated()
        }
    }

    /// Entries that reached [`Ctx::begin`] without their predecessor having closed itself.
    pub fn unclosed_entries(&self) -> u64 {
        self.unclosed_entries
    }

    /// Whether the entry gave back every region it opened and cell slot it took; a cell word in the
    /// answer is refused separately.
    pub fn cells_balanced(&self) -> bool {
        let arena = self.cells.arena();
        (arena.depth(), arena.live()) == self.cells_baseline
    }

    /// The singleton a nullary constructor is.
    pub fn nullary(&self, index: u32) -> Word {
        self.tables.nullaries[index as usize]
    }

    pub fn set_host(
        &mut self,
        binding: Arc<ply_eval::HostBinding>,
        runtime: Option<Rc<dyn ply_eval::HostRuntime>>,
    ) {
        self.binding = binding;
        self.runtime = runtime;
    }

    pub(crate) fn frames(&mut self) -> &mut Vec<HandlerFrame> {
        &mut self.stacks[self.current].list
    }

    /// A new stack's frames, chained under `parent`; its index names it.
    pub(crate) fn open_stack(&mut self, parent: Option<usize>) -> usize {
        self.stacks.push(Frames::under(parent));
        self.stacks.len() - 1
    }

    pub(crate) fn fail(&mut self, d: Diagnostic) -> i64 {
        self.fail_with(1, d)
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
        let Some(root) = u64::try_from(self.site_root)
            .ok()
            .and_then(|r| {
                let roots = &self.tables.roots;
                let at = roots.binary_search_by_key(&r, |(id, _)| *id).ok()?;
                Some(&roots[at].1)
            })
            .filter(|r| !r.is_dummy())
        else {
            return Span::DUMMY;
        };
        let at = |offset: i64| {
            i64::from(root.start)
                .checked_add(offset)
                .and_then(|o| u32::try_from(o).ok())
        };
        match (at(self.site_start), at(self.site_end)) {
            (Some(start), Some(end)) => Span::new(root.source, start, end),
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
        self.diagnostic.take()
    }

    /// The interpreter value a word denotes, for a builtin or an error message.
    pub(crate) fn value(&self, w: Word) -> Value {
        Heap::to_value(&self.tables.layouts, w)
    }

    pub(crate) fn word(&mut self, v: &Value) -> Word {
        let tables = Rc::clone(&self.tables);
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

/// Opens a `with cell` region; `unique` is the site's proof that no continuation crosses it.
pub unsafe extern "C" fn rt_region(ctx: *mut Ctx, unique: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let kind = match unique {
        0 => ply_eval::RegionKind::Shared,
        _ => ply_eval::RegionKind::Unique,
    };
    ctx.cells.open_region(kind, Span::DUMMY).0 as i64
}

/// Closes a region, reclaiming its cells. Only success emits it; a failed body leaves the entry
/// unbalanced and the seam falls back to the machine.
pub unsafe extern "C" fn rt_region_close(ctx: *mut Ctx, region: i64) {
    let ctx = unsafe { &mut *ctx };
    ctx.cells
        .close_region(ply_eval::arena::RegionId(region as u32));
}

/// Allocates a cell in the context's arena, shared with the interpreter's cell builtins.
pub unsafe extern "C" fn rt_cell(ctx: *mut Ctx, init: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    if !ctx.sims.is_empty() {
        ctx.trail.record_access(ply_eval::sim::Access::Alloc);
    }
    let slot = ctx.cells.alloc_cell(Held(init));
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

pub unsafe extern "C" fn rt_box_bool(_ctx: *mut Ctx, v: i64) -> i64 {
    heap::bool(v != 0)
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
const BINOPS: [BinOp; 17] = [
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
];

/// The machine's own negation of a value whose type the emitter cannot see. Takes it.
pub unsafe extern "C" fn rt_negate(ctx: *mut Ctx, a: i64) -> i64 {
    let c = unsafe { &mut *ctx };
    let vals = values_taken(c, &[a]);
    let answer = match &vals[0] {
        Value::Float(f) => Value::Float(-f),
        Value::Decimal(d) => Value::Decimal(-*d),
        Value::Fixed(f) => match ply_eval::Fixed::of(f.ty, -f.value()) {
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

/// The machine's own operator over two values whose type the emitter does not fix. Takes both.
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
        Err(d) => ctx.fail(d),
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

/// A loop's periodic check: past the entry's deadline, the body stops where it is.
pub unsafe extern "C" fn rt_tick(ctx: *mut Ctx) {
    let ctx = unsafe { &mut *ctx };
    if let Some(deadline) = ctx.deadline
        && std::time::Instant::now() > deadline
    {
        let d = error(format!(
            "ran past the time budget of {} ms",
            ctx.time_budget_ms
        ));
        ctx.fail_with(FAILED_OUT_OF_TIME, d);
    }
}

/// The wall-clock budget an entry begins with, in milliseconds; 0 is none. A command sets the
/// process's, and a caller that wants one evaluation bounded differently sets its thread's.
/// The default bounds a harness that never asked, so a loop that never ends fails there too.
static TIME_BUDGET_MS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(DEFAULT_TIME_BUDGET_MS);

pub const DEFAULT_TIME_BUDGET_MS: u64 = 60_000;

thread_local! {
    static THREAD_TIME_BUDGET_MS: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

pub fn set_time_budget(ms: u64) {
    TIME_BUDGET_MS.store(ms, std::sync::atomic::Ordering::Relaxed);
}

/// Runs `f` with entries on this thread bounded by `ms` rather than the process's budget.
pub fn with_time_budget<R>(ms: u64, f: impl FnOnce() -> R) -> R {
    let before = THREAD_TIME_BUDGET_MS.with(|t| t.replace(Some(ms)));
    let out = f();
    THREAD_TIME_BUDGET_MS.with(|t| t.set(before));
    out
}

pub fn time_budget_ms() -> u64 {
    THREAD_TIME_BUDGET_MS
        .with(|t| t.get())
        .unwrap_or_else(|| TIME_BUDGET_MS.load(std::sync::atomic::Ordering::Relaxed))
}

/// Room below the floor for the runtime's frames and the deepest compiled frame itself.
pub(crate) const STACK_MARGIN: usize = 512 * 1024;

/// The floor for this thread, asked of the platform once per thread (it can be a `/proc` read).
fn stack_floor() -> usize {
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
            let d = if op != 0 && b == 0 {
                error(format!("{what} by zero"))
            } else {
                error(format!("integer overflow in {what}"))
            };
            ctx.fail(d)
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
    ctx.fail(d);
}

pub unsafe extern "C" fn rt_overflow(ctx: *mut Ctx, what: i64) {
    let ctx = unsafe { &mut *ctx };
    let name = match what {
        0 => "addition",
        1 => "subtraction",
        _ => "negation",
    };
    let d = error(format!("integer overflow in {name}"));
    ctx.fail(d);
}

/// An `Int` outside the target width; `which` indexes [`ply_ty::INT_TYPES`].
pub unsafe extern "C" fn rt_not_that_width(ctx: *mut Ctx, which: i64, value: i64) {
    let ctx = unsafe { &mut *ctx };
    let t = ply_ty::INT_TYPES[which as usize];
    let d = error(format!(
        "`{}` was given {value}: `{t}` holds {} to {}",
        t.of_int_name(),
        t.min(),
        t.max()
    ));
    ctx.fail(d);
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

/// `++`: native strings append; anything else raises the interpreter's error. Takes both.
pub unsafe extern "C" fn rt_concat(ctx: *mut Ctx, a: i64, b: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    if heap::kind(a) == KIND_STR && heap::kind(b) == KIND_STR {
        let out = ctx.heap.append(a, unsafe { bytes_of(obj(b)) });
        heap::dec(b);
        return out;
    }
    let (l, r) = (ctx.value(a), ctx.value(b));
    heap::dec(a);
    heap::dec(b);
    let joined = match (l.as_str(Span::DUMMY, "`++`"), r.as_str(Span::DUMMY, "`++`")) {
        (Ok(x), Ok(y)) => format!("{x}{y}"),
        (Err(d), _) | (_, Err(d)) => return ctx.fail(d),
    };
    ctx.word(&Value::str(joined))
}

/// A builtin over taken arguments: natively over words where it can, else the interpreter's.
pub unsafe extern "C" fn rt_builtin(ctx: *mut Ctx, index: i64, args: *const i64, n: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let b = ctx.tables.builtins[index as usize];
    let args = args_of(args, n);
    ctx.builtin_calls += 1;
    if !ctx.sims.is_empty()
        && let Some(access) = crate::simulate::cell_access(ctx, b, args)
    {
        ctx.trail.record_access(access);
    }
    if let Some(w) = native_builtin(ctx, b, args) {
        return w;
    }
    builtin_over_values(ctx, b, args)
}

/// The interpreter's implementation of `b` over the values the words denote. Takes the arguments.
fn builtin_over_values(ctx: &mut Ctx, b: Builtin, args: &[Word]) -> Word {
    let values = values_taken(ctx, args);
    let site = ctx.site();
    match ply_eval::builtins::call(b, values, &mut ctx.scratch, site) {
        Ok(Step::Done(v)) => ctx.word(&v),
        // Unreachable: the emitter refuses higher-order builtins.
        Ok(_) => {
            let d = error(format!(
                "`{}` suspended, which the fragment excludes",
                b.name()
            ));
            ctx.fail(d)
        }
        Err(d) => ctx.fail(d),
    }
}

/// `bytes_concat_all` over a list literal's pieces, without building the list. Takes the pieces.
pub unsafe extern "C" fn rt_bytes_join(ctx: *mut Ctx, args: *const i64, n: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let pieces = args_of(args, n);
    ctx.builtin_calls += 1;
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

/// The builtins answered over native words; `None` hands the call to the interpreter.
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
            let updated = call_value(std::ptr::from_mut(ctx), *f, &[current.into_word()]);
            let held = if ctx.failed != 0 {
                Held::default()
            } else {
                Held(updated)
            };
            ctx.cells.arena_mut().put_back(slot, held);
            heap::dec(*c);
            Some(if ctx.failed != 0 { 0 } else { heap::unit() })
        }
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
        // Anything the interpreter would raise on answers `None` before touching a count.
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
            let out = position(ctx, at)?;
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
            let out = position(ctx, at)?;
            heap::dec(*hay);
            heap::dec(*needle);
            Some(out)
        }
        (Builtin::BytesIndexOfByte, [hay, byte]) if heap::kind(*hay) == KIND_BYTES => {
            let byte = u8::try_from(heap::as_int(*byte)?).ok()?;
            let at = memchr::memchr(byte, unsafe { bytes_of(obj(*hay)) });
            let out = position(ctx, at)?;
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
            // Absent is the interpreter's diagnostic to raise.
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
            let tables = Rc::clone(&ctx.tables);
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
            let tables = Rc::clone(&ctx.tables);
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
            let tables = Rc::clone(&ctx.tables);
            let entries = list::to_vec(obj(*xs));
            let n = entries.len();
            let (key, value) = (Symbol::new("key"), Symbol::new("value"));
            // Any other entry shape is the interpreter's to raise on.
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
            let tables = Rc::clone(&ctx.tables);
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

/// The byte offset of the `n`-th character boundary, as the interpreter's `char_offset` finds it.
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

/// `Some(at)` or `None` as the prelude's constructors, when the unit knows them.
fn position(ctx: &mut Ctx, at: Option<usize>) -> Option<Word> {
    let some = ctx.tables.layouts.some?;
    let none = ctx.tables.layouts.none?;
    Some(match at {
        Some(i) => {
            let c = ctx.heap.alloc(KIND_CTOR, 0, 1, some);
            unsafe { set_word(c, 0, heap::imm(i as i64)) };
            c as Word
        }
        None => ctx.nullary(none),
    })
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
    let frames = c.frames();
    frames.push(HandlerFrame {
        clauses,
        ret,
        simulate: false,
        detached: None,
    });
    (frames.len() - 1) as i64
}

/// A `handle` site's clause table: per clause the effect, resource (negative for none) and op
/// as field-table indices, the closure, and its `resumes`.
fn clauses_of(c: &Ctx, clauses: *const i64, n: i64) -> Vec<FrameClause> {
    let words = args_of(clauses, n * 5);
    let name = |i: i64| c.tables.fields[i as usize].clone();
    words
        .chunks(5)
        .map(|w| FrameClause {
            effect: name(w[0]),
            resource: (w[1] >= 0).then(|| name(w[1])),
            op: name(w[2]),
            closure: w[3],
            resumes: w[4] as u8,
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
unsafe extern "C" fn rt_resume_entry(ctx: *mut Ctx, args: *const i64) -> i64 {
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
        ((stack << 32) | depth) as i64,
    )
}

/// A unary closure over one immediate capture whose entry is a runtime function.
pub(crate) fn closure_of(c: &mut Ctx, entry: usize, capture: i64) -> Word {
    let o = c
        .heap
        .alloc(KIND_CLOSURE, 0, (1 + CLOSURE_CAPTURES) as u32, 1);
    unsafe {
        set_word(o, CLOSURE_CODE, entry as Word);
        set_word(o, CLOSURE_CAPTURES, heap::imm(capture));
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
    let resource = (resource >= 0).then(|| c.tables.fields[resource as usize].clone());
    let atom = EffectAtom::operation(
        effect.clone(),
        resource
            .clone()
            .map_or(Resource::Singleton, Resource::Named),
        if mode != 0 { Mode::Write } else { Mode::Read },
        op.clone(),
    );
    // A step's footprint is what it conflicts on, which is the mode; the scheduled operation's
    // own access (`OpSignature::step_access`) is that same mode atom.
    if !c.sims.is_empty() {
        c.trail
            .record_access(ply_eval::sim::Access::Atom(atom.mode_atom()));
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
                    // Moved in: a local owning memory across the switch is freed once per restore.
                    return unsafe {
                        crate::detached::stop(
                            ctx,
                            id,
                            closure,
                            args_of(args, n),
                            (effect, op, resource),
                        )
                    };
                }
                found = Some((stack, i, cl.closure, cl.resumes != 0));
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
    let Some((stack, depth, closure, resumes)) = found else {
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
    c.frames().push(HandlerFrame::simulate());
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
    c.sims.pop();
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
    if c.failed == FAILED_UNWIND
        && let Some((target_stack, target, v)) = c.unwind.take()
    {
        if target_stack == stack && target == depth {
            c.failed = 0;
            if let Some(f) = mine {
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

/// A builtin used as a value: the interpreter's own closure kind for it.
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

/// The value of the pure nullary function at `index`, memoized when world-independent.
pub unsafe extern "C" fn rt_constant(ctx: *mut Ctx, index: i64) -> i64 {
    let tables = Rc::clone(&unsafe { &*ctx }.tables);
    if let Some(Some(w)) = tables.memo.borrow().get(index as usize) {
        return *w;
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
                ClosureKind::Builtin(b) => {
                    let b = *b;
                    let values = values_taken(c, args);
                    c.builtin_calls += 1;
                    let site = c.site();
                    match ply_eval::builtins::call(b, values, &mut c.scratch, site) {
                        Ok(Step::Done(v)) => c.word(&v),
                        Ok(_) => {
                            let d = error(format!(
                                "`{}` suspended, which the fragment excludes",
                                b.name()
                            ));
                            c.fail(d)
                        }
                        Err(d) => c.fail(d),
                    }
                }
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
                ClosureKind::Native { .. } => {
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
            return c.fail(d);
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
            return c.fail(d);
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
    let d = match what {
        0 => error(format!(
            "`iterate` needs a budget of at least 1, and this is {n}"
        )),
        1 => error(format!("`iterate` did not stop within its budget of {n}")),
        _ => {
            let d = error(format!(
                "the step given to `iterate` answered {}, not `Continue` or `Stop`",
                ctx.type_name(n)
            ));
            heap::dec(n);
            d
        }
    };
    ctx.fail(d);
}

/// A range longer than [`RANGE_LIMIT`], raised where a fused loop would have walked it.
pub unsafe extern "C" fn rt_bad_range(ctx: *mut Ctx, lo: i64, hi: i64) {
    let ctx = unsafe { &mut *ctx };
    let d = error(format!(
        "`range` of {} elements exceeds the limit of {RANGE_LIMIT}",
        hi.saturating_sub(lo)
    ));
    ctx.fail(d);
}

/// The most elements the interpreter's `range` builds, which a fused loop holds to as well.
pub const RANGE_LIMIT: i64 = 10_000_000;

/// Element `i` of a list a fused loop checked, held once more. Reads the list.
pub unsafe extern "C" fn rt_list_get(_ctx: *mut Ctx, list: i64, i: i64) -> i64 {
    let w = list::get(obj(list), i as usize);
    heap::inc(w);
    w
}

/// `xs` with `x` appended, for a fused `map` or `filter` building its answer. Takes both.
pub unsafe extern "C" fn rt_list_push(ctx: *mut Ctx, xs: i64, x: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    ctx.heap.list_push(xs, x)
}

/// A fused loop was handed something other than a list.
pub unsafe extern "C" fn rt_not_a_list(ctx: *mut Ctx, which: i64, value: i64) {
    let ctx = unsafe { &mut *ctx };
    let what = match which {
        0 => "fold",
        1 => "map",
        _ => "filter",
    };
    let d = error(format!(
        "`{what}` needs a List, and this is {}",
        ctx.type_name(value)
    ));
    heap::dec(value);
    ctx.fail(d);
}

/// A shift count outside the word; `which` indexes [`ply_ty::INT_TYPES`], or is `-1` for `Int`.
pub unsafe extern "C" fn rt_shift_count(ctx: *mut Ctx, n: i64, which: i64) {
    let ctx = unsafe { &mut *ctx };
    let (ty, width) = match usize::try_from(which)
        .ok()
        .and_then(|i| ply_ty::INT_TYPES.get(i))
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

/// A record update writing `n` fields at `offsets` in `shape`: in place when the base is unique
/// and has that shape, else into a fresh copy. Takes the base and the written fields.
pub unsafe extern "C" fn rt_record_update(
    ctx: *mut Ctx,
    shape: i64,
    base: i64,
    args: *const i64,
    offsets: *const i64,
    n: i64,
) -> i64 {
    let ctx = unsafe { &mut *ctx };
    let written = args_of(args, n);
    let offsets = args_of(offsets, n);
    if heap::kind(base) != KIND_RECORD {
        let d = error(format!(
            "a record update needs a record, and this is {}",
            ctx.type_name(base)
        ));
        return ctx.fail(d);
    }
    let o = obj(base);
    let shape = shape as u32;
    let in_place = unsafe { (*o).layout } == shape && is_unique(base);
    let width = ctx.tables.layouts.shape_width(shape);
    ply_eval::rc::note_update_of(in_place, if in_place { 0 } else { width }, ctx.site());
    if in_place {
        for (w, at) in written.iter().zip(offsets) {
            unsafe {
                heap::dec(word_at(o, *at as usize));
                set_word(o, *at as usize, *w);
            }
        }
        unsafe { (*o).flags &= flat_over(written) | !heap::FLAT };
        return base;
    }
    // Unwritten fields come from the base by offset, or by name when the lowering guessed a base
    // of another shape.
    let tables = Rc::clone(&ctx.tables);
    let width = tables.layouts.shape_width(shape);
    let flat = flat_over(written) & unsafe { (*o).flags };
    let out = ctx.heap.alloc(KIND_RECORD, flat, width as u32, shape);
    // Which offsets were written: a bitmask up to 128 fields, a list past that.
    let mut mask = 0u128;
    let mut wide = Vec::new();
    if width > 128 {
        wide = vec![false; width];
    }
    for (w, at) in written.iter().zip(offsets) {
        let at = *at as usize;
        unsafe { set_word(out, at, *w) };
        if width > 128 {
            wide[at] = true;
        } else {
            mask |= 1 << at;
        }
    }
    let filled = |i: usize| {
        if width > 128 {
            wide[i]
        } else {
            mask >> i & 1 == 1
        }
    };
    if written.len() < width {
        let base_shape = unsafe { (*o).layout };
        if base_shape == shape {
            for i in (0..width).filter(|i| !filled(*i)) {
                let w = unsafe { word_at(o, i) };
                heap::inc(w);
                unsafe { set_word(out, i, w) };
            }
        } else {
            let names = tables.layouts.shape_names(shape);
            for i in (0..width).filter(|i| !filled(*i)) {
                let Some(at) = tables.layouts.offset(base_shape, &names[i]) else {
                    let d = error(format!("this record has no field `{}`", names[i]));
                    return ctx.fail(d);
                };
                let w = unsafe { word_at(o, at) };
                heap::inc(w);
                unsafe { set_word(out, i, w) };
            }
        }
    }
    heap::dec(base);
    out as Word
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

/// A builtin called directly, natively where it can be, else over values. Takes the arguments.
fn direct(ctx: &mut Ctx, b: Builtin, args: &[Word]) -> Word {
    ctx.builtin_calls += 1;
    match native_builtin(ctx, b, args) {
        Some(w) => w,
        None => builtin_over_values(ctx, b, args),
    }
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
    direct(unsafe { &mut *ctx }, Builtin::ListAt, &[xs, i])
}

pub unsafe extern "C" fn rt_list_set(ctx: *mut Ctx, xs: i64, i: i64, v: i64) -> i64 {
    direct(unsafe { &mut *ctx }, Builtin::ListSet, &[xs, i, v])
}

/// `list_at` for a `match` that unwraps its answer at once, like [`rt_map_lookup`].
pub unsafe extern "C" fn rt_list_lookup(ctx: *mut Ctx, xs: i64, i: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    if heap::kind(xs) == KIND_LIST
        && let Some(index) = heap::as_int(i)
    {
        ctx.builtin_calls += 1;
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
        ctx.builtin_calls += 1;
        return ctx.heap.list_push(xs, x);
    }
    direct(ctx, Builtin::Push, &[xs, x])
}

pub unsafe extern "C" fn rt_map_insert(ctx: *mut Ctx, m: i64, k: i64, v: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    if heap::kind(m) == KIND_MAP && heap::native_key(k) {
        ctx.builtin_calls += 1;
        let tables = Rc::clone(&ctx.tables);
        return ctx.heap.map_insert(&tables.layouts, m, k, v);
    }
    direct(ctx, Builtin::MapInsert, &[m, k, v])
}

pub unsafe extern "C" fn rt_map_contains(ctx: *mut Ctx, m: i64, k: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    if heap::kind(m) == KIND_MAP && heap::native_key(k) {
        ctx.builtin_calls += 1;
        let found = map::get(&ctx.tables.layouts, obj(m), k).is_some();
        heap::dec(m);
        heap::dec(k);
        return heap::bool(found);
    }
    direct(ctx, Builtin::MapContains, &[m, k])
}

pub unsafe extern "C" fn rt_map_get(ctx: *mut Ctx, m: i64, k: i64) -> i64 {
    direct(unsafe { &mut *ctx }, Builtin::MapGet, &[m, k])
}

pub unsafe extern "C" fn rt_compare(ctx: *mut Ctx, a: i64, b: i64) -> i64 {
    direct(unsafe { &mut *ctx }, Builtin::Compare, &[a, b])
}

pub unsafe extern "C" fn rt_byte_of_int(ctx: *mut Ctx, n: i64) -> i64 {
    let ctx = unsafe { &mut *ctx };
    match heap::as_int(n).and_then(|v| u8::try_from(v).ok()) {
        Some(b) => {
            ctx.builtin_calls += 1;
            ctx.tables.byte(b)
        }
        None => direct(ctx, Builtin::ByteOfInt, &[n]),
    }
}

pub unsafe extern "C" fn rt_bytes_scan(
    ctx: *mut Ctx,
    hay: i64,
    from: i64,
    members: i64,
    max: i64,
) -> i64 {
    direct(
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
    direct(
        unsafe { &mut *ctx },
        Builtin::BytesScanUntil,
        &[hay, from, members, max],
    )
}

pub unsafe extern "C" fn rt_bytes_slice(ctx: *mut Ctx, b: i64, s: i64, e: i64) -> i64 {
    direct(unsafe { &mut *ctx }, Builtin::BytesSlice, &[b, s, e])
}

pub unsafe extern "C" fn rt_bytes_concat(ctx: *mut Ctx, a: i64, b: i64) -> i64 {
    direct(unsafe { &mut *ctx }, Builtin::BytesConcat, &[a, b])
}
