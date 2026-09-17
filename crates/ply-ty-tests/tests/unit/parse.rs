use ply_span::{SourceId, Symbol};
use ply_ty::print::region_type_name;
use ply_ty::*;
use std::collections::BTreeMap;

fn atom(effect: &str, resource: Option<&str>, mode: Mode) -> EffectAtom {
    EffectAtom::new(
        effect,
        resource.map_or(Resource::Singleton, |r| Resource::Named(Symbol::new(r))),
        mode,
    )
}

fn func(params: Vec<Type>, ret: Type, effects: Row) -> Type {
    Type::Fn {
        params,
        ret: Box::new(ret),
        effects,
    }
}

fn record(fields: &[(&str, Type)]) -> Type {
    Type::Record(
        fields
            .iter()
            .map(|(k, t)| (Symbol::new(k), t.clone()))
            .collect::<BTreeMap<_, _>>(),
    )
}

/// Every shape the printer can write, each in a form that exercises one rule.
fn shapes() -> Vec<Type> {
    let db = atom("std.db", Some("users"), Mode::Read);
    let fs = atom("std.fs", None, Mode::Write);
    vec![
        Type::int(),
        Type::Var(TyVar(9)),
        Type::list(Type::option(Type::string())),
        Type::map(Type::Var(TyVar(3)), Type::list(Type::Var(TyVar(3)))),
        Type::Con(Symbol::new("store.orders.Order"), vec![]),
        func(vec![], Type::unit(), Row::empty()),
        func(vec![Type::int(), Type::int()], Type::bool(), Row::empty()),
        func(
            vec![Type::Var(TyVar(0))],
            Type::Var(TyVar(1)),
            Row::open(RowVar(4)),
        ),
        func(
            vec![Type::int()],
            Type::unit(),
            Row::closed([db.clone(), fs.clone()]),
        ),
        func(
            vec![Type::int()],
            Type::unit(),
            Row {
                atoms: [db.clone()].into(),
                tail: Some(RowVar(0)),
            },
        ),
        func(
            vec![func(
                vec![Type::Var(TyVar(0))],
                Type::Var(TyVar(1)),
                Row::empty(),
            )],
            func(
                vec![Type::list(Type::Var(TyVar(0)))],
                Type::list(Type::Var(TyVar(1))),
                Row::open(RowVar(1)),
            ),
            Row::empty(),
        ),
        record(&[("_0", Type::int()), ("_1", Type::string())]),
        record(&[("_0", Type::int())]),
        record(&[("name", Type::string()), ("age", Type::int())]),
        record(&[]),
        record(&[("go", func(vec![], Type::unit(), Row::closed([fs.clone()])))]),
        Type::Con(
            Symbol::new("Cell"),
            vec![Type::con(&region_type_name("counter")), Type::int()],
        ),
        Type::Con(
            Symbol::new("Cell"),
            vec![Type::Var(TyVar(5)), Type::list(Type::int())],
        ),
        Type::secret(Type::string()),
        Type::con("U8"),
    ]
}

#[test]
fn every_printed_type_reads_back_to_the_same_text() {
    for t in shapes() {
        let text = print_type(&t);
        let back = parse_type(&text).unwrap_or_else(|e| panic!("{text}: {e}"));
        assert_eq!(print_type(&back), text);
    }
}

#[test]
fn parsing_is_structural_where_the_printer_is_injective() {
    let t = func(
        vec![Type::int(), Type::list(Type::Var(TyVar(2)))],
        record(&[("_0", Type::Var(TyVar(2))), ("_1", Type::bool())]),
        Row::closed([atom("std.db", Some("users"), Mode::Write)]),
    );
    let back = parse_type(&print_type(&t)).unwrap();
    let Type::Fn {
        params,
        ret,
        effects,
    } = back
    else {
        panic!("not a function");
    };
    assert_eq!(params, vec![Type::int(), Type::list(Type::Var(TyVar(0)))]);
    assert_eq!(
        *ret,
        record(&[("_0", Type::Var(TyVar(0))), ("_1", Type::bool())])
    );
    assert_eq!(
        effects,
        Row::closed([atom("std.db", Some("users"), Mode::Write)])
    );
}

#[test]
fn a_cell_with_a_region_keeps_it_and_one_without_gets_a_fresh_variable() {
    let known = parse_type("Cell[counter]<Int>").unwrap();
    assert_eq!(
        known,
        Type::Con(
            Symbol::new("Cell"),
            vec![Type::con(&region_type_name("counter")), Type::int()]
        )
    );
    let unknown = parse_type("Cell<Int>").unwrap();
    assert_eq!(
        unknown,
        Type::Con(Symbol::new("Cell"), vec![Type::Var(TyVar(0)), Type::int()])
    );
    assert_eq!(print_type(&unknown), "Cell<Int>");
}

#[test]
fn schemes_read_their_quantifiers_back_in_head_order() {
    let s = Scheme {
        ty_vars: vec![TyVar(7), TyVar(2)],
        row_vars: vec![RowVar(9)],
        ty: func(
            vec![Type::Var(TyVar(2))],
            Type::Var(TyVar(7)),
            Row::open(RowVar(9)),
        ),
    };
    let text = print_scheme(&s);
    assert_eq!(text, "<b, a | e>(a) -> b / e");
    let back = parse_scheme(&text).unwrap();
    assert_eq!(back.ty_vars, vec![TyVar(0), TyVar(1)]);
    assert_eq!(back.row_vars, vec![RowVar(0)]);
    assert_eq!(
        back.ty,
        func(
            vec![Type::Var(TyVar(1))],
            Type::Var(TyVar(0)),
            Row::open(RowVar(0))
        )
    );
    assert_eq!(print_scheme(&back), text);
    for text in ["<a>(a) -> a", "<| e>() -> Unit / e", "Int", "<a, b>(a, b)"] {
        assert_eq!(print_scheme(&parse_scheme(text).unwrap()), text);
    }
}

#[test]
fn a_scheme_head_may_quantify_a_variable_the_body_never_uses() {
    let s = Scheme {
        ty_vars: vec![TyVar(0), TyVar(1)],
        row_vars: vec![],
        ty: Type::Var(TyVar(1)),
    };
    let text = print_scheme(&s);
    assert_eq!(text, "<b, a>a");
    assert_eq!(print_scheme(&parse_scheme(text.as_str()).unwrap()), text);
}

#[test]
fn rows_read_back_in_every_form_the_printer_and_the_display_use() {
    let db = atom("std.db", Some("users"), Mode::Read);
    let net = atom("std.net", None, Mode::Write);
    let rows = [
        Row::empty(),
        Row::closed([db.clone(), net.clone()]),
        Row::open(RowVar(3)),
        Row {
            atoms: [db.clone()].into(),
            tail: Some(RowVar(0)),
        },
    ];
    for r in rows {
        let text = print_row(&r);
        assert_eq!(print_row(&parse_row(&text).unwrap()), text, "{text}");
        assert_eq!(print_row(&parse_row(&r.to_string()).unwrap()), text, "{r}");
    }
}

#[test]
fn atoms_and_footprints_read_the_check_dumps_form() {
    let a = parse_atom("std.db.read[users]").unwrap();
    assert_eq!(a, atom("std.db", Some("users"), Mode::Read));
    let b = parse_atom("net.write").unwrap();
    assert_eq!(b, atom("net", None, Mode::Write));
    assert_eq!(parse_footprint("").unwrap(), Footprint::empty());
    let f = parse_footprint("net.write,std.db.read[users]").unwrap();
    assert_eq!(f, Footprint::from_atoms([a, b]));
    assert_eq!(
        parse_footprint(
            &f.atoms()
                .map(|a| a.to_string())
                .collect::<Vec<_>>()
                .join(",")
        )
        .unwrap(),
        f
    );
}

#[test]
fn malformed_text_is_refused_with_the_position() {
    for (text, expected) in [
        ("List<Int", "expected `>`"),
        ("(Int", "expected `)`"),
        ("(Int)", "expected `->`"),
        ("{a: Int", "expected `}`"),
        ("{a Int}", "expected `:`"),
        ("Int Int", "expected the end of the text"),
        ("", "expected a type"),
        ("(a) -> b / {db.read | X}", "not a row variable"),
        ("<A>Int", "not a type variable"),
    ] {
        let err = parse_scheme(text).unwrap_err();
        assert!(err.contains(expected), "{text}: {err}");
        assert!(err.contains("at byte"), "{text}: {err}");
    }
    assert!(
        parse_atom("db")
            .unwrap_err()
            .contains("no `.read` or `.write`")
    );
    assert!(
        parse_atom("db.peek")
            .unwrap_err()
            .contains("not `read` or `write`")
    );
    assert!(parse_row("{db.read | e").is_err());
}

/// The program every real scheme comes from.
fn std_check() -> CheckOutput {
    let mut program = ply_syntax::ast::Program {
        modules: Vec::new(),
    };
    for (i, (name, text)) in ply_std::sources().enumerate() {
        let mut module =
            ply_syntax::parse_module(SourceId(i as u32), ModuleName::from_dotted(name), text)
                .unwrap_or_else(|d| panic!("{name} does not parse: {d:?}"));
        let expansion = ply_derive::expand_module(&mut module);
        assert!(expansion.is_empty(), "{name}: {expansion:?}");
        program.modules.push(module);
    }
    let resolved = ply_syntax::resolve(&mut program).expect("the standard library resolves");
    ply_core::check_program(&program, &resolved).expect("the standard library checks")
}

#[test]
fn every_scheme_and_type_the_standard_library_publishes_reads_back() {
    let check = std_check();
    let mut schemes = 0;
    let mut types = 0;
    let mut scheme = |s: &Scheme, what: &str| {
        let text = print_scheme(s);
        let back = parse_scheme(&text).unwrap_or_else(|e| panic!("{what}: {text}: {e}"));
        assert_eq!(print_scheme(&back), text, "{what}");
        schemes += 1;
    };
    let mut ty = |t: &Type, what: &str| {
        let text = print_type(t);
        let back = parse_type(&text).unwrap_or_else(|e| panic!("{what}: {text}: {e}"));
        assert_eq!(print_type(&back), text, "{what}");
        types += 1;
    };
    for (name, d) in &check.defs {
        scheme(&d.scheme, name);
    }
    for (name, c) in &check.ctors {
        scheme(&c.scheme, name);
        for f in &c.fields {
            ty(f, name);
        }
    }
    for (name, e) in &check.effects {
        for o in e.ops.values() {
            for p in &o.params {
                ty(p, name);
            }
            ty(&o.ret, name);
            if let Some(s) = &o.scheme {
                scheme(s, name);
            }
        }
    }
    for l in &check.laws {
        for b in &l.binders {
            ty(&b.ty, &l.key);
        }
    }
    assert!(schemes > 100, "{schemes} schemes");
    assert!(types > 20, "{types} types");
}
