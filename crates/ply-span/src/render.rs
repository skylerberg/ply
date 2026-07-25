use crate::{Diagnostic, Severity, SourceMap};
use ariadne::{Color, Config, Label as ALabel, Report, ReportKind};
use serde::Serialize;

#[derive(Serialize)]
struct JsonPos {
    line: u32,
    col: u32,
    offset: u32,
}

#[derive(Serialize)]
struct JsonLabel {
    file: String,
    start: JsonPos,
    end: JsonPos,
    message: String,
    primary: bool,
    snippet: String,
}

#[derive(Serialize)]
pub struct JsonDiagnostic {
    severity: Severity,
    code: &'static str,
    message: String,
    labels: Vec<JsonLabel>,
    notes: Vec<String>,
}

pub fn to_json(diag: &Diagnostic, sources: &SourceMap) -> JsonDiagnostic {
    let labels = diag
        .labels
        .iter()
        .filter_map(|l| {
            let file = sources.get(l.span.source)?;
            let (sl, sc) = file.line_col(l.span.start);
            let (el, ec) = file.line_col(l.span.end);
            Some(JsonLabel {
                file: file.path.display().to_string(),
                start: JsonPos { line: sl, col: sc, offset: l.span.start },
                end: JsonPos { line: el, col: ec, offset: l.span.end },
                message: l.message.clone(),
                primary: l.primary,
                snippet: sources.snippet(l.span).to_string(),
            })
        })
        .collect();

    JsonDiagnostic {
        severity: diag.severity,
        code: diag.code,
        message: diag.message.clone(),
        labels,
        notes: diag.notes.clone(),
    }
}

/// Diagnostics whose spans are all dummy still print a header, so a builtin's
/// error is never silently dropped.
pub fn to_terminal(diag: &Diagnostic, sources: &SourceMap) -> String {
    let kind = match diag.severity {
        Severity::Error => ReportKind::Error,
        Severity::Warning => ReportKind::Warning,
        Severity::Note => ReportKind::Advice,
    };

    let Some(anchor) = diag
        .labels
        .iter()
        .map(|l| l.span)
        .find(|s| !s.is_dummy() && sources.get(s.source).is_some())
    else {
        let notes = diag
            .notes
            .iter()
            .map(|n| format!("\n  = {n}"))
            .collect::<String>();
        return format!("{:?}[{}]: {}{}\n", diag.severity, diag.code, diag.message, notes);
    };

    let anchor_path = sources.get(anchor.source).unwrap().path.display().to_string();
    let mut report = Report::build(kind, (anchor_path.clone(), anchor.range()))
        .with_code(diag.code)
        .with_message(&diag.message)
        .with_config(Config::default().with_index_type(ariadne::IndexType::Byte));

    for l in &diag.labels {
        let Some(file) = sources.get(l.span.source) else { continue };
        let path = file.path.display().to_string();
        report = report.with_label(
            ALabel::new((path, l.span.range()))
                .with_message(&l.message)
                .with_color(if l.primary { Color::Red } else { Color::Blue })
                .with_order(if l.primary { 0 } else { 1 }),
        );
    }
    for n in &diag.notes {
        report = report.with_note(n);
    }

    let cache = ariadne::sources(
        sources
            .files()
            .iter()
            .map(|f| (f.path.display().to_string(), &*f.text)),
    );

    let mut out = Vec::new();
    match report.finish().write(cache, &mut out) {
        Ok(()) => String::from_utf8_lossy(&out).into_owned(),
        Err(_) => format!("[{}] {}\n", diag.code, diag.message),
    }
}

pub fn all_to_terminal(diags: &[Diagnostic], sources: &SourceMap) -> String {
    diags
        .iter()
        .map(|d| to_terminal(d, sources))
        .collect::<Vec<_>>()
        .join("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{codes, Span};

    fn fixture() -> (SourceMap, Diagnostic) {
        let mut sm = SourceMap::new();
        let id = sm.add("t.ply", "fn f() = 1 + true\n");
        let d = Diagnostic::error(codes::TYPE_MISMATCH, "type mismatch")
            .primary(Span::new(id, 13, 17), "expected Int, found Bool")
            .note("`+` is Int -> Int -> Int");
        (sm, d)
    }

    #[test]
    fn terminal_render_includes_code_and_label() {
        let (sm, d) = fixture();
        let out = to_terminal(&d, &sm);
        assert!(out.contains("E0201"));
        assert!(out.contains("expected Int, found Bool"));
    }

    #[test]
    fn json_render_carries_positions_and_snippet() {
        let (sm, d) = fixture();
        let v = serde_json::to_value(to_json(&d, &sm)).unwrap();
        assert_eq!(v["code"], "E0201");
        assert_eq!(v["labels"][0]["start"]["line"], 1);
        assert_eq!(v["labels"][0]["start"]["col"], 14);
        assert_eq!(v["labels"][0]["snippet"], "true");
        assert_eq!(v["labels"][0]["primary"], true);
    }

    #[test]
    fn dummy_span_still_renders_a_header() {
        let sm = SourceMap::new();
        let d = Diagnostic::error(codes::RUNTIME_ERROR, "boom").primary(Span::DUMMY, "here");
        let out = to_terminal(&d, &sm);
        assert!(out.contains("E0502"));
        assert!(out.contains("boom"));
    }
}
