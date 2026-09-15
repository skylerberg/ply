//! The shipped corpora on the compiled tier, through the `ply` binary: what the `same-tests` CI
//! job used to assert as shell steps, each now a test that partitions, retries and times out
//! like any other. Every assertion here is about the whole tree -- `examples/`, `tests/lang`,
//! the standard library, the compiler's own sources -- rather than a program written for it.

use assert_cmd::Command;
use serde_json::Value;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate lives two levels below the repository root")
        .to_path_buf()
}

fn ply(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("ply").unwrap();
    cmd.arg("--color").arg("never").current_dir(dir);
    cmd
}

/// The compiler's own sources in a directory of their own, as `ply test` takes a project.
fn compiler_copy() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for entry in std::fs::read_dir(repo().join("crates/ply-compiler/ply")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|x| x == "ply") {
            std::fs::copy(&path, dir.path().join(path.file_name().unwrap())).unwrap();
        }
    }
    dir
}

fn json(out: &std::process::Output) -> Value {
    let text = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(&text).unwrap_or_else(|e| {
        panic!(
            "stdout was not one JSON object: {e}\n---\n{text}\n---\n{}",
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

fn green(report: &Value, what: &str) {
    assert_eq!(report["ok"], Value::Bool(true), "{what} is red: {report}");
    assert_eq!(
        report["summary"]["failed"].as_u64(),
        Some(0),
        "{what} is red: {report}"
    );
}

fn entered(report: &Value, what: &str) {
    let backend = &report["backend"];
    assert!(
        backend["entered"].as_u64().is_some_and(|n| n > 0),
        "{what} entered nothing, so the run is green over a seam no call reached: {backend}"
    );
}

#[test]
fn the_code_generator_runs_examples_green_and_enters_the_seam() {
    let out = ply(&repo())
        .args(["test", "examples", "--backend", "c", "--json"])
        .output()
        .unwrap();
    let report = json(&out);
    green(&report, "the code generator over examples/");
    assert_eq!(report["backend"]["name"], "c", "{}", report["backend"]);
    entered(&report, "the code generator");
    assert!(
        report["backend"]["units"].as_u64().is_some_and(|n| n > 0),
        "no unit was compiled, so `c` is not a code generator here: {}",
        report["backend"]
    );
}

/// The emitter is asked for every body and answers; nothing here is the reference's.
#[test]
fn the_ply_emitter_produces_the_unit_and_examples_runs_green() {
    let cache = tempfile::tempdir().unwrap();
    let out = ply(&repo())
        .env("PLY_C_CACHE", cache.path())
        .env("PLY_C_REFUSALS", "1")
        .args(["test", "examples", "--backend", "c", "--no-cache", "--json"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    let answered = stderr.lines().any(|l| {
        l.strip_prefix("ply emitter answered ")
            .and_then(|rest| rest.split(' ').next())
            .and_then(|n| n.parse::<u64>().ok())
            .is_some_and(|n| n > 0)
    });
    assert!(
        answered,
        "the Ply emitter answered nothing, or was never asked:\n{stderr}"
    );
    green(
        &json(&out),
        "the C tier with the chain entered whole, over examples/",
    );
}

/// The compiler's own tests, run by the compiler it bootstraps into, with nothing else to fall
/// back on. It runs alone in CI (`.github/ci-shards.sh`), since it is the emitter over its own
/// sources once more.
#[test]
fn the_compiled_tier_runs_the_compilers_own_tests_as_the_only_engine() {
    let dir = compiler_copy();
    let out = ply(dir.path())
        .env("PLY_TIER_ONLY", "1")
        .args(["test", ".", "--no-cache", "--backend", "c", "--json"])
        .output()
        .unwrap();
    let report = json(&out);
    green(&report, "the compiled tier over the compiler's own sources");
    entered(&report, "the compiled tier");
}

#[test]
fn the_language_corpus_is_green_on_the_default_tier_and_as_the_only_engine() {
    ply(&repo())
        .args(["test", "tests/lang", "--no-cache"])
        .assert()
        .success();
    ply(&repo())
        .env("PLY_TIER_ONLY", "1")
        .args(["test", "tests/lang", "--backend", "c", "--no-cache"])
        .assert()
        .success();
}

#[test]
fn the_compiled_tier_is_the_only_engine_over_examples_and_the_standard_library() {
    for corpus in ["examples", "crates/ply-std/ply"] {
        ply(&repo())
            .env("PLY_TIER_ONLY", "1")
            .args(["test", corpus, "--backend", "c", "--no-cache"])
            .assert()
            .success();
    }
}

/// `ply prove --backend c` enters the propositions of every corpus and refutes none.
#[test]
fn the_compiled_tier_judges_the_corpus_specifications() {
    for corpus in ["examples", "crates/ply-std/ply"] {
        let out = ply(&repo())
            .env("PLY_C_REFUSALS", "1")
            .env("PLY_C_PHASES", "1")
            .args(["prove", corpus, "--no-cache", "--json", "--backend", "c"])
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.lines().any(|l| l.starts_with("entry:")),
            "the tier entered nothing while proving {corpus}:\n{stderr}"
        );
        let report = json(&out);
        let summary = &report["summary"];
        assert!(
            report["ok"] == Value::Bool(true)
                && summary["obligations"].as_u64().is_some_and(|n| n > 0)
                && summary["refuted"].as_u64() == Some(0),
            "the tier did not judge {corpus}'s specifications cleanly: {summary}"
        );
    }
}

/// `examples/hello.ply` served by the tier alone: it answers a request, the tier held every
/// definition, and the run exits when its connection count is served.
#[test]
fn a_served_example_with_the_tier_holding_its_accept_loop() {
    let dir = tempfile::tempdir().unwrap();
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let source = std::fs::read_to_string(repo().join("examples/hello.ply")).unwrap();
    assert!(
        source.contains("fn port() -> Int = 8080\n")
            && source.contains("fn connections() -> Int = 64\n"),
        "examples/hello.ply no longer declares its port and connection count as this test rewrites them"
    );
    let source = source
        .replace(
            "fn port() -> Int = 8080\n",
            &format!("fn port() -> Int = {port}\n"),
        )
        .replace(
            "fn connections() -> Int = 64\n",
            "fn connections() -> Int = 1\n",
        );
    std::fs::write(dir.path().join("hello.ply"), source).unwrap();

    let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin("ply"))
        .current_dir(dir.path())
        .env("PLY_TIER_ONLY", "1")
        .env("PLY_C_CACHE", dir.path().join("cache"))
        .env("PLY_C_REFUSALS", "1")
        .args(["--color", "never", "run", "--host", "--backend", "c"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let started = Instant::now();
    let mut stream = loop {
        if let Ok(s) = TcpStream::connect_timeout(
            &format!("127.0.0.1:{port}").parse().unwrap(),
            Duration::from_millis(200),
        ) {
            break s;
        }
        if let Some(status) = child.try_wait().unwrap() {
            let out = child.wait_with_output().unwrap();
            panic!(
                "the server exited {status} before listening:\n{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        assert!(
            started.elapsed() < Duration::from_secs(120),
            "the server did not listen within two minutes"
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
        .unwrap();
    let mut answer = Vec::new();
    stream.read_to_end(&mut answer).unwrap();
    let answer = String::from_utf8_lossy(&answer).into_owned();
    let out = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        answer.contains("hello from ply"),
        "the server did not answer the request:\n{answer}\n{stderr}"
    );
    assert!(
        !stderr.contains("c tier refused `hello."),
        "the tier refused part of the example, so the machine served it:\n{stderr}"
    );
    assert!(
        stderr
            .lines()
            .any(|l| l.starts_with("c tier took ") && l.contains(" definitions")),
        "no tier ran:\n{stderr}"
    );
    assert!(
        out.status.success(),
        "the run exited {}:\n{stderr}",
        out.status
    );
}

/// Sixty-four chunks: the tree path of the hash, and long enough that every loop in it comes
/// round with a record its tier may have held back.
#[test]
fn the_c_tier_hashes_a_long_input_consistently() {
    let dir = tempfile::tempdir().unwrap();
    let mut literal = String::from("b\"");
    for i in 0..65536u32 {
        let b = (i % 251) as u8;
        if !(0x20..0x7f).contains(&b) || b == b'"' || b == b'\\' {
            literal.push_str(&format!("\\x{b:02x}"));
        } else {
            literal.push(b as char);
        }
    }
    literal.push('"');
    std::fs::write(
        dir.path().join("input.ply"),
        format!("pub fn long() -> Bytes = {literal}\n"),
    )
    .unwrap();
    std::fs::write(
        dir.path().join("h.ply"),
        "import std.hash (blake3)\nimport input (long)\n\
         test \"a long input hashes to a stable digest\" {\n  assert_eq(blake3(long()), blake3(long()))\n}\n\
         pub fn digest() -> Bytes = blake3(long())\n",
    )
    .unwrap();
    let out = ply(dir.path())
        .args(["test", ".", "--no-cache", "--backend", "c", "--json"])
        .output()
        .unwrap();
    green(&json(&out), "the C tier over a long input");
}
