//! How `ply check --types` renders a signature, and the effect-set provenance `--explain` adds to
//! it.

use ply_core::print::Printer;
use ply_core::ty::{EffectAtom, Footprint, Resource, Row, Scheme, Type};
use ply_core::{CheckOutput, DefInfo};
use ply_span::Symbol;
use ply_syntax::ast::{AtomExpr, Item, ModuleName, Program, QName};
use ply_syntax::resolve::{Namespace, Resolved};
use std::collections::{BTreeSet, HashMap};

/// The column a wrapped line may reach, counted from the left edge of the terminal — so every
/// function below takes the indent it will be printed at and subtracts it.
pub const WIDTH: usize = 80;

/// The builtin effect `cell`, which is written bare and resolves to itself.
const CELL: &str = "cell";

/// A signature split at its top-level effect row.
pub struct Split {
    /// Everything up to the row: quantifiers, parameters and result.
    pub head: String,
    /// `None` for a pure definition — which prints no row at all, so that an empty one is the
    /// absence of a line rather than a `{}` to skip over.
    pub row: Option<RowText>,
}

/// A row as the pieces a line filler can place: never a pre-joined string, so that no atom is ever
/// broken across a line.
pub struct RowText {
    pub atoms: Vec<String>,
    /// The row variable, already named by the same [`Printer`] the head was printed with, so
    /// `{net.write[conn] | e}` and `<s | e>` agree.
    pub tail: Option<String>,
}

impl RowText {
    fn of_row(row: &Row, printer: &mut Printer) -> RowText {
        RowText {
            atoms: row.atoms.iter().map(|a| a.to_string()).collect(),
            // A tail alone prints as its own name, which is how the name this printer chose is read
            // back out of it.
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

/// Places `items` across as many lines as they need, `first` before the first and `rest` before
/// every other.
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
        // The separator's trailing space ends the line rather than overflowing it, so a row that
        // fits exactly is not wrapped for one blank column.
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

/// The lines one definition contributes to `ply check --types`.
pub fn definition_lines(
    indent: usize,
    label_width: usize,
    label: &str,
    scheme: &Scheme,
) -> Vec<String> {
    let split = split(scheme);
    let mut lines = vec![format!("{label:label_width$} : {}", split.head)];
    if let Some(row) = &split.row {
        // Under the head, not under the name: the row belongs to the type.
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

// --- effect sets ------------------------------------------------------------

/// One `effect set` as `--explain` reports it.
pub struct EffectSetView {
    pub name: String,
    /// The expansion, resolved to program-wide atoms and sorted exactly as a row is — so that these
    /// are the same strings the definitions below print.
    pub atoms: Vec<String>,
    /// Definitions in this module whose written row names it, directly or through another set that
    /// does.
    pub used_by: usize,
}

impl EffectSetView {
    /// The block what the reviewing command prints specifies: the name, the expansion, and how much of the module is
    /// annotated with it.
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

/// What a definition's row was *written* as, and what its body actually performed — the two things
/// the expansion alone cannot show.
#[derive(Default)]
pub struct Provenance {
    /// The sets its row named, in source order.
    pub aliases: Vec<String>,
    /// The body's inferred row, and `None` when it equals the declared one — which is every
    /// unannotated definition, and would otherwise print the same row twice under most of a file.
    pub performed: Option<RowText>,
    /// Declared minus performed: what the annotation admits that the body never reaches.
    pub unperformed: Vec<String>,
}

impl Provenance {
    pub fn is_empty(&self) -> bool {
        self.aliases.is_empty() && self.performed.is_none()
    }

    /// Indented under the definition it belongs to.
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

/// What `--explain` adds to one definition's signature.
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

/// Every `effect set` a parsed module declares, in source order.
pub fn effect_sets(
    program: &Program,
    resolved: &Resolved,
    check: &CheckOutput,
    module: &ModuleName,
    defs: &[&DefInfo],
) -> Vec<EffectSetView> {
    let Some(index) = resolved.index_of(module) else {
        return Vec::new();
    };
    let Some(ast) = program.modules.get(index) else {
        return Vec::new();
    };

    let mut includes: HashMap<Symbol, Vec<Symbol>> = HashMap::new();
    let mut order = Vec::new();
    for item in &ast.items {
        let Item::EffectSet(def) = item else { continue };
        order.push(def);
        includes.insert(
            def.name.name.clone(),
            def.includes.iter().map(|q| q.symbol().clone()).collect(),
        );
    }
    if order.is_empty() {
        return Vec::new();
    }

    // A row names a set directly; that set may include others.
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

    order
        .into_iter()
        .map(|def| {
            let atoms: BTreeSet<EffectAtom> = def
                .expansion
                .iter()
                .filter_map(|a| atom_of(a, resolved, check, index))
                .collect();
            EffectSetView {
                name: def.name.name.to_string(),
                atoms: atoms.iter().map(|a| a.to_string()).collect(),
                used_by: uses.get(&def.name.name).copied().unwrap_or(0),
            }
        })
        .collect()
}

/// A written atom as the program-wide atom a row would carry.
fn atom_of(
    atom: &AtomExpr,
    resolved: &Resolved,
    check: &CheckOutput,
    module: usize,
) -> Option<EffectAtom> {
    let effect = effect_name(&atom.effect, resolved, check, module)?;
    let resource = match &atom.resource {
        Some(r) => Resource::Named(r.name.clone()),
        None => Resource::Singleton,
    };
    Some(EffectAtom::new(effect, resource, atom.mode))
}

fn effect_name(
    q: &QName,
    resolved: &Resolved,
    check: &CheckOutput,
    module: usize,
) -> Option<Symbol> {
    if q.is_bare() && q.symbol().as_str() == CELL {
        return Some(Symbol::new(CELL));
    }
    match resolved.lookup(module, Namespace::Effect, q) {
        Ok(binding) if check.effects.contains_key(&binding.qualified) => {
            Some(binding.qualified.clone())
        }
        _ if q.is_bare() && ply_core::prelude::is_prelude_effect(q.symbol()) => {
            Some(q.symbol().clone())
        }
        _ => None,
    }
}
