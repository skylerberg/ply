//! What a host may lend a program, by name.

use ply_machine::policy;

/// Where the `ply` binary's own program declares each family's effect.
fn own(effect: &str) -> String {
    match effect {
        "prover" => "claims".to_string(),
        other => other.to_string(),
    }
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
/// names has to be the one its operations are on.
#[test]
fn a_family_lends_the_operations_of_the_effect_it_names_and_no_other() {
    for family in policy::FAMILIES {
        let ops = policy::lent(family.name, &own).expect("the family is listed");
        for (op, _) in &ops {
            assert_eq!(
                op.effect.as_str(),
                family.effect,
                "`{}` lends `{}.{}`, of an effect it does not name",
                family.name,
                op.effect,
                op.op
            );
        }
    }
}

fn check_of(module: &str, source: &str) -> ply_ty::CheckOutput {
    ply_codegen::c::producer::ensure_default();
    ply_codegen::c::producer::checked_front(
        &[(module.to_string(), source.to_string())],
        &[ply_span::SourceId(0)],
    )
    .expect("the fixture checks")
    .check
}

#[test]
fn a_grant_needs_the_effect_the_family_lends_rather_than_one_of_its_name() {
    let consumer = check_of(
        "cli.claims",
        "pub nondet effect prover {\n  read ask[r]() -> Int\n}\n",
    );
    let lent = policy::granted(&consumer, &["claims".to_string()])
        .unwrap_or_else(|d| panic!("the program declares `prover`: {}", d.message));
    assert!(!lent.is_empty());

    let misnamed = check_of(
        "m",
        "pub nondet effect claims {\n  read ask[r]() -> Int\n}\n",
    );
    let refused = match policy::granted(&misnamed, &["claims".to_string()]) {
        Ok(_) => panic!("a program that declares no `prover` was lent the family"),
        Err(d) => d,
    };
    assert_eq!(refused.code, ply_span::codes::CAPABILITY_UNDECLARED);
    assert!(
        refused.message.contains("`prover`"),
        "the refusal names the effect to declare: {}",
        refused.message
    );
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
    // The machine and claims families build values of types their program declares; lending one
    // to a consumer that imports the CLI's module is the same operations, named as it names them.
    let consumer = |effect: &str| format!("cli.{}", own(effect));
    for family in ["machine", "claims"] {
        let as_cli = policy::lent(family, &own).expect("the family is there");
        let as_consumer = policy::lent(family, &consumer).expect("the family is there");
        assert_eq!(
            as_cli.len(),
            as_consumer.len(),
            "the same operations, named differently"
        );
    }
}
