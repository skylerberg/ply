use ply_corpus::build::generate;
use ply_corpus::measure::{fixture_cost, multi_shot, scheduling, stack_cost, throughput};
use ply_corpus::spec::CorpusSpec;
use ply_corpus::write::write;
use std::path::Path;

fn corpus_at(root: &Path) {
    let spec = CorpusSpec {
        seed: 4,
        modules: 5,
        defs_per_module: 6,
        tests: 10,
        depth: 2,
        ..CorpusSpec::default()
    };
    write(root, &spec, &generate(&spec)).unwrap();
}

#[test]
fn a_pass_reports_what_it_ran() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("corpus");
    corpus_at(&root);

    let t = throughput(&root, 1).unwrap();
    assert!(t.pass.steady_pass_millis > 0.0);
    assert!(t.pass.performs > 0, "the corpus performed no atom");
    assert!(t.lower_test_bodies_millis > 0.0);
}

/// The machine lowers on first call, so setup must not be read as interpreter speed.
#[test]
fn setup_is_reported_apart_from_evaluation() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("corpus");
    corpus_at(&root);

    let t = throughput(&root, 9).unwrap();
    assert!(
        t.pass.first_pass_millis >= t.pass.steady_pass_millis * 0.5,
        "a first pass of {} ms against a steady {} ms",
        t.pass.first_pass_millis,
        t.pass.steady_pass_millis
    );
}

/// The forkable world's version of this asserted that a fork of a 10,000-cell fixture cost what
/// a fork of a one-cell fixture did.
#[test]
fn opening_a_fixture_beats_rebuilding_it_once_the_fixture_is_real() {
    let points = fixture_cost(&[1, 10_000], 3);
    assert_eq!(points.len(), 2);
    for p in &points {
        println!(
            "  {:>6} cells: open {:>10.1} ns, rebuild {:>10.1} ns ({:.2}x)",
            p.cells, p.open_nanos, p.rebuild_nanos, p.rebuild_over_open
        );
    }
    assert!(
        points[1].rebuild_over_open > 1.0,
        "opening a 10,000-cell fixture cost {} ns against {} ns to rebuild it",
        points[1].open_nanos,
        points[1].rebuild_nanos
    );
    assert!(
        points[1].open_nanos > points[0].open_nanos,
        "an open is O(the fixture); a 10,000-cell one cost {} ns against {} ns for one cell",
        points[1].open_nanos,
        points[0].open_nanos
    );
}

/// If `capture` walked the segment it cut, the 100,000-frame row would cost four orders of
/// magnitude more than the 8-frame one.
#[test]
fn capture_and_resume_are_flat_in_the_frames_they_move() {
    let points = stack_cost(3);
    let small = points.first().expect("a first row");
    let large = points.last().expect("a last row");
    assert_eq!(large.captured_frames, 100_000);
    assert_eq!(large.segments, 1);
    let flat = |slow: f64, fast: f64| slow < fast * 100.0 + 500.0;
    assert!(
        flat(large.capture_nanos, small.capture_nanos),
        "capturing {} frames cost {} ns against {} ns for {}",
        large.pending_frames,
        large.capture_nanos,
        small.capture_nanos,
        small.pending_frames
    );
    assert!(
        flat(large.resume_nanos, small.resume_nanos),
        "splicing {} frames cost {} ns against {} ns for {}",
        large.pending_frames,
        large.resume_nanos,
        small.resume_nanos,
        small.pending_frames
    );
}

#[test]
fn every_resumption_costs_about_what_the_first_one_did() {
    let ms = multi_shot(3).unwrap();
    let one = ms
        .resumptions
        .iter()
        .find(|r| r.resumptions == 1)
        .expect("a one-resumption row");
    let four = ms
        .resumptions
        .iter()
        .find(|r| r.resumptions == 4)
        .expect("a four-resumption row");
    assert!(
        four.marginal_micros < one.micros * 2.0,
        "the fourth resumption cost {} us against {} us for a whole one-resumption call",
        four.marginal_micros,
        one.micros
    );
}

#[test]
fn scheduling_reports_the_group_count_the_shared_tests_alone_need() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("corpus");
    corpus_at(&root);

    let s = scheduling(&root).unwrap();
    assert_eq!(s.isolated + s.shared, s.tests);
    assert_eq!(s.groups, s.shared_groups.max(1));
}
