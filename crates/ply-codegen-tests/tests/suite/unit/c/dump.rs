use ply_codegen::c::dump;
use ply_eval::{
    Carry, DefHash, Edit, EffectAtom, Fields, Fix, Footprint, INT_TYPES, IntTy, Mode, Resource,
    Severity, SourceId, Span, Symbol, Value, codes,
};
use std::sync::Arc;

fn record(fields: Vec<(&str, Value)>) -> Value {
    Value::Record(Arc::new(Fields::from_unsorted(
        fields
            .into_iter()
            .map(|(name, value)| (Symbol::new(name), value))
            .collect(),
    )))
}

/// `value` with one field put in place of what it held.
fn with(value: &Value, name: &str, field: Value) -> Value {
    let Value::Record(fields) = value else {
        panic!("not a record: {value:?}");
    };
    let mut out: Vec<(Symbol, Value)> =
        fields.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    let slot = out
        .iter_mut()
        .find(|(k, _)| k.as_str() == name)
        .unwrap_or_else(|| panic!("no field `{name}`"));
    slot.1 = field;
    Value::Record(Arc::new(Fields::from_unsorted(out)))
}

fn field<'v>(value: &'v Value, name: &str) -> &'v Value {
    let Value::Record(fields) = value else {
        panic!("not a record: {value:?}");
    };
    fields
        .named(name)
        .unwrap_or_else(|| panic!("no field `{name}`"))
}

fn first(list: &Value) -> Value {
    let Value::List(items) = list else {
        panic!("not a list: {list:?}");
    };
    items.iter().next().expect("a first item").clone()
}

/// [`holding`] with the module that holds it, read from source 0.
fn placed(footprint: Value) -> Value {
    let module = record(vec![
        ("name", Value::bytes("m")),
        ("index", Value::Int(0)),
        ("items", Value::list(vec![Value::bytes("m.f")])),
        ("imports", Value::list(vec![])),
    ]);
    with(&holding(footprint), "modules", Value::list(vec![module]))
}

#[test]
fn a_malformed_answer_names_the_path_to_what_is_wrong() {
    let good = placed(Value::list(vec![]));
    let ids = [SourceId(0)];
    let front = dump::read(&good, &ids).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(front.check.modules[&Symbol::new("m")].source, SourceId(0));
    assert!(front.check.defs.contains_key(&Symbol::new("m.f")));

    let def = with(&first(field(&good, "defs")), "footprint", Value::Int(7));
    let err = dump::read(&with(&good, "defs", Value::list(vec![def])), &ids).unwrap_err();
    assert_eq!(
        err.path, "the front end's answer.defs[0].footprint",
        "{err}"
    );
    assert!(err.message.contains("expected a list"), "{err}");

    let short = with(&good, "hashes_digest", Value::bytes([0u8; 31]));
    let err = dump::read(&short, &ids).unwrap_err();
    assert_eq!(err.path, "the front end's answer.hashes_digest", "{err}");
    assert!(err.message.contains("expected 32 bytes, found 31"), "{err}");

    let err = dump::read(&good, &[]).unwrap_err();
    assert_eq!(err.path, "the front end's answer.modules[0].index", "{err}");
    assert!(
        err.message.contains("only 0 sources were handed over"),
        "{err}"
    );

    let atom = record(vec![
        ("effect", Value::bytes("m.log")),
        ("resource", Value::ctor("RSingleton", vec![])),
        ("mode", Value::ctor("MBad", vec![])),
        ("op", Value::ctor("None", vec![])),
    ]);
    let def = with(
        &first(field(&good, "defs")),
        "footprint",
        Value::list(vec![atom]),
    );
    let err = dump::read(&with(&good, "defs", Value::list(vec![def])), &ids).unwrap_err();
    assert_eq!(
        err.path, "the front end's answer.defs[0].footprint[0].mode",
        "{err}"
    );
}

fn width(ty: IntTy) -> Value {
    let n = INT_TYPES
        .iter()
        .position(|t| *t == ty)
        .expect("every width is numbered");
    Value::ctor("CWidth", vec![Value::Int(n as i64)])
}

fn plain() -> Value {
    Value::ctor("CPlain", vec![])
}

fn var(n: i64) -> Value {
    Value::ctor("CVar", vec![Value::Int(n)])
}

/// A root's carries and a constructor's read back as the compiler published them: lists, records,
/// sums, functions, widths and type variables by number.
#[test]
fn a_root_and_a_constructor_read_the_carries_published_for_them() {
    let pick = record(vec![
        ("root", Value::bytes("m.pick")),
        ("arity", Value::Int(2)),
        ("pure", Value::Bool(true)),
        ("at", nowhere()),
        (
            "params",
            Value::list(vec![
                Value::ctor("CList", vec![var(0)]),
                Value::ctor(
                    "CRecord",
                    vec![Value::list(vec![
                        record(vec![("name", Value::bytes("n")), ("carry", plain())]),
                        record(vec![
                            ("name", Value::bytes("v")),
                            ("carry", width(IntTy::I16)),
                        ]),
                    ])],
                ),
            ]),
        ),
        (
            "answer",
            Value::ctor("CSum", vec![Value::list(vec![var(0)])]),
        ),
        ("witnesses", Value::list(vec![])),
    ]);
    let apply = record(vec![
        ("root", Value::bytes("m.apply")),
        ("arity", Value::Int(2)),
        ("pure", Value::Bool(false)),
        ("at", nowhere()),
        (
            "params",
            Value::list(vec![
                Value::ctor("CFn", vec![Value::list(vec![plain()]), var(0)]),
                Value::ctor("CMap", vec![width(IntTy::U8), plain()]),
            ]),
        ),
        ("answer", var(0)),
        ("witnesses", Value::list(vec![Value::Int(1)])),
    ]);
    let pair = record(vec![
        ("name", Value::bytes("m.Pair")),
        ("arity", Value::Int(2)),
        ("fields", Value::list(vec![var(1), var(0)])),
    ]);
    let answer = with(
        &with(
            &holding(Value::list(vec![])),
            "emitter_roots",
            Value::list(vec![pick, apply]),
        ),
        "emitter_ctors",
        Value::list(vec![pair]),
    );
    let front = dump::read(&answer, &[]).unwrap_or_else(|e| panic!("{e}"));
    let [pick, apply] = &front.emitter_roots[..] else {
        panic!("two roots: {:?}", front.emitter_roots);
    };
    assert_eq!(
        (pick.root.as_str(), pick.arity, pick.pure),
        ("m.pick", 2, true)
    );
    assert!(!pick.constant());
    assert_eq!(
        pick.params,
        [
            Carry::List(Box::new(Carry::Var(0))),
            Carry::Record(vec![
                (Symbol::new("n"), Carry::Plain),
                (Symbol::new("v"), Carry::Width(IntTy::I16)),
            ]),
        ]
    );
    assert_eq!(pick.answer, Carry::Sum(vec![Carry::Var(0)]));
    assert_eq!(
        apply.params,
        [
            Carry::Fn(vec![Carry::Plain], Box::new(Carry::Var(0))),
            Carry::Map(Box::new(Carry::Width(IntTy::U8)), Box::new(Carry::Plain)),
        ]
    );
    assert_eq!(apply.answer, Carry::Var(0));
    assert_eq!(apply.witnesses, [1]);
    assert_eq!(front.emitter_ctors, [(Symbol::new("m.Pair"), 2)]);
    assert_eq!(
        front.ctor_carries[&Symbol::new("m.Pair")],
        [Carry::Var(1), Carry::Var(0)]
    );
}

fn label(module: i64, start: i64, end: i64, primary: bool, text: &str) -> Value {
    record(vec![
        ("module", Value::Int(module)),
        ("start", Value::Int(start)),
        ("end", Value::Int(end)),
        ("primary", Value::Bool(primary)),
        ("text", Value::bytes(text)),
    ])
}

fn edit(module: i64, start: i64, end: i64, text: &str) -> Value {
    record(vec![
        ("module", Value::Int(module)),
        ("start", Value::Int(start)),
        ("end", Value::Int(end)),
        ("text", Value::bytes(text)),
    ])
}

fn diag(code: &str, severity: &str, message: &str, labels: Vec<Value>, notes: &[&str]) -> Value {
    record(vec![
        ("code", Value::bytes(code)),
        ("notes", Value::Int(notes.len() as i64)),
        ("labels", Value::list(labels)),
        ("text", Value::bytes("")),
        ("message", Value::bytes(message)),
        (
            "notes_text",
            Value::list(notes.iter().map(Value::bytes).collect()),
        ),
        ("severity", Value::bytes(severity)),
        ("fixes", Value::list(vec![])),
    ])
}

/// A refused program's answer is its diagnostics alone, each label and edit placed in the source
/// its module index names, and a span outside every module placed nowhere.
#[test]
fn an_error_is_the_whole_answer_with_its_labels_notes_and_fixes() {
    let failed = with(
        &diag(
            "E0201",
            "error",
            "type mismatch",
            vec![
                label(1, 13, 17, true, "expected Int, found Bool"),
                label(0, 2, 5, false, "the parameter is declared here"),
            ],
            &[
                "`+` is Int -> Int -> Int",
                "a second note\nwith a line break",
            ],
        ),
        "fixes",
        Value::list(vec![record(vec![
            ("title", Value::bytes("add the annotation")),
            (
                "edits",
                Value::list(vec![edit(1, 17, 17, ": Int"), edit(0, 2, 5, "")]),
            ),
        ])]),
    );
    let warned = diag(
        "E0101",
        "warning",
        "no such name `frob`",
        vec![label(4294967295, 0, 0, true, "")],
        &[],
    );
    let refused = record(vec![("diags", Value::list(vec![failed, warned]))]);
    let sources = [SourceId(9), SourceId(4)];
    let front = dump::read(&refused, &sources).unwrap_or_else(|e| panic!("{e}"));
    assert!(front.has_error());
    assert!(front.check.defs.is_empty() && front.check.modules.is_empty());

    let [failed, warned] = &front.diagnostics[..] else {
        panic!("two diagnostics: {:?}", front.diagnostics);
    };
    assert_eq!(failed.code, codes::TYPE_MISMATCH);
    assert_eq!(failed.severity, Severity::Error);
    assert_eq!(failed.message, "type mismatch");
    assert_eq!(failed.labels[0].span, Span::new(SourceId(4), 13, 17));
    assert_eq!(failed.labels[0].message, "expected Int, found Bool");
    assert!(failed.labels[0].primary);
    assert_eq!(failed.labels[1].span, Span::new(SourceId(9), 2, 5));
    assert!(!failed.labels[1].primary);
    assert_eq!(
        failed.notes,
        [
            "`+` is Int -> Int -> Int",
            "a second note\nwith a line break"
        ]
    );
    assert_eq!(
        failed.fixes.to_vec(),
        vec![Fix {
            title: "add the annotation".to_string(),
            edits: vec![
                Edit {
                    span: Span::new(SourceId(4), 17, 17),
                    text: ": Int".to_string(),
                },
                Edit {
                    span: Span::new(SourceId(9), 2, 5),
                    text: String::new(),
                },
            ],
        }]
    );
    assert_eq!(warned.code, codes::UNKNOWN_NAME);
    assert_eq!(warned.severity, Severity::Warning);
    assert_eq!(warned.labels[0].span, Span::DUMMY);

    let err = dump::read(&refused, &[SourceId(9)]).unwrap_err();
    assert_eq!(
        err.path, "the front end's answer.diags[0].labels[0].module",
        "{err}"
    );
}

// --- A footprint's numbering ----------------------------------------------------------

fn some(v: Value) -> Value {
    Value::ctor("Some", vec![v])
}

fn none() -> Value {
    Value::ctor("None", vec![])
}

fn atom(effect: &str, resource: Value, mode: &str, op: Option<&str>) -> Value {
    record(vec![
        ("effect", Value::bytes(effect)),
        ("resource", resource),
        ("mode", Value::ctor(mode, vec![])),
        ("op", op.map_or_else(none, |op| some(Value::bytes(op)))),
    ])
}

fn nowhere() -> Value {
    record(vec![
        ("module", Value::Int(4294967295)),
        ("start", Value::Int(0)),
        ("end", Value::Int(0)),
    ])
}

/// A whole answer holding one definition and nothing else.
fn holding(footprint: Value) -> Value {
    let empty = || Value::list(vec![]);
    let def = record(vec![
        ("name", Value::bytes("m.f")),
        ("module", Value::bytes("m")),
        ("simple_name", Value::bytes("f")),
        ("footprint", footprint.clone()),
        ("performed", footprint),
        ("at", nowhere()),
    ]);
    let mut tables: Vec<(&str, Value)> = [
        "diags",
        "modules",
        "types",
        "tests",
        "laws",
        "effects",
        "hashes",
        "emitter_roots",
        "emitter_ctors",
        "ordinals",
    ]
    .into_iter()
    .map(|name| (name, empty()))
    .collect();
    tables.push(("defs", Value::list(vec![def])));
    tables.push(("hashes_digest", Value::bytes([0u8; 32])));
    record(tables)
}

/// However the checker numbered a footprint's label variables, they read back numbered where they
/// first appear, and each prints as the first letter no resource beside it holds.
#[test]
fn a_footprint_reads_back_numbered_as_its_labels_first_appear() {
    let var = |v: i64| Value::ctor("RVar", vec![Value::Int(v)]);
    let footprint = Value::list(vec![
        atom("m.net", var(30), "MWrite", Some("recv")),
        atom("m.net", var(12), "MWrite", Some("send")),
        atom("m.net", var(30), "MRead", None),
        atom(
            "m.net",
            Value::ctor("RNamed", vec![Value::bytes("l")]),
            "MRead",
            None,
        ),
    ]);
    let front = dump::read(&holding(footprint), &[]).unwrap_or_else(|e| panic!("{e}"));
    let f = &front.check.defs[&Symbol::new("m.f")];

    let net =
        |resource: Resource, op: &str| EffectAtom::operation("m.net", resource, Mode::Write, op);
    assert_eq!(
        f.footprint,
        Footprint::from_atoms([
            net(Resource::Var(0), "recv"),
            net(Resource::Var(1), "send"),
            EffectAtom::new("m.net", Resource::Var(0), Mode::Read),
            EffectAtom::new("m.net", Resource::Named(Symbol::new("l")), Mode::Read),
        ])
    );
    assert_eq!(
        f.footprint.to_string(),
        "{m.net.read[l], m.net.read[m], m.net.recv[m], m.net.send[n]}"
    );
}

// --- A native program's ---------------------------------------------------------------

/// What a native program carries reads as an answer of its entry alone: the one definition and the
/// one root, its row's atoms given the modes their operations declare, and the lines of its hashes
/// as they were written.
#[test]
fn a_native_programs_front_end_reads_as_an_answer_of_its_entry_alone() {
    let emit = record(vec![
        ("name", Value::bytes("emit")),
        ("mode", Value::bytes("read")),
        ("resource_param", Value::Bool(false)),
        ("at", nowhere()),
        ("carries", Value::list(vec![width(IntTy::U8)])),
    ]);
    let log = record(vec![
        ("name", Value::bytes("m.log")),
        ("module", Value::bytes("m")),
        ("simple_name", Value::bytes("log")),
        ("nondet", Value::Bool(false)),
        ("ops", Value::list(vec![emit])),
        ("at", nowhere()),
    ]);
    let row = Value::list(vec![atom(
        "m.log",
        Value::ctor("RSingleton", vec![]),
        "MWrite",
        Some("emit"),
    )]);
    let entry = record(vec![
        ("name", Value::bytes("m.main")),
        ("module", Value::bytes("m")),
        ("simple_name", Value::bytes("main")),
        ("footprint", row.clone()),
        ("performed", row),
        (
            "at",
            record(vec![
                ("module", Value::Int(1)),
                ("start", Value::Int(3)),
                ("end", Value::Int(9)),
            ]),
        ),
    ]);
    let root = record(vec![
        ("root", Value::bytes("m.main")),
        ("arity", Value::Int(0)),
        ("pure", Value::Bool(false)),
        ("at", nowhere()),
        ("params", Value::list(vec![])),
        ("answer", plain()),
        ("witnesses", Value::list(vec![])),
    ]);
    let pair = record(vec![
        ("name", Value::bytes("m.Pair")),
        ("arity", Value::Int(2)),
        ("fields", Value::list(vec![var(1), var(0)])),
    ]);
    let carried = record(vec![
        ("entry", entry),
        ("root", root),
        ("effects", Value::list(vec![log])),
        ("ctors", Value::list(vec![pair])),
        ("digest", Value::bytes([7u8; 32])),
        ("definitions", Value::bytes("m.main ab\n")),
        ("modules", Value::Int(2)),
    ]);
    let linked = dump::linked(&carried).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(linked.entry.as_str(), "m.main");
    assert_eq!(linked.definitions, "m.main ab\n");

    let front = &linked.front;
    let main = &front.check.defs[&Symbol::new("m.main")];
    assert_eq!(front.check.defs.len(), 1);
    assert_eq!(main.span, Span::new(SourceId(1), 3, 9));
    assert_eq!(
        main.footprint,
        Footprint::from_atoms([EffectAtom::operation(
            "m.log",
            Resource::Singleton,
            Mode::Read,
            "emit"
        )])
    );
    assert_eq!(
        front.check.effects[&Symbol::new("m.log")].ops[&Symbol::new("emit")].params,
        [Carry::Width(IntTy::U8)]
    );
    let [root] = &front.emitter_roots[..] else {
        panic!("one root: {:?}", front.emitter_roots);
    };
    assert_eq!((root.root.as_str(), root.arity), ("m.main", 0));
    assert_eq!(front.hashes_digest, DefHash([7u8; 32]));
    assert_eq!(front.emitter_ctors, [(Symbol::new("m.Pair"), 2)]);
    assert_eq!(
        front.ctor_carries[&Symbol::new("m.Pair")],
        [Carry::Var(1), Carry::Var(0)]
    );
    assert!(front.hashes.defs.is_empty() && front.check.modules.is_empty());

    let err = dump::linked(&with(&carried, "modules", Value::Int(1)))
        .err()
        .expect("a span past the modules the program says its spans run over");
    assert_eq!(
        err.path, "a native program's front end.entry.at.module",
        "{err}"
    );
}
