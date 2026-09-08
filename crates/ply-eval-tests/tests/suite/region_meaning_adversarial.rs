//! Adversarial probes for the one property the region model may not break.

use ply_core::check_program;
use ply_eval::Machine;
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

/// Runs every test in a probe and requires all of them to pass.
#[track_caller]
fn holds(src: &str) {
    let (program, resolved) = load(src);
    let mut machine = Machine::for_program(&program, &resolved);
    let count = machine.test_count();
    assert!(count > 0, "this probe declares no test\n{src}");
    for i in 0..count {
        if let Err(d) = machine.eval_test(i) {
            panic!(
                "probe {i} (`{}`) must pass: [{}] {}\n{src}",
                machine.test_name(i).unwrap_or("?"),
                d.code,
                d.message
            );
        }
    }
}
















/// Evaluation order, recorded rather than inferred.
#[test]
fn regions_do_not_move_relative_to_the_effects_around_them() {
    holds(
        r#"
effect note { write at[j](what: String) -> Unit }

fn mark(what: String) -> Int / {note.write[j]} = { note.at[j](what); 0 }

fn both(a: Int, b: Int) -> Int = a + b

test "a region's open and close sit where they always did" {
  with_cell[journal]([]) { j ->
    handle {
      let outer = with_cell[a](mark("init-a")) { c -> {
        mark("body-a");
        both(
          with_cell[b](mark("init-b")) { d -> mark("body-b") },
          with_cell[e](mark("init-e")) { f -> mark("body-e") })
      } };
      assert_eq(outer, 0);
      let nested = with_cell[g](with_cell[h](mark("init-h")) { i -> mark("body-h") }) { k ->
        mark("body-g")
      };
      assert_eq(nested, 0);
      assert_eq(cell_get(j), [
        "init-a", "body-a", "init-b", "body-b", "init-e", "body-e",
        "init-h", "body-h", "body-g"])
    } with {
      note.at[j](w) -> cell_set(j, push(cell_get(j), w)),
    }
  }
}
"#,
    );
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



/// A tail-resumptive clause writing the cell of the region that encloses its own
/// handler — the shape the tail-resumptive refinement moved from `shared` to `unique`.
#[test]
fn a_tail_resumptive_clause_writing_its_own_region_still_threads() {
    holds(
        r#"
effect amb { read flip[coin]() -> Bool }

fn twice() -> Int =
  with_cell[trace](0) { c ->
    handle { { let a = amb.flip[coin](); let b = amb.flip[coin](); cell_get(c) } } with {
      amb.flip[coin]() -> { cell_set(c, cell_get(c) + 1); true },
      return x -> x } }

fn nested() -> Int =
  with_cell[outer](0) { o ->
    with_cell[inner](0) { i ->
      handle { { let a = amb.flip[coin](); cell_get(o) * 10 + cell_get(i) } } with {
        amb.flip[coin]() -> { cell_set(o, 7); cell_set(i, 3); true },
        return x -> x } } }

test "the write is threaded through both resumptions" { assert(twice() == 2, None) }
test "a nested region under the same clause is threaded too" { assert(nested() == 73, None) }
"#,
    );
}
