//! A deserializable `Diagnostic`: its `&'static str` code cannot borrow from a runtime file.

use ply_span::{Diagnostic, Label, Severity};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

#[derive(Serialize, Deserialize)]
pub struct DiagnosticRepr {
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

/// Leaks each distinct code once, rather than once per cache read.
pub fn intern_code(code: &str) -> &'static str {
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
