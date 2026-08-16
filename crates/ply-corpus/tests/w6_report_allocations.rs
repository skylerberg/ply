//! The staleness guard that cannot be blamed on a machine.
//!
//! `w6_report_integrity.rs` compares the shipped ladder's *times* against a
//! fresh take, and a reader is entitled to ask whether a wall clock on a busy
//! laptop proves anything. This asks the same question in allocations, which
//! are counted rather than timed: the same program on any machine makes the
//! same number of them, so a difference here is a difference in the program.
//!
//! The number under test is the one ADR 0016 publishes as the size of a whole
//! lever — what one `/health` request allocates to produce a 107-byte response
//! — and which `benches/w6-ladder.json` carries in the `boxing on hot paths`
//! alternative. A lever's size is what C3 is decided against.
//!
//! The count in the file is produced by `w6-alloc`, which `ply-corpus
//! w6-ladder` runs and folds in, so re-taking the ladder re-takes this too.
//!
//! A `#[global_allocator]` is a whole-binary decision, which is why this is its
//! own test binary rather than an assertion inside `w6_report_integrity.rs`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::path::{Path, PathBuf};

thread_local! {
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
    static BYTES: Cell<usize> = const { Cell::new(0) };
}

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
        let _ = BYTES.try_with(|c| c.set(c.get() + layout.size()));
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn counted<T>(f: impl FnOnce() -> T) -> (T, usize, usize) {
    ALLOCS.with(|c| c.set(0));
    BYTES.with(|c| c.set(0));
    let out = f();
    (out, ALLOCS.with(Cell::get), BYTES.with(Cell::get))
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate lives two levels below the repository root")
        .to_path_buf()
}

/// The first comma-grouped integer in `text` that is followed by `after`.
fn number_before(text: &str, after: &str) -> Option<f64> {
    let at = text.find(after)?;
    let head = &text[..at];
    let digits: String = head
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit() || *c == ',' || *c == '.')
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    digits.replace(',', "").trim().parse().ok()
}

/// What the shipped report says a request allocates, against what one does.
///
/// The band is a factor of two either way. Allocation counts do move with a
/// refactor, and this is not a performance assertion — it is an assertion that
/// the file and the program are describing the same request.
#[test]
fn the_shipped_allocation_evidence_still_describes_this_request_path() {
    let text = std::fs::read_to_string(repo().join("benches/w6-ladder.json"))
        .expect("the ladder file is what the lever's size is published in");
    let report: ply_corpus::w6::Report =
        serde_json::from_str(&text).expect("the ladder file is a W6 report");
    let alternative = report
        .alternatives
        .iter()
        .find(|a| a.name.contains("boxing"))
        .expect("the report prices boxing and allocation as a lever");
    let claimed_allocs =
        number_before(&alternative.what, " times").expect("the lever states an allocation count");
    let claimed_mb =
        number_before(&alternative.what, " MB").expect("the lever states a byte count");

    let loaded = ply_corpus::w6_run::program(&repo()).expect("the service compiles");
    let request = ply_corpus::w6_run::head();
    // One warm pass, so lazily-built machine state is not charged to the count.
    loaded
        .over_sim(vec![vec![request.clone()]])
        .expect("the service serves one connection");

    const N: usize = 200;
    let script: Vec<Vec<Vec<u8>>> = (0..N).map(|_| vec![request.clone()]).collect();
    let (_, allocs, bytes) = counted(|| loaded.over_sim(script).expect("the service serves"));
    let per_request = allocs as f64 / N as f64;
    let mb_per_request = bytes as f64 / N as f64 / 1e6;

    println!(
        "the report says {claimed_allocs:.0} allocations and {claimed_mb:.2} MB per /health \
         request; this tree makes {per_request:.0} and {mb_per_request:.3} MB"
    );

    let ratio = claimed_allocs / per_request;
    assert!(
        (0.5..=2.0).contains(&ratio),
        "`benches/w6-ladder.json` publishes {claimed_allocs:.0} allocations per /health request \
         and this tree makes {per_request:.0} — {ratio:.1}x apart. Allocation counts do not vary \
         with a machine, so the file and the program are not describing the same request path, \
         and every share, projection and reopen threshold read off that file is about a program \
         that is not here. Re-take it: `ply-corpus w6-ladder --repo . --db <url> --machine <name> \
         --postgres <version> --out benches/w6-ladder.json`, which runs `w6-alloc` for this \
         number itself."
    );
}
