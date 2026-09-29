use crate::support::generate;
use ply_corpus::simulate::{Trial, race_power, reduction, seed_rate, stat, summarize};
use std::path::PathBuf;

/// One `simulate` test of `tasks` tasks at `density`, over a two-module corpus; the directory is
/// kept for as long as the corpus under it is read.
fn corpus(density: &str, tasks: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("corpus");
    generate(
        &root,
        &[
            "--seed",
            "17",
            "--modules",
            "2",
            "--defs-per-module",
            "4",
            "--tests",
            "2",
            "--depth",
            "1",
            "--concurrent-tests",
            "1",
            "--tasks-per-test",
            tasks,
            "--steps-per-task",
            "2",
            "--conflict-density",
            density,
        ],
    );
    (dir, root)
}

#[test]
fn pruning_never_costs_more_than_not_pruning() {
    let (_dir, root) = corpus("0", "2");
    let measured = reduction(&root, 512, 100_000).unwrap();
    assert!(!measured.is_empty());
    for r in &measured {
        assert!(
            r.pruned <= r.naive,
            "{}: {} pruned against {} naive",
            r.key,
            r.pruned,
            r.naive
        );
        assert!(r.reduction >= 1.0);
    }
}

#[test]
fn a_search_that_cannot_see_the_synchronization_never_runs_fewer() {
    let (_dir, root) = corpus("0", "3");
    for r in reduction(&root, 4096, 100_000).unwrap() {
        assert!(
            r.pruned <= r.unsynchronized,
            "{}: {} with clocks against {} without",
            r.key,
            r.pruned,
            r.unsynchronized
        );
    }
}

#[test]
fn a_test_that_never_fails_reports_misses_and_no_ratio() {
    let (_dir, root) = corpus("1", "2");
    let power = race_power(&root, 2, 16, 100_000).unwrap();
    for p in &power {
        assert_eq!(p.dpor_misses, p.trials);
        assert_eq!(p.dpor_median, None);
        assert_eq!(p.median_ratio, None);
    }
}

#[test]
fn a_rate_is_reported_per_seeded_test_and_counts_the_seeds_it_ran() {
    let (_dir, root) = corpus("0.5", "2");
    let rates = seed_rate(&root, 8, 100_000).unwrap();
    assert!(!rates.is_empty());
    for r in &rates {
        assert_eq!(r.interleavings, 8);
        assert!(r.seeds_per_second > 0.0);
    }
}

#[test]
fn a_summary_carries_its_misses() {
    let trials = [
        Trial {
            root: 0,
            interleavings: Some(4),
        },
        Trial {
            root: 1,
            interleavings: None,
        },
        Trial {
            root: 2,
            interleavings: Some(2),
        },
    ];
    let (median, worst, misses) = summarize(&trials);
    assert_eq!(median, Some(3.0));
    assert_eq!(worst, Some(4));
    assert_eq!(misses, 1);
    assert_eq!(stat(median, misses), "3+1✗");
}
