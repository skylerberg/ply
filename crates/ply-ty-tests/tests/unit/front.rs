use ply_span::{Diagnostic, SourceId, Span, Symbol, codes};
use ply_ty::*;
use std::collections::BTreeSet;

fn sym(s: &str) -> Symbol {
    Symbol::new(s)
}

fn hash(n: u8) -> DefHash {
    DefHash([n; 32])
}

fn atom(effect: &str, resource: Option<&str>, mode: Mode) -> EffectAtom {
    EffectAtom::new(
        effect,
        resource.map_or(Resource::Singleton, |r| Resource::Named(Symbol::new(r))),
        mode,
    )
}

fn span(source: u32, start: u32, end: u32) -> Span {
    Span::new(SourceId(source), start, end)
}

/// One of everything the protocol carries, over two modules.
fn sample() -> Front {
    let db = atom("std.db", Some("users"), Mode::Read);
    let net = atom("std.net", None, Mode::Write);
    let mut check = CheckOutput::default();
    check.modules.insert(
        sym("std.db"),
        ModuleInfo {
            name: ModuleName::from_dotted("std.db"),
            source: SourceId(0),
            items: vec![sym("std.db.Db"), sym("std.db.Row"), sym("std.db.query")],
            imports: vec![],
        },
    );
    check.modules.insert(
        sym("m"),
        ModuleInfo {
            name: ModuleName::from_dotted("m"),
            source: SourceId(1),
            items: vec![
                sym("m.count"),
                sym("m.Shape"),
                sym("m.Circle"),
                sym("m.Square"),
            ],
            imports: vec![ModuleName::from_dotted("std.db")],
        },
    );
    let scheme = Scheme {
        ty_vars: vec![TyVar(3)],
        row_vars: vec![RowVar(1)],
        ty: Type::Fn {
            params: vec![Type::list(Type::Var(TyVar(3)))],
            ret: Box::new(Type::int()),
            effects: Row {
                atoms: [db.clone()].into(),
                tail: Some(RowVar(1)),
            },
        },
    };
    check.defs.insert(
        sym("m.count"),
        DefInfo {
            name: sym("m.count"),
            module: ModuleName::from_dotted("m"),
            simple_name: sym("count"),
            scheme: scheme.clone(),
            footprint: Footprint::from_atoms([db.clone(), net.clone()]),
            performed: Footprint::from_atoms([db.clone()]),
            row_aliases: vec![sym("io"), sym("store")],
            constraints: vec![DefConstraint {
                deriver: Deriver::Eq,
                param: 0,
            }],
            spec: vec![
                SpecInfo {
                    kind: SpecKind::Requires,
                    index: 0,
                    footprint: Footprint::empty(),
                    span: span(1, 10, 20),
                },
                SpecInfo {
                    kind: SpecKind::Ensures,
                    index: 1,
                    footprint: Footprint::empty(),
                    span: span(1, 21, 30),
                },
            ],
            internally_effectful: true,
            span: span(1, 0, 40),
        },
    );
    check.defs.insert(
        sym("std.db.query"),
        DefInfo {
            name: sym("std.db.query"),
            module: ModuleName::from_dotted("std.db"),
            simple_name: sym("query"),
            scheme: Scheme::mono(Type::Fn {
                params: vec![Type::string()],
                ret: Box::new(Type::list(Type::con("std.db.Row"))),
                effects: Row::closed([db.clone()]),
            }),
            footprint: Footprint::from_atoms([db.clone()]),
            performed: Footprint::from_atoms([db.clone()]),
            row_aliases: vec![],
            constraints: vec![],
            spec: vec![],
            internally_effectful: false,
            span: span(0, 5, 50),
        },
    );
    check.tests.push(TestInfo {
        name: "counts the users".to_string(),
        module: ModuleName::from_dotted("m"),
        key: sym("m.counts the users"),
        index: 0,
        nondet: true,
        footprint: Footprint::from_atoms([db.clone()]),
        span: span(1, 41, 60),
    });
    check.laws.push(LawInfo {
        name: "count is non-negative".to_string(),
        module: ModuleName::from_dotted("m"),
        key: sym("m.count is non-negative"),
        index: 0,
        binders: vec![LawBinder {
            name: sym("xs"),
            ty: Type::list(Type::int()),
            span: span(1, 62, 70),
        }],
        has_guard: true,
        host: false,
        footprint: Footprint::empty(),
        span: span(1, 61, 90),
    });
    let mut ops = indexmap::IndexMap::new();
    ops.insert(
        sym("query"),
        OpInfo {
            name: sym("query"),
            mode: Mode::Read,
            resource_param: true,
            params: vec![Type::string(), Type::int()],
            ret: Type::list(Type::con("std.db.Row")),
            span: span(0, 1, 4),
            scheme: None,
        },
    );
    ops.insert(
        sym("spawn"),
        OpInfo {
            name: sym("spawn"),
            mode: Mode::Write,
            resource_param: false,
            params: vec![],
            ret: Type::unit(),
            span: Span::DUMMY,
            scheme: Some(Scheme {
                ty_vars: vec![TyVar(0)],
                row_vars: vec![RowVar(0)],
                ty: Type::Fn {
                    params: vec![Type::Fn {
                        params: vec![],
                        ret: Box::new(Type::Var(TyVar(0))),
                        effects: Row::open(RowVar(0)),
                    }],
                    ret: Box::new(Type::unit()),
                    effects: Row::open(RowVar(0)),
                },
            }),
        },
    );
    check.effects.insert(
        sym("std.db"),
        EffectInfo {
            name: sym("std.db"),
            module: ModuleName::from_dotted("std.db"),
            simple_name: sym("db"),
            nondet: false,
            ops,
            span: span(0, 0, 4),
        },
    );
    check.ctors.insert(
        sym("m.Circle"),
        CtorInfo {
            name: sym("m.Circle"),
            module: ModuleName::from_dotted("m"),
            simple_name: sym("Circle"),
            type_name: sym("m.Shape"),
            index: 0,
            arity: 1,
            fields: vec![Type::float()],
            scheme: Scheme::mono(Type::Fn {
                params: vec![Type::float()],
                ret: Box::new(Type::con("m.Shape")),
                effects: Row::empty(),
            }),
            span: span(1, 91, 99),
        },
    );
    check.ctors.insert(
        sym("Some"),
        CtorInfo {
            name: sym("Some"),
            module: ModuleName::anonymous(),
            simple_name: sym("Some"),
            type_name: sym("Option"),
            index: 0,
            arity: 1,
            fields: vec![Type::Var(TyVar(0))],
            scheme: Scheme {
                ty_vars: vec![TyVar(0)],
                row_vars: vec![],
                ty: Type::Fn {
                    params: vec![Type::Var(TyVar(0))],
                    ret: Box::new(Type::option(Type::Var(TyVar(0)))),
                    effects: Row::empty(),
                },
            },
            span: Span::DUMMY,
        },
    );

    let mut hashes = HashOutput::default();
    hashes.defs.insert(sym("std.db.query"), hash(1));
    hashes.defs.insert(sym("m.count"), hash(2));
    hashes.own.insert(sym("std.db.query"), hash(3));
    hashes.own.insert(sym("m.count"), hash(4));
    hashes.decls.insert(sym("std.db.Db"), hash(5));
    hashes.decls.insert(sym("m.Shape"), hash(6));
    hashes.tests.push(hash(7));
    hashes.laws.push(hash(8));
    hashes.law_texts.push(hash(9));
    hashes
        .specs
        .insert(sym("m.count"), vec![hash(10), hash(11)]);
    hashes
        .spec_texts
        .insert(sym("m.count"), vec![hash(12), hash(13)]);
    for (name, deps) in [
        ("std.db.Db", vec![]),
        ("std.db.query", vec!["std.db.Db"]),
        ("m.Shape", vec![]),
        ("m.count", vec!["std.db.query", "m.Shape"]),
        ("m.counts the users", vec!["m.count"]),
        ("m.count is non-negative", vec!["m.count"]),
    ] {
        hashes
            .deps
            .insert(sym(name), deps.iter().map(|d| sym(d)).collect());
        let closure: BTreeSet<Symbol> = deps.iter().chain([&name]).map(|d| sym(d)).collect();
        hashes.closure.insert(sym(name), closure);
    }

    Front {
        diagnostics: vec![
            Diagnostic::warning(codes::UNKNOWN_NAME, "a warning that does not stop the dump")
                .primary(span(1, 0, 1), "here"),
        ],
        order: vec![sym("std.db"), sym("m")],
        check,
        hashes,
        hash_order: vec![
            Hashed::Def(sym("std.db.Db")),
            Hashed::Def(sym("std.db.query")),
            Hashed::Def(sym("m.Shape")),
            Hashed::Def(sym("m.count")),
            Hashed::Test(0),
            Hashed::Law(0),
        ],
        ordinals: vec![
            (
                sym("std.db"),
                vec![Ordinal::Fn(sym("std.db.query"), vec![])],
            ),
            (
                sym("m"),
                vec![
                    Ordinal::Fn(sym("m.count"), vec![SpecKind::Requires, SpecKind::Ensures]),
                    Ordinal::Test(sym("m.counts the users")),
                    Ordinal::Law(sym("m.count is non-negative")),
                ],
            ),
        ],
        bodies: vec![
            (sym("std.db.Db"), vec![0, 9]),
            (sym("std.db.query"), vec![0, 1, 2, 255]),
            (sym("m.Shape"), vec![]),
            (sym("m.count"), vec![1, 0, 0, 0, 0, 16]),
        ],
        test_bodies: vec![vec![0, 7, 7]],
    }
}

const SOURCES: [SourceId; 2] = [SourceId(0), SourceId(1)];

#[test]
fn a_front_writes_reads_and_writes_to_the_same_text() {
    let front = sample();
    let text = write_front(&front, &SOURCES).unwrap();
    let back = read_front(&text, &SOURCES).unwrap_or_else(|e| panic!("{e}\n{text}"));
    assert_eq!(write_front(&back, &SOURCES).unwrap(), text);

    assert_eq!(back.order, front.order);
    assert_eq!(back.hashes, front.hashes);
    assert_eq!(back.ordinals, front.ordinals);
    assert_eq!(back.bodies, front.bodies);
    assert_eq!(back.test_bodies, front.test_bodies);
    assert_eq!(back.diagnostics.len(), 1);
    assert_eq!(back.check.modules[&sym("m")].source, SourceId(1));
    assert_eq!(
        back.check.modules[&sym("m")].imports,
        vec![ModuleName::from_dotted("std.db")]
    );
    let count = &back.check.defs[&sym("m.count")];
    assert_eq!(
        print_scheme(&count.scheme),
        "<a | e>(List<a>) -> Int / {std.db.read[users] | e}"
    );
    assert_eq!(
        count.constraints,
        front.check.defs[&sym("m.count")].constraints
    );
    assert_eq!(count.row_aliases, vec![sym("io"), sym("store")]);
    assert_eq!(count.spec.len(), 2);
    assert_eq!(count.spec[1].kind, SpecKind::Ensures);
    assert_eq!(count.spec[1].span, span(1, 21, 30));
    assert!(count.internally_effectful);
    assert_eq!(count.footprint, front.check.defs[&sym("m.count")].footprint);
    // The layouts `crates/ply-compiler/ply/front.ply` pins.
    assert!(text.contains("test 0 "), "{text}");
    assert_eq!(back.hash_order, front.hash_order);
    assert!(
        text.contains("item 27\nfn m.count requires,ensures"),
        "{text}"
    );
    assert!(
        text.contains(
            "op 72\nspawn write 0 0 1 4294967295 0 0\nUnit\n<a | e>(() -> a / e) -> Unit / e\n"
        ),
        "{text}"
    );
    assert!(text.contains("binder 20\nxs 1 62 70\nList<Int>"), "{text}");
    assert!(text.contains("testhash 0 "), "{text}");
    let test = &back.check.tests[0];
    assert_eq!(test.name, "counts the users");
    assert_eq!(test.key, sym("m.counts the users"));
    assert!(test.nondet);
    let law = &back.check.laws[0];
    assert_eq!(law.binders[0].name, sym("xs"));
    assert_eq!(law.binders[0].ty, Type::list(Type::int()));
    assert!(law.has_guard && !law.host);
    let db = &back.check.effects[&sym("std.db")];
    assert_eq!(
        db.ops[&sym("query")].params,
        vec![Type::string(), Type::int()]
    );
    assert!(db.ops[&sym("query")].resource_param);
    assert!(db.ops[&sym("spawn")].span.is_dummy());
    assert_eq!(
        print_scheme(db.ops[&sym("spawn")].scheme.as_ref().unwrap()),
        "<a | e>(() -> a / e) -> Unit / e"
    );
    let some = &back.check.ctors[&sym("Some")];
    assert!(some.module.is_anonymous());
    assert!(some.span.is_dummy());
    assert_eq!(some.arity, 1);
}

#[test]
fn an_error_diagnostic_ends_the_dump() {
    let mut front = sample();
    front.diagnostics.push(
        Diagnostic::error(codes::TYPE_MISMATCH, "expected Int").primary(span(1, 2, 3), "here"),
    );
    let text = write_front(&front, &SOURCES).unwrap();
    assert!(!text.contains("\norder _ "), "{text}");
    let back = read_front(&text, &SOURCES).unwrap();
    assert_eq!(back.diagnostics.len(), 2);
    assert!(back.check.defs.is_empty());

    let continued = format!("{text}order _ 0\n");
    let err = read_front(&continued, &SOURCES).unwrap_err();
    assert!(err.contains("continues past an error"), "{err}");
}

#[test]
fn the_reader_names_what_it_refuses() {
    let text = write_front(&sample(), &SOURCES).unwrap();

    let err = read_front("", &SOURCES).unwrap_err();
    assert!(err.contains("ends after the diagnostics"), "{err}");

    let err = read_front(&text.replace("def m.count ", "defn m.count "), &SOURCES).unwrap_err();
    assert!(err.contains("unknown frame kind `defn`"), "{err}");

    let err = read_front(
        &text.replace("simple_name 5\ncount", "simple_nane 5\ncount"),
        &SOURCES,
    )
    .unwrap_err();
    assert!(
        err.contains("def `m.count`: unknown field `simple_nane`"),
        "{err}"
    );

    let err = read_front(&text[..text.len() - 3], &SOURCES).unwrap_err();
    assert!(err.contains("truncated"), "{err}");

    let err = read_front(&text.replace("nondet 1\n1", "nondet 1\n2"), &SOURCES).unwrap_err();
    assert!(
        err.contains("test `0`: `nondet` is `2`, not 0 or 1"),
        "{err}"
    );

    let err = read_front(&text.replace("testhash 0 ", "testhash 1 "), &SOURCES).unwrap_err();
    assert!(
        err.contains("testhash `1` names test 1, and only 1 were declared"),
        "{err}"
    );

    let err = read_front(&text.replace("lawhash 0 ", "lawhash 1 "), &SOURCES).unwrap_err();
    assert!(
        err.contains("lawhash `1` names law 1, and only 1 were declared"),
        "{err}"
    );

    // The `testhash 0` frame whole, once more: its header line, then as many bytes as it says.
    let at = text.find("testhash 0 ").unwrap();
    let header_end = at + text[at..].find('\n').unwrap();
    let length: usize = text[at + "testhash 0 ".len()..header_end].parse().unwrap();
    let frame = &text[at..header_end + 1 + length];
    let twice = format!("{}{frame}{}", &text[..at], &text[at..]);
    let err = read_front(&twice, &SOURCES).unwrap_err();
    assert!(err.contains("testhash `0` is written twice"), "{err}");

    let err = read_front(&text.replace("testbody 0 ", "testbody 1 "), &SOURCES).unwrap_err();
    assert!(
        err.contains("testbody `1` is numbered out of order"),
        "{err}"
    );

    let err = read_front(
        &text.replace("index 1\n0nondet", "index 1\n7nondet"),
        &SOURCES,
    )
    .unwrap_err();
    assert!(
        err.contains("test `0`: `index` is 7, but the frame is numbered 0"),
        "{err}"
    );

    // Same length, so the frame's own length still holds.
    let dropped = text.replace("internally_effectful 1\n1", "row_alias 11\n12345678901");
    let err = read_front(&dropped, &SOURCES).unwrap_err();
    assert!(
        err.contains("def `m.count` has no `internally_effectful`"),
        "{err}"
    );

    let bad_scheme = text.replace("(List<a>) -> Int", "(List<a>) -) Int");
    let err = read_front(&bad_scheme, &[SourceId(0), SourceId(1)]).unwrap_err();
    assert!(err.contains("def `m.count`: scheme:"), "{err}");

    let err = read_front(&text, &[SourceId(0)]).unwrap_err();
    assert!(err.contains("only 1 sources were handed over"), "{err}");
}

#[test]
fn the_writer_refuses_a_front_the_protocol_cannot_carry() {
    let mut front = sample();
    front.hashes.tests.clear();
    let err = write_front(&front, &SOURCES).unwrap_err();
    assert!(err.contains("0 test hashes beside 1 tests"), "{err}");

    let mut front = sample();
    front.hashes.defs.insert(sym("m.orphan"), hash(99));
    let err = write_front(&front, &SOURCES).unwrap_err();
    assert!(err.contains("`m.orphan` is in the hashes' `defs`"), "{err}");

    let mut front = sample();
    front.hash_order.pop();
    let err = write_front(&front, &SOURCES).unwrap_err();
    assert!(err.contains("names 1 of 1 tests and 0 of 1 laws"), "{err}");

    let mut front = sample();
    front.hash_order.push(Hashed::Def(sym("m.count")));
    let err = write_front(&front, &SOURCES).unwrap_err();
    assert!(err.contains("names `m.count` twice"), "{err}");

    let mut front = sample();
    front.check.modules[&sym("m")].source = SourceId(7);
    let err = write_front(&front, &SOURCES).unwrap_err();
    assert!(err.contains("module `m` is source 7"), "{err}");
}
