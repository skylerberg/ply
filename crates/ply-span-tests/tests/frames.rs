use ply_span::frames::{read_diagnostics, write_diagnostics};
use ply_span::{Diagnostic, Severity, SourceId, Span, codes};
use ply_span::{Edit, Fix};

fn sources() -> [SourceId; 2] {
    [SourceId(0), SourceId(1)]
}

fn two() -> Vec<Diagnostic> {
    vec![
        Diagnostic::error(codes::TYPE_MISMATCH, "type mismatch")
            .primary(Span::new(SourceId(1), 13, 17), "expected Int, found Bool")
            .secondary(
                Span::new(SourceId(0), 2, 5),
                "the parameter is declared here",
            )
            .note("`+` is Int -> Int -> Int")
            .note("a second note\nwith a line break")
            .fix(
                "add the annotation",
                vec![
                    Edit {
                        span: Span::new(SourceId(1), 17, 17),
                        text: ": Int".to_string(),
                    },
                    Edit {
                        span: Span::new(SourceId(0), 2, 5),
                        text: String::new(),
                    },
                ],
            ),
        Diagnostic::warning(codes::UNKNOWN_NAME, "no such name `frob`\nsaid twice")
            .primary(Span::DUMMY, ""),
    ]
}

#[test]
fn a_dump_round_trips_with_its_spans_labels_notes_and_severity() {
    let written = write_diagnostics(&two(), &sources()).expect("both diagnostics encode");
    let read = read_diagnostics(&written, &sources()).expect("what was written reads back");
    assert_eq!(read.len(), 2, "{written}");

    assert_eq!(read[0].code, codes::TYPE_MISMATCH);
    assert_eq!(read[0].severity, Severity::Error);
    assert_eq!(read[0].message, "type mismatch");
    assert_eq!(read[0].labels.len(), 2);
    assert_eq!(read[0].labels[0].span, Span::new(SourceId(1), 13, 17));
    assert_eq!(read[0].labels[0].message, "expected Int, found Bool");
    assert!(read[0].labels[0].primary);
    assert_eq!(read[0].labels[1].span, Span::new(SourceId(0), 2, 5));
    assert!(!read[0].labels[1].primary);
    assert_eq!(
        read[0].notes,
        vec![
            "`+` is Int -> Int -> Int".to_string(),
            "a second note\nwith a line break".to_string()
        ]
    );

    assert_eq!(
        read[0].fixes,
        vec![Fix {
            title: "add the annotation".to_string(),
            edits: vec![
                Edit {
                    span: Span::new(SourceId(1), 17, 17),
                    text: ": Int".to_string()
                },
                Edit {
                    span: Span::new(SourceId(0), 2, 5),
                    text: String::new()
                },
            ],
        }]
    );
    assert_eq!(read[1].code, codes::UNKNOWN_NAME);
    assert_eq!(read[1].severity, Severity::Warning);
    assert_eq!(read[1].message, "no such name `frob`\nsaid twice");
    assert_eq!(read[1].labels[0].span, Span::DUMMY);
    assert!(read[1].notes.is_empty());

    assert_eq!(
        write_diagnostics(&read, &sources()).expect("encodes again"),
        written
    );
}

#[test]
fn the_text_is_the_protocol_as_documented() {
    let d = [Diagnostic::error(codes::UNKNOWN_NAME, "no such name")
        .primary(Span::new(SourceId(1), 4, 8), "here")];
    assert_eq!(
        write_diagnostics(&d, &sources()).unwrap(),
        "diag 0 72\n\
         code 5\nE0101\
         severity 5\nerror\
         message 12\nno such name\
         label 12\n1 4 8 1\nhere"
    );
}

#[test]
fn a_module_index_is_the_position_in_the_sources_handed_over() {
    let d = [Diagnostic::error(codes::UNKNOWN_NAME, "m").primary(Span::new(SourceId(7), 0, 1), "")];
    let written = write_diagnostics(&d, &[SourceId(3), SourceId(7)]).unwrap();
    assert!(written.contains("label 8\n1 0 1 1\n"), "{written}");
    let read = read_diagnostics(&written, &[SourceId(9), SourceId(4)]).unwrap();
    assert_eq!(read[0].labels[0].span.source, SourceId(4));

    let outside = write_diagnostics(&d, &[SourceId(3)]).unwrap_err();
    assert!(outside.contains("source 7"), "{outside}");
    let beyond = read_diagnostics(&written, &[SourceId(9)]).unwrap_err();
    assert!(beyond.contains("module 1"), "{beyond}");
}

#[test]
fn an_unknown_field_key_is_refused_by_name() {
    let dump = "diag 0 40\ncode 5\nE0101severity 5\nerrorcolour 3\nred";
    let err = read_diagnostics(dump, &sources()).unwrap_err();
    assert!(err.contains("unknown field `colour`"), "{err}");
}

#[test]
fn an_unknown_frame_kind_is_refused_by_name() {
    let err = read_diagnostics("warn 0 0\n", &sources()).unwrap_err();
    assert!(err.contains("unknown frame kind `warn`"), "{err}");
}

#[test]
fn an_unknown_code_is_refused_by_name() {
    let dump = "diag 0 39\ncode 5\nE9999severity 5\nerrormessage 1\nm";
    let err = read_diagnostics(dump, &sources()).unwrap_err();
    assert!(err.contains("unknown code `E9999`"), "{err}");
}

#[test]
fn a_truncated_payload_is_an_error() {
    let whole = write_diagnostics(&two(), &sources()).unwrap();
    for cut in [whole.len() - 1, whole.len() / 2, 7, 3] {
        let err = read_diagnostics(&whole[..cut], &sources())
            .expect_err("a dump cut short must not read as fewer diagnostics");
        assert!(
            err.contains("truncated") || err.contains("never ends") || err.contains("length"),
            "cut at {cut}: {err}"
        );
    }
}

#[test]
fn a_diagnostic_missing_a_required_field_is_an_error() {
    let dump = "diag 0 28\ncode 5\nE0101severity 5\nerror";
    let err = read_diagnostics(dump, &sources()).unwrap_err();
    assert!(err.contains("no `message`"), "{err}");
}

#[test]
fn frames_are_numbered_in_report_order() {
    let mut one = write_diagnostics(&two()[..1], &sources()).unwrap();
    one.replace_range(0.."diag 0".len(), "diag 1");
    let err = read_diagnostics(&one, &sources()).unwrap_err();
    assert!(err.contains("numbered 1"), "{err}");
}
