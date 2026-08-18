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
//!
//! # The window is part of the measurement
//!
//! `w3::Loaded::over_sim` builds one `Machine` for the whole script, so anything
//! a machine does once — lowering a definition the first time it is reached,
//! deciding a region's kind, building the definition table — is divided by
//! however many requests the script carries. Two windows over the same code
//! therefore disagree about the total *and about the ranking*, because the
//! one-time work's share falls as the window grows while the per-request work's
//! share rises.
//!
//! That is the whole difference between this file's figure and
//! `./target/release/w6-alloc --repo . --requests 200`, and
//! [`the_two_allocation_harnesses_are_one_measurement_read_at_two_windows`]
//! asserts it rather than leaving it to be rediscovered: the two are checked
//! against each other at the *same* window, and the per-request slope is
//! reported separately from the per-machine intercept so a lever is chosen
//! against the one it would actually move.

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

/// One armed window: what it allocated, and where.
///
/// The map is handed back rather than left in the thread-local, because the
/// question this file exists to answer needs two windows side by side.
struct Window {
    requests: usize,
    total: usize,
    sites: HashMap<String, (usize, usize)>,
}

impl Window {
    fn count(&self, site: &str) -> f64 {
        self.sites.get(site).map(|v| v.0).unwrap_or(0) as f64
    }

    fn per_request(&self) -> f64 {
        self.total as f64 / self.requests as f64
    }
}

fn capture<T>(requests: usize, f: impl FnOnce() -> T) -> Window {
    SITES.with(|s| s.borrow_mut().clear());
    TOTAL.with(|c| c.set(0));
    ARMED.with(|c| c.set(true));
    let answered = f();
    ARMED.with(|c| c.set(false));
    drop(answered);
    Window {
        requests,
        total: TOTAL.with(Cell::get),
        sites: SITES.with(|s| s.borrow().clone()),
    }
}

/// One connection per request, which is the script `w6-alloc` drives and so the
/// only shape whose totals are comparable with it.
fn script(request: &[u8], requests: usize) -> Vec<Vec<Vec<u8>>> {
    (0..requests).map(|_| vec![request.to_vec()]).collect()
}

#[test]
fn the_request_paths_allocation_sites_are_ranked() {
    let loaded = ply_corpus::w6_run::program(&repo()).expect("the service must compile");
    let bench = loaded
        .full("w6_bench")
        .expect("the driver declares w6_bench");
    let request = ply_corpus::w6_run::head();

    for (mode, name) in [(1i64, "endpoint"), (2, "framing"), (3, "routing")] {
        loaded
            .pure_call(&bench, vec![Value::Int(mode), Value::Int(4)], 1)
            .expect("the driver runs");
        let window = capture(20, || {
            loaded
                .pure_call(&bench, vec![Value::Int(mode), Value::Int(20)], 1)
                .expect("the driver runs")
        });
        report(name, &window);
    }

    loaded
        .over_sim(vec![vec![request.clone()]])
        .expect("the service serves one connection");
    let window = capture(SMALL, || {
        loaded
            .over_sim(script(&request, SMALL))
            .expect("the service serves")
    });
    report("whole request over SimNet", &window);
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

fn report(name: &str, window: &Window) {
    let total = window.total;
    let n = window.requests;
    let mut rows: Vec<(String, usize, usize)> = window
        .sites
        .iter()
        .map(|(k, v)| (k.clone(), v.0, v.1))
        .collect();
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
    assert!(
        total > 0,
        "`{name}` allocated nothing, so nothing was ranked"
    );
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

// ------------------------------------------------- the two windows, reconciled

/// The window this file's ranking has always been taken over.
const SMALL: usize = 20;

/// The window `w6-alloc` is published at, and the one ADR 0017's readings are
/// taken over.
const LARGE: usize = 200;

/// How far the two harnesses may disagree at the same window before the
/// disagreement is a defect in one of them rather than the noise of a warm-up.
///
/// They count the same event through two different global allocators over the
/// same call, so the only legitimate spread is the handful of allocations one
/// process makes before arming that the other does not. It has measured well
/// inside a tenth of a percent; the band is wide enough that a lazily-built
/// table moving between them is not a failure and narrow enough that a route or
/// a script change is.
const HARNESS_BAND: f64 = 0.02;

#[test]
fn the_two_allocation_harnesses_are_one_measurement_read_at_two_windows() {
    let Some(counter) = w6_alloc_binary() else {
        println!(
            "skipped: `w6-alloc` is not beside this test binary, so the two harnesses were not \
             cross-checked; build it with `cargo build --release --workspace`"
        );
        return;
    };

    let loaded = ply_corpus::w6_run::program(&repo()).expect("the service must compile");
    let request = ply_corpus::w6_run::head();
    loaded
        .over_sim(vec![vec![request.clone()]])
        .expect("the service serves one connection");

    let small = capture(SMALL, || {
        loaded
            .over_sim(script(&request, SMALL))
            .expect("the service serves")
    });
    let large = capture(LARGE, || {
        loaded
            .over_sim(script(&request, LARGE))
            .expect("the service serves")
    });

    // The per-request slope and the per-machine intercept, from two points on
    // one line rather than from an assumption about which frames are one-time.
    let span = (LARGE - SMALL) as f64;
    let marginal = |site: &str| (large.count(site) - small.count(site)) / span;
    let per_machine = |site: &str| small.count(site) - marginal(site) * SMALL as f64;
    let marginal_total = (large.total as f64 - small.total as f64) / span;
    let fixed_total = small.total as f64 - marginal_total * SMALL as f64;

    let mut sites: Vec<String> = large
        .sites
        .keys()
        .chain(small.sites.keys())
        .cloned()
        .collect();
    sites.sort();
    sites.dedup();

    println!(
        "\n== /health over SimNet, the same call at two windows\n  \
         {:>4} requests: {:>9.1} allocations per request\n  \
         {:>4} requests: {:>9.1} allocations per request\n  \
         fit:            {:>9.1} per request + {:.0} once per Machine",
        SMALL,
        small.per_request(),
        LARGE,
        large.per_request(),
        marginal_total,
        fixed_total
    );

    let mut by_marginal: Vec<&String> = sites.iter().collect();
    by_marginal.sort_by(|a, b| marginal(b).total_cmp(&marginal(a)));
    println!(
        "\n== per-request work, ranked by the slope (the {LARGE}-request window is {:.0}% of it)",
        100.0 * marginal_total / large.per_request()
    );
    println!(
        "  {:>9} {:>7} {:>10} {:>9}  site",
        "per req", "share", "per Machine", "at n=20"
    );
    for site in by_marginal.iter().take(20) {
        println!(
            "  {:>9.1} {:>6.1}% {:>10.0} {:>8.1}%  {site}",
            marginal(site),
            100.0 * marginal(site) / marginal_total,
            per_machine(site),
            100.0 * small.count(site) / small.total as f64
        );
    }

    let mut by_fixed: Vec<&String> = sites.iter().collect();
    by_fixed.sort_by(|a, b| per_machine(b).total_cmp(&per_machine(a)));
    println!(
        "\n== one-time work, ranked by the intercept ({:.0}% of the {SMALL}-request window)",
        100.0 * fixed_total / (small.total as f64)
    );
    for site in by_fixed.iter().take(15) {
        println!(
            "  {:>9.0} {:>6.1}% {:>10.1} per req  {site}",
            per_machine(site),
            100.0 * per_machine(site) / fixed_total,
            marginal(site)
        );
    }

    // The two candidates R3 was planned against, rolled up over every chain a
    // frame of theirs appears in: both are whole-program compile-time analyses
    // and a per-site row understates them by splitting one pass across its
    // recursion depths.
    println!("\n== the two hoist candidates, over every site their frames appear in");
    println!(
        "  {:>9} {:>10} {:>9} {:>9}  family",
        "per req", "per Machine", "at n=20", "at n=200"
    );
    for family in HOISTS {
        let (slope, intercept) = fit(family, &small, &large);
        println!(
            "  {slope:>9.1} {intercept:>10.0} {:>8.1}% {:>8.1}%  {family}",
            100.0 * family_count(family, &small) / small.total as f64,
            100.0 * family_count(family, &large) / large.total as f64,
        );
    }

    // `Machine::definition` memoizes a lowered top-level body; the
    // `ClosureKind::Fn` arm beside it does not, and lowers a lambda's body on
    // every apply. So a slope of zero on one route is not a property of
    // lowering, and a second path is fitted rather than generalized to.
    let routing = loaded
        .full("w6_bench")
        .expect("the driver declares w6_bench");
    let iterate = |n: usize| {
        loaded
            .pure_call(&routing, vec![Value::Int(3), Value::Int(n as i64)], 1)
            .expect("the driver runs")
    };
    iterate(4);
    let small_routing = capture(SMALL, || iterate(SMALL));
    let large_routing = capture(LARGE, || iterate(LARGE));
    println!(
        "\n== the same two families on the routing rung, a second path\n  \
         {:>4} iterations: {:>9.1} allocations each\n  \
         {:>4} iterations: {:>9.1} allocations each",
        SMALL,
        small_routing.per_request(),
        LARGE,
        large_routing.per_request()
    );
    for family in HOISTS {
        let (slope, intercept) = fit(family, &small_routing, &large_routing);
        println!("  {slope:>9.1} per iteration {intercept:>10.0} per Machine  {family}");
    }

    for window in [&small, &large] {
        let counted = w6_alloc(&counter, window.requests);
        let spread = (window.per_request() - counted).abs() / counted;
        assert!(
            spread <= HARNESS_BAND,
            "at {} requests the site harness counts {:.1} allocations per request and `w6-alloc` \
             counts {counted:.1}, a spread of {:.1}%: the two are no longer measuring one call",
            window.requests,
            window.per_request(),
            100.0 * spread
        );
    }

    assert!(
        fixed_total > 0.0,
        "the two windows fit a negative per-Machine cost ({fixed_total:.0}), so the totals do not \
         lie on one line and neither window can be read as a per-request figure"
    );
    assert!(
        marginal_total > 0.0,
        "the slope is {marginal_total:.1}, so a request costs nothing and the fit is wrong"
    );
    let top = by_marginal[0];
    assert!(
        top.starts_with("ply_"),
        "the largest per-request site is `{top}`, not a Ply frame: the build's symbols did not \
         resolve and the ranking names nothing"
    );
}

/// The two compile-time analyses R3 proposes to hoist off the request path.
///
/// Rolled up over every chain a frame of theirs appears in, because both are
/// recursive whole-program passes and a per-site row splits one pass across its
/// own recursion depths.
const HOISTS: [&str; 2] = ["ply_eval::region_kind", "ply_eval::code::lower"];

fn family_count(family: &str, window: &Window) -> f64 {
    window
        .sites
        .iter()
        .filter(|(site, _)| site.contains(family))
        .map(|(_, v)| v.0 as f64)
        .sum()
}

/// A family's per-iteration slope and its per-`Machine` intercept, from the two
/// windows.
fn fit(family: &str, small: &Window, large: &Window) -> (f64, f64) {
    let slope = (family_count(family, large) - family_count(family, small))
        / (large.requests - small.requests) as f64;
    (
        slope,
        family_count(family, small) - slope * small.requests as f64,
    )
}

/// The counting binary beside this test binary — `target/<profile>/w6-alloc`
/// against `target/<profile>/deps/w6_alloc_sites-<hash>`.
///
/// `cargo test -p ply-corpus` builds the package's binaries, so it is normally
/// there; a `--test` run against a target directory that has never built the
/// workspace is the case that skips.
fn w6_alloc_binary() -> Option<PathBuf> {
    let mine = std::env::current_exe().ok()?;
    let path = mine.parent()?.parent()?.join("w6-alloc");
    path.exists().then_some(path)
}

fn w6_alloc(counter: &Path, requests: usize) -> f64 {
    let out = std::process::Command::new(counter)
        .arg("--repo")
        .arg(repo())
        .arg("--requests")
        .arg(requests.to_string())
        .output()
        .unwrap_or_else(|e| panic!("running `{}`: {e}", counter.display()));
    assert!(
        out.status.success(),
        "`{}` failed: {}",
        counter.display(),
        String::from_utf8_lossy(&out.stderr).trim()
    );
    let counted: serde_json::Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("`{}` did not print a count: {e}", counter.display()));
    counted["allocations_per_request"]
        .as_f64()
        .expect("`w6-alloc` prints allocations_per_request")
}
