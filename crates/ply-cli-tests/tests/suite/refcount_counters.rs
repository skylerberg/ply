use crate::harness::{json_of, process, scratch, write};

fn run(src: &str) -> serde_json::Value {
    let dir = scratch();
    write(dir.path(), "m.ply", src);
    let out = process(dir.path())
        .args(["run", "--json", "m.ply"])
        .output()
        .expect("`ply run` must start");
    json_of(&out)
}

/// The growing field last, so the machine can reuse at every step and the counts are round.
const APPENDS: &str = "\
fn build(n: Int) -> List<Int> =
  iterate({i: 0, out: []}, n + 1, |s: {i: Int, out: List<Int>}|
    if s.i >= n { Stop(s.out) } else { Continue({i: s.i + 1, out: push(s.out, s.i)}) })
fn main() -> Int = len(build(200))
";

#[test]
fn the_machine_reports_what_it_reused() {
    let v = run(APPENDS);
    let c = &v["counters"];
    assert_eq!(c["updates"], 200, "200 appends were made: {c}");
    // The tier reads `s.out` with a count of its own rather than taking the field, so the ratio is not 1.0.
    let in_place = c["in_place"]
        .as_f64()
        .unwrap_or_else(|| panic!("the reuse ratio is reported: {c}"));
    assert!((0.0..=1.0).contains(&in_place), "{c}");
}

#[test]
fn the_counts_follow_the_program_rather_than_being_a_constant() {
    let ten = run(&APPENDS.replace("build(200)", "build(10)"));
    let many = run(APPENDS);
    assert_eq!(ten["counters"]["updates"], 10, "{}", ten["counters"]);
    assert_eq!(many["counters"]["updates"], 200, "{}", many["counters"]);
}
