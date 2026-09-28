//! What one served request allocates, counted by the launcher rather than by this harness.
//!
//! The count comes from `ply --count-allocs`, so what is measured is what a run allocates — the
//! launcher's own instrument, over the service's own entry, with the network handled inside the
//! program. Nothing here counts anything itself.

use anyhow::{Context, Result};
use std::path::PathBuf;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mut repo = PathBuf::from(".");
    let mut requests = 200u32;
    let mut out: Option<PathBuf> = None;
    let mut sites = false;
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--repo" => repo = PathBuf::from(args.next().unwrap_or_default()),
            "--requests" => requests = args.next().unwrap_or_default().parse().unwrap_or(200),
            "--out" => out = Some(PathBuf::from(args.next().unwrap_or_default())),
            "--sites" => sites = true,
            other => {
                anyhow::bail!(
                    "`{other}` is not a flag of w6-alloc; it takes --repo, --requests, --sites and \
                     --out"
                )
            }
        }
    }

    let ply = ply_corpus::serve::ply_binary()?;
    let dir = ply_corpus::w6_run::counting_project(&repo)?;

    // Two windows, because one cannot tell a request from a run: a window's total charges the
    // entry's own startup to every request in it, and the startup is the size of the program —
    // `std`, and whatever the service pulls in — rather than of the request path. The difference
    // between two windows is the request, and what is left over is the startup.
    let small_requests = requests / 10;
    anyhow::ensure!(
        small_requests >= 10,
        "a per-request cost is read off two windows, so `--requests` is at least 100, not {requests}"
    );
    // Every window is counted on a run that is not the first of that window: a launcher's first run
    // of a program pays for caches — the emitted unit above all — that the run after it reuses, and
    // that cost is the size of the program rather than of the request.
    let stage = tempfile::tempdir().context("a stage for these windows")?;
    ply_corpus::w6_run::counted(&ply, dir.path(), small_requests, sites, stage.path())?;
    ply_corpus::w6_run::counted(&ply, dir.path(), requests, sites, stage.path())?;
    let small = ply_corpus::w6_run::counted(&ply, dir.path(), small_requests, sites, stage.path())?;
    let large = ply_corpus::w6_run::counted(&ply, dir.path(), requests, sites, stage.path())?;
    let span = f64::from(requests - small_requests);
    let allocations_per_request = (large.allocations as f64 - small.allocations as f64) / span;
    let bytes_per_request = (large.bytes as f64 - small.bytes as f64) / span;
    eprintln!(
        "w6-alloc: {small_requests} requests {:.1} allocations each, {requests} requests {:.1} \
         each; one request costs {allocations_per_request:.2} allocations and \
         {bytes_per_request:.1} bytes, and one run starts at {:.0} allocations",
        small.allocations as f64 / f64::from(small_requests),
        large.allocations as f64 / f64::from(requests),
        small.allocations as f64 - allocations_per_request * f64::from(small_requests),
    );
    if sites {
        // The largest window's sites, which is the window a per-request count is read at.
        let mut rows: Vec<&ply_corpus::w6_run::Site> = large.sites.iter().collect();
        rows.sort_by(|a, b| b.allocations.cmp(&a.allocations));
        eprintln!("w6-alloc: the {requests}-request window allocated at:");
        for site in rows.iter().take(20) {
            eprintln!(
                "  {:.1} per request  {:>9} bytes  {}",
                site.allocations as f64 / f64::from(requests),
                site.bytes,
                site.site
            );
        }
    }

    let figures = ply_corpus::w6_run::Allocation {
        route: "/health".to_string(),
        requests: requests as usize,
        response_bytes: response_bytes(&repo)?,
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

/// The answer's size, which the figure carries so a reader can see what it is per byte of.
fn response_bytes(repo: &std::path::Path) -> Result<usize> {
    let loaded = ply_corpus::w6_run::program(repo)?;
    Ok(loaded
        .response_over_sim(&ply_corpus::w6_run::head())
        .context("the service answers `/health`")?
        .len())
}
