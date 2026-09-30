use ply_eval::{Diagnostic, Edit, SourceMap, Span, codes};

#[test]
fn line_col_is_one_based_and_char_counted() {
    let mut sm = SourceMap::new();
    let id = sm.add("t.ply", "abc\nlét x = 1\n");
    let f = sm.get(id).unwrap();
    assert_eq!(f.line_col(0), (1, 1));
    assert_eq!(f.line_col(4), (2, 1));
    // `é` is two bytes; the column after it is still counted in chars.
    assert_eq!(f.line_col(7), (2, 3));
}

#[test]
fn a_span_is_bounded_by_its_texts_length_alone() {
    let mut sm = SourceMap::new();
    let id = sm.add("t.ply", "fn f() = \"é\"\n");
    for outside in [Span::new(id, 21, 26), Span::new(id, 5, 2)] {
        assert!(sm.containing(outside).is_none());
        assert_eq!(sm.snippet(outside), "");
    }
    let halved = Span::new(id, 11, 12);
    assert!(sm.containing(halved).is_some());
    assert_eq!(sm.snippet(halved), "\u{fffd}");
}

/// What `ply` prints for a failure no program is there to render: the heading, a line per note and
/// per fix, and no label, since nothing holds the source a label points into.
#[test]
fn a_diagnostic_prints_its_heading_then_its_notes_and_fixes() {
    let d = Diagnostic::error(
        codes::INTERNAL_ERROR,
        "the `ply` program could not be built",
    )
    .primary(Span::DUMMY, "this is Ply's fault, not the program's")
    .note("the program is `crates/ply-cli/ply`")
    .fix(
        "rebuild it",
        vec![Edit {
            span: Span::DUMMY,
            text: String::new(),
        }],
    );
    assert_eq!(
        d.to_string(),
        [
            "Error[E0505]: the `ply` program could not be built",
            "  = the program is `crates/ply-cli/ply`",
            "  = fix: rebuild it",
        ]
        .join("\n")
    );
    assert_eq!(
        Diagnostic::warning(codes::CACHE_CORRUPT, "cache corrupt").to_string(),
        "Warning[W0602]: cache corrupt"
    );
}

/// Placed for a reader that will not hold the sources: each label that lands in one keeps its place
/// as a note, and a label outside them is dropped rather than misplaced.
#[test]
fn a_placed_diagnostic_keeps_each_label_it_can_place_as_a_note() {
    let mut sources = SourceMap::new();
    let id = sources.add("app/m.ply", "fn f() -> Int = 1\nfn g() -> Int = \"x\"\n");
    let d = Diagnostic::error(codes::TYPE_MISMATCH, "type mismatch")
        .primary(Span::new(id, 34, 37), "expected Int, found String")
        .secondary(Span::new(id, 0, 2), "")
        .primary(Span::DUMMY, "nowhere")
        .note("an earlier note");
    assert_eq!(
        d.placed(&sources).to_string(),
        [
            "Error[E0201]: type mismatch",
            "  = an earlier note",
            "  = at app/m.ply:2:17: expected Int, found String",
            "  = at app/m.ply:1:1",
        ]
        .join("\n")
    );
}
