/// The registry narrowing, which is measurement scaffolding and is therefore the easiest thing in
/// this file to get quietly wrong.
use ply_eval::backend::*;
use ply_span::SourceId;
use ply_syntax::ast::ModuleName;
use ply_syntax::ast::Program;
use ply_syntax::resolve::Resolved;
use ply_syntax::resolve::resolve;
use ply_ty::CheckOutput;

fn checked(text: &str) -> (Program, Resolved, CheckOutput) {
    let inputs = vec![(SourceId(0), ModuleName::from_dotted("m"), text)];
    let mut program = ply_syntax::parse_program(inputs).expect("parses");
    let resolved = resolve(&mut program).expect("resolves");
    let check = ply_core::check_program(&program, &resolved).expect("typechecks");
    (program, resolved, check)
}

const SRC: &str = "
fn one(n: Int) -> Int = n + 1
fn two(n: Int) -> Int = n + 2
fn three(n: Int) -> Int = n + 3
";

/// Delete the `only.is_none_or(..)` filter in [`registry`] and this reads three against three:
/// the narrowed run is the unnarrowed one under a different label, which is exactly the reading
/// a time series would then attribute to the narrowing.
#[test]
fn a_narrowed_registry_holds_only_the_names_it_was_given() {
    let (_program, _resolved, check) = checked(SRC);
    let types = ply_eval::compiled::CarriedTypes::over(Some(&check));
    let whole = registry(&check, &types, None);
    assert_eq!(whole.len(), 3, "the unnarrowed registry is the control");

    let only = names_in("m.one, m.three");
    let narrowed = registry(&check, &types, Some(&only));
    let held: Vec<&str> = narrowed.iter().map(|n| n.as_str()).collect();
    assert_eq!(held, ["m.one", "m.three"]);

    // It intersects rather than replaces: a name that is not in the fragment to begin with is
    // not added by asking for it.
    let wishful = names_in("m.one,m.nonesuch");
    let narrowed = registry(&check, &types, Some(&wishful));
    let held: Vec<&str> = narrowed.iter().map(|n| n.as_str()).collect();
    assert_eq!(held, ["m.one"]);
}

/// Drop the `filter(|name| !name.is_empty())` and `"m.one,"` asks for a definition named by the
/// empty string, which no program has — silently a different experiment from `"m.one"`.
#[test]
fn a_trailing_comma_is_the_same_experiment_as_no_trailing_comma() {
    assert_eq!(names_in("m.one,"), names_in("m.one"));
    assert_eq!(names_in(" m.one , m.two "), names_in("m.one,m.two"));
    assert!(names_in("").is_empty());
    assert!(names_in(",,").is_empty());
}
