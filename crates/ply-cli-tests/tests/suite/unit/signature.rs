use ply_cli::signature::*;
use ply_span::Symbol;
use ply_ty::DefInfo;
use ply_ty::ty::{EffectAtom, Footprint, Resource, Row, Scheme, Type};
use ply_ty::{Mode, ModuleName};

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

fn def(declared: Footprint, performed: Footprint) -> DefInfo {
    DefInfo {
        name: Symbol::new("m.create_order"),
        module: ModuleName::from_dotted("m"),
        simple_name: Symbol::new("create_order"),
        scheme: Scheme {
            ty_vars: Vec::new(),
            row_vars: Vec::new(),
            label_vars: Vec::new(),
            ty: Type::Fn {
                params: vec![Type::Con(Symbol::new("Request"), Vec::new())],
                ret: Box::new(Type::Con(Symbol::new("Response"), Vec::new())),
                effects: Row::empty(),
            },
        },
        footprint: declared,
        performed,
        row_aliases: Vec::new(),
        constraints: Vec::new(),
        spec: Vec::new(),
        // The slack a frame carries is read off the two rows and nothing else.
        internally_effectful: true,
        span: ply_span::Span::DUMMY,
    }
}

#[test]
fn a_frame_wider_than_its_body_names_what_it_gave_up() {
    let slack = unperformed(&def(
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
    assert_eq!(slack, ["db.read[orders]"]);
}

#[test]
fn a_frame_exactly_its_body_gives_nothing_up() {
    let exact = Footprint::from_atoms([atom("log", Mode::Write, None)]);
    assert!(unperformed(&def(exact.clone(), exact)).is_empty());
    assert!(unperformed(&def(Footprint::empty(), Footprint::empty())).is_empty());
}
