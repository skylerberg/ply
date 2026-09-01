//! What one served request allocates, counted rather than timed.

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
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--repo" => repo = PathBuf::from(args.next().unwrap_or_default()),
            "--requests" => requests = args.next().unwrap_or_default().parse().unwrap_or(200),
            other => {
                anyhow::bail!("`{other}` is not a flag of w6-alloc; it takes --repo and --requests")
            }
        }
    }

    let loaded = ply_corpus::w6_run::program(&repo)?;
    let request = ply_corpus::w6_run::head();
    let response = loaded.response_over_sim(&request)?;
    // One warm pass, so lazily built machine state is not charged to the count.
    loaded.over_sim(vec![vec![request.clone()]])?;

    let script: Vec<Vec<Vec<u8>>> = (0..requests).map(|_| vec![request.clone()]).collect();
    ALLOCS.with(|c| c.set(0));
    BYTES.with(|c| c.set(0));
    loaded.over_sim(script)?;
    let allocations = ALLOCS.with(Cell::get) as f64 / requests as f64;
    let bytes = BYTES.with(Cell::get) as f64 / requests as f64;

    println!(
        "{}",
        serde_json::json!({
            "route": "/health",
            "requests": requests,
            "response_bytes": response.len(),
            "allocations_per_request": allocations,
            "bytes_per_request": bytes,
        })
    );
    Ok(())
}
