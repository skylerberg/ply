//! What a host may lend a program, by name.

use ply_privileged::policy;

/// Where the `ply` binary's own program declares each family's effect.
fn own(effect: &str) -> String {
    match effect {
        "prover" => "claims",
        "shipped" => "compiler.unit",
        other => other,
    }
    .to_string()
}

#[test]
fn every_family_lends_something_and_has_a_name_and_a_summary() {
    for family in policy::FAMILIES {
        assert!(
            !family.name.is_empty() && !family.summary.is_empty(),
            "a family needs a name and a summary a reviewer can read"
        );
        let ops = policy::lent(family.name, &own)
            .unwrap_or_else(|| panic!("`{}` is listed and lends nothing", family.name));
        assert!(
            !ops.is_empty(),
            "`{}` lends no operation, so lending it means nothing",
            family.name
        );
    }
    let mut names = policy::names();
    names.sort_unstable();
    let mut unique = names.clone();
    unique.dedup();
    assert_eq!(names, unique, "a family is listed twice");
}

/// Only a family's hermetic half lends deterministic operations.
#[test]
fn a_family_lends_under_the_effect_a_program_declares_and_its_hermetic_half() {
    for family in policy::FAMILIES {
        let effects = policy::effects_of(family.name)
            .unwrap_or_else(|| panic!("`{}` lends under no effect", family.name));
        let hermetic = policy::hermetic_of(family.name);
        let ops = policy::lent(family.name, &own).expect("the family is listed");
        for (op, _) in &ops {
            let on_half = Some(op.effect.as_str()) == hermetic;
            assert!(
                effects.contains(&op.effect.as_str()) || on_half,
                "`{}` lends `{}.{}`, of an effect it does not name",
                family.name,
                op.effect,
                op.op
            );
            if on_half {
                assert!(
                    op.determinism.is_deterministic(),
                    "`{}.{}` is the hermetic half's and answers nondeterministically",
                    op.effect,
                    op.op
                );
            }
        }
        if let Some(half) = hermetic {
            assert!(
                ops.iter().any(|(op, _)| op.effect.as_str() == half),
                "`{}` names a hermetic half `{half}` and lends nothing of it",
                family.name
            );
        }
    }
    // A family whose effect is not named for it, and one that lends two: a grant is checked
    // against the effects.
    assert_eq!(policy::effects_of("claims"), Some(&["prover"][..]));
    assert_eq!(
        policy::effects_of("toolchain"),
        Some(&["compilers", "toolchain", "memory"][..])
    );
    assert_eq!(policy::effects_of("cache"), None);
    assert_eq!(policy::effects_of("everything"), None);
}

#[test]
fn lending_every_family_lends_what_a_native_program_is_lent() {
    let all = policy::all();
    let summed: usize = policy::FAMILIES
        .iter()
        .map(|f| policy::lent(f.name, &own).unwrap().len())
        .sum();
    assert_eq!(all.len(), summed, "`all()` does not lend every family once");
}

#[test]
fn a_family_that_does_not_exist_is_refused_by_name() {
    let err = match policy::lent_for(&["machine", "everything"], &own) {
        Ok(_) => panic!("an unknown family is refused"),
        Err(why) => why,
    };
    assert!(err.contains("`everything`"), "{err}");
    assert!(
        err.contains("machine"),
        "the refusal names the families there are: {err}"
    );
}

#[test]
fn a_familys_values_are_named_by_the_module_the_program_declares_it_in() {
    // These families build values of types their program declares; lending one to a consumer that
    // imports the CLI's modules is the same operations, named as it names them.
    let consumer = |effect: &str| format!("cli.{}", own(effect));
    for family in ["claims", "hosts"] {
        let as_cli = policy::lent(family, &own).expect("the family is there");
        let as_consumer = policy::lent(family, &consumer).expect("the family is there");
        assert_eq!(
            as_cli.len(),
            as_consumer.len(),
            "the same operations, named differently"
        );
    }
}

/// A load answers what the machine read and nothing a program's module would have to name.
#[test]
fn a_load_answers_a_record_of_what_was_read() {
    let found = ply_privileged::drive::FoundData {
        root: "app".to_string(),
        files: vec!["app/m.ply".to_string()],
        places: vec![("app/m.ply".to_string(), b"fn main() -> Int = 1\n".to_vec())],
    };
    match &ply_privileged::drive::found_value(&found) {
        ply_core::Value::Record(fields) => {
            let names: Vec<&str> = fields.keys().map(|name| name.as_str()).collect();
            assert_eq!(names, ["files", "places", "root"]);
        }
        other => panic!("a load answers a record, not {}", other.type_name()),
    }
}
