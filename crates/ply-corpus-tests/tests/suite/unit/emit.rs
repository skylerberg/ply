use ply_corpus::build::generate;
use ply_corpus::emit::{
    ARG_BOUND, call_expr, emit_concurrent_test, emit_def, emit_law, emit_module, emit_specimen,
    emit_specimen_test, emit_task, escape, handler_clauses, wrap_body,
};
use ply_corpus::model::{Corpus, LawKind, TaskBody};
use ply_corpus::spec::CorpusSpec;

fn corpus() -> Corpus {
    generate(&CorpusSpec {
        seed: 3,
        modules: 6,
        defs_per_module: 10,
        tests: 20,
        depth: 3,
        ..CorpusSpec::default()
    })
}

#[test]
fn a_module_imports_effects_exactly_when_it_needs_the_binder() {
    let corpus = corpus();
    for module in &corpus.modules {
        let text = emit_module(&corpus, module);
        let mentions = text.contains("effects::");
        assert_eq!(
            text.contains("import core.effects"),
            mentions,
            "module {} imports and uses `effects` inconsistently",
            module.name
        );
    }
}

#[test]
fn a_reference_is_qualified_exactly_when_it_leaves_its_module() {
    let corpus = corpus();
    let mut crossed = 0;
    for def in &corpus.defs {
        for call in def
            .shape
            .calls()
            .into_iter()
            .chain(def.extras.iter().copied())
        {
            let expr = call_expr(&corpus, def.module, call, "x");
            let owner = corpus.defs[call.target].module;
            if owner == def.module {
                assert!(!expr.contains("::"), "{expr} qualifies a same-module call");
            } else {
                crossed += 1;
                let binder = corpus.modules[owner].binder();
                assert!(
                    expr.starts_with(&format!("{binder}::")),
                    "{expr} is not qualified"
                );
            }
        }
    }
    assert!(
        crossed > 0,
        "no call crossed a module, so nothing was checked"
    );
}

#[test]
fn a_definition_is_pub_in_the_source_exactly_when_the_model_says_so() {
    let corpus = corpus();
    for def in &corpus.defs {
        let text = emit_def(&corpus, def);
        assert_eq!(text.starts_with("pub fn "), def.public, "{}", def.name);
    }
}

#[test]
fn a_declared_row_lists_every_atom_the_definition_can_perform() {
    let corpus = corpus();
    for def in corpus.defs.iter().filter(|d| !d.footprint.is_empty()) {
        let text = emit_def(&corpus, def);
        for atom in &def.footprint {
            assert!(
                text.contains(&atom.render()),
                "{} omits {}",
                def.name,
                atom.render()
            );
        }
    }
}

#[test]
fn a_test_grants_one_clause_per_performed_atom_and_no_more() {
    let corpus = corpus();
    for test in &corpus.tests {
        let clauses = handler_clauses(&corpus, test, &test.granted);
        assert_eq!(clauses.len(), test.granted.len(), "test `{}`", test.label);
        assert!(
            test.granted.is_subset(&corpus.defs[test.root].footprint),
            "test `{}` grants an atom its root never declares",
            test.label
        );
    }
}

/// Handlers are granted only for what fires, so declaring more than a call path performs leaves atoms behind.
#[test]
fn some_tests_leave_a_declared_atom_ungranted() {
    let corpus = generate(&CorpusSpec {
        seed: 3,
        modules: 8,
        defs_per_module: 12,
        tests: 60,
        depth: 3,
        tables: 3,
        regions: 2,
        ..CorpusSpec::default()
    });
    let partial = corpus
        .tests
        .iter()
        .filter(|t| t.granted != corpus.defs[t.root].footprint)
        .count();
    assert!(partial > 0, "every test granted its root's whole footprint");
}

fn concurrent(density: f64) -> Corpus {
    generate(&CorpusSpec {
        seed: 4,
        modules: 4,
        defs_per_module: 6,
        tests: 8,
        depth: 2,
        concurrent_tests: 4,
        tasks_per_test: 3,
        steps_per_task: 2,
        conflict_density: density,
        ..CorpusSpec::default()
    })
}

#[test]
fn a_task_declares_exactly_the_one_shard_it_bumps() {
    let corpus = concurrent(0.5);
    assert!(!corpus.concurrent.is_empty());
    for test in &corpus.concurrent {
        for task in &test.tasks {
            let text = emit_task(&corpus, task);
            for (i, label) in corpus.shards.iter().enumerate() {
                let named = text.contains(&format!("[{label}]"));
                assert_eq!(named, i == task.shard, "{} and `{label}`", task.name);
            }
            assert_eq!(
                text.matches("effects::counter.bump").count(),
                task.steps.len()
            );
            assert_eq!(text.matches("task.yield()").count(), task.steps.len() - 1);
        }
    }
}

#[test]
fn a_concurrent_test_opens_one_cell_and_grants_one_clause_per_shard_it_uses() {
    let corpus = concurrent(0.5);
    for test in &corpus.concurrent {
        let text = emit_concurrent_test(&corpus, test);
        assert_eq!(text.matches("with_cell[").count(), test.shards.len());
        assert_eq!(text.matches("-> cell_set(").count(), test.shards.len());
        assert_eq!(text.matches("task.spawn(").count(), test.tasks.len());
        assert_eq!(text.matches("task.join(").count(), test.tasks.len());
        for &shard in &test.shards {
            assert!(
                text.contains(&format!(
                    "assert_eq(cell_get(c{shard}), {})",
                    test.shard_total(shard)
                )),
                "`{}` does not assert shard {shard}'s total\n{text}",
                test.label
            );
        }
    }
}

#[test]
fn the_asserted_totals_are_the_models_and_they_add_up() {
    for density in [0.0, 0.5, 1.0] {
        let corpus = concurrent(density);
        for test in &corpus.concurrent {
            let by_shard: i64 = test.shards.iter().map(|&s| test.shard_total(s)).sum();
            assert_eq!(by_shard, test.total(), "`{}`", test.label);
            assert!(test.total() > 0);
            assert!(
                emit_concurrent_test(&corpus, test).contains(&format!("), {});", test.total())),
                "`{}` does not assert what the tasks returned",
                test.label
            );
        }
    }
}

#[test]
fn a_task_body_is_the_same_at_every_density_but_for_the_shard_it_names() {
    let disjoint = concurrent(0.0);
    let contended = concurrent(1.0);
    let erase = |corpus: &Corpus, task: &TaskBody| {
        emit_task(corpus, task).replace(&corpus.shards[task.shard], "_")
    };
    let mut compared = 0;
    for (a, b) in disjoint.concurrent.iter().zip(&contended.concurrent) {
        for (x, y) in a.tasks.iter().zip(&b.tasks) {
            assert_eq!(erase(&disjoint, x), erase(&contended, y));
            compared += 1;
        }
        assert!(a.shards.len() > b.shards.len());
    }
    assert!(compared > 0, "no task was compared");
}

#[test]
fn a_label_with_a_quote_in_it_cannot_break_out_of_the_string() {
    assert_eq!(escape(r#"a "b" \c"#), r#"a \"b\" \\c"#);
}

fn specified() -> Corpus {
    generate(&CorpusSpec {
        seed: 3,
        modules: 4,
        defs_per_module: 8,
        tests: 10,
        depth: 2,
        spec_fraction: 0.6,
        specimens_per_module: 3,
        ..CorpusSpec::default()
    })
}

#[test]
fn a_specified_definition_carries_exactly_one_clause_of_each_kind() {
    let corpus = specified();
    let mut specified_count = 0;
    for def in &corpus.defs {
        let text = emit_def(&corpus, def);
        let expected = usize::from(def.claim.is_some());
        assert_eq!(
            text.matches("\n  requires ").count(),
            expected,
            "{}",
            def.name
        );
        assert_eq!(
            text.matches("\n  ensures ").count(),
            expected,
            "{}",
            def.name
        );
        if def.claim.is_some() {
            specified_count += 1;
            assert!(
                text.contains(&format!("x > 0 && x < {ARG_BOUND}")),
                "{} has no bound on its argument\n{text}",
                def.name
            );
            assert_eq!(
                text.contains("y > 0"),
                def.arity >= 2,
                "{} bounds the wrong parameters",
                def.name
            );
        }
    }
    assert!(specified_count > 0);
}

/// The benchmark's edit sites are textual, so a clause between header and body must not hide the body.
#[test]
fn a_clause_does_not_stop_a_one_line_body_being_rewritten() {
    let corpus = specified();
    let mut wrapped = 0;
    for def in corpus.defs.iter().filter(|d| d.shape.is_one_liner()) {
        let text = emit_def(&corpus, def);
        let rewritten = wrap_body(&text)
            .unwrap_or_else(|| panic!("`{}` is a one-liner but did not wrap:\n{text}", def.name));
        assert!(rewritten.contains("prim::clamp("));
        assert_eq!(
            rewritten.matches("\n  ensures ").count(),
            usize::from(def.claim.is_some()),
            "rewriting `{}` disturbed its claim",
            def.name
        );
        wrapped += 1;
    }
    assert!(wrapped > 0);
}

#[test]
fn a_specimen_states_its_claim_and_its_law_names_it() {
    let corpus = specified();
    assert!(!corpus.specimens.is_empty());
    for specimen in &corpus.specimens {
        let text = emit_specimen(&corpus, specimen);
        assert_eq!(text.matches("\n  ensures ").count(), 1, "{text}");
        assert!(
            text.starts_with(&format!("fn {}(", specimen.name)),
            "{text}"
        );
    }
    for law in &corpus.laws {
        let text = emit_law(&corpus, law);
        assert!(
            text.starts_with(&format!("law \"{}\"", law.label)),
            "{text}"
        );
        if let LawKind::Length { specimen } = law.kind {
            assert!(text.contains(&corpus.specimens[specimen].name), "{text}");
        }
    }
}

#[test]
fn every_specimen_is_asserted_by_the_test_its_module_carries() {
    let corpus = specified();
    for module in &corpus.modules {
        let text = emit_specimen_test(&corpus, module);
        for specimen in corpus.specimens_in(module.id) {
            assert!(
                text.contains(&format!("{}(", specimen.name)),
                "`{}` is never called by its module's test\n{text}",
                specimen.name
            );
        }
    }
}
