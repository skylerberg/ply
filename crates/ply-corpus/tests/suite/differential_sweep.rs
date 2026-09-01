//! One seed is one call graph, and a divergence that depends on the shape of a
//! call graph is invisible to a single generated corpus.

use ply_corpus::{CorpusSpec, build::generate, pipeline, write};
use ply_eval::differential::compare_tests;
use ply_eval::{Fixture, Interp, Machine};

#[test]
fn the_two_engines_agree_over_a_sweep_of_generated_corpora() {
    let mut compared = 0;
    let mut atoms = 0;
    for seed in 1..=12u64 {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("corpus");
        let spec = CorpusSpec {
            seed,
            modules: 10,
            defs_per_module: 14,
            tests: 60,
            depth: 3,
            ..CorpusSpec::default()
        };
        write::write(&root, &spec, &generate(&spec)).unwrap();

        let front = pipeline::front(&root).unwrap();
        let mut treewalk = Interp::for_program(&front.program, &front.resolved);
        let mut machine = Machine::for_program(&front.program, &front.resolved);
        let report = compare_tests(&mut treewalk, &mut machine, &Fixture::empty());
        assert!(report.is_clean(), "seed {seed}\n{report}");
        // A sweep that compared no footprints agrees on two axes of three, and
        // a generated corpus is where the third one earns its keep.
        assert_eq!(report.footprints_compared, report.compared, "seed {seed}");
        compared += report.compared;
        atoms += machine.trace().performs();
    }
    assert_eq!(compared, 12 * 60);
    assert!(atoms > 0, "no generated corpus performed an effect");
}
