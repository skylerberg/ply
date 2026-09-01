//! An adversarial audit of `std.http`, written as an attacker rather than as an author.

use assert_cmd::prelude::*;
use std::path::Path;
use std::process::Command;

/// What one `ply test` run over a generated project answered.
struct Outcome {
    passed: bool,
    output: String,
}

impl Outcome {
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

/// The head parser, reduced to the number a table of cases reads as: the status an input earns,
/// `-1` for "still arriving" and `0` for accepted.
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

/// A serve loop over a scripted peer: the first `recv` answers `first` and every later one answers
/// `more`, and the bytes the server wrote come back.
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

/// The whole of request smuggling in one test: a message whose length can be read two ways must
/// never be accepted, whichever way this parser would have picked.
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

/// Everything about a request line that a second implementation might read differently.
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

/// The chunk-size line, which is the other place a length is decided.
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

/// The three size bounds, each bought with one packet.
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

/// **The measurement behind the header-block off-by-two**, pinned rather than argued about.
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

/// A body that never ends and a head that never ends both stop, and both stop because of a bound
/// rather than because the peer relented.
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

/// Response splitting, refused rather than sanitized — ADR 0013 §2.6.
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

/// The framing fields belong to `encode`, and a program that sets one has written a second answer
/// to "how long is this message".
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

/// A handler that fails takes the whole run with it: no 500, no response, and no further
/// connections.
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

/// **`max_stream_chunks` is a policy number, not the evaluator's call ceiling.**
#[test]
fn a_streamed_response_may_exceed_the_evaluators_call_budget() {
    let out = ply_test(
        r#"
import std.net (net)
import std.http (respond_chunked, response, last_chunk, Http11, Limits)

fn wide(n: Int) -> Limits =
  {max_request_line: 8192, max_header_bytes: 65536, max_header_count: 100,
   max_body: 1048576, max_chunk_size: 1048576, max_chunk_line: 4096,
   max_trailer_bytes: 8192, max_keep_alive: 100, max_stream_chunks: n,
   header_timeout_ms: 5000, body_timeout_ms: 30000, idle_timeout_ms: 5000,
   write_timeout_ms: 30000}

fn twenty_thousand(seed: Int) -> Option<{ chunk: Bytes, next: Int }> =
  if seed >= 20000 { None } else { Some({chunk: b"x", next: seed + 1}) }

test "a producer of twenty thousand chunks under a bound of fifty thousand" {
  with_cell[outbox](b"") { outbox -> {
    handle {
      assert(respond_chunked(7, Http11, response(200, b""), true, wide(50000), 0,
                             twenty_thousand))
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
        "`max_stream_chunks` must be answerable from how many chunks a response may have and          from nothing else: a bound a user cannot raise past `DEFAULT_MAX_CALLS` is the          evaluator's ceiling wearing an HTTP server's configuration surface",
    );
}

// --- The cost of routing ----------------------------------------------------

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

/// The milliseconds `ply test` printed for one test, which is the only timing this suite reads: a
/// wall clock around the whole process would be measuring compilation and process start.
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
            // A test that took under a millisecond is noise, not a measurement, and dividing by it
            // would be too.
            return value.max(0.05);
        }
    }
    panic!("`ply test` printed no duration for `{name}`\n\n{output}");
}
