//! What one served request allocates, counted rather than timed.

use anyhow::Context;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::path::PathBuf;

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

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let mut repo = PathBuf::from(".");
    let mut requests = 200usize;
    let mut out: Option<PathBuf> = None;
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--repo" => repo = PathBuf::from(args.next().unwrap_or_default()),
            "--requests" => requests = args.next().unwrap_or_default().parse().unwrap_or(200),
            "--out" => out = Some(PathBuf::from(args.next().unwrap_or_default())),
            other => {
                anyhow::bail!(
                    "`{other}` is not a flag of w6-alloc; it takes --repo, --requests and --out"
                )
            }
        }
    }

    // The stage is pointed at a directory of this run's own, so the measurement does not depend on
    // whether some earlier run left a compiled stage behind: whether bodies enter compiled code at
    // all is a function of the stage cache, and the slope between two windows then describes the
    // cache as much as the request path. A shipped figure is a cap, so it is taken in the state
    // that allocates the most, the same way every time.
    let stage = tempfile::tempdir().context("a temp dir for the stage")?;
    // SAFETY: this binary is single-threaded until the machine runs, and nothing reads the
    // environment between here and the stage's first use. `PLY_C_STAGE` is read at each compil.
    unsafe { std::env::set_var("PLY_C_STAGE", stage.path()) };

    let loaded = ply_corpus::w6_run::program(&repo)?;
    let request = ply_corpus::w6_run::head();
    let response = loaded.response_over_sim(&request)?;
    // One warm pass, so lazily built machine state is not charged to the count.
    loaded.over_sim(vec![vec![request.clone()]])?;

    // Two windows, because one cannot tell a request from a run: a window's total charges the
    // Machine's own startup to every request in it, and the startup is the size of the program —
    // `std`, and whatever the service pulls in — rather than of the request path. The difference
    // between two windows is the request, and what is left over is the startup.
    let small_requests = requests / 10;
    anyhow::ensure!(
        small_requests >= 10,
        "a per-request cost is read off two windows, so `--requests` is at least 100, not \
         {requests}"
    );
    let small = counted(&loaded, &request, small_requests)?;
    let large = counted(&loaded, &request, requests)?;
    let span = (requests - small_requests) as f64;
    let allocations_per_request = (large.0 as f64 - small.0 as f64) / span;
    let bytes_per_request = (large.1 as f64 - small.1 as f64) / span;
    // The same slope through the small window: what one run costs before it serves anything.
    eprintln!(
        "w6-alloc: {} requests {:.1} allocations each, {requests} requests {:.1} each; one request \
         costs {allocations_per_request:.2} allocations and {bytes_per_request:.1} bytes, and one \
         run starts at {:.0} allocations",
        small_requests,
        small.0 as f64 / small_requests as f64,
        large.0 as f64 / requests as f64,
        small.0 as f64 - allocations_per_request * small_requests as f64,
    );
    let figures = ply_corpus::w6_run::Allocation {
        route: "/health".to_string(),
        requests,
        response_bytes: response.len(),
        allocations_per_request,
        bytes_per_request,
    };

    let rendered = format!("{}\n", serde_json::to_string_pretty(&figures)?);
    print!("{rendered}");
    if let Some(out) = out {
        std::fs::write(&out, rendered)?;
        eprintln!("wrote {}", out.display());
    }
    Ok(())
}

/// A window of `n` requests: the allocations and the bytes it makes.
fn counted(
    loaded: &ply_corpus::w3::Loaded,
    request: &[u8],
    n: usize,
) -> anyhow::Result<(usize, usize)> {
    let script: Vec<Vec<Vec<u8>>> = (0..n).map(|_| vec![request.to_vec()]).collect();
    ALLOCS.with(|c| c.set(0));
    BYTES.with(|c| c.set(0));
    loaded.over_sim(script)?;
    Ok((ALLOCS.with(Cell::get), BYTES.with(Cell::get)))
}
