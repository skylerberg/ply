use ply_corpus::w4::{Program, served_args};

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

/// A served section builds a `ply run --host` command line, and a flag the CLI does not declare is
/// a refusal when the section finally runs — which no CI job reaches. `ply hosts` parses the same
/// flags and resolves the same configuration without opening a database, so the shape is checked
/// here: the sections' own `served_args`, driven through the real CLI.
#[test]
fn a_served_run_takes_the_flags_the_sections_pass() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir = tempfile::tempdir().unwrap();
    std::fs::copy(
        root.join("crates/ply-corpus/fixtures/desk-sequential-postgres.ply"),
        dir.path().join("desk.ply"),
    )
    .unwrap();

    let url = "postgres://nobody@127.0.0.1:1/none";
    let mut args = vec![
        "hosts".to_string(),
        "desk.ply".to_string(),
        "--host".to_string(),
    ];
    args.extend(served_args(8199, 8, "bench-key", Some(url)));
    args.extend([
        "--trace".to_string(),
        "off".to_string(),
        "--drain-ms".to_string(),
        "100".to_string(),
    ]);

    let out = std::process::Command::new(crate::support::ply())
        .args(&args)
        .current_dir(dir.path())
        .output()
        .expect("the CLI runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "`ply hosts` refused the flags a served section passes:\n{stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(&format!("DESK_DATABASE={url}")),
        "the setting a served run carries did not reach the listing:\n{stdout}"
    );
    assert!(
        !stderr.contains("unexpected argument"),
        "the CLI does not take a flag the sections pass:\n{stderr}"
    );
}
