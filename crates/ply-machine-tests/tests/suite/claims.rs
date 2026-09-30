//! The table that says where each type this side marshals is declared, held to the program it is
//! marshalled into. It lives here rather than in `ply-machine`'s own tests because its subject is
//! `crates/ply-cli/ply` -- a project the suite loads and the library does not.

use ply_machine::claims::MARSHALLED;
use std::path::PathBuf;

/// The CLI tree, whose package declares the marshalled types.
fn cli_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../ply-cli/ply")
}

/// Every row has to name a module that declares the type, and every case it builds.
///
/// The tag this side builds is `<module>.<constructor>` and the program matches it against the name
/// its own spine gives the constructor, so a type that moved module and left its row behind, or a
/// case spelled otherwise than the program declares it, is a value no arm matches. `replay`'s
/// `the_fixture_declares_the_payload_where_the_machine_names_it` is the same claim about the fixture
/// rather than about the program.
#[test]
fn every_marshalled_type_is_declared_where_this_side_says() {
    let loaded = ply_machine::load::load(&cli_root()).expect("the CLI tree loads");
    for (home, ty, cases) in MARSHALLED {
        assert!(
            declares(&loaded, home, ty),
            "`{ty}` is not declared in `{home}`, so a tag built from it names nothing the \
             program matches"
        );
        for case in *cases {
            let tag = format!("{home}.{case}");
            assert!(
                loaded
                    .front
                    .emitter_ctors
                    .iter()
                    .any(|(name, _)| name.as_str() == tag),
                "`{home}` declares no case `{case}`, so the claims family builds a value no arm \
                 matches"
            );
        }
    }
}

/// Whether `module` declares a type whose simple name is `ty`: a constructor of it is `module`'s.
fn declares(loaded: &ply_machine::load::Loaded, module: &str, ty: &str) -> bool {
    loaded
        .front
        .types
        .values()
        .any(|t| t.simple_name.as_str() == ty && t.module.as_str() == module)
}

/// The claims family names a refusal by the module that declares `prover`, so the program has to
/// declare its `Refusal` there too.
#[test]
fn the_refusal_is_declared_beside_the_effect_it_is_named_by() {
    let loaded = ply_machine::load::load(&cli_root()).expect("the CLI tree loads");
    let prover = loaded
        .check
        .effects
        .values()
        .find(|e| e.simple_name.as_str() == "prover")
        .expect("the CLI declares `prover`");
    assert!(
        declares(&loaded, prover.module.as_str(), "Refusal"),
        "`Refusal` is not declared in `{}`, where `prover` is, so a refusal names nothing the \
         program matches",
        prover.module
    );
}

/// Every case the tester builds has to be one the program declares, of the type and in the module
/// the tester says: a case name that names nothing is a placeless `no arm of this match matched`
/// the moment the program matches the value.
#[test]
fn every_case_the_tester_builds_is_declared_where_it_says() {
    let loaded = ply_machine::load::load(&cli_root()).expect("the CLI tree loads");
    for (home, ty, cases) in ply_machine::tester::MARSHALLED {
        assert!(
            declares(&loaded, home, ty),
            "`{home}` does not declare `{ty}`, so the tester builds a value no arm matches"
        );
        for case in *cases {
            let tag = format!("{home}.{case}");
            assert!(
                loaded
                    .front
                    .emitter_ctors
                    .iter()
                    .any(|(name, _)| name.as_str() == tag),
                "`{home}` declares no case `{case}`, so the tester builds a value no arm matches"
            );
        }
    }
}
