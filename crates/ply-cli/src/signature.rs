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

#[cfg(test)]
mod tests {
    use super::*;
    use ply_core::ty::{Resource, RowVar, TyVar};
    use ply_syntax::ast::Mode;

    fn atom(effect: &str, mode: Mode, resource: Option<&str>) -> EffectAtom {
        EffectAtom::new(
            Symbol::new(effect),
            match resource {
                Some(r) => Resource::Named(Symbol::new(r)),
                None => Resource::Singleton,
            },
            mode,
        )
    }

    fn row(atoms: Vec<EffectAtom>, tail: Option<RowVar>) -> Row {
        Row {
            atoms: atoms.into_iter().collect(),
            tail,
        }
    }

    fn fn_scheme(row: Row, row_vars: Vec<RowVar>) -> Scheme {
        Scheme {
            ty_vars: Vec::new(),
            row_vars,
            ty: Type::Fn {
                params: vec![Type::Con(Symbol::new("Request"), Vec::new())],
                ret: Box::new(Type::Con(Symbol::new("Response"), Vec::new())),
                effects: row,
            },
        }
    }

    #[test]
    fn a_pure_definition_prints_no_row_at_all() {
        let scheme = fn_scheme(Row::empty(), Vec::new());
        let lines = definition_lines(5, 12, "endpoint_of", &scheme);
        assert_eq!(lines, ["endpoint_of  : (Request) -> Response"]);
    }

    /// The property W3's exit criterion rests on: an endpoint's row is legible at a glance, which
    /// means wrapped at a fixed column with the atoms hanging under the first one rather than run
    /// off the right edge.
    #[test]
    fn a_long_row_wraps_inside_the_brace_and_aligns() {
        let scheme = fn_scheme(
            row(
                vec![
                    atom("db", Mode::Read, Some("inventory")),
                    atom("db", Mode::Read, Some("orders")),
                    atom("db", Mode::Read, Some("users")),
                    atom("db", Mode::Write, Some("orders")),
                    atom("http", Mode::Write, Some("outbound")),
                    atom("log", Mode::Write, None),
                ],
                None,
            ),
            Vec::new(),
        );
        let lines = definition_lines(5, 12, "create_order", &scheme);
        assert_eq!(
            lines,
            [
                "create_order : (Request) -> Response",
                "               / {db.read[inventory], db.read[orders], db.write[orders],",
                "                  db.read[users], http.write[outbound], log.write}",
            ]
        );
        assert!(
            lines.iter().all(|l| l.chars().count() + 5 <= WIDTH),
            "the indent these are printed at is part of the budget: {lines:?}"
        );
    }

    /// A row variable is named once, by one printer, so the quantifier and the row cannot drift
    /// apart across the split.
    #[test]
    fn a_row_variable_survives_the_split_with_one_name() {
        let v = RowVar(3);
        let scheme = fn_scheme(
            row(vec![atom("net", Mode::Write, Some("conn"))], Some(v)),
            vec![v],
        );
        let lines = definition_lines(5, 4, "serve", &scheme);
        assert_eq!(
            lines,
            [
                "serve : <| e>(Request) -> Response",
                "        / {net.write[conn] | e}"
            ]
        );
    }

    #[test]
    fn a_bare_row_variable_prints_without_braces() {
        let v = RowVar(0);
        let scheme = fn_scheme(row(Vec::new(), Some(v)), vec![v]);
        let lines = definition_lines(5, 3, "run", &scheme);
        assert_eq!(lines, ["run : <| e>(Request) -> Response", "      / e"]);
    }

    #[test]
    fn a_type_variable_keeps_its_letter_across_the_split() {
        let t = TyVar(1);
        let v = RowVar(2);
        let scheme = Scheme {
            ty_vars: vec![t],
            row_vars: vec![v],
            ty: Type::Fn {
                params: vec![Type::Var(t)],
                ret: Box::new(Type::Var(t)),
                effects: row(vec![atom("log", Mode::Write, None)], Some(v)),
            },
        };
        let lines = definition_lines(5, 2, "id", &scheme);
        assert_eq!(lines, ["id : <a | e>(a) -> a", "     / {log.write | e}"]);
    }

    #[test]
    fn filling_never_splits_an_item_even_when_one_item_is_too_wide() {
        let items = vec!["a".repeat(90), "b".to_string()];
        let lines = fill("[", " ", &items, "]", 20);
        assert_eq!(lines[0], format!("[{}, ", "a".repeat(90)).trim_end());
        assert_eq!(lines[1], " b]");
    }

    #[test]
    fn an_empty_row_still_renders_its_delimiters() {
        assert_eq!(fill("= {", "   ", &[], "}", 40), ["= {}"]);
    }

    /// The set block is the one place an alias is allowed to appear, and it has to carry its
    /// expansion beside it or it is the abbreviation without the definition.
    #[test]
    fn the_set_block_names_the_set_and_spells_out_its_expansion() {
        let view = EffectSetView {
            name: "Web".to_string(),
            atoms: vec![
                "db.read[inventory]".to_string(),
                "db.read[orders]".to_string(),
                "db.read[users]".to_string(),
                "db.write[orders]".to_string(),
                "http.write[outbound]".to_string(),
                "log.write".to_string(),
            ],
            used_by: 4,
        };
        assert_eq!(
            view.lines(5).join("\n"),
            "\
effect set Web
  = {db.read[inventory], db.read[orders], db.read[users], db.write[orders],
     http.write[outbound], log.write}
  used by 4 definitions"
        );
    }

    #[test]
    fn one_use_is_singular() {
        let view = EffectSetView {
            name: "Web".to_string(),
            atoms: vec!["log.write".to_string()],
            used_by: 1,
        };
        assert_eq!(view.lines(5)[2], "  used by 1 definition");
    }

    fn def(aliases: &[&str], declared: Footprint, performed: Footprint) -> DefInfo {
        DefInfo {
            name: Symbol::new("m.create_order"),
            module: ModuleName::from_dotted("m"),
            simple_name: Symbol::new("create_order"),
            scheme: fn_scheme(Row::empty(), Vec::new()),
            footprint: declared,
            performed,
            row_aliases: aliases.iter().copied().map(Symbol::new).collect(),
            constraints: Vec::new(),
            spec: Vec::new(),
            // Provenance rendering reads the two rows and nothing else.
            internally_effectful: true,
            span: ply_span::Span::DUMMY,
        }
    }

    #[test]
    fn provenance_names_the_alias_and_the_difference_the_alias_hides() {
        let p = provenance(&def(
            &["Web"],
            Footprint::from_atoms([
                atom("db", Mode::Read, Some("orders")),
                atom("db", Mode::Read, Some("users")),
                atom("log", Mode::Write, None),
            ]),
            Footprint::from_atoms([
                atom("db", Mode::Read, Some("users")),
                atom("log", Mode::Write, None),
            ]),
        ));
        assert_eq!(
            p.lines(5),
            [
                "  written as     / {Web}",
                "  body performs  {db.read[users], log.write}",
                "  declared, not performed: db.read[orders]",
            ]
        );
    }

    /// An alias whose expansion the body reaches entirely costs nothing, so there is no difference
    /// to report and the row is not printed twice.
    #[test]
    fn an_alias_the_body_uses_completely_reports_only_how_it_was_written() {
        let exact = Footprint::from_atoms([atom("log", Mode::Write, None)]);
        let p = provenance(&def(&["Web"], exact.clone(), exact));
        assert_eq!(p.lines(5), ["  written as     / {Web}"]);
        assert!(p.unperformed.is_empty());
    }

    #[test]
    fn a_definition_that_named_no_set_and_declared_no_slack_has_nothing_to_print() {
        let p = provenance(&def(&[], Footprint::empty(), Footprint::empty()));
        assert!(p.is_empty());
        assert!(p.lines(5).is_empty());
    }

    /// A row written out by hand can be over-broad too, and the cost is the same one.
    #[test]
    fn a_written_row_wider_than_its_body_is_reported_without_any_alias() {
        let p = provenance(&def(
            &[],
            Footprint::from_atoms([
                atom("db", Mode::Read, Some("orders")),
                atom("log", Mode::Write, None),
            ]),
            Footprint::from_atoms([atom("log", Mode::Write, None)]),
        ));
        assert_eq!(
            p.lines(5),
            [
                "  body performs  {log.write}",
                "  declared, not performed: db.read[orders]",
            ]
        );
    }
}
