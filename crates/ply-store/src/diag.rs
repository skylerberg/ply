//! `Diagnostic::code` is `&'static str`, which makes `Diagnostic` deserializable
//! only from `&'static` input — not from a file read at runtime. These mirror
//! types carry the code as an owned `String` on the wire and re-establish the
//! `'static` lifetime by interning on the way back in.

use ply_span::{Diagnostic, Label, Severity};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

#[derive(Serialize, Deserialize)]
pub(crate) struct DiagnosticRepr {
    severity: Severity,
    code: String,
    message: String,
    #[serde(default)]
    labels: Vec<Label>,
    #[serde(default)]
    notes: Vec<String>,
}

impl From<&Diagnostic> for DiagnosticRepr {
    fn from(d: &Diagnostic) -> Self {
        DiagnosticRepr {
            severity: d.severity,
            code: d.code.to_string(),
            message: d.message.clone(),
            labels: d.labels.clone(),
            notes: d.notes.clone(),
        }
    }
}

impl From<DiagnosticRepr> for Diagnostic {
    fn from(r: DiagnosticRepr) -> Self {
        Diagnostic {
            severity: r.severity,
            code: intern_code(&r.code),
            message: r.message,
            labels: r.labels,
            notes: r.notes,
        }
    }
}

/// Interning bounds the leak by the number of distinct codes the process has
/// ever read, rather than by the number of cache reads.
fn intern_code(code: &str) -> &'static str {
    static POOL: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut pool = POOL
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(existing) = pool.get(code) {
        return existing;
    }
    let leaked: &'static str = Box::leak(code.to_owned().into_boxed_str());
    pool.insert(leaked);
    leaked
}

#[cfg(test)]
mod tests {
    use super::*;
    use ply_span::{Span, codes};

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
}
