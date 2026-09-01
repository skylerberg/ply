//! `Bytes` and the text builtins through the real binary.
//!
//! Everything else about them is unit-tested; what only this can reach is the
//! whole pipeline — lex a `b"..."`, resolve it, type it, hash it, store the
//! hash, evaluate it on both engines, and read the cache back. A primitive that
//! works in `ply-eval` and falls over in the normalizer is a primitive that
//! poisons a cache, and the cache is the one wrong answer this project cannot
//! take back.

use assert_cmd::Command;
use std::path::Path;
use tempfile::TempDir;

/// A request head arriving in three pieces, parsed the way a server has to:
/// bytes off the wire, a decode that can fail, and text handling after it.
const SERVER: &str = r#"
fn head(a: Bytes, b: Bytes, c: Bytes) -> Bytes =
  bytes_concat(bytes_concat(a, b), c)

fn is_get(head: Bytes) -> Bool = bytes_slice(head, 0, 4) == b"GET "

fn first_line(head: Bytes) -> String =
  string_of_bytes(bytes_slice(head, 0, crlf(head)))

fn crlf(head: Bytes) -> Int =
  fold(range(0, bytes_len(head) - 1), -1, |found, i|
    if found >= 0 { found }
    else if bytes_at(head, i) == 13 && bytes_at(head, i + 1) == 10 { i }
    else { found })

fn header_value(line: String) -> String =
  if string_contains(line, ": ") {
    string_trim(string_slice(line, string_find(line, ": ") + 2, string_len(line)))
  } else {
    ""
  }

test "a head split across reads parses" {
  let h = head(b"GET /a HT", b"TP/1.1\r\nHost: ex", b"ample.com\r\n\r\n");
  assert(is_get(h));
  assert_eq(first_line(h), "GET /a HTTP/1.1");
  assert_eq(string_split(first_line(h), " "), ["GET", "/a", "HTTP/1.1"])
}

test "a header value is trimmed and case folded" {
  let lines = string_split("Host: Example.COM \r\nAccept: */*", "\r\n");
  assert_eq(string_lower(header_value("Host: Example.COM ")), "example.com");
  assert_eq(len(lines), 2);
  assert(string_starts_with("Content-Type: text/plain", "Content-"));
  assert(string_ends_with("Content-Type: text/plain", "plain"))
}

test "text that is not ascii survives the round trip" {
  let body = "héllo — ✓";
  assert_eq(string_of_bytes(bytes_of_string(body)), body);
  assert_eq(string_len(body), 9);
  assert_eq(bytes_len(bytes_of_string(body)), 14);
  assert_eq(string_slice(body, 0, 2), "hé")
}

test "a cut multi-byte character is refused rather than replaced" {
  let cut = bytes_slice(bytes_of_string("é"), 0, 1);
  assert(!bytes_is_utf8(cut));
  assert_eq(string_of_bytes_lossy(cut), "�");
  assert_eq(string_len(string_of_bytes_lossy(cut)), 1)
}
"#;

/// The failing half, kept out of `SERVER` so the green run above stays green:
/// each of these is a partial builtin reached at an input it refuses.
const REFUSALS: &str = r#"
test "a slice past the end is refused" {
  assert_eq(bytes_len(bytes_slice(b"abc", 0, 4)), 0)
}
"#;

fn project(source: &str) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("m.ply"), source).unwrap();
    dir
}

fn ply(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("ply").unwrap();
    cmd.arg("--color").arg("never").current_dir(dir);
    cmd
}

fn stdout_of(output: &std::process::Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

#[test]
fn a_program_over_bytes_and_text_passes_on_both_engines() {
    let dir = project(SERVER);
    let out = ply(dir.path())
        .arg("test")
        .arg("--engine")
        .arg("both")
        .output()
        .unwrap();
    let text = stdout_of(&out);
    assert!(
        out.status.success(),
        "{text}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(text.contains("4 passed"), "{text}");
}

/// The second run must select nothing: a `Bytes` literal that normalized
/// unstably would re-run every test on every invocation, and a green suite is
/// exactly where nobody would look.
#[test]
fn a_second_run_over_bytes_selects_nothing() {
    let dir = project(SERVER);
    ply(dir.path()).arg("test").assert().success();
    let out = ply(dir.path()).arg("test").output().unwrap();
    let text = stdout_of(&out);
    assert!(text.contains("selected 0 of 4"), "{text}");
}

/// Renaming changes no hash, which is the M3 invariant a new literal tag is
/// most likely to break — the tag carries a length, and a length written
/// against the wrong cursor moves every hash after it.
#[test]
fn renaming_a_definition_over_bytes_selects_nothing() {
    let dir = project(SERVER);
    ply(dir.path()).arg("test").assert().success();
    std::fs::write(
        dir.path().join("m.ply"),
        SERVER.replace("first_line", "request_line"),
    )
    .unwrap();
    let out = ply(dir.path()).arg("test").output().unwrap();
    let text = stdout_of(&out);
    assert!(text.contains("selected 0 of 4"), "{text}");
}

#[test]
fn a_slice_past_the_end_fails_the_test_rather_than_clamping() {
    let dir = project(REFUSALS);
    let out = ply(dir.path()).arg("test").output().unwrap();
    let text = stdout_of(&out);
    assert!(!out.status.success(), "{text}");
    assert!(text.contains("never clamped"), "{text}");
}
