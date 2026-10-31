use super::*;

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub type Entry = unsafe extern "C" fn(*mut Ctx, *const i64) -> i64;

pub struct Tables {
    pub fields: Vec<Symbol>,
    pub functions: Vec<usize>,
    pub memo: Box<[AtomicI64]>,
    pub immortals: Mutex<Heap>,
    pub roots: Vec<Root>,
}

pub struct Root {
    pub id: u64,
    pub name: Symbol,
    pub span: Span,
}

impl Tables {
    pub fn memoized(&self, index: usize) -> Option<Word> {
        let w = self.memo.get(index)?.load(Acquire);
        (w != 0).then_some(w)
    }

    pub fn keep(&self, index: usize, kept: Word) {
        if let Some(slot) = self.memo.get(index) {
            let _ = slot.compare_exchange(0, kept, AcqRel, Acquire);
        }
    }
}
