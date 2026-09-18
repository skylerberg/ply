use ply_corpus::build::generate;
use ply_corpus::model::*;
use ply_corpus::spec::CorpusSpec;
use std::collections::BTreeSet;

fn small() -> CorpusSpec {
    CorpusSpec {
        seed: 5,
        modules: 8,
        defs_per_module: 12,
        tests: 24,
        depth: 3,
        ..CorpusSpec::default()
    }
}

fn concurrent(density: f64) -> CorpusSpec {
    CorpusSpec {
        concurrent_tests: 6,
        tasks_per_test: 4,
        steps_per_task: 3,
        conflict_density: density,
        ..small()
    }
}

#[test]
fn no_concurrency_is_asked_for_by_default_and_none_is_generated() {
    assert_eq!(CorpusSpec::default().concurrent_tests, 0);
    assert!(generate(&small()).concurrent.is_empty());
}

#[test]
fn density_zero_gives_every_task_its_own_shard_and_density_one_gives_them_all_the_same() {
    let disjoint = generate(&concurrent(0.0));
    for test in &disjoint.concurrent {
        let shards: BTreeSet<usize> = test.tasks.iter().map(|t| t.shard).collect();
        assert_eq!(shards.len(), test.tasks.len(), "`{}`", test.label);
        assert_eq!(test.contention(), 0.0);
    }

    let contended = generate(&concurrent(1.0));
    for test in &contended.concurrent {
        let shards: BTreeSet<usize> = test.tasks.iter().map(|t| t.shard).collect();
        assert_eq!(shards.len(), 1, "`{}`", test.label);
        assert_eq!(test.contention(), 1.0);
    }
}

#[test]
fn contention_rises_with_the_density_asked_for() {
    let mean = |d: f64| {
        let corpus = generate(&concurrent(d));
        corpus
            .concurrent
            .iter()
            .map(|t| t.contention())
            .sum::<f64>()
            / corpus.concurrent.len() as f64
    };
    let (low, mid, high) = (mean(0.0), mean(0.5), mean(1.0));
    assert!(low < mid && mid < high, "{low} {mid} {high}");
}

#[test]
fn density_changes_the_shard_assignment_and_nothing_else() {
    let disjoint = generate(&concurrent(0.0));
    let contended = generate(&concurrent(1.0));
    assert_eq!(disjoint.concurrent.len(), contended.concurrent.len());
    for (a, b) in disjoint.concurrent.iter().zip(&contended.concurrent) {
        assert_eq!(a.module, b.module);
        assert_eq!(a.total(), b.total());
        for (x, y) in a.tasks.iter().zip(&b.tasks) {
            assert_eq!(x.name, y.name);
            assert_eq!(x.steps, y.steps);
        }
    }
}

#[test]
fn a_module_hosting_a_concurrent_test_imports_the_effect_it_performs() {
    let corpus = generate(&concurrent(0.5));
    assert!(!corpus.concurrent.is_empty());
    for test in &corpus.concurrent {
        assert!(
            corpus.modules[test.module].needs_effects,
            "module {} hosts `{}` and would not import `core.effects`",
            corpus.modules[test.module].name, test.label
        );
    }
}

#[test]
fn a_task_body_is_named_apart_from_every_generated_definition() {
    let corpus = generate(&concurrent(0.5));
    let defs: BTreeSet<&str> = corpus.defs.iter().map(|d| d.name.as_str()).collect();
    let mut workers = BTreeSet::new();
    for test in &corpus.concurrent {
        for task in &test.tasks {
            assert!(!defs.contains(task.name.as_str()), "{}", task.name);
            assert!(workers.insert(task.name.clone()), "{}", task.name);
        }
    }
    assert_eq!(workers.len(), 6 * 4);
}

#[test]
fn the_same_seed_produces_the_same_concurrent_tests() {
    let a = generate(&concurrent(0.5));
    let b = generate(&concurrent(0.5));
    for (x, y) in a.concurrent.iter().zip(&b.concurrent) {
        assert_eq!(x.label, y.label);
        assert_eq!(x.total(), y.total());
        assert_eq!(
            x.tasks.iter().map(|t| t.steps.clone()).collect::<Vec<_>>(),
            y.tasks.iter().map(|t| t.steps.clone()).collect::<Vec<_>>()
        );
    }
}

#[test]
fn the_same_seed_produces_the_same_corpus() {
    let a = generate(&small());
    let b = generate(&small());
    assert_eq!(a.defs.len(), b.defs.len());
    for (x, y) in a.defs.iter().zip(&b.defs) {
        assert_eq!(x.name, y.name);
        assert_eq!(format!("{:?}", x.shape), format!("{:?}", y.shape));
        assert_eq!(x.footprint, y.footprint);
    }
    for (x, y) in a.tests.iter().zip(&b.tests) {
        assert_eq!(x.label, y.label);
        assert_eq!(x.expected, y.expected);
    }
}

#[test]
fn a_different_seed_produces_a_different_corpus() {
    let a = generate(&small());
    let b = generate(&CorpusSpec { seed: 6, ..small() });
    let same = a
        .defs
        .iter()
        .zip(&b.defs)
        .filter(|(x, y)| format!("{:?}", x.shape) == format!("{:?}", y.shape))
        .count();
    assert!(
        same < a.defs.len(),
        "seed 6 reproduced every shape of seed 5"
    );
}

#[test]
fn every_call_points_backwards_so_the_graph_is_acyclic() {
    let corpus = generate(&small());
    for def in &corpus.defs {
        for call in def
            .shape
            .calls()
            .into_iter()
            .chain(def.extras.iter().copied())
        {
            assert!(call.target < def.id, "{} calls forward", def.name);
        }
    }
}

#[test]
fn a_module_only_imports_from_a_lower_layer() {
    let corpus = generate(&small());
    for module in &corpus.modules {
        for &imported in &module.imports {
            assert!(corpus.modules[imported].layer < module.layer);
        }
    }
}

#[test]
fn a_footprint_contains_everything_its_callees_perform() {
    let corpus = generate(&small());
    for def in &corpus.defs {
        for call in def
            .shape
            .calls()
            .into_iter()
            .chain(def.extras.iter().copied())
        {
            for atom in &corpus.defs[call.target].footprint {
                assert!(
                    def.footprint.contains(atom),
                    "{} lost {} from {}",
                    def.name,
                    atom.render(),
                    corpus.defs[call.target].name
                );
            }
        }
    }
}

#[test]
fn no_definition_exceeds_the_weight_cap() {
    let spec = small();
    let corpus = generate(&spec);
    for def in &corpus.defs {
        assert!(
            def.weight <= spec.max_weight,
            "{} weighs {}",
            def.name,
            def.weight
        );
    }
}

#[test]
fn a_definition_is_public_exactly_when_another_module_calls_it() {
    let corpus = generate(&small());
    let mut reached = vec![false; corpus.defs.len()];
    for def in &corpus.defs {
        for call in def
            .shape
            .calls()
            .into_iter()
            .chain(def.extras.iter().copied())
        {
            if corpus.defs[call.target].module != def.module {
                reached[call.target] = true;
            }
        }
    }
    for (def, expected) in corpus.defs.iter().zip(reached) {
        assert_eq!(
            def.public, expected,
            "{} has the wrong visibility",
            def.name
        );
    }
}

fn specified(fraction: f64, specimens: usize) -> CorpusSpec {
    CorpusSpec {
        spec_fraction: fraction,
        specimens_per_module: specimens,
        ..small()
    }
}

#[test]
fn no_obligations_are_asked_for_by_default_and_none_are_generated() {
    let corpus = generate(&small());
    assert!(corpus.defs.iter().all(|d| d.claim.is_none()));
    assert!(corpus.specimens.is_empty());
    assert!(corpus.laws.is_empty());
    assert_eq!(corpus.obligations_by_intent(), [0, 0, 0]);
}

#[test]
fn the_density_asked_for_is_roughly_the_density_generated() {
    for fraction in [0.25, 0.5, 1.0] {
        let corpus = generate(&specified(fraction, 0));
        let share = corpus.specified_defs() as f64 / corpus.defs.len() as f64;
        assert!(
            (share - fraction).abs() < 0.15,
            "asked {fraction}, got {share}"
        );
    }
    assert_eq!(generate(&specified(0.0, 0)).specified_defs(), 0);
}

#[test]
fn raising_the_density_only_ever_adds_claims() {
    let thin = generate(&specified(0.3, 0));
    let thick = generate(&specified(0.9, 0));
    for (lean, full) in thin.defs.iter().zip(&thick.defs) {
        assert!(
            lean.claim.is_none() || full.claim.is_some(),
            "`{}` lost its claim when the density rose",
            lean.name
        );
    }
    assert!(thick.specified_defs() > thin.specified_defs());
}

/// Nothing hands `ply prove` a handler, so an obligation on an effectful definition cannot be attempted.
#[test]
fn a_claim_on_an_effectful_definition_is_built_as_a_gap() {
    let corpus = generate(&specified(1.0, 0));
    let mut gaps = 0;
    for def in &corpus.defs {
        let claim = def.claim.expect("every definition is specified at 1.0");
        let expected = if def.footprint.is_empty() {
            Intent::Sampled
        } else {
            gaps += 1;
            Intent::Gap
        };
        assert_eq!(claim.intent, expected, "{}", def.name);
    }
    assert!(gaps > 0, "no effectful definition carried a claim");
}

#[test]
fn specimens_span_the_tiers_and_do_not_move_with_the_seed() {
    let corpus = generate(&specified(0.0, 3));
    assert_eq!(corpus.specimens.len(), 3 * corpus.modules.len());
    assert_eq!(corpus.laws.len(), corpus.specimens.len());

    let [decided, sampled, gaps] = corpus.obligations_by_intent();
    assert_eq!(gaps, 0, "a specimen is pure, so none of them is a gap");
    assert!(
        decided > 0 && sampled > 0,
        "{decided} decided, {sampled} sampled"
    );

    let other = generate(&CorpusSpec {
        seed: 99,
        ..specified(0.0, 3)
    });
    assert_eq!(
        other.obligations_by_intent(),
        corpus.obligations_by_intent()
    );
}

#[test]
fn a_law_reports_the_intent_of_its_kind_and_names_a_specimen_that_exists() {
    let corpus = generate(&specified(0.0, 3));
    for law in &corpus.laws {
        match law.kind {
            LawKind::Length { specimen } => {
                let named = &corpus.specimens[specimen];
                assert_eq!(named.module, law.module);
                assert!(matches!(named.kind, SpecimenKind::Length));
                assert_eq!(law.kind.intent(), Intent::Sampled);
            }
            _ => assert_eq!(law.kind.intent(), Intent::Decided),
        }
    }
}

#[test]
fn every_specimen_and_law_is_named_apart_from_everything_else() {
    let corpus = generate(&specified(0.5, 4));
    let mut names: BTreeSet<&str> = corpus.defs.iter().map(|d| d.name.as_str()).collect();
    for specimen in &corpus.specimens {
        assert!(names.insert(&specimen.name), "{}", specimen.name);
    }
    let mut labels: BTreeSet<(ModuleId, &str)> = BTreeSet::new();
    for law in &corpus.laws {
        assert!(
            labels.insert((law.module, law.label.as_str())),
            "two laws in one module share `{}`",
            law.label
        );
    }
}

#[test]
fn the_conflict_graph_is_not_all_pure_and_not_one_clique() {
    let corpus = generate(&CorpusSpec {
        tests: 60,
        ..small()
    });
    let effectful = corpus
        .tests
        .iter()
        .filter(|t| !corpus.defs[t.root].footprint.is_empty());
    assert!(
        effectful.count() > 5,
        "a corpus with no effectful tests proves nothing"
    );

    let writers: Vec<&Test> = corpus
        .tests
        .iter()
        .filter(|t| corpus.defs[t.root].footprint.iter().any(|a| a.write))
        .collect();
    assert!(writers.len() > 2, "no test writes anything");
    let distinct: std::collections::BTreeSet<_> = writers
        .iter()
        .flat_map(|t| corpus.defs[t.root].footprint.iter().filter(|a| a.write))
        .map(|a| a.resource.clone())
        .collect();
    assert!(distinct.len() > 1, "every writer touches the same resource");
}
