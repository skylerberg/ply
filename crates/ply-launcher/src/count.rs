//! Counting what an entry allocates. A counting allocator is a whole-binary decision, so it
//! lives here beside the big-stack thread: the binary installs [`Counting`], and a run that asked
//! for the count opens a window around the entry. Nothing is counted outside a window.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// What one window allocated.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counted {
    pub allocations: u64,
    pub bytes: u64,
}

static ON: AtomicBool = AtomicBool::new(false);
static ALLOCS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);

/// The allocator a binary installs as its `#[global_allocator]`. Every thread's allocations are
/// counted, so a served request's own tasks are counted too, and the check outside a window is
/// one relaxed load.
pub struct Counting;

impl Counting {
    #[inline]
    fn note(layout: &Layout) {
        if ON.load(Ordering::Relaxed) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
    }
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        Counting::note(&layout);
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        Counting::note(&layout);
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // A realloc is an allocation; the bytes it holds are the ones it asked for now.
        if ON.load(Ordering::Relaxed) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(
                new_size.saturating_sub(layout.size()) as u64,
                Ordering::Relaxed,
            );
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

/// Runs `f` with the window open, answering what it allocated. Windows do not nest: the outermost
/// one is the entry.
pub fn window<R>(f: impl FnOnce() -> R) -> (R, Counted) {
    ALLOCS.store(0, Ordering::Relaxed);
    BYTES.store(0, Ordering::Relaxed);
    ON.store(true, Ordering::Relaxed);
    let answer = f();
    ON.store(false, Ordering::Relaxed);
    let counted = Counted {
        allocations: ALLOCS.load(Ordering::Relaxed),
        bytes: BYTES.load(Ordering::Relaxed),
    };
    (answer, counted)
}

/// `--count-allocs=PATH` or `--count-allocs PATH`, wherever it is written: the launcher's own
/// flag, taken out of the line before the program parses it. `None` when it was not given.
pub fn flag(argv: &mut Vec<String>) -> Result<Option<std::path::PathBuf>, String> {
    for i in 0..argv.len() {
        if let Some(path) = argv[i].strip_prefix("--count-allocs=") {
            let path = std::path::PathBuf::from(path);
            argv.remove(i);
            return Ok(Some(path));
        }
        if argv[i] == "--count-allocs" {
            if i + 1 >= argv.len() {
                return Err("`--count-allocs` takes a path to write the count to".to_string());
            }
            let path = std::path::PathBuf::from(&argv[i + 1]);
            argv.drain(i..i + 2);
            return Ok(Some(path));
        }
    }
    Ok(None)
}

/// Writes the count where the flag asked for it. Two numbers, so the document is written here
/// rather than through a JSON library the launcher would carry for nothing.
pub fn write(path: &std::path::Path, counted: Counted) -> std::io::Result<()> {
    std::fs::write(
        path,
        format!(
            "{{\n  \"allocations\": {},\n  \"bytes\": {}\n}}\n",
            counted.allocations, counted.bytes
        ),
    )
}
