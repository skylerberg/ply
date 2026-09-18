//! How `ply check --types` renders a signature, and the effect-set provenance `--explain` adds.

use ply_span::Symbol;
use ply_syntax::ast::ModuleName;
use ply_ty::print::Printer;
use ply_ty::ty::{Footprint, Row, Scheme, Type};
use ply_ty::{DefInfo, Front};
use std::collections::{BTreeSet, HashMap};

/// Counted from the terminal's left edge, so callers subtract their indent.
pub const WIDTH: usize = 80;

/// A signature split at its top-level effect row.
pub struct Split {
    pub head: String,
    /// `None` for a pure definition, which prints no row at all.
    pub row: Option<RowText>,
}

/// A row as separate atoms, so no atom is ever broken across a line.
pub struct RowText {
    pub atoms: Vec<String>,
    /// Named by the head's [`Printer`], so the row and the head agree on it.
    pub tail: Option<String>,
}

impl RowText {
    fn of_row(row: &Row, printer: &mut Printer) -> RowText {
        RowText {
            atoms: row.atoms.iter().map(|a| a.to_string()).collect(),
            // A tail alone prints as the name this printer chose for it.
            tail: row.tail.map(|v| {
                printer.row(&Row {
                    atoms: BTreeSet::new(),
                    tail: Some(v),
                })
            }),
        }
    }

    fn of_footprint(footprint: &Footprint) -> RowText {
        RowText {
            atoms: footprint.atoms().map(|a| a.to_string()).collect(),
            tail: None,
        }
    }
}

pub fn split(scheme: &Scheme) -> Split {
    let mut printer = Printer::new();
    match &scheme.ty {
        Type::Fn {
            params,
            ret,
            effects,
        } if !effects.is_pure() => {
            let head = Scheme {
                ty_vars: scheme.ty_vars.clone(),
                row_vars: scheme.row_vars.clone(),
                ty: Type::Fn {
                    params: params.clone(),
                    ret: ret.clone(),
                    effects: Row::empty(),
                },
            };
            let head = printer.scheme(&head);
            let row = RowText::of_row(effects, &mut printer);
            Split {
                head,
                row: Some(row),
            }
        }
        _ => Split {
            head: printer.scheme(scheme),
            row: None,
        },
    }
}

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

pub fn definition_lines(
    indent: usize,
    label_width: usize,
    label: &str,
    scheme: &Scheme,
) -> Vec<String> {
    let split = split(scheme);
    let mut lines = vec![format!("{label:label_width$} : {}", split.head)];
    if let Some(row) = &split.row {
        let gutter = " ".repeat(label_width.max(label.chars().count()) + 3);
        lines.extend(row_lines(indent, &gutter, row));
    }
    lines
}

/// `/ {a, b, c}`, wrapped, with continuations aligned inside the brace.
fn row_lines(indent: usize, gutter: &str, row: &RowText) -> Vec<String> {
    let first = format!("{gutter}/ {{");
    let rest = format!("{gutter}   ");
    let suffix = match &row.tail {
        Some(tail) if row.atoms.is_empty() => return vec![format!("{gutter}/ {tail}")],
        Some(tail) => format!(" | {tail}}}"),
        None => "}".to_string(),
    };
    fill(&first, &rest, &row.atoms, &suffix, WIDTH - indent)
}

/// One `effect set` as `--explain` reports it.
pub struct EffectSetView {
    pub name: String,
    /// Program-wide atoms, sorted as a row is.
    pub atoms: Vec<String>,
    /// Definitions whose written row names it, directly or through another set.
    pub used_by: usize,
}

impl EffectSetView {
    pub fn lines(&self, indent: usize) -> Vec<String> {
        let mut lines = vec![format!("effect set {}", self.name)];
        lines.extend(fill("  = {", "     ", &self.atoms, "}", WIDTH - indent));
        lines.push(format!(
            "  used by {} {}",
            self.used_by,
            crate::commands::common::plural(self.used_by, "definition")
        ));
        lines
    }
}

/// What a definition's row was written as, and what its body actually performed.
#[derive(Default)]
pub struct Provenance {
    pub aliases: Vec<String>,
    /// `None` when it equals the declared row.
    pub performed: Option<RowText>,
    /// Declared minus performed.
    pub unperformed: Vec<String>,
}

impl Provenance {
    pub fn is_empty(&self) -> bool {
        self.aliases.is_empty() && self.performed.is_none()
    }

    pub fn lines(&self, indent: usize) -> Vec<String> {
        let width = WIDTH - indent;
        let mut lines = Vec::new();
        if !self.aliases.is_empty() {
            lines.extend(fill(
                "  written as     / {",
                "                     ",
                &self.aliases,
                "}",
                width,
            ));
        }
        if let Some(performed) = &self.performed {
            lines.extend(fill(
                "  body performs  {",
                "                  ",
                &performed.atoms,
                "}",
                width,
            ));
        }
        if !self.unperformed.is_empty() {
            lines.extend(fill(
                "  declared, not performed: ",
                "    ",
                &self.unperformed,
                "",
                width,
            ));
        }
        lines
    }
}

pub fn provenance(def: &DefInfo) -> Provenance {
    let aliases: Vec<String> = def.row_aliases.iter().map(|a| a.to_string()).collect();
    let unperformed: Vec<String> = def
        .footprint
        .atoms()
        .filter(|a| !def.performed.contains(a))
        .map(|a| a.to_string())
        .collect();
    Provenance {
        aliases,
        performed: (!unperformed.is_empty()).then(|| RowText::of_footprint(&def.performed)),
        unperformed,
    }
}

pub fn effect_sets(front: &Front, module: &ModuleName, defs: &[&DefInfo]) -> Vec<EffectSetView> {
    let Some(sets) = front.effect_sets.get(module.as_symbol()) else {
        return Vec::new();
    };
    let includes: HashMap<&Symbol, &[Symbol]> = sets
        .iter()
        .map(|set| (&set.name, set.includes.as_slice()))
        .collect();

    let mut uses: HashMap<Symbol, usize> = HashMap::new();
    for def in defs {
        let mut reached: BTreeSet<Symbol> = BTreeSet::new();
        let mut frontier: Vec<Symbol> = def.row_aliases.clone();
        while let Some(name) = frontier.pop() {
            if !reached.insert(name.clone()) {
                continue;
            }
            if let Some(inner) = includes.get(&name) {
                frontier.extend(inner.iter().cloned());
            }
        }
        for name in reached {
            *uses.entry(name).or_default() += 1;
        }
    }

    sets.iter()
        .map(|set| EffectSetView {
            name: set.name.to_string(),
            atoms: set.atoms.atoms().map(|a| a.to_string()).collect(),
            used_by: uses.get(&set.name).copied().unwrap_or(0),
        })
        .collect()
}
