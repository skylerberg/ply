use ply_corpus::build::generate;
use ply_corpus::simulate::{Trial, race_power, reduction, seed_rate, stat, summarize};
use ply_corpus::spec::CorpusSpec;
use ply_corpus::write;

fn corpus(density: f64, tasks: usize) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let spec = CorpusSpec {
        seed: 17,
        modules: 2,
        defs_per_module: 4,
        tests: 2,
        depth: 1,
        concurrent_tests: 1,
        tasks_per_test: tasks,
        steps_per_task: 2,
        conflict_density: density,
        ..CorpusSpec::default()
    };
    write::write(dir.path(), &spec, &generate(&spec)).unwrap();
    dir
}

/// The claim, at the size a unit test can afford: pruning never runs more interleavings than
/// not pruning, and the search reaches its frontier.
#[test]
fn pruning_never_costs_more_than_not_pruning() {
    let dir = corpus(0.0, 2);
    let measured = reduction(dir.path(), 512, 100_000).unwrap();
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

/// Withholding the recording's clocks may only cost interleavings, never save them: the filter
/// refuses reorderings that are unreachable, so a search that cannot see the ordering has
/// strictly more to do.
#[test]
fn a_search_that_cannot_see_the_synchronization_never_runs_fewer() {
    let dir = corpus(0.0, 3);
    for r in reduction(dir.path(), 4096, 100_000).unwrap() {
        assert!(
            r.pruned <= r.unsynchronized,
            "{}: {} with clocks against {} without",
            r.key,
            r.pruned,
            r.unsynchronized
        );
    }
}

/// A corpus with no failing test reports misses rather than a median over nothing, and never a
/// zero that reads as "found immediately".
#[test]
fn a_test_that_never_fails_reports_misses_and_no_ratio() {
    let dir = corpus(1.0, 2);
    let power = race_power(dir.path(), 2, 16, 100_000).unwrap();
    for p in &power {
        assert_eq!(p.dpor_misses, p.trials);
        assert_eq!(p.dpor_median, None);
        assert_eq!(p.median_ratio, None);
    }
}

#[test]
fn a_rate_is_reported_per_seeded_test_and_counts_the_seeds_it_ran() {
    let dir = corpus(0.5, 2);
    let rates = seed_rate(dir.path(), 8, 100_000).unwrap();
    assert!(!rates.is_empty());
    for r in &rates {
        assert_eq!(r.interleavings, 8);
        assert!(r.seeds_per_second > 0.0);
    }
}

/// A statistic that dropped its misses would report the search as better than it is, which is
/// the one direction this module must not err in.
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
