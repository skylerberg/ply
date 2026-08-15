//! An adversarial audit of `std.http`, written as an attacker rather than as
//! an author.
//!
//! ADR 0013 §2 puts HTTP/1.1 framing in Ply precisely so that a protocol defect
//! is a failing test rather than a line in the trusted computing base. This file
//! takes that at its word: every case below is a message a peer can put on a
//! socket, and the question asked of each is the only one that matters for
//! framing — **does the server refuse, or does it interpret?** A message two
//! reasonable parsers would frame differently and this server accepts is a
//! request smuggle; a message that makes the server stop being a server is a
//! denial of service.
//!
//! `crates/ply-std/ply/http.ply` carries its own tests for the rules ADR 0013
//! §11 lists. This file is not those. It holds the cases that file does not
//! reach, and the ones where the code and the contract disagree.
//!
//! The last section holds the cases this audit found broken. Every one of them
//! is now green, and every one keeps the assertion it was written with: the test
//! states what the server must do, and its doc comment states what it used to do
//! instead and why that was the bug. Nothing here was relaxed to make it pass.

use assert_cmd::prelude::*;
use std::path::Path;
use std::process::Command;

/// What one `ply test` run over a generated project answered.
struct Outcome {
    passed: bool,
    output: String,
}

impl Outcome {
    /// The suite was green. `why` is what a reader who sees it red needs in
    /// order to know what the server does instead.
    #[track_caller]
    fn green(&self, why: &str) {
        assert!(self.passed, "{why}\n\n{}", self.output);
    }

    #[track_caller]
    fn says(&self, needle: &str) {
        assert!(
            self.output.contains(needle),
            "the run never mentioned `{needle}`\n\n{}",
            self.output
        );
    }
}

/// One `main.ply` in a temporary directory, run under `ply test`.
///
/// A fresh directory per call, so nothing here reads a cache another test
/// wrote and a red result is a property of the source rather than of the order
/// the suite ran in.
fn ply_test(source: &str) -> Outcome {
    let dir = tempfile::tempdir().expect("a temp dir");
    write(dir.path(), source);
    let out = Command::cargo_bin("ply")
        .unwrap()
        .arg("--color")
        .arg("never")
        .arg("test")
        .current_dir(dir.path())
        .output()
        .expect("`ply test` ran");
    Outcome {
        passed: out.status.success(),
        output: format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    }
}

fn write(dir: &Path, source: &str) {
    std::fs::write(dir.join("main.ply"), source).unwrap();
}

/// The head parser, reduced to the number a table of cases reads as: the status
/// an input earns, `-1` for "still arriving" and `0` for accepted.
const HEAD: &str = r#"
import std.http (parse_head, default_limits, Limits, Parsed, Refused, Incomplete)

fn status(raw: Bytes) -> Int =
  match parse_head(raw, default_limits()) {
    Refused(r) -> r.status,
    Incomplete -> -1,
    Parsed(_) -> 0,
  }

fn filler(n: Int) -> Bytes = bytes_concat_all(map(range(0, n), |i: Int| b"a"))
"#;

/// The chunk decoder, reduced the same way.
const BODY: &str = r#"
import std.http (default_limits, body_start, body_step, Await, Complete, Rejected, Chunked)

fn chunk_status(buf: Bytes) -> Int =
  match body_step(body_start(Chunked, default_limits()), buf) {
    Rejected(r) -> r.status,
    Await(_) -> -1,
    Complete(_) -> 0,
  }
"#;

/// A serve loop over a scripted peer: the first `recv` answers `first` and
/// every later one answers `more`, and the bytes the server wrote come back.
///
/// This is the shape that decides the questions a pure parser cannot answer —
/// what a refusal does to the connection, and how long a peer can hold one.
const SERVE: &str = r#"
import std.net (net)
import std.http (default_limits, text_response, serve_connection, Request, Response,
                 method_name)

fn echo(req: Request) -> Response = text_response(200, method_name(req.method) ++ " " ++ req.path)

fn drive(first: Bytes, more: Bytes) -> { out: Bytes, reads: Int } =
  with_cell[outbox](b"") { outbox -> {
  with_cell[reads](0) { reads -> {
    handle {
      serve_connection(7, default_limits(), echo)
    } with {
      net.recv[conn](c, max, t) -> {
        let n = cell_get(reads);
        cell_set(reads, n + 1);
        Some(if n == 0 { first } else { more })
      },
      net.send[conn](c, payload, t) -> {
        cell_set(outbox, bytes_concat(cell_get(outbox), payload));
        Some(bytes_len(payload))
      },
      net.close[conn](c) -> (),
    };
    {out: cell_get(outbox), reads: cell_get(reads)}
  } }
  } }
"#;

// --- Framing: what a second parser in front of this one would decide ---------

/// The whole of request smuggling in one test: a message whose length can be
/// read two ways must never be accepted, whichever way this parser would have
/// picked.
///
/// The last two cases are the ones `http.ply` does not carry. A duplicate
/// `Content-Length` that differs only in case is still two field lines, and a
/// `Transfer-Encoding` value with a form feed in it is a token to nobody and a
/// coding name to a lenient parser.
#[test]
fn no_message_with_two_available_framings_is_accepted() {
    let out = ply_test(&format!(
        "{HEAD}
test \"two framings\" {{
  assert_eq(status(b\"POST / HTTP/1.1\\r\\nHost: x\\r\\nContent-Length: 5\\r\\nTransfer-Encoding: chunked\\r\\n\\r\\nhello\"), 400);
  assert_eq(status(b\"POST / HTTP/1.1\\r\\nHost: x\\r\\nTransfer-Encoding: chunked\\r\\nContent-Length: 5\\r\\n\\r\\nhello\"), 400);
  assert_eq(status(b\"POST / HTTP/1.1\\r\\nHost: x\\r\\nContent-Length: 5\\r\\nContent-length: 5\\r\\n\\r\\nhello\"), 400);
  assert_eq(status(b\"POST / HTTP/1.1\\r\\nHost: x\\r\\nCONTENT-LENGTH: 5\\r\\nContent-Length: 6\\r\\n\\r\\nhello\"), 400);
  assert_eq(status(b\"POST / HTTP/1.1\\r\\nHost: x\\r\\nTransfer-Encoding: chunked\\r\\nTransfer-Encoding: chunked\\r\\n\\r\\n0\\r\\n\\r\\n\"), 400);
  assert_eq(status(b\"POST / HTTP/1.1\\r\\nHost: x\\r\\nTransfer-Encoding: ChUnKeD, gzip\\r\\n\\r\\n\"), 400);
  assert_eq(status(b\"POST / HTTP/1.1\\r\\nHost: x\\r\\nTransfer-Encoding: chunked\\x0c\\r\\n\\r\\n\"), 400)
}}"
    ));
    out.green("a message with two available framings was accepted, which is a request smuggle");
}

/// Everything about a request line that a second implementation might read
/// differently. A leading empty line — which RFC 9112 §2.2 lets a server skip —
/// is refused here, and refusing is the safe branch.
#[test]
fn a_request_line_is_refused_wherever_it_is_ambiguous() {
    let out = ply_test(&format!(
        "{HEAD}
test \"request line\" {{
  assert_eq(status(b\"\\r\\nGET / HTTP/1.1\\r\\nHost: x\\r\\n\\r\\n\"), 400);
  assert_eq(status(b\"\\nGET / HTTP/1.1\\r\\nHost: x\\r\\n\\r\\n\"), 400);
  assert_eq(status(b\"GET /a\\x00b HTTP/1.1\\r\\nHost: x\\r\\n\\r\\n\"), 400);
  assert_eq(status(b\"GET  / HTTP/1.1\\r\\nHost: x\\r\\n\\r\\n\"), 400);
  assert_eq(status(b\"GET / HTTP/1.1\\r\\r\\nHost: x\\r\\n\\r\\n\"), 400);
  assert_eq(status(b\"GET / HTTP/1.1\\r\\nHost: x\\r\\n\\n\"), 400);
  assert_eq(status(b\"GET example.com:443 HTTP/1.1\\r\\nHost: x\\r\\n\\r\\n\"), 400)
}}"
    ));
    out.green("a request line two parsers would split differently was accepted");
}

/// The chunk-size line, which is the other place a length is decided. A padded
/// size, a signed one, an empty one and an extension carrying a CRLF inside a
/// quoted string are each a boundary a lenient parser would place elsewhere.
#[test]
fn every_malformed_chunk_size_line_is_refused() {
    let out = ply_test(&format!(
        "{BODY}
test \"chunk sizes\" {{
  assert_eq(chunk_status(b\"5 \\r\\nhello\\r\\n0\\r\\n\\r\\n\"), 400);
  assert_eq(chunk_status(b\" 5\\r\\nhello\\r\\n0\\r\\n\\r\\n\"), 400);
  assert_eq(chunk_status(b\"+5\\r\\nhello\\r\\n0\\r\\n\\r\\n\"), 400);
  assert_eq(chunk_status(b\"-5\\r\\nhello\\r\\n0\\r\\n\\r\\n\"), 400);
  assert_eq(chunk_status(b\"\\r\\nhello\\r\\n0\\r\\n\\r\\n\"), 400);
  assert_eq(chunk_status(b\"0\\n\\r\\n\"), 400);
  assert_eq(chunk_status(b\"5;a=\\\"b\\r\\nc\\\"\\r\\nhello\\r\\n0\\r\\n\\r\\n\"), 400);
  assert_eq(chunk_status(b\"0000000000000005\\r\\nhello\\r\\n0\\r\\n\\r\\n\"), 0);
  assert_eq(chunk_status(b\"5;\\r\\nhello\\r\\n0\\r\\n\\r\\n\"), 0)
}}"
    ));
    out.green("a chunk-size line two parsers would size differently was accepted");
}

/// A chunked body whose terminator has not arrived is never completed short.
/// Completing it would hand the handler a truncated body and leave the rest of
/// the peer's bytes to be framed as the next request.
#[test]
fn a_chunked_body_missing_its_terminator_is_never_completed() {
    let out = ply_test(&format!(
        "{BODY}
test \"terminators\" {{
  assert_eq(chunk_status(b\"5\\r\\nhello\\r\\n0\\r\\n\"), -1);
  assert_eq(chunk_status(b\"5\\r\\nhello\\r\\n0\"), -1);
  assert_eq(chunk_status(b\"5\\r\\nhello\\r\\n\"), -1);
  assert_eq(chunk_status(b\"5\\r\\nhello\\r\\n0\\r\\n\\r\\n\"), 0)
}}"
    ));
    out.green("a chunked body was completed before its terminator arrived");
}

/// The three size bounds, each bought with one packet. ADR 0013 §4: the bound
/// must cost the bound, so these also have to finish quickly rather than after
/// a fold over what the peer sent.
#[test]
fn a_head_a_peer_sized_is_refused_at_the_bound() {
    let out = ply_test(&format!(
        "{HEAD}
test \"bounds\" {{
  assert_eq(status(bytes_concat_all([b\"GET / HTTP/1.1\\r\\nHost: x\\r\\n\",
                                     bytes_concat_all(map(range(0, 10000), |i: Int| b\"A: b\\r\\n\")),
                                     b\"\\r\\n\"])),
            431);
  assert_eq(status(bytes_concat_all([b\"GET / HTTP/1.1\\r\\nHost: x\\r\\nA: \", filler(70000),
                                     b\"\\r\\n\\r\\n\"])),
            431);
  assert_eq(status(bytes_concat_all([b\"GET /\", filler(9000), b\" HTTP/1.1\\r\\nHost: x\\r\\n\\r\\n\"])),
            414)
}}"
    ));
    out.green("a head a peer sized past the bound was not refused at the bound");
}

/// **The measurement behind the header-block off-by-two**, pinned rather than
/// argued about.
///
/// ADR 0013 §3.8 rule 28 bounds "every field line after the request line, **up
/// to and including the terminator**". `field_lines` charges each line's content
/// and its CRLF against `max_header_bytes` as it advances, but the terminating
/// blank line's own CRLF is never charged, so a block of `max_header_bytes + 2`
/// bytes is accepted. Two bytes over sixty-four kilobytes is not an exhaustion
/// vector; it is stated so that a bound in the contract and a bound in the code
/// are known to differ.
#[test]
fn the_header_block_bound_does_not_charge_the_terminator() {
    let out = ply_test(&format!(
        "{HEAD}
fn limits_of(header_bytes: Int) -> Limits =
  {{max_request_line: 8192, max_header_bytes: header_bytes, max_header_count: 100,
   max_body: 1048576, max_chunk_size: 1048576, max_chunk_line: 4096, max_trailer_bytes: 8192,
   max_keep_alive: 100, max_stream_chunks: 2048,
   header_timeout_ms: 5000, body_timeout_ms: 30000, idle_timeout_ms: 5000,
   write_timeout_ms: 30000}}

// `Host: x` and its CRLF is 9 bytes, `A: ` plus n plus CRLF is n + 5, and the
// terminator is 2: the block is n + 16 bytes.
fn block(n: Int) -> Bytes =
  bytes_concat_all([b\"GET / HTTP/1.1\\r\\nHost: x\\r\\nA: \", filler(n), b\"\\r\\n\\r\\n\"])

fn status_with(raw: Bytes, l: Limits) -> Int =
  match parse_head(raw, l) {{
    Refused(r) -> r.status,
    Incomplete -> -1,
    Parsed(_) -> 0,
  }}

test \"the terminator is not charged\" {{
  // 64 bytes: the bound exactly, and accepted.
  assert_eq(status_with(block(48), limits_of(64)), 0);
  // 65 and 66 bytes are over the bound, and only 66 is refused.
  assert_eq(status_with(block(49), limits_of(64)), 0);
  assert_eq(status_with(block(50), limits_of(64)), 431)
}}"
    ));
    out.green(
        "the header-block accounting has changed; if the terminator is now charged, this test \
         should assert that `block(49)` is 431",
    );
}

// --- Resource exhaustion ----------------------------------------------------

/// A body that never ends and a head that never ends both stop, and both stop
/// because of a bound rather than because the peer relented.
///
/// All three are 408 rather than 431: a peer dribbling one byte per read runs
/// out of the deadline before it runs out of the header-block bound, and ADR
/// 0013 §3.9 rules 31 and 32 make an expired deadline a 408. A head that reaches
/// `max_header_bytes` without a terminator is still 431 — that case is
/// `a_head_a_peer_sized_is_refused_at_the_bound`.
#[test]
fn a_peer_that_never_finishes_is_answered_rather_than_waited_on() {
    let out = ply_test(&format!(
        "{SERVE}
fn answer(r: {{ out: Bytes, reads: Int }}) -> String = string_of_bytes_lossy(bytes_slice(r.out, 9, 12))

test \"never ends\" {{
  // A head that never terminates.
  assert_eq(answer(drive(b\"a\", b\"a\")), \"408\");
  // A chunked body with one more chunk, forever.
  assert_eq(answer(drive(b\"POST /a HTTP/1.1\\r\\nHost: x\\r\\nTransfer-Encoding: chunked\\r\\n\\r\\n\",
                         b\"1\\r\\na\\r\\n\")),
            \"408\");
  // A length-framed body arriving one byte at a time and never completing.
  assert_eq(answer(drive(b\"POST /a HTTP/1.1\\r\\nHost: x\\r\\nContent-Length: 1048576\\r\\n\\r\\n\",
                         b\"a\")),
            \"408\")
}}"
    ));
    out.green("a peer that never finishes was waited on rather than answered");
}

/// **The slow-loris finding, now stated as the deadline it was missing.**
///
/// ADR 0013 §3.9 rule 31 says "the whole head must arrive within
/// `limits.header_timeout_ms`, **measured from the first byte of the
/// request**", and rule 32 says the same of the body. `header_timeout_ms` used
/// to be passed whole to each individual `net.recv` — which
/// `ply_host::tcp::recv` turns into one `set_read_timeout` per syscall — so the
/// deadline restarted on every byte and the only real bound was the read count:
/// 2048 × 5000 ms ≈ 2.8 hours on the head and 2049 × 30000 ms ≈ 17 hours on the
/// body, on one socket, while `serve` — a sequential accept loop — accepted
/// nothing else.
///
/// Ply has no clock, so the deadline is enforced by dividing it: each read
/// carries a slice and the budget is the number of slices. This is that property
/// measured rather than argued about — the sum of every timeout the server asked
/// for over one dribbling head, and over one dribbling body, against the
/// deadline each was given.
#[test]
fn the_head_and_body_deadlines_bound_the_whole_message() {
    let out = ply_test(
        r#"
import std.net (net)
import std.http (default_limits, serve_connection, text_response, Request, Response,
                 method_name)

fn echo(req: Request) -> Response = text_response(200, method_name(req.method) ++ " " ++ req.path)

// The total of every deadline the server asked a `recv` for, driving a peer that
// answers one byte at a time and never finishes.
fn granted(first: Bytes) -> Int =
  with_cell[total](0) { total -> {
  with_cell[reads](0) { reads -> {
    handle {
      serve_connection(7, default_limits(), echo)
    } with {
      net.recv[conn](c, max, t) -> {
        let n = cell_get(reads);
        cell_set(reads, n + 1);
        if n == 0 && bytes_len(first) > 0 {
          Some(first)
        } else {
          cell_set(total, cell_get(total) + t);
          Some(b"a")
        }
      },
      net.send[conn](c, payload, t) -> Some(bytes_len(payload)),
      net.close[conn](c) -> (),
    };
    cell_get(total)
  } }
  } }

test "the whole message is bounded, not each read" {
  let l = default_limits();
  assert(granted(b"") <= l.header_timeout_ms);
  assert(granted(b"POST /a HTTP/1.1\r\nHost: x\r\nContent-Length: 1048576\r\n\r\n")
         <= l.body_timeout_ms);
  // And the bound is not vacuous: a peer that dribbles is granted most of it.
  assert(granted(b"") > l.header_timeout_ms / 2)
}
"#,
    );
    out.green(
        "the head or body deadline is no longer bounded by the timeout it was given, which is \
         the slow-loris window reopening: a peer sending one byte per read must not be able to \
         hold a connection for longer than `header_timeout_ms` plus `body_timeout_ms`",
    );
}

// --- Response correctness ---------------------------------------------------

/// Response splitting, refused rather than sanitized — ADR 0013 §2.6. The value
/// came from the program, so stripping it would turn an attempt into a response
/// the attacker partly controls and nobody noticed.
#[test]
fn a_program_supplied_header_value_with_cr_or_lf_refuses_the_encode() {
    let out = ply_test(
        r#"
import std.http (encode, Get, Http11, response, set_header, empty_headers)

test "a CR in a value splits nothing" {
  assert_eq(bytes_len(encode(Get, Http11, true,
    {status: 200, headers: set_header(empty_headers(), "x-echo", "a\r\nSet-Cookie: evil=1"),
     body: b"ok"})), 0)
}
"#,
    );
    assert!(
        !out.passed,
        "a header value carrying CRLF was encoded into the response\n\n{}",
        out.output
    );
    out.says("contains CR or LF, which would split the response");
}

/// The framing fields belong to `encode`, and a program that sets one has
/// written a second answer to "how long is this message".
#[test]
fn a_program_may_not_set_a_framing_field_on_a_response() {
    let out = ply_test(
        r#"
import std.http (encode, Get, Http11, response, set_header, empty_headers)

test "a second Content-Length" {
  assert_eq(bytes_len(encode(Get, Http11, true,
    {status: 200, headers: set_header(empty_headers(), "content-length", "0"), body: b"ok"})), 0)
}
"#,
    );
    assert!(
        !out.passed,
        "a program set its own Content-Length beside the computed one\n\n{}",
        out.output
    );
    out.says("may not be set by the program");
}

/// A handler that fails takes the whole run with it: no 500, no response, and
/// no further connections.
///
/// Pinned rather than argued about. Ply has no exceptions, so `serve_connection`
/// cannot turn a failing handler into a status — but a reader deciding whether
/// to put this server in front of traffic needs the consequence written down,
/// and a future `serve` that isolates a handler should make this test change.
#[test]
fn a_handler_that_fails_ends_the_run_rather_than_the_connection() {
    let out = ply_test(
        r#"
import std.net (net)
import std.http (default_limits, serve_connection, Request, Response)

fn boom(req: Request) -> Response = panic("the handler had a bug")

test "a failing handler" {
  with_cell[outbox](b"") { outbox -> {
    handle {
      serve_connection(7, default_limits(), boom)
    } with {
      net.recv[conn](c, max, t) -> Some(b"GET /a HTTP/1.1\r\nHost: x\r\n\r\n"),
      net.send[conn](c, payload, t) -> {
        cell_set(outbox, bytes_concat(cell_get(outbox), payload));
        Some(bytes_len(payload))
      },
      net.close[conn](c) -> (),
    };
    assert_eq(bytes_len(cell_get(outbox)), 0)
  } }
}
"#,
    );
    assert!(
        !out.passed,
        "a failing handler produced a response; if `serve_connection` now isolates the handler \
         this test should assert the status it answers\n\n{}",
        out.output
    );
    out.says("the handler had a bug");
}

// --- Defects ----------------------------------------------------------------
//
// Everything below is red, and each one is a finding. The assertion is what the
// server must do; the message is what it does instead.

/// **Was a remote denial of service, one request.**
///
/// ADR 0013 §3.6 rule 19 bounds a chunk size at 16 hexadecimal digits, and
/// `hex_to_int` carries the comment "at most 16 digits by the guard in `sized`,
/// so the accumulator cannot overflow". Sixteen hex digits is sixty-four bits
/// and `Int` is signed, so any size at or above `8000000000000000` overflows
/// the multiply. Integer overflow is `E0502 RUNTIME_ERROR`, which is not a
/// `Refusal` — it unwinds out of `body_step`, out of `serve_connection`, and
/// ends the run.
///
/// `printf 'POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\nFFFFFFFFFFFFFFFF\r\n'`
/// is a whole-server kill.
#[test]
fn a_chunk_size_that_does_not_fit_in_an_int_is_refused_rather_than_overflowing() {
    let out = ply_test(&format!(
        "{BODY}
fn refused(s: Int) -> Bool = s == 400 || s == 413

test \"a chunk size at the top of the 16-digit range\" {{
  assert(refused(chunk_status(b\"FFFFFFFFFFFFFFFF\\r\\nx\\r\\n0\\r\\n\\r\\n\")));
  assert(refused(chunk_status(b\"8000000000000000\\r\\nx\\r\\n0\\r\\n\\r\\n\")));
  assert(refused(chunk_status(b\"ffffffffffffffff;e=1\\r\\nx\\r\\n0\\r\\n\\r\\n\")))
}}"
    ));
    out.green(
        "a 16-hexadecimal-digit chunk size overflowed `Int` in `std.http.hex_to_int`, and the \
         overflow is `E0502 RUNTIME_ERROR` rather than a `Refusal`, so one request ended the \
         run instead of the connection. `sized` must reject a size that does not fit before \
         `hex_to_int` multiplies",
    );
}

/// **Was a remote denial of service, one request.**
///
/// `absolute_form` read a scheme with `string_of_bytes(bytes_slice(target, 0,
/// min_int(n, 8)))`. The target is checked for UTF-8 as a whole, but that slice
/// cuts at byte 8 regardless of where a character starts, and `string_of_bytes`
/// is strict — a cut sequence is `E0502 RUNTIME_ERROR` at `http.ply:449`, which
/// ends the run rather than the connection.
///
/// `GET €€€ HTTP/1.1` is a whole-server kill: three U+20AC are nine bytes, the
/// target is not origin-form and is not `*`, and byte 8 lands mid-character.
#[test]
fn a_target_that_is_not_origin_form_is_refused_rather_than_ending_the_run() {
    let out = ply_test(&format!(
        "{HEAD}
test \"a target cut mid-character\" {{
  assert_eq(status(b\"GET \\xe2\\x82\\xac\\xe2\\x82\\xac\\xe2\\x82\\xac HTTP/1.1\\r\\nHost: x\\r\\n\\r\\n\"), 400);
  assert_eq(status(b\"GET \\xf0\\x9f\\x92\\xa9\\xf0\\x9f\\x92\\xa9 HTTP/1.1\\r\\nHost: x\\r\\n\\r\\n\"), 400)
}}"
    ));
    out.green(
        "`std.http.absolute_form` must compare the scheme as `Bytes` and never slice the \
         request target into a `String`: a target whose eighth byte falls inside a character \
         made the strict `string_of_bytes` an `E0502 RUNTIME_ERROR`, which ends the run",
    );
}

/// **Whitespace inside a request target was accepted.**
///
/// `line_stops` deliberately omits HTAB, because HTAB is legal inside a *field
/// value*. It is not legal inside a request line: RFC 9112 §3 is
/// `method SP request-target SP HTTP-version`, and §11.2 names recovering from
/// whitespace in the request line as a smuggling vector, because a recipient
/// that splits on HTAB and one that does not disagree about what the target was.
///
/// `GET /a<TAB>b HTTP/1.1` is accepted here with `target = "/a\tb"`.
#[test]
fn whitespace_inside_a_request_target_is_refused() {
    let out = ply_test(&format!(
        "{HEAD}
test \"a tab in the request line\" {{
  assert_eq(status(b\"GET /a\\tb HTTP/1.1\\r\\nHost: x\\r\\n\\r\\n\"), 400);
  assert_eq(status(b\"GET /a\\x0bb HTTP/1.1\\r\\nHost: x\\r\\n\\r\\n\"), 400)
}}"
    ));
    out.green(
        "HTAB inside the request target must be refused: `GET /a\\tb HTTP/1.1` parsed with \
         the tab in `target` and `path`, and RFC 9112 §11.2 calls recovering from whitespace \
         in the request line a smuggling vector",
    );
}

/// **An absolute-form authority could carry userinfo.**
///
/// ADR 0013 §3.2 rule 8 exposes the target's authority as `Request::authority`
/// so a program can compare it against `Host`. What it is handed for
/// `GET http://trusted.example@evil.example/x` is
/// `trusted.example@evil.example`, because `split_authority` takes everything up
/// to the first `/`, `?` or `#`. A program checking that authority against an
/// allowlist by prefix, or logging it, sees the wrong host — RFC 9110 §4.2.4
/// deprecates userinfo in `http` URIs and tells a recipient to treat its
/// presence in a reference from an untrusted source as an error.
#[test]
fn an_absolute_form_target_carrying_userinfo_is_refused() {
    let out = ply_test(&format!(
        "{HEAD}
test \"userinfo in the authority\" {{
  assert_eq(status(b\"GET http://trusted.example@evil.example/x HTTP/1.1\\r\\nHost: h\\r\\n\\r\\n\"), 400);
  assert_eq(status(b\"GET http://u:p@evil.example/x HTTP/1.1\\r\\nHost: h\\r\\n\\r\\n\"), 400)
}}"
    ));
    out.green(
        "`std.http.split_authority` must refuse a userinfo subcomponent: it used to keep \
         one, so `Request::authority` was `trusted.example@evil.example` for \
         `http://trusted.example@evil.example/x`, and RFC 9110 §4.2.4 says to treat that as \
         an error",
    );
}

/// **The contract and the code disagreed about an unsupported transfer
/// coding.**
///
/// ADR 0013 §3.5 rule 17: "A transfer coding other than `chunked` — `gzip`,
/// `deflate`, `compress`, `identity` — is `501`." `chunked_of` used to answer
/// 501 only when `chunked` was *absent*, so `Transfer-Encoding: gzip, chunked`
/// was accepted and the handler was handed gzip bytes as `Request::body` with
/// nothing saying so. RFC 9112 §6.1 says a server that receives a transfer
/// coding it does not understand should answer 501.
///
/// This is not a framing desync — the framing is chunked and unambiguous — but
/// it is the case where an intermediary that honours `gzip` and a server that
/// ignores it disagree about what the body was.
#[test]
fn a_transfer_coding_other_than_chunked_is_501_even_when_chunked_is_last() {
    let out = ply_test(&format!(
        "{HEAD}
test \"gzip then chunked\" {{
  assert_eq(status(b\"POST / HTTP/1.1\\r\\nHost: x\\r\\nTransfer-Encoding: gzip, chunked\\r\\n\\r\\n0\\r\\n\\r\\n\"), 501);
  assert_eq(status(b\"POST / HTTP/1.1\\r\\nHost: x\\r\\nTransfer-Encoding: gzip\\r\\nTransfer-Encoding: chunked\\r\\n\\r\\n0\\r\\n\\r\\n\"), 501)
}}"
    ));
    out.green(
        "`Transfer-Encoding: gzip, chunked` must be 501: accepting it hands the handler the \
         undecoded gzip bytes as the body, and ADR 0013 §3.5 rule 17 says any coding other \
         than `chunked` is 501",
    );
}

/// **A streamed response could be truncated without its terminating chunk.**
///
/// `stream_chunks` used to carry `max_reads()` as fuel and answer `false` when
/// it ran out, having written neither `0\r\n\r\n` nor anything that ends the
/// message. ADR 0013 §2.5 says a response always carries either
/// `Content-Length` or `Transfer-Encoding: chunked`, "so an unframed response is
/// not a case anyone has to handle" — but that one was framed and never
/// terminated, and a caller that kept the connection alive after `false` had its
/// next response read as chunk data by the client, which is response smuggling.
///
/// The bound now lives in `Limits.max_stream_chunks`, where ADR 0013 §4 says
/// every bound belongs, and `max_reads()` is a read bound again.
#[test]
fn a_streamed_response_always_ends_with_its_terminating_chunk() {
    let out = ply_test(
        r#"
import std.net (net)
import std.http (default_limits, respond_chunked, response, last_chunk, Http11)

fn counted(seed: Int) -> Option<{ chunk: Bytes, next: Int }> =
  if seed >= 3000 { None } else { Some({chunk: b"x", next: seed + 1}) }

test "a producer with more chunks than the stream bound" {
  with_cell[outbox](b"") { outbox -> {
    handle {
      respond_chunked(7, Http11, response(200, b""), true, default_limits(), 0, counted)
    } with {
      net.send[conn](c, payload, t) -> {
        cell_set(outbox, bytes_concat(cell_get(outbox), payload));
        Some(bytes_len(payload))
      },
    };
    assert(bytes_ends_with(cell_get(outbox), last_chunk()))
  } }
}
"#,
    );
    out.green(
        "`std.http.stream_chunks` must terminate the message before it gives up: leaving a \
         framed but unterminated chunked response on a connection the caller may reuse is \
         response smuggling",
    );
}

// --- The cost of routing ----------------------------------------------------

/// **The cost property, for the router.**
///
/// ADR 0013 §4: "W2 removed the property that a request's cost is proportional
/// to its bytes; a parser that re-scans the accumulated buffer on every read
/// would restore it as O(n²), quietly." Required test 26 states that property
/// for `parse_head` and nothing stated it for `route` — which is how
/// `percent_decode` came to accumulate its output with `push`, a `List`
/// operation that copies, once per escape. That is O(k²) element copies for k
/// escapes: a 7,681-byte path of escapes cost 125.8 ms against 0.1 ms for the
/// same-length plain path, at a length the default `max_request_line` admits,
/// and `route` reaches it for every request before it has decided anything.
///
/// The shape rather than the constant, because the constant is a machine's: four
/// times the escapes must cost about four times the time and not sixteen. The
/// threshold is deliberately loose — quadratic is 16x and this refuses at 9x —
/// so that a slow or contended machine cannot make it red while a re-introduced
/// `push` accumulator cannot make it green.
#[test]
fn routing_a_path_of_escapes_costs_its_length_and_not_its_square() {
    let source = r#"
import std.http
import std.router

fn table() -> List<router::Route<Int>> =
  [{method: http::Get, path: router::pattern_of_string("/orders/{id:Int}"), endpoint: 1}]

fn grow(s: String, k: Int) -> String = fold(range(0, k), s, |a: String, _i: Int| a ++ a)

fn escapes(k: Int) -> String = "/" ++ grow("%41", k)

test "k" { assert_eq(router::route(table(), http::Get, escapes(11)), router::NotFound) }
test "4k" { assert_eq(router::route(table(), http::Get, escapes(13)), router::NotFound) }
"#;
    let out = ply_test(source);
    out.green("the router refused a path of escapes rather than routing it");
    let one = duration_of(&out.output, "main.k");
    let four = duration_of(&out.output, "main.4k");
    assert!(
        four <= one * 9.0,
        "four times the escapes cost {four:.1}ms against {one:.1}ms for k, which is {:.1}x — \
         `std.router.percent_decode` is accumulating with an operation that copies\n\n{}",
        four / one,
        out.output
    );
}

/// The milliseconds `ply test` printed for one test, which is the only timing
/// this suite reads: a wall clock around the whole process would be measuring
/// compilation and process start.
#[track_caller]
fn duration_of(output: &str, name: &str) -> f64 {
    for line in output.lines() {
        let Some(rest) = line.split_once(name) else {
            continue;
        };
        let field = rest.1.trim();
        if let Some(ms) = field.strip_suffix("ms")
            && let Ok(value) = ms.trim().parse::<f64>()
        {
            // A test that took under a millisecond is noise, not a measurement,
            // and dividing by it would be too.
            return value.max(0.05);
        }
    }
    panic!("`ply test` printed no duration for `{name}`\n\n{output}");
}
