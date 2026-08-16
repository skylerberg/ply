//! Which call sites a request's allocations come from.
//!
//! `w6_request_cost.rs` says how many allocations a layer costs. It does not say
//! *which code* allocates, and a lever chosen from a layer total is a lever
//! chosen from an aggregate. This captures a backtrace per allocation and ranks
//! the sites, so an optimization is aimed at a frame rather than at a rung.
//!
//! The capture is re-entrant-guarded because `Backtrace` allocates; the guard is
//! why the numbers here are a *sample of sites* and the counts in
//! `w6_request_cost.rs` remain the authority on totals.

use ply_eval::Value;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
    static INSIDE: Cell<bool> = const { Cell::new(false) };
    static SITES: RefCell<HashMap<String, (usize, usize)>> = RefCell::new(HashMap::new());
    static TOTAL: Cell<usize> = const { Cell::new(0) };
}

struct Tracing;

unsafe impl GlobalAlloc for Tracing {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let armed = ARMED.try_with(Cell::get).unwrap_or(false);
        if armed && !INSIDE.try_with(Cell::get).unwrap_or(true) {
            let _ = INSIDE.try_with(|c| c.set(true));
            let _ = TOTAL.try_with(|c| c.set(c.get() + 1));
            let key = site();
            let _ = SITES.try_with(|s| {
                let mut s = s.borrow_mut();
                let e = s.entry(key).or_insert((0, 0));
                e.0 += 1;
                e.1 += layout.size();
            });
            let _ = INSIDE.try_with(|c| c.set(false));
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Tracing = Tracing;

/// The nearest few `ply_*` frames, which is the attribution that matters: a
/// `RawVec::grow` frame names the allocator, not the code that wanted the room.
fn site() -> String {
    let bt = std::backtrace::Backtrace::force_capture();
    let text = format!("{bt}");
    let mut frames: Vec<String> = Vec::new();
    for line in text.lines() {
        let Some(at) = line.find(": ") else { continue };
        let name = line[at + 2..].trim();
        if name.starts_with("ply_") && !name.contains("w6_alloc_sites") {
            let cut = name.rfind("::h").map(|i| &name[..i]).unwrap_or(name);
            frames.push(cut.to_string());
            if frames.len() == 3 {
                break;
            }
        }
    }
    if frames.is_empty() {
        "<no ply frame>".to_string()
    } else {
        frames.join(" < ")
    }
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate lives two levels below the repository root")
        .to_path_buf()
}

#[test]
fn the_request_paths_allocation_sites_are_ranked() {
    let loaded = ply_corpus::w6_run::program(&repo()).expect("the service must compile");
    let bench = loaded.full("w6_bench").expect("the driver declares w6_bench");
    let request = ply_corpus::w6_run::head();

    for (mode, name) in [(1i64, "endpoint"), (2, "framing"), (3, "routing")] {
        loaded
            .pure_call(&bench, vec![Value::Int(mode), Value::Int(4)], 1)
            .expect("the driver runs");
        SITES.with(|s| s.borrow_mut().clear());
        TOTAL.with(|c| c.set(0));
        ARMED.with(|c| c.set(true));
        loaded
            .pure_call(&bench, vec![Value::Int(mode), Value::Int(20)], 1)
            .expect("the driver runs");
        ARMED.with(|c| c.set(false));
        report(name, 20);
    }

    loaded
        .over_sim(vec![vec![request.clone()]])
        .expect("the service serves one connection");
    SITES.with(|s| s.borrow_mut().clear());
    TOTAL.with(|c| c.set(0));
    ARMED.with(|c| c.set(true));
    loaded
        .over_sim((0..20).map(|_| vec![request.clone()]).collect())
        .expect("the service serves");
    ARMED.with(|c| c.set(false));
    report("whole request over SimNet", 20);
}

/// The share above which one site owning a request is a finding rather than a
/// profile.
///
/// W6 found the control stack's link push at 47% across its three call sites and
/// pooled it; the largest single site after that is 29%. A regression that put a
/// fresh allocation back on a per-step path would show here as one frame owning
/// the request, and so would a build whose symbols did not resolve — every
/// sample would land in `<no ply frame>`, which is 100% and is meant to fail.
const CONCENTRATION_LIMIT: f64 = 0.50;

fn report(name: &str, n: usize) {
    let total = TOTAL.with(Cell::get);
    let mut rows: Vec<(String, usize, usize)> =
        SITES.with(|s| s.borrow().iter().map(|(k, v)| (k.clone(), v.0, v.1)).collect());
    rows.sort_by_key(|r| std::cmp::Reverse(r.1));
    println!("\n== {name}: {} allocations per request", total / n);
    for (site, count, bytes) in rows.iter().take(25) {
        println!(
            "  {:>8.1} {:>9.0}B  {:>5.1}%  {site}",
            *count as f64 / n as f64,
            *bytes as f64 / n as f64,
            100.0 * *count as f64 / total as f64
        );
    }
    assert!(total > 0, "`{name}` allocated nothing, so nothing was ranked");
    let counted: usize = rows.iter().map(|r| r.1).sum();
    assert_eq!(
        counted, total,
        "`{name}` ranked {counted} of {total} allocations, so the table is not the whole request"
    );
    let (top, count, _) = &rows[0];
    let share = *count as f64 / total as f64;
    assert!(
        share <= CONCENTRATION_LIMIT,
        "`{name}`: {top} owns {:.1}% of the request's allocations",
        100.0 * share
    );
}
