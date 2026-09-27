use crate::harness::{json_of, ply, project};

const HOLDS: &str = "\
fn size(m: Map<String, Int>) -> Int = map_len(m)

law \"a map's key count is its length\"
  forall (m: Map<String, Int>) {
    len(map_keys(m)) == size(m)
  }

law \"inserting a key you already have does not grow the map\"
  forall (m: Map<String, Int>, k: String, v: Int) where map_contains(m, k) {
    map_len(map_insert(m, k, v)) == map_len(m)
  }
";

#[test]
fn a_law_over_a_map_is_discharged() {
    let dir = project(HOLDS);
    let out = ply(dir.path()).args(["prove", "--json"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", json_of(&out));
    let v = json_of(&out);
    let obligations = v["obligations"].as_array().unwrap();
    assert_eq!(obligations.len(), 2, "{v}");
    for o in obligations {
        // A binder the generator refuses reports a gap and no tier, which is what `E0418` looks like from here.
        assert!(
            o["tier"].is_string(),
            "a map law must earn a tier rather than a gap: {o}"
        );
        assert!(o["gap"].is_null(), "{o}");
    }
    // The unguarded law is sampled over the whole domain, so it earns the stronger label.
    assert_eq!(obligations[0]["tier"], "property", "{v}");
}

/// Entries shrink before values, so the witness is the smallest map that still breaks the law.
#[test]
fn a_refuted_map_law_shrinks_toward_the_empty_map() {
    let dir = project(
        "law \"every map is empty\"\n  forall (m: Map<String, Int>) {\n    map_len(m) == 0\n  }\n",
    );
    let out = ply(dir.path()).args(["prove", "--json"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1), "a false law must fail the run");
    let v = json_of(&out);
    let o = &v["obligations"].as_array().unwrap()[0];
    assert_eq!(o["outcome"], "refuted", "{o}");
    let binding = &o["counterexample"]["bindings"][0];
    assert_eq!(binding["name"], "m", "{o}");
    // Any larger witness means the shrinker stopped early or never entered the map.
    assert_eq!(binding["value"], "{\"\": 0}", "{o}");
    assert_ne!(
        o["counterexample"]["original"][0]["value"], binding["value"],
        "the witness must have been shrunk, not merely drawn: {o}"
    );
}
