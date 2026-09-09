//! Adversarial probes for the one property the region model may not break.

use ply_core::check_program;
use ply_span::{SourceId, SourceMap};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::parse_program;
use ply_syntax::resolve::{Resolved, resolve};

fn load(src: &str) -> (Program, Resolved) {
    let mut map = SourceMap::new();
    let id: SourceId = map.add("adversarial.ply", src.to_string());
    let mut program = match parse_program([(id, ModuleName::from_dotted("adversarial"), src)]) {
        Ok(p) => p,
        Err(ds) => panic!("the probe must parse: {ds:#?}\n{src}"),
    };
    let resolved = resolve(&mut program).expect("the probe must resolve");
    (program, resolved)
}

/// **A route out of a region that the escape brand says is closed, and is not.**
#[test]
fn a_general_clause_inside_a_region_carries_that_regions_atoms_out_of_it() {
    let src = r#"
effect amb { read flip[coin]() -> Bool }

fn leaks(n: Int) -> Int =
  with_cell[t](n) { c ->
    handle { let b = amb.flip[coin](); cell_set(c, cell_get(c) + 1); cell_get(c) }
    with { amb.flip[coin]() resume k -> k(true), return x -> x }
  }

fn discharges(n: Int) -> Int =
  with_cell[t](n) { c ->
    handle { cell_set(c, cell_get(c) + 1); cell_get(c) }
    with { amb.flip[coin]() -> true, return x -> x }
  }

test "through a general clause" { assert_eq(leaks(1), 2) }
test "through a tail-resumptive one" { assert_eq(discharges(1), 2) }
"#;
    let (program, resolved) = load(src);
    let check = check_program(&program, &resolved).expect("the probe must typecheck");
    let atoms = |name: &str| -> Vec<String> {
        let at = check
            .tests
            .iter()
            .position(|t| t.name == name)
            .unwrap_or_else(|| panic!("no test named {name:?}"));
        check.tests[at]
            .footprint
            .atoms()
            .map(|a| a.to_string())
            .collect()
    };
    assert_eq!(
        atoms("through a general clause"),
        vec!["cell.read[t]".to_string(), "cell.write[t]".to_string()],
        "if this is ever empty, the escape brand's claim has become true and should be re-read \
         rather than this test deleted"
    );
    assert_eq!(
        atoms("through a tail-resumptive one"),
        Vec::<String>::new(),
        "the shape every handler in `examples/` is written in still discharges"
    );
}
