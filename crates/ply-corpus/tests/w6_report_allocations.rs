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
//!
//! # The second test reads `README.md`, and it is the only test in the tree that
//! reads a prose document
//!
//! It exists because that figure went stale twice in one milestone and the
//! second time it went stale **inside the correction block written for the first
//! time**. `README.md` §"Where this is not competitive" is what
//! `CONTRIBUTING.md` §"Say how it was checked, or say it was not" holds up as
//! the model for honest reporting, and it carried a present-tense count of
//! 1,035 after the tree made 1,122, then a present-tense 1,122 after R3 took the
//! tree to 1,082. Both were found by an adversarial reader rather than by
//! anything that runs. `docs/ONBOARDING.md` §7's checked/written boundary moves
//! by exactly this one line.

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

/// What one `/health` request allocates, in a 200-request window.
///
/// 200 because that is the window every published figure was taken at, and the
/// byte count may only be read at the window its baseline was taken at:
/// `bytes_per_request` *rises* with the window, undiagnosed —
/// `CONTRIBUTING.md` §"Things known to be broken" item 8.
fn per_request() -> (f64, f64) {
    let loaded = ply_corpus::w6_run::program(&repo()).expect("the service compiles");
    let request = ply_corpus::w6_run::head();
    // One warm pass, so lazily-built machine state is not charged to the count.
    loaded
        .over_sim(vec![vec![request.clone()]])
        .expect("the service serves one connection");

    const N: usize = 200;
    let script: Vec<Vec<Vec<u8>>> = (0..N).map(|_| vec![request.clone()]).collect();
    let (_, allocs, bytes) = counted(|| loaded.over_sim(script).expect("the service serves"));
    (allocs as f64 / N as f64, bytes as f64 / N as f64)
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

    let (per_request, bytes) = per_request();
    let mb_per_request = bytes / 1e6;

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

/// What `README.md` says a request allocates, against what one does.
///
/// The band is **1%**, not the factor of two above, and the difference is
/// deliberate: `benches/w6-ladder.json` is a dated artifact and its figure is
/// past tense, while this sentence is present tense about this tree. A refactor
/// that moves the count by one allocation moves what the README says, and the
/// only thing that has ever caught that here is a reader.
///
/// Re-take it with `./target/release/w6-alloc --repo . --requests 200` and
/// correct the sentence in place, keeping the withdrawn figure beside the new
/// one — `CONTRIBUTING.md` §"Correct, do not delete".
///
/// The line number in the message below drifts with every edit above it in
/// `README.md` — it moved 363 → 387 on 2026-08-27 — so the marker this searches
/// for, not the number, is what finds the sentence.
#[test]
fn the_readme_still_describes_this_request_path() {
    let text = std::fs::read_to_string(repo().join("README.md")).expect("the repository ships one");
    let marker = "One `/health` request makes";
    let at = text.find(marker).unwrap_or_else(|| {
        panic!(
            "`README.md` no longer contains \"{marker}\", so the sentence this guards was moved \
             or reworded. Point this test at wherever the request-path allocation count now \
             lives, or delete it and say in `docs/ONBOARDING.md` §7 that no test reads a prose \
             document again."
        )
    });
    let claimed_allocs = number_before(&text[at..], " allocations")
        .expect("the sentence states an allocation count before the word `allocations`");
    let claimed_bytes = number_before(&text[at..], " bytes")
        .expect("the sentence states a byte count before the word `bytes`");

    let (allocs, bytes) = per_request();
    println!(
        "`README.md` says {claimed_allocs:.0} allocations and {claimed_bytes:.0} bytes per \
         /health request; this tree makes {allocs:.2} and {bytes:.2}"
    );

    for (what, claimed, measured) in [
        ("allocations", claimed_allocs, allocs),
        ("bytes", claimed_bytes, bytes),
    ] {
        let drift = (claimed - measured).abs() / measured;
        assert!(
            drift <= 0.01,
            "`README.md:387` says one /health request makes {claimed:.0} {what} and this tree \
             makes {measured:.2} — {:.1}% apart. That sentence is present tense about this tree \
             and it has gone stale twice, the second time inside the block correcting the first. \
             Re-take it: `./target/release/w6-alloc --repo . --requests 200`.",
            drift * 100.0
        );
    }
}
