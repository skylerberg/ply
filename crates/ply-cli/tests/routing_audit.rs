//! Adversarial audit of `std.router`, driven the way a service drives it.
//!
//! The route table is data and `route` is a pure function over it, which is
//! what makes this auditable at all — so the audit is written in Ply, against
//! the shipped `std.router`, and run by `ply test`. Every claim below is a
//! `det`, hermetic, cacheable test, which is the argument ADR 0013 makes for
//! writing the protocol in Ply rather than behind a host handler.
//!
//! What is attacked, and why each one is a security property rather than a
//! nicety:
//!
//! - **A path has one meaning.** `..`, `%2e%2e`, `%2E%2E`, `%2F`, a
//!   double-encoded escape and a NUL byte all have to reach the table as the
//!   segments the splitting rule says they are. Two answers to "which path is
//!   this" is how a route and an authorization check come to disagree.
//! - **Precedence is a function of the table, not of the order it was written
//!   in.** Otherwise `conflicts([]) == []` is a test that proves nothing.
//! - **404 and 405 are decided by the whole table**, and the `Allow` list is
//!   sorted, deduplicated and independent of declaration order.
//! - **A wildcard takes what is left and no more**, and cannot reach a path its
//!   literal prefix does not cover.
//!
//! `std.router`'s own tests cover the cases it was designed against. These are
//! the ones written to break it.

use assert_cmd::prelude::*;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn project(source: &str) -> TempDir {
    let dir = TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("main.ply"), source).expect("write main.ply");
    dir
}

fn ply(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("ply").expect("the ply binary");
    cmd.arg("--color").arg("never").current_dir(dir);
    cmd
}

fn output(out: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[track_caller]
fn run_tests(source: &str) {
    let dir = project(source);
    let out = ply(dir.path()).arg("test").output().expect("run ply test");
    let text = output(&out);
    assert!(
        out.status.success(),
        "the routing audit must pass:\n{text}\n--- source ---\n{source}"
    );
    assert!(
        !text.contains("0 passed"),
        "the audit must actually have run tests:\n{text}"
    );
}

const PRELUDE: &str = r#"import std.http
import std.router

type Endpoint = Admin | Files | Item | Slug | Posted | Deleted | Any | One

fn table() -> List<router::Route<Endpoint>> = [
  {method: http::Get, path: router::pattern_of_string("/admin"), endpoint: Admin},
  {method: http::Get, path: router::pattern_of_string("/files/{..rest}"), endpoint: Files},
  {method: http::Get, path: router::pattern_of_string("/orders/{id:Int}"), endpoint: Item},
  {method: http::Get, path: router::pattern_of_string("/orders/{slug}"), endpoint: Slug},
  {method: http::Post, path: router::pattern_of_string("/orders/{slug}"), endpoint: Posted},
  {method: http::Delete, path: router::pattern_of_string("/orders/{slug}"), endpoint: Deleted},
]

// The same six routes, written in the opposite order. A table is a value, so
// "the same table" is a claim a test can make.
fn upside_down() -> List<router::Route<Endpoint>> = [
  {method: http::Delete, path: router::pattern_of_string("/orders/{slug}"), endpoint: Deleted},
  {method: http::Post, path: router::pattern_of_string("/orders/{slug}"), endpoint: Posted},
  {method: http::Get, path: router::pattern_of_string("/orders/{slug}"), endpoint: Slug},
  {method: http::Get, path: router::pattern_of_string("/orders/{id:Int}"), endpoint: Item},
  {method: http::Get, path: router::pattern_of_string("/files/{..rest}"), endpoint: Files},
  {method: http::Get, path: router::pattern_of_string("/admin"), endpoint: Admin},
]

fn get(p: String) -> router::Matched<Endpoint> = router::route(table(), http::Get, p)

fn hit(m: router::Matched<Endpoint>) -> Endpoint =
  match m {
    router::Found(h) -> h.endpoint,
    router::NotFound -> panic("expected a route to match"),
    router::MethodNotAllowed(_) -> panic("expected a route to match under this method"),
  }

fn captured(m: router::Matched<Endpoint>, name: String) -> String =
  match m {
    router::Found(h) -> router::param_or(h.params, name, "<unbound>"),
    _ -> "<no match>",
  }

fn repeat(n: Int, s: String) -> String =
  fold(range(0, n), "", |acc: String, _i: Int| acc ++ s)
"#;

/// Traversal, in every spelling. `route` normalizes nothing, so a `..` reaches
/// the table as the segment it is and matches only a pattern that asked for one
/// — which is why a path cannot walk out of the route it was matched against.
#[test]
fn dot_segments_are_segments_and_never_traversal() {
    run_tests(&format!(
        r#"{PRELUDE}
test "a dot-dot segment never leaves the route it was matched against" {{
  assert_eq(router::path_segments("/public/../admin"), ["public", "..", "admin"]);
  assert_eq(get("/public/../admin"), router::NotFound);
  assert_eq(get("/files/../admin"), router::Found({{
    endpoint: Files, params: map_of_entries([{{key: "rest", value: "../admin"}}])}}));
  assert_eq(hit(get("/admin")), Admin)
}}

test "an encoded dot is a dot and is still only a segment" {{
  assert_eq(router::path_segments("/public/%2e%2e/admin"), ["public", "..", "admin"]);
  assert_eq(router::path_segments("/public/%2E%2E/admin"), ["public", "..", "admin"]);
  assert_eq(get("/public/%2e%2e/admin"), router::NotFound);
  assert_eq(captured(get("/files/%2e%2e/%2e%2e/etc/passwd"), "rest"), "../../etc/passwd")
}}

test "decoding happens exactly once" {{
  assert_eq(router::path_segments("/a/%252e%252e/b"), ["a", "%2e%2e", "b"]);
  assert_eq(router::percent_decode("%252F"), "%2F");
  assert_eq(router::normalize_path("/a/%2e%2e/b"), "/b");
  assert_eq(router::normalize_path("/a/%252e%252e/b"), "/a/%252e%252e/b")
}}

test "an encoded separator is a byte inside one segment" {{
  assert_eq(router::path_segments("/files%2F..%2F..%2Fetc"), ["files/../../etc"]);
  assert_eq(get("/files%2F..%2F..%2Fetc"), router::NotFound)
}}

test "a null byte is a byte and matches no literal that lacks one" {{
  assert_eq(get("/admin%00"), router::NotFound);
  assert_eq(len(router::path_segments("/admin%00")), 1);
  assert_eq(captured(get("/orders/x%00"), "slug"), "x\0")
}}
"#
    ));
}

/// The table is a value, so which route wins has to be a function of that value
/// rather than of the order it happened to be assembled in. When `conflicts` is
/// empty that is exactly what "unambiguous" is claiming, and this is the test
/// that makes the claim mean something.
#[test]
fn precedence_is_a_function_of_the_table_and_not_of_its_order() {
    run_tests(&format!(
        r#"{PRELUDE}
test "the reversed table answers every path the same way" {{
  assert_eq(router::conflicts(table()), []);
  assert_eq(router::conflicts(upside_down()), []);
  assert_eq(router::well_formed(table()), []);
  for_each_path(|p: String|
    assert_eq(router::route(table(), http::Get, p), router::route(upside_down(), http::Get, p)))
}}

test "a method that matches beats one that only would have" {{
  let t = [
    {{method: http::Post, path: router::pattern_of_string("/p"), endpoint: Posted}},
    {{method: http::Get, path: router::pattern_of_string("/p"), endpoint: Admin}},
  ];
  assert_eq(hit(router::route(t, http::Get, "/p")), Admin);
  assert_eq(hit(router::route(t, http::Post, "/p")), Posted)
}}

test "a typed parameter that does not parse falls through rather than 404ing" {{
  assert_eq(hit(get("/orders/7")), Item);
  assert_eq(hit(get("/orders/abc")), Slug);
  assert_eq(hit(get("/orders/007")), Slug);
  assert_eq(hit(get("/orders/")), Slug);
  assert_eq(captured(get("/orders/"), "slug"), "")
}}

fn for_each_path(check: (String) -> Unit) -> Unit =
  fold(paths(), (), |_acc: Unit, p: String| check(p))

fn paths() -> List<String> = [
  "/", "/admin", "/admin/", "/ADMIN", "/orders", "/orders/", "/orders/7",
  "/orders/007", "/orders/abc", "/orders/7/lines", "/files", "/files/",
  "/files/a/b", "/files/a%2Fb", "/a//b", "/nowhere", "", "/orders/%2e%2e",
]
"#
    ));
}

/// A `Rest` takes what is left and nothing else. The failure worth catching is
/// the one where it reaches past its own literal prefix, which would make one
/// wildcard route the whole service.
#[test]
fn a_wildcard_takes_what_is_left_and_no_more() {
    run_tests(&format!(
        r#"{PRELUDE}
test "a wildcard cannot reach outside its prefix" {{
  assert_eq(hit(get("/files")), Files);
  assert_eq(hit(get("/files/")), Files);
  assert_eq(captured(get("/files"), "rest"), "");
  assert_eq(captured(get("/files/"), "rest"), "");
  assert_eq(get("/file"), router::NotFound);
  assert_eq(get("/filesx"), router::NotFound);
  assert_eq(get("/x/files/a"), router::NotFound)
}}

test "a wildcard written first shadows nothing that follows it" {{
  let t = [
    {{method: http::Get, path: router::pattern_of_string("/{{..any}}"), endpoint: Any}},
    {{method: http::Get, path: router::pattern_of_string("/admin"), endpoint: Admin}},
    {{method: http::Get, path: router::pattern_of_string("/{{one}}"), endpoint: One}},
  ];
  assert_eq(hit(router::route(t, http::Get, "/admin")), Admin);
  assert_eq(hit(router::route(t, http::Get, "/other")), One);
  assert_eq(hit(router::route(t, http::Get, "/a/b")), Any);
  assert_eq(router::conflicts(t), [])
}}

test "a route that can only ever lose is a reported conflict" {{
  let t = [
    {{method: http::Get, path: router::pattern_of_string("/x/{{a}}"), endpoint: Admin}},
    {{method: http::Get, path: router::pattern_of_string("/x/{{b}}"), endpoint: Slug}},
  ];
  assert_eq(hit(router::route(t, http::Get, "/x/1")), Admin);
  assert_eq(router::conflicts(t), [{{first: 0, second: 1}}])
}}
"#
    ));
}

/// 404 and 405 are the two answers a table can give about a path it does not
/// serve under this method, and the difference between them is the difference
/// between "no such thing" and "not like that". RFC 9110 §15.5.6 makes the
/// `Allow` list a MUST, so the list has to be complete, deduplicated and a
/// function of the table.
#[test]
fn the_405_is_distinguished_from_the_404_and_lists_every_method() {
    run_tests(&format!(
        r#"{PRELUDE}
test "a path no pattern matches is 404 and one no method matches is 405" {{
  assert_eq(router::route(table(), http::Put, "/nowhere"), router::NotFound);
  assert_eq(router::route(table(), http::Put, "/orders/abc"),
            router::MethodNotAllowed([http::Delete, http::Get, http::Post]));
  assert_eq(router::route(table(), http::Put, "/orders/7"),
            router::MethodNotAllowed([http::Delete, http::Get, http::Post]))
}}

test "the Allow list does not depend on the order the table was written in" {{
  assert_eq(router::route(upside_down(), http::Put, "/orders/abc"),
            router::route(table(), http::Put, "/orders/abc"));
  assert_eq(router::allowed(upside_down(), "/orders/abc"),
            router::allowed(table(), "/orders/abc"))
}}

test "the Allow list is deduplicated across the routes that produced it" {{
  let t = [
    {{method: http::Get, path: router::pattern_of_string("/p/{{a}}"), endpoint: Slug}},
    {{method: http::Get, path: router::pattern_of_string("/p/x"), endpoint: Admin}},
    {{method: http::Get, path: router::pattern_of_string("/p/{{..r}}"), endpoint: Files}},
    {{method: http::Post, path: router::pattern_of_string("/p/x"), endpoint: Posted}},
  ];
  assert_eq(router::route(t, http::Put, "/p/x"),
            router::MethodNotAllowed([http::Get, http::Post]))
}}

test "an unrecognised method is routed and answered rather than refused" {{
  assert_eq(router::route(table(), http::Other("BREW"), "/orders/abc"),
            router::MethodNotAllowed([http::Delete, http::Get, http::Post]));
  let t = [{{method: http::Other("BREW"), path: router::pattern_of_string("/coffee"),
            endpoint: Admin}}];
  assert_eq(hit(router::route(t, http::Other("BREW"), "/coffee")), Admin);
  assert_eq(router::route(t, http::Get, "/coffee"),
            router::MethodNotAllowed([http::Other("BREW")]))
}}

test "a method is matched byte-exactly, so a lowercase token is another method" {{
  assert_eq(router::route(table(), http::Other("get"), "/admin"),
            router::MethodNotAllowed([http::Get]));
  assert_eq(get("/ADMIN"), router::NotFound);
  assert_eq(get("/Admin"), router::NotFound)
}}
"#
    ));
}

/// The shapes a path can take that are not a path: an empty one, a trailing
/// slash, an empty middle segment, one that carries a query the caller forgot
/// to strip, and one with far more segments than any table has patterns.
#[test]
fn the_edges_of_a_path_are_answered_rather_than_smoothed_over() {
    run_tests(&format!(
        r#"{PRELUDE}
test "empty segments are kept and a trailing slash is another path" {{
  assert_eq(router::path_segments("/"), [""]);
  assert_eq(router::path_segments("/a//b"), ["a", "", "b"]);
  assert_eq(router::path_segments("/admin/"), ["admin", ""]);
  assert_eq(get("/admin/"), router::NotFound);
  assert_eq(get("//admin"), router::NotFound);
  assert_eq(get("/"), router::NotFound)
}}

test "a query string a caller failed to strip does not match" {{
  assert_eq(router::path_segments("/admin?x=1"), ["admin?x=1"]);
  assert_eq(get("/admin?x=1"), router::NotFound)
}}

test "an invalid escape is text rather than a refusal" {{
  assert_eq(router::percent_decode("%zz"), "%zz");
  assert_eq(router::percent_decode("%4"), "%4");
  assert_eq(router::percent_decode("%"), "%");
  assert_eq(captured(get("/orders/%zz"), "slug"), "%zz")
}}

test "a path with two thousand segments is answered" {{
  let deep = repeat(2000, "/a");
  assert_eq(len(router::path_segments(deep)), 2000);
  assert_eq(router::route(table(), http::Get, deep), router::NotFound);
  assert_eq(string_len(captured(router::route(table(), http::Get, "/files" ++ deep), "rest")),
            3999)
}}
"#
    ));
}
