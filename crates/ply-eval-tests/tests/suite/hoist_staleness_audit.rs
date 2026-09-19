use crate::fixture::Compiled;
use ply_eval::Value;
use ply_span::Span;

impl Compiled {
    #[track_caller]
    fn call(&self, name: &str) -> Value {
        let mut machine = self.machine();
        machine
            .call(name, Vec::new(), Span::DUMMY)
            .unwrap_or_else(|d| panic!("[{}] {}", d.code, d.message))
    }
}

#[track_caller]
fn int(value: Value) -> i64 {
    match value {
        Value::Int(i) => i,
        other => panic!("expected an Int, got {other:?}"),
    }
}

const TAIL_RESUMPTIVE: &str = "effect log { write note[tape](n: Int) -> Int }\n\nfn go() -> Int =\n  with_cell[tape](0) { c -> { let total = handle { log.note[tape](1) + log.note[tape](2) } with { log.note[tape](n) -> { cell_set(c, cell_get(c) * 10 + n); n } }; total + cell_get(c) * 1000 } }\n";

#[test]
fn a_tail_resumptive_handler_inside_a_cell_region_answers_honestly() {
    assert_eq!(int(Compiled::new(TAIL_RESUMPTIVE).call("m.go")), 12003);
}

#[test]
fn a_binding_a_closure_captured_is_not_moved_out_from_under_a_second_call() {
    let compiled = Compiled::new(
        r#"
fn go() -> Int {
  let xs = [1, 2, 3];
  let g = || len(push(xs, 4));
  g() + g() + len(xs)
}
"#,
    );
    assert_eq!(
        int(compiled.call("m.go")),
        4 + 4 + 3,
        "a binding a closure captured was moved out of the scope the closure shares"
    );
}
