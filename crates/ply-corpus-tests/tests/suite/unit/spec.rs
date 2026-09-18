use ply_corpus::spec::CorpusSpec;

#[test]
fn the_default_spec_is_valid() {
    CorpusSpec::default().validate().unwrap();
}

#[test]
fn a_depth_deeper_than_the_module_count_is_rejected() {
    let spec = CorpusSpec {
        modules: 3,
        depth: 8,
        ..CorpusSpec::default()
    };
    assert!(
        spec.validate()
            .unwrap_err()
            .to_string()
            .contains("--depth 8")
    );
}

#[test]
fn density_walks_the_shard_count_from_one_each_to_one_between_all() {
    let spec = |d: f64| CorpusSpec {
        tasks_per_test: 4,
        conflict_density: d,
        ..CorpusSpec::default()
    };
    assert_eq!(spec(0.0).shards_per_test(), 4);
    assert_eq!(spec(1.0).shards_per_test(), 1);
    assert_eq!(spec(0.5).shards_per_test(), 3);
    assert_eq!(
        CorpusSpec {
            tasks_per_test: 2,
            conflict_density: 0.4,
            ..CorpusSpec::default()
        }
        .shards_per_test(),
        2
    );
}

#[test]
fn a_concurrent_test_needs_two_tasks_and_a_step() {
    let spec = CorpusSpec {
        concurrent_tests: 4,
        tasks_per_test: 1,
        ..CorpusSpec::default()
    };
    assert!(
        spec.validate()
            .unwrap_err()
            .to_string()
            .contains("nothing to interleave")
    );
    let spec = CorpusSpec {
        concurrent_tests: 4,
        steps_per_task: 0,
        ..CorpusSpec::default()
    };
    assert!(spec.validate().is_err());
    assert!(
        CorpusSpec {
            concurrent_tests: 0,
            tasks_per_test: 1,
            ..CorpusSpec::default()
        }
        .validate()
        .is_ok(),
        "a corpus with no concurrent tests is not constrained by their shape"
    );
}

#[test]
fn a_spec_written_before_m8_deserializes_to_a_corpus_with_no_obligations() {
    let spec: CorpusSpec = serde_json::from_str(
        r#"{"seed":1,"modules":2,"defs_per_module":3,"tests":1,"depth":1,
            "tables":2,"regions":1,"effect_fraction":0.3,"nondet_fraction":0.0,
            "hub_modules":1,"max_weight":64}"#,
    )
    .unwrap();
    assert_eq!(spec.spec_fraction, 0.0);
    assert_eq!(spec.specimens_per_module, 0);
    spec.validate().unwrap();
}

#[test]
fn out_of_range_fractions_are_rejected() {
    let spec = CorpusSpec {
        effect_fraction: 1.5,
        ..CorpusSpec::default()
    };
    assert!(spec.validate().is_err());
    let spec = CorpusSpec {
        nondet_fraction: -0.1,
        ..CorpusSpec::default()
    };
    assert!(spec.validate().is_err());
    let spec = CorpusSpec {
        conflict_density: 1.5,
        ..CorpusSpec::default()
    };
    assert!(spec.validate().is_err());
    let spec = CorpusSpec {
        spec_fraction: 1.2,
        ..CorpusSpec::default()
    };
    assert!(
        spec.validate()
            .unwrap_err()
            .to_string()
            .contains("--spec-fraction")
    );
}
