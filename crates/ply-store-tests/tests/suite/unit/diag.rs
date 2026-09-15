use ply_span::{Diagnostic, Severity, Span, codes};
use ply_store::diag::*;

#[test]
fn round_trip_preserves_every_field() {
    let source = ply_span::SourceId(7);
    let original = Diagnostic::error(codes::ASSERTION_FAILED, "expected 0, found -5")
        .primary(Span::new(source, 12, 20), "here")
        .secondary(Span::new(source, 4, 8), "from this call")
        .note("suspects: apply_debit");

    let json = serde_json::to_string(&DiagnosticRepr::from(&original)).unwrap();
    let back: Diagnostic = serde_json::from_str::<DiagnosticRepr>(&json)
        .unwrap()
        .into();

    assert_eq!(back.severity, Severity::Error);
    assert_eq!(back.code, codes::ASSERTION_FAILED);
    assert_eq!(back.message, original.message);
    assert_eq!(back.notes, original.notes);
    assert_eq!(back.labels.len(), 2);
    assert_eq!(back.labels[0].span, Span::new(source, 12, 20));
    assert!(back.labels[0].primary);
    assert_eq!(back.labels[1].span, Span::new(source, 4, 8));
    assert!(!back.labels[1].primary);
}

#[test]
fn unknown_code_survives_instead_of_being_dropped() {
    let d = Diagnostic::warning("E9999", "from a future version");
    let json = serde_json::to_string(&DiagnosticRepr::from(&d)).unwrap();
    let back: Diagnostic = serde_json::from_str::<DiagnosticRepr>(&json)
        .unwrap()
        .into();
    assert_eq!(back.code, "E9999");
    assert_eq!(back.severity, Severity::Warning);
}

#[test]
fn interning_is_stable_across_reads() {
    let a = intern_code("E0001");
    let b = intern_code(&String::from("E0001"));
    assert!(std::ptr::eq(a, b));
}

#[test]
fn missing_optional_fields_default_to_empty() {
    let r: DiagnosticRepr =
        serde_json::from_str(r#"{"severity":"note","code":"E0101","message":"m"}"#).unwrap();
    let d: Diagnostic = r.into();
    assert!(d.labels.is_empty());
    assert!(d.notes.is_empty());
    assert_eq!(d.severity, Severity::Note);
}
