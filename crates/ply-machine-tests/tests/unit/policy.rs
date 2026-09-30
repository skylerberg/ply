//! What a host may lend a program, by name.

use ply_machine::policy;

#[test]
fn every_family_lends_something_and_has_a_name_and_a_summary() {
    for family in policy::FAMILIES {
        assert!(
            !family.name.is_empty() && !family.summary.is_empty(),
            "a family needs a name and a summary a reviewer can read"
        );
        let ops = policy::lent(family.name, "machine")
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

#[test]
fn lending_every_family_lends_what_the_launcher_lends() {
    let all = policy::all();
    let summed: usize = policy::FAMILIES
        .iter()
        .map(|f| policy::lent(f.name, "machine").unwrap().len())
        .sum();
    assert_eq!(all.len(), summed, "`all()` does not lend every family once");
}

#[test]
fn a_family_that_does_not_exist_is_refused_by_name() {
    let err = match policy::lent_for(&["machine", "everything"], "machine") {
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
fn a_family_lends_under_one_effect_and_that_is_the_one_a_program_declares() {
    for family in policy::FAMILIES {
        let effect = policy::effect_of(family.name)
            .unwrap_or_else(|| panic!("`{}` lends under no effect", family.name));
        let ops = policy::lent(family.name, "machine").expect("the family is there");
        assert!(
            ops.iter().all(|(op, _)| op.effect.as_str() == effect),
            "`{}` lends under more than one effect",
            family.name
        );
    }
    // The two whose effect is not named for the family: a grant is checked against the effect.
    assert_eq!(policy::effect_of("claims").as_deref(), Some("prover"));
    assert_eq!(policy::effect_of("cache").as_deref(), Some("store"));
    assert_eq!(policy::effect_of("everything"), None);
}

#[test]
fn the_machine_module_is_the_one_the_program_declares() {
    // The machine family is the one whose values carry their module's name; lending it twice
    // under different names is two different sets of constructors.
    let as_cli = policy::lent("machine", "machine").expect("the family is there");
    let as_consumer = policy::lent("machine", "cli.machine").expect("the family is there");
    assert_eq!(
        as_cli.len(),
        as_consumer.len(),
        "the same operations, named differently"
    );
}
