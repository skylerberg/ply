use ply_core::{Diagnostic, Edit, SourceMap, Span, codes};

#[test]
fn line_col_is_one_based_and_char_counted() {
    let mut sm = SourceMap::new();
    let id = sm.add("t.ply", "abc\nlét x = 1\n");
    let f = sm.get(id).unwrap();
    assert_eq!(f.line_col(0), (1, 1));
    assert_eq!(f.line_col(4), (2, 1));
    assert_eq!(f.line_col(7), (2, 3));
}

#[test]
fn a_span_past_the_end_of_the_text_or_reversed_has_an_empty_snippet_and_a_span_cutting_a_character_gives_a_replacement_character()
 {
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

#[test]
fn a_diagnostic_prints_its_heading_then_its_notes_and_fixes() {
    let d = Diagnostic::error(
        codes::INTERNAL_ERROR,
        "the `ply` program could not be built",
    )
    .primary(Span::DUMMY, "this is Ply's fault, not the program's")
    .note("the program is `packages/cli`")
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
            "  = the program is `packages/cli`",
            "  = fix: rebuild it",
        ]
        .join("\n")
    );
    assert_eq!(
        Diagnostic::warning(codes::DRAIN_INCOMPLETE, "drain incomplete").to_string(),
        "Warning[W0608]: drain incomplete"
    );
}

#[test]
fn placing_a_diagnostic_turns_each_label_that_has_a_source_span_into_a_note_with_file_line_and_column_and_drops_the_others()
 {
    let mut sources = SourceMap::new();
    let id = sources.add("app/m.ply", "fn f() -> Int = 1\nfn g() -> Int = \"x\"\n");
    let d = Diagnostic::error(codes::RUNTIME_ERROR, "type mismatch")
        .primary(Span::new(id, 34, 37), "expected Int, found String")
        .secondary(Span::new(id, 0, 2), "")
        .primary(Span::DUMMY, "nowhere")
        .note("an earlier note");
    assert_eq!(
        d.placed(&sources).to_string(),
        [
            "Error[E0502]: type mismatch",
            "  = an earlier note",
            "  = at app/m.ply:2:17: expected Int, found String",
            "  = at app/m.ply:1:1",
        ]
        .join("\n")
    );
}
