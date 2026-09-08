//! The observables that decide whether the region model preserved meaning.

use ply_eval::Machine;
use ply_span::{SourceId, SourceMap};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::parse_program;
use ply_syntax::resolve::{Resolved, resolve};

fn load(src: &str) -> (Program, Resolved) {
    let mut map = SourceMap::new();
    let id: SourceId = map.add("meaning.ply", src.to_string());
    let mut program = match parse_program([(id, ModuleName::from_dotted("meaning"), src)]) {
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

const STATE: &str = r#"
effect state {
  read  get[s]()        -> Int
  write put[s](v: Int)  -> Unit
}
"#;


/// The same handler in the **tail-resumptive** form, which is the shape every handler in the
/// standard library and the examples is written in.
#[test]
fn a_tail_resumptive_clause_write_is_visible_to_the_computation_it_resumes() {
    holds(&format!(
        r#"{STATE}
test "a tail-resumptive put is seen by the following get" {{
  with_cell[s](0) {{ c ->
    assert_eq(
      handle {{ state.put[s](5); state.get[s]() }} with {{
        state.get[s]() -> cell_get(c),
        state.put[s](v) -> cell_set(c, v),
        return x -> x
      }},
      5)
  }}
}}
"#
    ));
}








/// W5's collecting trace sink, reduced to its discriminating core.
#[test]
fn a_collecting_sink_accumulates_across_handler_boundaries() {
    holds(
        r#"
effect log {
  write note[c](name: String) -> Unit
  write open[c](name: String) -> Int
  write shut[c](id: Int)      -> Unit
}

fn work() -> Unit / {log.write[orders]} = {
  let span = log.open[orders]("place");
  log.note[orders]("counted");
  log.shut[orders](span)
}

test "a collecting handler accumulates every record it was handed" {
  with_cell[sink]([]) { s -> {
    handle { work() } with {
      log.open[orders](n) -> { cell_set(s, push(cell_get(s), n)); 1 },
      log.note[orders](n) -> cell_set(s, push(cell_get(s), n)),
      log.shut[orders](i) -> cell_set(s, push(cell_get(s), "closed")),
    };
    assert_eq(cell_get(s), ["place", "counted", "closed"])
  } }
}
"#,
    );
}

