use crate::{Diagnostic, Severity, SourceMap};
use serde::Serialize;
use std::fmt::Write as _;

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
struct JsonEdit {
    file: String,
    start: JsonPos,
    end: JsonPos,
    text: String,
}

#[derive(Serialize)]
struct JsonFix {
    title: String,
    edits: Vec<JsonEdit>,
}

#[derive(Serialize)]
pub struct JsonDiagnostic {
    severity: Severity,
    code: &'static str,
    message: String,
    labels: Vec<JsonLabel>,
    notes: Vec<String>,
    fixes: Vec<JsonFix>,
}

fn pos(file: &crate::SourceFile, offset: u32) -> JsonPos {
    let (line, col) = file.line_col(offset);
    JsonPos { line, col, offset }
}

pub fn to_json(diag: &Diagnostic, sources: &SourceMap) -> JsonDiagnostic {
    let labels = diag
        .labels
        .iter()
        .filter_map(|l| {
            let file = sources.containing(l.span)?;
            let (sl, sc) = file.line_col(l.span.start);
            let (el, ec) = file.line_col(l.span.end);
            Some(JsonLabel {
                file: file.path.display().to_string(),
                start: JsonPos {
                    line: sl,
                    col: sc,
                    offset: l.span.start,
                },
                end: JsonPos {
                    line: el,
                    col: ec,
                    offset: l.span.end,
                },
                message: l.message.clone(),
                primary: l.primary,
                snippet: sources.snippet(l.span).into_owned(),
            })
        })
        .collect();

    let fixes = diag
        .fixes
        .iter()
        .map(|f| JsonFix {
            title: f.title.clone(),
            edits: f
                .edits
                .iter()
                .filter_map(|e| {
                    let file = sources.containing(e.span)?;
                    Some(JsonEdit {
                        file: file.path.display().to_string(),
                        start: pos(file, e.span.start),
                        end: pos(file, e.span.end),
                        text: e.text.clone(),
                    })
                })
                .collect(),
        })
        .collect();
    JsonDiagnostic {
        severity: diag.severity,
        code: diag.code,
        message: diag.message.clone(),
        labels,
        notes: diag.notes.clone(),
        fixes,
    }
}

/// A fix as a note line: its title, which says what the edits do.
fn fix_lines(diag: &Diagnostic) -> impl Iterator<Item = String> + '_ {
    diag.fixes.iter().map(|f| format!("fix: {}", f.title))
}

fn titled(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "Error",
        Severity::Warning => "Warning",
        Severity::Note => "Note",
    }
}

/// Colour is paint on the shape and nothing else: strip the escapes and the styled render is the
/// plain one, byte for byte.
fn painted(styled: bool, severity: Severity, text: &str) -> String {
    if !styled {
        return text.to_string();
    }
    let code = match severity {
        Severity::Error => "31",
        Severity::Warning => "33",
        Severity::Note => "2",
    };
    format!("\x1b[{code}m{text}\x1b[0m")
}

/// Bytes, not characters, because a span that cuts a character in half is still placed; the only
/// offsets that are known to be character boundaries are the line's own ends.
fn line_start(text: &[u8], offset: usize) -> usize {
    text[..offset]
        .iter()
        .rposition(|b| *b == b'\n')
        .map_or(0, |i| i + 1)
}

/// The newline closing the line that begins at `from`, or the end of the text.
fn end_of_line(text: &[u8], from: usize) -> usize {
    text[from..]
        .iter()
        .position(|b| *b == b'\n')
        .map_or(text.len(), |i| from + i)
}

/// Characters, counted as the bytes that are not the tail of one.
fn chars_between(text: &[u8], from: usize, to: usize) -> usize {
    text[from..to].iter().filter(|b| *b & 0xc0 != 0x80).count()
}

/// The caret run under one line: the span's own width, or one caret for an empty span, and never
/// past the end of the line the span opens on.
fn carets(text: &[u8], start: usize, end: usize) -> String {
    let from = line_start(text, start);
    let stop = end.min(end_of_line(text, from));
    let width = chars_between(text, start, stop).max(1);
    format!(
        "{}{}",
        " ".repeat(chars_between(text, from, start)),
        "^".repeat(width)
    )
}

/// One diagnostic: the heading, a block per label it can place, then a line per note and fix.
/// A label whose span [`SourceMap::containing`] cannot place is dropped, as `to_json` drops it,
/// and a diagnostic left with none is still its heading, so a builtin's error is never silently
/// lost. The shape is `crates/ply-cli/ply/diagnostic.ply`'s, byte for byte.
pub fn to_terminal(diag: &Diagnostic, sources: &SourceMap, styled: bool) -> String {
    let heading = format!("{}[{}]", titled(diag.severity), diag.code);
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{}: {}",
        painted(styled, diag.severity, &heading),
        diag.message
    );
    for l in &diag.labels {
        let Some(file) = sources.containing(l.span) else {
            continue;
        };
        let text = file.text.as_bytes();
        let start = l.span.start as usize;
        let from = line_start(text, start);
        let (line, col) = file.line_col(l.span.start);
        let said = if l.message.is_empty() {
            String::new()
        } else {
            format!(" {}", l.message)
        };
        let _ = writeln!(out, "  --> {}:{line}:{col}", file.path.display());
        let _ = writeln!(
            out,
            "   | {}",
            String::from_utf8_lossy(&text[from..end_of_line(text, from)])
        );
        let _ = writeln!(
            out,
            "   | {}{said}",
            painted(
                styled,
                diag.severity,
                &carets(text, start, l.span.end as usize)
            )
        );
    }
    for n in diag.notes.iter().cloned().chain(fix_lines(diag)) {
        let _ = writeln!(out, "  = {n}");
    }
    out
}

pub fn all_to_terminal(diags: &[Diagnostic], sources: &SourceMap, styled: bool) -> String {
    diags
        .iter()
        .map(|d| to_terminal(d, sources, styled))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Edit, Span, codes};

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
        let out = to_terminal(&d, &sm, false);
        assert!(out.contains("E0201"));
        assert!(out.contains("expected Int, found Bool"));
    }

    const TWO_LINES: &str = "fn f() -> Int = \"x\"\nfn g() -> Int = 1\n";

    /// Lines with the newline that ends each of them, as the renderer writes them.
    fn text(lines: &[&str]) -> String {
        lines.iter().map(|l| format!("{l}\n")).collect()
    }

    /// The shape, spelled out: it is the one `crates/ply-cli/ply/diagnostic.ply` writes, whose own
    /// tests spell out these same lines for this same diagnostic.
    #[test]
    fn a_placed_label_is_a_heading_a_place_the_line_and_a_caret_run() {
        let mut sm = SourceMap::new();
        let id = sm.add("m.ply", TWO_LINES);
        let d = Diagnostic::error(codes::TYPE_MISMATCH, "type mismatch: function body type")
            .primary(Span::new(id, 16, 19), "expected `Int`, found `String`")
            .note("the body must answer the written type");
        assert_eq!(
            to_terminal(&d, &sm, false),
            text(&[
                "Error[E0201]: type mismatch: function body type",
                "  --> m.ply:1:17",
                "   | fn f() -> Int = \"x\"",
                "   |                 ^^^ expected `Int`, found `String`",
                "  = the body must answer the written type",
            ])
        );
    }

    #[test]
    fn an_empty_span_gets_one_caret_and_a_fix_is_a_note_of_its_own() {
        let mut sm = SourceMap::new();
        let id = sm.add("m.ply", TWO_LINES);
        let d = Diagnostic::warning(codes::MISSING_SIGNATURE, "this needs a type")
            .primary(Span::new(id, 5, 5), "")
            .fix(
                "add the type",
                vec![Edit {
                    span: Span::new(id, 5, 5),
                    text: ": Int".to_string(),
                }],
            );
        assert_eq!(
            to_terminal(&d, &sm, false),
            text(&[
                "Warning[E0126]: this needs a type",
                "  --> m.ply:1:6",
                "   | fn f() -> Int = \"x\"",
                "   |      ^",
                "  = fix: add the type",
            ])
        );
    }

    /// A span over several lines underlines the line it opens on and stops at its newline.
    #[test]
    fn a_caret_run_never_leaves_the_line_the_span_opens_on() {
        let mut sm = SourceMap::new();
        let id = sm.add("m.ply", TWO_LINES);
        let d = Diagnostic::error(codes::TYPE_MISMATCH, "over two lines")
            .primary(Span::new(id, 16, 25), "from here");
        assert_eq!(
            to_terminal(&d, &sm, false),
            text(&[
                "Error[E0201]: over two lines",
                "  --> m.ply:1:17",
                "   | fn f() -> Int = \"x\"",
                "   |                 ^^^ from here",
            ])
        );
    }

    /// Colour is paint on one shape: strip the escapes and the styled render is the plain one.
    #[test]
    fn styling_a_render_changes_nothing_but_the_escapes() {
        let (sm, d) = fixture();
        let styled = to_terminal(&d, &sm, true);
        assert!(styled.contains("\x1b[31mError[E0201]\x1b[0m"), "{styled}");
        assert_eq!(
            styled.replace("\x1b[31m", "").replace("\x1b[0m", ""),
            to_terminal(&d, &sm, false)
        );
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
    fn a_span_outside_its_text_renders_without_its_label() {
        let mut sm = SourceMap::new();
        let id = sm.add("t.ply", "fn f() = \"é\"\n");
        for span in [Span::new(id, 21, 26), Span::new(id, 5, 2)] {
            assert_eq!(sm.snippet(span), "");
            let d = Diagnostic::error(codes::RUNTIME_ERROR, "boom").primary(span, "here");
            let v = serde_json::to_value(to_json(&d, &sm)).unwrap();
            assert_eq!(v["labels"].as_array().map(Vec::len), Some(0), "{v}");
            let out = to_terminal(&d, &sm, false);
            assert!(out.contains("E0502") && out.contains("boom"), "{out}");
            let file = sm.get(id).unwrap();
            let _ = (file.line_col(span.start), file.line_col(span.end));
        }
    }

    /// A span that cuts a character in half is a defect in whoever built it; the label is still
    /// placed, on the line it opens, and the snippet is what those bytes lossily say.
    #[test]
    fn a_span_cutting_a_character_in_half_is_still_placed() {
        let mut sm = SourceMap::new();
        let id = sm.add("t.ply", "fn f() = \"é\"\n");
        let span = Span::new(id, 11, 12);
        let d = Diagnostic::error(codes::RUNTIME_ERROR, "boom").primary(span, "here");

        assert_eq!(sm.snippet(span), "\u{fffd}");
        let v = serde_json::to_value(to_json(&d, &sm)).unwrap();
        assert_eq!(v["labels"].as_array().map(Vec::len), Some(1), "{v}");
        assert_eq!(v["labels"][0]["snippet"], "\u{fffd}");
        assert_eq!(
            to_terminal(&d, &sm, false),
            text(&[
                "Error[E0502]: boom",
                "  --> t.ply:1:12",
                "   | fn f() = \"é\"",
                "   |            ^ here",
            ])
        );
    }

    #[test]
    fn dummy_span_still_renders_a_header() {
        let sm = SourceMap::new();
        let d = Diagnostic::error(codes::RUNTIME_ERROR, "boom").primary(Span::DUMMY, "here");
        let out = to_terminal(&d, &sm, false);
        assert!(out.contains("E0502"));
        assert!(out.contains("boom"));
    }
}
