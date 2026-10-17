//! A unit's tables, which every thread running the unit reads: its constants, roots and the identities of what it keeps.

use super::*;

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
    /// What computing each memoized answer cost, in calls, allocations and their bytes, stored
    /// before the answer is: a later read charges it, so a cost is the same whether or not it was
    /// memoized.
    pub memo_costs: Box<[(AtomicI64, AtomicI64, AtomicI64)]>,
    /// Each function's closure over nothing, by index, once a body asked for it; `0` is not yet.
    pub lambdas: Box<[AtomicI64]>,
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
pub(super) const PARTS_LIMIT: usize = 64;

/// A handed-out value's allocation, plus a list's window onto it: an identity without a walk.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Identity {
    Record(usize),
    Str(usize),
    Bytes(usize),
    List(usize),
    Array(usize),
}

/// Each of `values`, the parts of record, constructor or array `o` in order, beside the word `o`
/// holds it as; a part held as its bits is no word of its own and is left out.
pub(super) fn held_words<'v>(
    layouts: &Layouts,
    o: *mut heap::Obj,
    values: impl Iterator<Item = &'v Value>,
) -> Vec<(Word, &'v Value)> {
    layouts
        .parts(o)
        .into_iter()
        .zip(values)
        .filter_map(|((at, slot), part)| unsafe { heap::part_word(o, at, slot) }.map(|w| (w, part)))
        .collect()
}

pub(super) fn identity(v: &Value) -> Option<Identity> {
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
        if let Some((steps, allocations, bytes)) = self.memo_costs.get(index) {
            steps.store(cost.steps, Release);
            allocations.store(cost.allocations, Release);
            bytes.store(cost.bytes, Release);
        }
        self.memoize(index, w)
    }

    /// What computing the memoized answer at `index` cost.
    pub fn memo_cost(&self, index: usize) -> Cost {
        match self.memo_costs.get(index) {
            Some((steps, allocations, bytes)) => Cost {
                steps: steps.load(Acquire),
                allocations: allocations.load(Acquire),
                bytes: bytes.load(Acquire),
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
        let layouts = &self.layouts;
        let parts: Vec<(Word, &Value)> = match v {
            Value::Record(fields) => held_words(layouts, o, fields.iter().map(|(_, part)| part)),
            Value::Ctor { args, .. } => held_words(layouts, o, args.iter()),
            Value::List(items) if items.len() <= PARTS_LIMIT => items
                .iter()
                .enumerate()
                .map(|(i, part)| (list::get(o, i), part))
                .collect(),
            Value::Array(items) if items.len() <= PARTS_LIMIT => {
                held_words(layouts, o, items.iter())
            }
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
