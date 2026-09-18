use crate::fixture::port_check;

/// A route out of a region that the escape brand says is closed, and is not.
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
    let check = port_check(&[("adversarial", src)]);
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
