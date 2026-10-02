//! What a host may lend a program, by name.

use ply_machine::policy;

/// Where the `ply` binary's own program declares each family's effect.
fn own(effect: &str) -> String {
    match effect {
        "prover" => "claims",
        "store" => "cache",
        "archive" => "bootstrap",
        "tcb" => "hosts",
        "edit" => "replace",
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

/// `--allow` refuses a family whose effect the program does not declare, so the effect a family
/// names has to be the one every one of its operations is on.
#[test]
fn a_family_lends_under_one_effect_and_that_is_the_one_a_program_declares() {
    for family in policy::FAMILIES {
        let effect = policy::effect_of(family.name)
            .unwrap_or_else(|| panic!("`{}` lends under no effect", family.name));
        let ops = policy::lent(family.name, &own).expect("the family is listed");
        for (op, _) in &ops {
            assert_eq!(
                op.effect.as_str(),
                effect,
                "`{}` lends `{}.{}`, of an effect it does not name",
                family.name,
                op.effect,
                op.op
            );
        }
    }
    // The one whose effect is not named for the family: a grant is checked against the effect.
    assert_eq!(policy::effect_of("claims"), Some("prover"));
    assert_eq!(policy::effect_of("cache"), None);
    assert_eq!(policy::effect_of("everything"), None);
}

#[test]
fn lending_every_family_lends_what_the_launcher_lends() {
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
    for family in ["machine", "claims", "cache", "hosts"] {
        let as_cli = policy::lent(family, &own).expect("the family is there");
        let as_consumer = policy::lent(family, &consumer).expect("the family is there");
        assert_eq!(
            as_cli.len(),
            as_consumer.len(),
            "the same operations, named differently"
        );
    }
    let found = ply_machine::drive::FoundData::Project {
        root: String::new(),
        files: Vec::new(),
        places: Vec::new(),
        mains: Vec::new(),
        modules: Vec::new(),
    };
    for module in ["machine", "cli.machine"] {
        match &ply_machine::drive::found_value(&found, module) {
            ply_eval::Value::Ctor { name, .. } => {
                assert_eq!(name.as_str(), format!("{module}.Project"))
            }
            other => panic!("a load answers a `Target`, not {}", other.type_name()),
        }
    }
}
