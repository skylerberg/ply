use ply_corpus::w5::{Program, Rung, sink_for};
use ply_eval::Value;

#[test]
fn the_bench_program_checks_and_publishes_one_channel() {
    let program = Program::parse().expect("the bench program checks");
    for simple in [
        "bare",
        "events",
        "debug_events",
        "spans",
        "counters",
        "twin_events",
        "twin_spans",
        "twin_counters",
    ] {
        program
            .full(simple)
            .unwrap_or_else(|e| panic!("`{simple}` is missing: {e}"));
    }
    assert_eq!(program.footprint("bare").unwrap().to_string(), "{}");
    assert_eq!(
        program.footprint("events").unwrap().to_string(),
        "{std.trace.trace.event[bench]}"
    );
}

#[test]
fn a_twin_entry_point_reaches_nothing() {
    let program = Program::parse().unwrap();
    for simple in ["twin_events", "twin_spans", "twin_counters"] {
        assert_eq!(
            program.footprint(simple).unwrap().to_string(),
            "{}",
            "`{simple}` publishes a row, so it is not hermetic"
        );
    }
}

#[test]
fn every_rung_runs_the_same_loop() {
    let program = Program::parse().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let (_, bare) = program.call_pure("bare", 32).unwrap();
    assert_eq!(bare, Value::Int(64));
    for simple in ["twin_events", "twin_spans", "twin_counters"] {
        assert_eq!(program.call_pure(simple, 32).unwrap().1, Value::Int(64));
    }
    for (rung, entry) in [
        (Rung::Discard, "events"),
        (Rung::Discard, "spans"),
        (Rung::Discard, "counters"),
        (Rung::JsonNull, "events"),
    ] {
        let host = ply_host::Host::new().traced(sink_for(rung, dir.path()).unwrap());
        assert_eq!(
            program.call_traced(&host, entry, 32).unwrap().1,
            Value::Int(64),
            "`{entry}` under `{}`",
            rung.label()
        );
    }
}
