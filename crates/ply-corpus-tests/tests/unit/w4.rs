use ply_corpus::w4::{Program, Workload};
use ply_eval::Value;

/// The program the whole module measures has to be a program.
#[test]
fn the_bench_program_checks() {
    let program = Program::parse().expect("the bench program checks");
    for simple in [
        "selects",
        "selects_by",
        "inserts",
        "transactions",
        "selects_at",
        "transactions_at",
        "twin_selects",
        "twin_transactions",
        "ddl",
    ] {
        program
            .full(simple)
            .unwrap_or_else(|e| panic!("`{simple}` is missing: {e}"));
    }
}

/// The rows a `ply hosts` listing would print for the workloads, which is the exit criterion W3
/// stated and W4 must not have widened: a read workload's row names one table and one mode.
#[test]
fn a_workload_publishes_the_table_it_names() {
    let program = Program::parse().unwrap();
    assert_eq!(
        program.footprint("selects").unwrap().to_string(),
        "{std.db.db.read[part]}"
    );
    assert_eq!(
        program.footprint("transactions").unwrap().to_string(),
        "{std.db.db.write[part], std.db.db.write}"
    );
}

/// The twin discharges every `db` atom, so a twin entry point's row is empty — which is what
/// makes a twin-backed test `det`, cached and hermetic.
#[test]
fn a_twin_entry_point_reaches_nothing() {
    let program = Program::parse().unwrap();
    for simple in ["twin_selects", "twin_inserts", "twin_transactions"] {
        assert_eq!(
            program.footprint(simple).unwrap().to_string(),
            "{}",
            "`{simple}` publishes a row, so it is not hermetic"
        );
    }
}

/// The DDL comes from the program rather than from a copy of the schema in this file, and it
/// has to be the one table the workloads name.
#[test]
fn the_fixture_ddl_is_the_programs_own() {
    let program = Program::parse().unwrap();
    let ddl = program.ddl().unwrap();
    assert_eq!(ddl.len(), 1, "the fixture is one table");
    assert!(ddl[0].starts_with("create table \"part\""), "{}", ddl[0]);
    assert!(ddl[0].contains("numeric(10,4)"), "{}", ddl[0]);
}

/// Every workload runs against the twin with no host at all, which is the property that makes
/// the `ply-twin` rung a rung rather than a stub.
#[test]
fn every_workload_runs_against_the_twin() {
    let program = Program::parse().unwrap();
    for workload in Workload::ALL {
        let (_, answered) = program
            .call_pure(workload.twin(), workload.args(500, 4))
            .unwrap_or_else(|e| panic!("`{}` failed: {e}", workload.label()));
        assert_eq!(
            answered,
            Value::Int(4),
            "`{}` answered {answered}",
            workload.label()
        );
    }
}
