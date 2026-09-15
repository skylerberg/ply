use ply_corpus::w5::{Program, Rung, sink_for};
use ply_eval::Value;

/// The program every row of [`events`] runs has to be a program, and the rows it publishes have
/// to be the ones the substitution rests on.
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
        "{std.trace.trace.write[bench]}"
    );
}

/// The twin discharges every `trace` atom, which is what makes the `twin` rung a rung rather
/// than a stub: it runs on a machine with no host at all.
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

/// Every rung answers the same count, which is the whole of what makes a difference between two
/// rows the operation rather than the work.
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
