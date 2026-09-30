//! A deserializable `Diagnostic`: its `&'static str` code cannot borrow from a runtime file.

use ply_eval::{Diagnostic, Fix, Label, Plain, Severity, Sparse, intern_code};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct DiagnosticRepr {
    severity: Severity,
    code: String,
    message: String,
    #[serde(default)]
    labels: Vec<Label>,
    #[serde(default)]
    notes: Vec<String>,
    #[serde(default)]
    fixes: Vec<Fix>,
    #[serde(default, skip_serializing_if = "Sparse::is_empty")]
    values: Sparse<Plain>,
}

impl From<&Diagnostic> for DiagnosticRepr {
    fn from(d: &Diagnostic) -> Self {
        DiagnosticRepr {
            severity: d.severity,
            code: d.code.to_string(),
            message: d.message.clone(),
            labels: d.labels.clone(),
            notes: d.notes.clone(),
            fixes: d.fixes.to_vec(),
            values: d.values.clone(),
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
            fixes: r.fixes.into(),
            values: r.values,
        }
    }
}
