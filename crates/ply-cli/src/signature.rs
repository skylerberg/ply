//! Placing a list of atoms across lines, and the declared ones a body never touched.

use ply_ty::DefInfo;
use ply_ty::print::Printer;
use ply_ty::ty::EffectAtom;
use std::collections::BTreeSet;

/// Counted from the terminal's left edge, so callers subtract their indent.
pub const WIDTH: usize = 80;

/// Places `items` across lines, `first` before the first line and `rest` before every other.
pub fn fill(first: &str, rest: &str, items: &[String], suffix: &str, width: usize) -> Vec<String> {
    if items.is_empty() {
        return vec![format!("{first}{suffix}")];
    }
    let start = rest.chars().count();
    let mut lines = Vec::new();
    let mut line = first.to_string();
    let mut col = first.chars().count();
    let opened = col;

    for (i, item) in items.iter().enumerate() {
        let last = i + 1 == items.len();
        let piece = if last {
            format!("{item}{suffix}")
        } else {
            format!("{item}, ")
        };
        // The separator's trailing space may overflow, so an exact fit is not wrapped.
        let printed = piece.chars().count() - usize::from(!last);
        let fresh = if lines.is_empty() { opened } else { start };
        if col > fresh && col + printed > width {
            lines.push(line.trim_end().to_string());
            line = rest.to_string();
            col = start;
        }
        line.push_str(&piece);
        col += piece.chars().count();
    }
    lines.push(line.trim_end().to_string());
    lines
}

/// The declared atoms covering nothing this definition's body performed.
pub fn unperformed(def: &DefInfo) -> Vec<String> {
    let undone: BTreeSet<EffectAtom> = def
        .footprint
        .atoms()
        .filter(|a| !def.performed.atoms().any(|p| a.covers(p)))
        .cloned()
        .collect();
    Printer::new().atoms(&undone)
}
