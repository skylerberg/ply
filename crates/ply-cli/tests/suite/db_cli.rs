//! The database as the command line configures it: `--db`, the pool knobs, `--db-schema`, and what
//! a run is allowed to say about any of it afterwards.

use assert_cmd::Command;
use serde_json::Value;
use std::path::Path;
use tempfile::TempDir;

/// A program that performs nothing at all.
const HOSTED: &str = "\
fn main() -> Int = 20 + 22

test \"arithmetic still works\" { assert_eq(1 + 1, 2) }
";

const URL: &str = "postgres://ply@127.0.0.1:5433/desk?sslmode=disable";
const PASSWORD: &str = "correct-horse-battery-staple";

fn project(source: &str) -> TempDir {
    let dir = tempfile::tempdir().expect("a temp dir");
    std::fs::write(dir.path().join("m.ply"), source).expect("the fixture is written");
    dir
}

fn ply(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("ply").expect("the binary is built");
    cmd.arg("--color")
        .arg("never")
        .current_dir(dir)
        // Inherited values would make every assertion below depend on the machine the suite ran on.
        .env_remove(ply_cli::db::URL_ENV)
        .env_remove(ply_cli::db::PASSWORD_ENV);
    cmd
}

fn stdout_of(output: &std::process::Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("stdout is utf-8")
}

fn stderr_of(output: &std::process::Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("stderr is utf-8")
}

/// Every byte under a directory, concatenated.
fn bytes_under(dir: &Path) -> String {
    let mut out = String::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.push_str(&bytes_under(&path));
        } else if let Ok(bytes) = std::fs::read(&path) {
            out.push_str(&String::from_utf8_lossy(&bytes));
        }
    }
    out
}

// --- the flags --------------------------------------------------------------

/// A flag that configures a binding, with no binding to configure, is refused rather than ignored —
/// the rule `--tls` established, applied to every knob `--db` brought with it.
#[test]
fn every_database_flag_without_host_is_refused_rather_than_silently_dropped() {
    let dir = project(HOSTED);
    for flag in [
        vec!["--db", URL],
        vec!["--db-pool", "4"],
        vec!["--db-acquire-ms", "100"],
        vec!["--db-statement-ms", "100"],
        vec!["--db-idle-txn-ms", "100"],
        vec!["--db-connect-ms", "100"],
        vec!["--db-statement-cache", "16"],
        vec!["--db-schema", "m.schema"],
    ] {
        for command in ["run", "test", "hosts"] {
            let output = ply(dir.path())
                .arg(command)
                .args(&flag)
                .output()
                .expect("ply runs");
            assert!(
                !output.status.success(),
                "`ply {command} {}` was accepted with nothing bound",
                flag.join(" ")
            );
        }
    }
}

/// Zero is not "no bound"; a pool of zero connections and a timeout of zero milliseconds are both
/// configurations that can only hang or fail.
#[test]
fn a_zero_bound_is_refused_rather_than_meaning_unlimited() {
    let dir = project(HOSTED);
    for flag in ["--db-pool", "--db-acquire-ms", "--db-connect-ms"] {
        let output = ply(dir.path())
            .args(["hosts", "--host", "--db", URL, flag, "0"])
            .output()
            .expect("ply runs");
        assert!(!output.status.success(), "`{flag} 0` was accepted");
    }
}

// --- malformed configuration ------------------------------------------------

/// Every refusal is `E0431`, at start-up, and none of them echoes the string it was handed: the
/// caller cannot know whether the operator put a password in it, and a diagnostic reaches the
/// result cache.
#[test]
fn a_malformed_connection_string_is_e0431_and_is_never_echoed() {
    let dir = project(HOSTED);
    for bad in [
        format!("postgres://ply:{PASSWORD}@127.0.0.1:5433"),
        format!("host=127.0.0.1 password={PASSWORD}"),
        format!("postgres://ply:{PASSWORD}@127.0.0.1:5433/desk?sslmode=require"),
        format!("postgres://ply:{PASSWORD}@127.0.0.1:notaport/desk"),
    ] {
        let output = ply(dir.path())
            .args(["run", "--host", "--db", &bad])
            .output()
            .expect("ply runs");
        let rendered = format!("{}{}", stdout_of(&output), stderr_of(&output));
        assert!(!output.status.success(), "`{bad}` was accepted");
        assert!(rendered.contains("E0431"), "`{bad}` gave:\n{rendered}");
        assert!(
            !rendered.contains(PASSWORD),
            "`{bad}` echoed the password back:\n{rendered}"
        );
    }
}

/// the trusted computing base listing: TLS to postgres is not wired up in W4, so a word that promised encryption would be
/// a label that lies — and this project's whole posture is that a label is a truth claim.
#[test]
fn an_sslmode_that_promises_encryption_names_the_decision_that_refused_it() {
    let dir = project(HOSTED);
    let output = ply(dir.path())
        .args([
            "run",
            "--host",
            "--db",
            "postgres://ply@127.0.0.1:5433/desk?sslmode=verify-full",
        ])
        .output()
        .expect("ply runs");
    let rendered = stderr_of(&output);
    assert!(rendered.contains("E0431"), "{rendered}");
    assert!(rendered.contains("the trusted computing base listing"), "{rendered}");
}

/// A `--json` command emits exactly one object on stdout, and a start-up refusal is not an
/// exception to it.
#[test]
fn a_refused_configuration_still_emits_one_json_object() {
    let dir = project(HOSTED);
    let output = ply(dir.path())
        .args(["test", "--json", "--host", "--db", "not-a-url"])
        .output()
        .expect("ply runs");
    let report: Value =
        serde_json::from_str(&stdout_of(&output)).expect("exactly one object on stdout");
    assert_eq!(report["ok"], false);
    assert_eq!(report["exit_code"], 2);
    assert_eq!(report["diagnostics"][0]["code"], "E0431");
}

// --- the environment --------------------------------------------------------

/// The environment says *which* database; only `--host` says that there is one.
#[test]
fn the_environment_cannot_cause_a_binding() {
    let dir = project(HOSTED);
    let output = ply(dir.path())
        .args(["test", "--json"])
        .env(ply_cli::db::URL_ENV, "this is not a connection string")
        .env(ply_cli::db::PASSWORD_ENV, PASSWORD)
        .output()
        .expect("ply runs");
    assert!(
        output.status.success(),
        "a hermetic run read the environment:\n{}{}",
        stdout_of(&output),
        stderr_of(&output)
    );
    let report: Value = serde_json::from_str(&stdout_of(&output)).expect("one object");
    assert_eq!(report["binding"], "hermetic");
    assert_eq!(report["hosts"]["database"], Value::Null);
}

#[test]
fn the_environment_supplies_the_url_under_host_and_the_report_says_so() {
    let dir = project(HOSTED);
    let output = ply(dir.path())
        .args(["hosts", "--host", "--json"])
        .env(ply_cli::db::URL_ENV, URL)
        .output()
        .expect("ply runs");
    let report: Value = serde_json::from_str(&stdout_of(&output)).expect("one object");
    assert_eq!(report["database"]["source"], ply_cli::db::URL_ENV);
    assert_eq!(
        report["database"]["url"],
        "postgres://ply@127.0.0.1:5433/desk?sslmode=disable"
    );
}

#[test]
fn a_malformed_environment_url_names_the_variable_rather_than_the_flag() {
    let dir = project(HOSTED);
    let output = ply(dir.path())
        .args(["hosts", "--host"])
        .env(ply_cli::db::URL_ENV, "postgres://ply@host")
        .output()
        .expect("ply runs");
    let rendered = stderr_of(&output);
    assert!(rendered.contains("E0431"), "{rendered}");
    assert!(rendered.contains(ply_cli::db::URL_ENV), "{rendered}");
}

// --- the password -----------------------------------------------------------

/// The whole reason `PLY_DB_PASSWORD` exists: an argument is readable by every process on the
/// machine and lands in a shell history.
#[test]
fn the_password_reaches_no_output_and_no_cache() {
    let dir = project(HOSTED);
    let output = ply(dir.path())
        .args(["test", "--host", "--db", URL, "--explain"])
        .env(ply_cli::db::PASSWORD_ENV, PASSWORD)
        .output()
        .expect("ply runs");
    let rendered = format!("{}{}", stdout_of(&output), stderr_of(&output));
    assert!(output.status.success(), "the run failed:\n{rendered}");
    assert!(
        !rendered.contains(PASSWORD),
        "the run printed it:\n{rendered}"
    );

    // And again, over every byte the run left behind.
    let stored = bytes_under(&dir.path().join(".ply-cache"));
    assert!(
        !stored.is_empty(),
        "the run wrote no cache, so this test would pass vacuously"
    );
    assert!(!stored.contains(PASSWORD), "the password reached the store");
}

/// The same claim for the form that puts the secret in the string itself, which an operator will do
/// whatever the documentation says.
#[test]
fn a_password_inside_the_url_is_redacted_everywhere_it_is_reported() {
    let dir = project(HOSTED);
    let output = ply(dir.path())
        .args([
            "hosts",
            "--host",
            "--db",
            &format!("postgres://ply:{PASSWORD}@127.0.0.1:5433/desk"),
        ])
        .output()
        .expect("ply runs");
    let rendered = format!("{}{}", stdout_of(&output), stderr_of(&output));
    assert!(output.status.success(), "{rendered}");
    assert!(!rendered.contains(PASSWORD), "{rendered}");
    assert!(
        rendered.contains("postgres://ply:****@127.0.0.1:5433/desk"),
        "{rendered}"
    );
}

#[test]
fn a_password_in_both_places_is_refused() {
    let dir = project(HOSTED);
    let output = ply(dir.path())
        .args([
            "hosts",
            "--host",
            "--db",
            "postgres://ply:in-the-url@127.0.0.1:5433/desk",
        ])
        .env(ply_cli::db::PASSWORD_ENV, PASSWORD)
        .output()
        .expect("ply runs");
    let rendered = stderr_of(&output);
    assert!(!output.status.success(), "{rendered}");
    assert!(rendered.contains("E0431"), "{rendered}");
    assert!(
        !rendered.contains(PASSWORD) && !rendered.contains("in-the-url"),
        "{rendered}"
    );
}

/// A definition's hash is a function of the program and of nothing else.
#[test]
fn no_credential_reaches_a_definition_s_hash() {
    let dir = project(HOSTED);
    let plain = ply(dir.path())
        .args(["hash", "--json"])
        .output()
        .expect("ply runs");
    let configured = ply(dir.path())
        .args(["hash", "--json"])
        .env(ply_cli::db::URL_ENV, URL)
        .env(ply_cli::db::PASSWORD_ENV, PASSWORD)
        .output()
        .expect("ply runs");
    assert_eq!(
        stdout_of(&plain),
        stdout_of(&configured),
        "a credential moved a hash"
    );
    assert!(!stdout_of(&configured).contains(PASSWORD));
}

// --- what `ply hosts` discloses ---------------------------------------------

/// The `database` block exists for the same reason W3's `transport` block does: a fact the rows
/// cannot carry and a reviewer must not have to derive.
#[test]
fn the_database_block_names_the_pool_the_scanner_and_what_is_not_connected() {
    let dir = project(HOSTED);
    let output = ply(dir.path())
        .args(["hosts", "--host", "--db", URL, "--db-pool", "3"])
        .output()
        .expect("ply runs");
    let rendered = stdout_of(&output);
    assert!(rendered.contains("database"), "{rendered}");
    assert!(
        rendered.contains("3 connections · acquire 5000ms · statement 30000ms"),
        "{rendered}"
    );
    assert!(
        rendered.contains("ply_host::db::scan · select insert update delete values with"),
        "{rendered}"
    );
    assert!(rendered.contains("not connected"), "{rendered}");
    assert!(
        rendered.contains("E0433"),
        "a run with no `--db-schema` must say what that costs:\n{rendered}"
    );
}

/// A program with no database in reach and a run that named none must print and hash exactly what
/// it did before W4 — which is what keeps every existing corpus's digest where it was.
#[test]
fn a_run_with_no_database_says_nothing_about_one() {
    let dir = project(HOSTED);
    let output = ply(dir.path())
        .args(["hosts", "--host"])
        .output()
        .expect("ply runs");
    let rendered = stdout_of(&output);
    assert!(
        !rendered.contains("database"),
        "an unconfigured run disclosed a database block:\n{rendered}"
    );
}

/// The digest is the one line a CI check pins.
#[test]
fn the_digest_moves_with_the_pool_and_not_with_the_database_name() {
    let dir = project(HOSTED);
    let digest = |args: &[&str]| {
        let output = ply(dir.path())
            .args(["hosts", "--host", "--digest"])
            .args(args)
            .output()
            .expect("ply runs");
        assert!(output.status.success(), "{}", stderr_of(&output));
        stdout_of(&output).trim().to_string()
    };

    let base = digest(&["--db", URL]);
    assert!(base.starts_with("b3:"), "{base}");
    assert_ne!(base, digest(&["--db", URL, "--db-pool", "4"]));
    assert_ne!(base, digest(&["--db", URL, "--db-statement-cache", "32"]));
    assert_eq!(
        base,
        digest(&[
            "--db",
            "postgres://ply@127.0.0.1:5433/other?sslmode=disable"
        ]),
        "a CI check that broke on a renamed database is one people learn to ignore"
    );
    assert_ne!(
        base,
        digest(&[]),
        "a configured database is not the same trusted computing base as none"
    );
}

// --- `--db-schema` ----------------------------------------------------------

/// There is no migration tool: a schema is a value, and W4's job is to check that the database
/// matches the one the program describes.
#[test]
fn a_db_schema_that_names_nothing_is_refused_with_what_the_program_has() {
    let dir = project(HOSTED);
    let output = ply(dir.path())
        .args([
            "hosts",
            "--host",
            "--db",
            URL,
            "--db-schema",
            "m.not_a_schema",
        ])
        .output()
        .expect("ply runs");
    let rendered = stderr_of(&output);
    assert!(!output.status.success(), "{rendered}");
    assert!(rendered.contains("E0431"), "{rendered}");
    assert!(
        rendered.contains("E0433"),
        "the reader is not told what dropping the flag costs:\n{rendered}"
    );
}

#[test]
fn a_db_schema_that_is_not_module_dot_fn_is_refused_before_the_program_is_consulted() {
    let dir = project(HOSTED);
    let output = ply(dir.path())
        .args(["hosts", "--host", "--db", URL, "--db-schema", "schema"])
        .output()
        .expect("ply runs");
    let rendered = stderr_of(&output);
    assert!(!output.status.success(), "{rendered}");
    assert!(rendered.contains("<module>.<fn>"), "{rendered}");
}

/// A function of the right shape is accepted, materialised, and its size reported — with `declared`
/// rather than `verified`, because nothing compared it to a server.
#[test]
fn a_resolvable_schema_is_materialised_and_reported_as_declared() {
    let dir = project(
        "\
type Column = { name: String }
type Table = { name: String, columns: List<Column> }
type Schema = { tables: List<Table> }

fn schema() -> Schema = {
  tables: [
    { name: \"part\", columns: [{ name: \"sku\" }, { name: \"price\" }] },
    { name: \"bin\", columns: [{ name: \"id\" }] }
  ],
}

fn main() -> Int = 1
",
    );
    let output = ply(dir.path())
        .args([
            "hosts",
            "--host",
            "--db",
            URL,
            "--db-schema",
            "m.schema",
            "--json",
        ])
        .output()
        .expect("ply runs");
    let report: Value = serde_json::from_str(&stdout_of(&output))
        .unwrap_or_else(|e| panic!("one object: {e}\n{}", stderr_of(&output)));
    assert_eq!(report["database"]["schema"]["function"], "m.schema");
    assert_eq!(report["database"]["schema"]["tables"], 2);
    assert_eq!(report["database"]["schema"]["columns"], 3);
    assert_eq!(report["database"]["schema"]["state"], "declared");
}

/// The digest covers the schema function's *name* and not the shape it materialises to: the table
/// count is a property of the database, and a digest that moved when someone else's migration ran
/// would be pinning the wrong thing.
#[test]
fn the_digest_covers_the_schema_name_and_not_its_size() {
    let with = |tables: &str| {
        let dir = project(&format!(
            "\
type Column = {{ name: String }}
type Table = {{ name: String, columns: List<Column> }}
type Schema = {{ tables: List<Table> }}

fn schema() -> Schema = {{ tables: {tables} }}

fn main() -> Int = 1
"
        ));
        let output = ply(dir.path())
            .args([
                "hosts",
                "--host",
                "--digest",
                "--db",
                URL,
                "--db-schema",
                "m.schema",
            ])
            .output()
            .expect("ply runs");
        assert!(output.status.success(), "{}", stderr_of(&output));
        stdout_of(&output).trim().to_string()
    };
    assert_eq!(
        with("[{ name: \"part\", columns: [{ name: \"sku\" }] }]"),
        with("[{ name: \"part\", columns: [{ name: \"sku\" }, { name: \"price\" }] }]"),
    );
}

// --- reporting --------------------------------------------------------------

/// A run that reached a database has to say so on the line a person reads last.
#[test]
fn a_hermetic_run_claims_no_database_on_the_line_a_person_reads_last() {
    let dir = project(HOSTED);
    let output = ply(dir.path()).arg("test").output().expect("ply runs");
    let rendered = stdout_of(&output);
    assert!(output.status.success(), "{}", stderr_of(&output));
    assert!(rendered.contains("0 failed, 1 passed"), "{rendered}");
    assert!(!rendered.contains("real database"), "{rendered}");
    assert!(!rendered.contains("database"), "{rendered}");
}
