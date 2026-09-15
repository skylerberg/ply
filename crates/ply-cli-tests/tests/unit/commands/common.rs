use ply_cli::commands::common::*;
use ply_span::{Diagnostic, SourceMap, Span, codes};

#[test]
fn location_is_one_based_and_names_the_file() {
    let mut sources = SourceMap::new();
    let id = sources.add("src/ledger.ply", "fn f() = 1\nfn g() = 2\n");
    assert_eq!(
        location(&sources, Span::new(id, 11, 13)).unwrap(),
        "src/ledger.ply:2:1"
    );
}

#[test]
fn a_dummy_span_has_no_location_rather_than_a_made_up_one() {
    let sources = SourceMap::new();
    assert_eq!(location(&sources, Span::DUMMY), None);
}

#[test]
fn diagnostic_json_carries_positions_not_raw_offsets() {
    let mut sources = SourceMap::new();
    let id = sources.add("t.ply", "fn f() = 1 + true\n");
    let d = Diagnostic::error(codes::TYPE_MISMATCH, "type mismatch")
        .primary(Span::new(id, 13, 17), "expected Int, found Bool");
    let v = diagnostic_json(&d, &sources);
    assert_eq!(v["code"], "E0201");
    assert_eq!(v["labels"][0]["start"]["line"], 1);
    assert_eq!(v["labels"][0]["snippet"], "true");
}

#[test]
fn plurals_do_not_say_one_errors() {
    assert_eq!(plural(1, "error"), "error");
    assert_eq!(plural(0, "error"), "errors");
    assert_eq!(plural(2, "group"), "groups");
    assert_eq!(plural(1, "body"), "body");
    assert_eq!(plural(0, "body"), "bodies");
    assert_eq!(plural(2, "key"), "keys");
}
