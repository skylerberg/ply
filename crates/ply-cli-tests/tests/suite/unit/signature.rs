use ply_cli::signature::*;
use ply_span::Symbol;
use ply_syntax::ast::{Mode, ModuleName};
use ply_ty::DefInfo;
use ply_ty::ty::{EffectAtom, Footprint, Resource, Row, RowVar, Scheme, TyVar, Type};

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

/// Wrapped at a fixed column, with the atoms hanging under the first one rather than run off the right edge.
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

/// A row variable is named once, by one printer, so the quantifier and row cannot drift apart across the split.
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
