use ply_span::frames::read_diagnostics;
use ply_span::{Edit, Fix, Severity, SourceId, Span, codes, intern_code};

fn sources() -> [SourceId; 2] {
    [SourceId(0), SourceId(1)]
}

/// `<header> <length>\n<body>`: a frame when the header is `diag <n>`, a field otherwise.
fn unit(header: &str, body: &str) -> String {
    format!("{header} {}\n{body}", body.len())
}

fn frame(index: usize, fields: &[(&str, &str)]) -> String {
    let payload: String = fields.iter().map(|(key, text)| unit(key, text)).collect();
    unit(&format!("diag {index}"), &payload)
}

/// Every field kind, a label outside every module, and texts that hold a line break.
fn two() -> String {
    frame(
        0,
        &[
            ("code", "E0201"),
            ("severity", "error"),
            ("message", "type mismatch"),
            ("label", "1 13 17 1\nexpected Int, found Bool"),
            ("label", "0 2 5 0\nthe parameter is declared here"),
            ("note", "`+` is Int -> Int -> Int"),
            ("note", "a second note\nwith a line break"),
            ("fix", "add the annotation"),
            ("edit", "1 17 17\n: Int"),
            ("edit", "0 2 5\n"),
        ],
    ) + &frame(
        1,
        &[
            ("code", "E0101"),
            ("severity", "warning"),
            ("message", "no such name `frob`\nsaid twice"),
            ("label", "4294967295 0 0 1\n"),
        ],
    )
}

#[test]
fn a_dump_reads_with_its_spans_labels_notes_fixes_and_severity() {
    let dump = two();
    let read = read_diagnostics(&dump, &sources()).expect("the dump reads");
    assert_eq!(read.len(), 2, "{dump}");

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
}

#[test]
fn the_text_is_the_protocol_as_documented() {
    let dump = "diag 0 72\n\
                code 5\nE0101\
                severity 5\nerror\
                message 12\nno such name\
                label 12\n1 4 8 1\nhere";
    assert_eq!(
        dump,
        frame(
            0,
            &[
                ("code", "E0101"),
                ("severity", "error"),
                ("message", "no such name"),
                ("label", "1 4 8 1\nhere"),
            ],
        ),
        "the frames these tests build are not the protocol's"
    );
    let read = read_diagnostics(dump, &sources()).unwrap();
    assert_eq!(read.len(), 1);
    assert_eq!(read[0].code, codes::UNKNOWN_NAME);
    assert_eq!(read[0].severity, Severity::Error);
    assert_eq!(read[0].message, "no such name");
    assert_eq!(read[0].labels[0].span, Span::new(SourceId(1), 4, 8));
    assert_eq!(read[0].labels[0].message, "here");
    assert!(read[0].labels[0].primary);
}

#[test]
fn a_module_index_is_the_position_in_the_sources_handed_over() {
    let dump = frame(
        0,
        &[
            ("code", "E0101"),
            ("severity", "error"),
            ("message", "m"),
            ("label", "1 0 1 1\n"),
        ],
    );
    let read = read_diagnostics(&dump, &[SourceId(9), SourceId(4)]).unwrap();
    assert_eq!(read[0].labels[0].span, Span::new(SourceId(4), 0, 1));

    let beyond = read_diagnostics(&dump, &[SourceId(9)]).unwrap_err();
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

/// The reader holds no list of codes, so a code no Rust source names reads back as written.
#[test]
fn a_code_reads_back_as_the_front_end_wrote_it() {
    let dump = "diag 0 39\ncode 5\nE0136severity 5\nerrormessage 1\nm";
    let read = read_diagnostics(dump, &sources()).unwrap();
    assert_eq!(read[0].code, codes::DEPENDENCY_VERSION);
    assert_eq!(read[0].message, "m");

    let unnamed = read_diagnostics(&dump.replace("E0136", "E9999"), &sources()).unwrap();
    assert_eq!(unnamed[0].code, "E9999");
}

#[test]
fn a_code_is_interned_once_however_often_it_is_read() {
    let a = intern_code("E0001");
    let b = intern_code(&String::from("E0001"));
    assert!(std::ptr::eq(a, b));
}

#[test]
fn a_truncated_payload_is_an_error() {
    let whole = two();
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
    let misnumbered = frame(
        1,
        &[("code", "E0101"), ("severity", "error"), ("message", "m")],
    );
    let err = read_diagnostics(&misnumbered, &sources()).unwrap_err();
    assert!(err.contains("numbered 1"), "{err}");
}
