use ply_codegen::c::dump;
use ply_eval::{
    Carry, Edit, EffectAtom, Fields, Fix, Footprint, IntTy, Mode, Ordinal, Resource, Severity,
    SourceId, Span, Symbol, Value, Visibility, codes,
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

/// The front end's answer over modules that import nothing shipped, as the builder writes it.
fn answer(modules: &[(&str, &str)]) -> Value {
    let files: Vec<(String, String)> = modules
        .iter()
        .map(|(name, text)| (format!("{}.ply", name.replace('.', "/")), text.to_string()))
        .collect();
    let bytes = ply_machine::builds::answered(&files)
        .unwrap_or_else(|d| panic!("the builder answers: {}", d.message));
    let answer = ply_machine::runnable::front_value(&bytes)
        .unwrap_or_else(|why| panic!("the answer reads: {why}"));
    field(&answer, "dump").clone()
}

const PROGRAM: &str = "pub effect log { write emit(Bytes) -> Unit }\n\
pub type Shape = | Dot | Line(Int)\n\
pub fn say(b: Bytes) -> Unit / { log.write } = log.emit(b)\n\
pub fn pick<a, b>(x: a, y: b) -> a = x\n\
fn positive(n: Int) -> Bool = n > 0\n\
test \"says\" { say(b\"x\") }\n\
law \"picks the first\" forall (n: Int) where n > 2 { pick(n, true) == n }\n";

#[test]
fn a_real_answer_reads_to_the_program_it_describes() {
    let front =
        dump::read(&answer(&[("m", PROGRAM)]), &[SourceId(0)]).unwrap_or_else(|e| panic!("{e}"));
    let named = |name: &str| Symbol::new(name);

    let say = &front.check.defs[&named("m.say")];
    let log_write =
        Footprint::from_atoms([EffectAtom::new("m.log", Resource::Singleton, Mode::Write)]);
    assert_eq!(say.footprint, log_write);
    // An operation atom takes the mode its declaration gives it.
    assert_eq!(
        say.performed,
        Footprint::from_atoms([EffectAtom::operation(
            "m.log",
            Resource::Singleton,
            Mode::Write,
            "emit"
        )])
    );
    assert_eq!(front.defs_written[&named("m.say")].vis, Visibility::Public);
    assert_eq!(
        front.defs_written[&named("m.positive")].vis,
        Visibility::Private
    );

    let pick = &front.check.defs[&named("m.pick")];
    assert!(pick.footprint.is_empty());

    let test = &front.check.tests[0];
    assert_eq!(test.key, named("m.says"));
    assert_eq!(test.footprint, log_write);

    let law = &front.check.laws[0];
    assert_eq!(law.key, named("m.picks the first"));

    let log = &front.check.effects[&named("m.log")];
    let emit = &log.ops[&named("emit")];
    assert_eq!(emit.mode, Mode::Write);
    assert!(!emit.resource_param);
    assert_eq!(front.effects_written[&named("m.log")], Visibility::Public);

    for ctor in [(named("m.Dot"), 0), (named("m.Line"), 1)] {
        assert!(front.emitter_ctors.contains(&ctor), "{ctor:?}");
    }
    assert_eq!(front.types[&named("m.Shape")].arity, 0);

    for def in ["m.say", "m.pick", "m.positive"] {
        assert!(front.hashes.defs.contains_key(&named(def)), "{def}");
    }
    assert_eq!(front.hashes.tests.len(), 1);
    assert!(front.hashes.deps.contains_key(&named("m.picks the first")));
    assert_eq!(front.test_bodies.len(), 1);

    assert_eq!(
        front.ordinals,
        vec![(
            named("m"),
            vec![
                Ordinal::Fn(named("m.say"), vec![]),
                Ordinal::Fn(named("m.pick"), vec![]),
                Ordinal::Fn(named("m.positive"), vec![]),
                Ordinal::Test(named("m.says")),
                Ordinal::Law(named("m.picks the first")),
            ],
        )]
    );
    assert_eq!(front.check.modules[&named("m")].source, SourceId(0));
    let roots: Vec<&str> = front
        .emitter_roots
        .iter()
        .map(|r| r.root.as_str())
        .collect();
    assert_eq!(
        roots,
        [
            "m.say",
            "m.pick",
            "m.positive",
            "m.test#0",
            "m.law#0.guard",
            "m.law#0.body"
        ]
    );

    // A warning, placed in the module it is about.
    assert!(!front.has_error());
    let unused = front
        .diagnostics
        .iter()
        .find(|d| d.message.contains("m.positive"))
        .unwrap_or_else(|| panic!("no warning about `m.positive`: {:?}", front.diagnostics));
    assert_eq!(unused.severity, Severity::Warning);
    assert_eq!(unused.labels[0].span.source, SourceId(0));
}

const PURITY: &str = "effect log { write emit(Bytes) -> Unit }\n\
fn origin() -> Int = 0\n\
fn hello() -> Unit / { log.write } = log.emit(b\"hi\")\n\
fn same<a>(x: a, y: a) -> Bool where derivable(eq, a) = x == y\n\
fn positive(n: Int) -> Int requires n > 0 = n\n\
test \"origin\" { assert_eq(origin(), 0) }\n\
law \"same\" forall (n: Int) { same(n, n) }\n";

/// A definition with a row or a constraint is impure, a test, a clause or a law part is never
/// pure, and only a pure root of no arguments is a constant.
#[test]
fn each_root_reads_the_purity_the_compiler_published() {
    let front =
        dump::read(&answer(&[("m", PURITY)]), &[SourceId(0)]).unwrap_or_else(|e| panic!("{e}"));
    let read: Vec<(&str, bool)> = front
        .emitter_roots
        .iter()
        .map(|r| (r.root.as_str(), r.pure))
        .collect();
    assert_eq!(
        read,
        [
            ("m.origin", true),
            ("m.hello", false),
            ("m.same", false),
            ("m.positive", true),
            ("m.test#0", false),
            ("m.positive#requires#0", false),
            ("m.law#0.body", false),
        ]
    );
    let constants: Vec<&str> = front
        .emitter_roots
        .iter()
        .filter(|r| r.constant())
        .map(|r| r.root.as_str())
        .collect();
    assert_eq!(constants, ["m.origin"]);
}

const WIDTHS: &str = "fn low_bit(b: U8) -> Int requires int_of_u8(b) < 200 ensures result < 2 = int_of_u8(b & 1u8)\n\
fn narrow(n: Int) -> U8 / {abort.raise} requires n >= 0 ensures int_of_u8(result) == n = u8_of_int(n)\n\
fn inc(x: Int) -> Int requires x > 0 ensures result > x = x + 1\n\
law \"low\" forall (b: U8) where int_of_u8(b) > 0 { low_bit(b) < 2 }\n\
law \"grows\" forall (x: Int) where x > 0 { inc(x) > x }\n";

/// A clause carries its owner's parameters, and `result` too for an `ensures`, and a law part its
/// binders: each reads a width exactly where those types hold one.
#[test]
fn each_root_reads_the_carries_the_compiler_published() {
    let front =
        dump::read(&answer(&[("m", WIDTHS)]), &[SourceId(0)]).unwrap_or_else(|e| panic!("{e}"));
    let u8 = || Carry::Width(IntTy::U8);
    let read: Vec<(&str, Vec<Carry>, Carry)> = front
        .emitter_roots
        .iter()
        .map(|r| (r.root.as_str(), r.params.clone(), r.answer.clone()))
        .collect();
    assert_eq!(
        read,
        [
            ("m.low_bit", vec![u8()], Carry::Plain),
            ("m.narrow", vec![Carry::Plain], u8()),
            ("m.inc", vec![Carry::Plain], Carry::Plain),
            ("m.low_bit#requires#0", vec![u8()], Carry::Plain),
            (
                "m.low_bit#ensures#0",
                vec![u8(), Carry::Plain],
                Carry::Plain
            ),
            ("m.narrow#requires#0", vec![Carry::Plain], Carry::Plain),
            ("m.narrow#ensures#0", vec![Carry::Plain, u8()], Carry::Plain),
            ("m.inc#requires#0", vec![Carry::Plain], Carry::Plain),
            (
                "m.inc#ensures#0",
                vec![Carry::Plain, Carry::Plain],
                Carry::Plain
            ),
            ("m.law#0.guard", vec![u8()], Carry::Plain),
            ("m.law#0.body", vec![u8()], Carry::Plain),
            ("m.law#1.guard", vec![Carry::Plain], Carry::Plain),
            ("m.law#1.body", vec![Carry::Plain], Carry::Plain),
        ]
    );
}

const NOMINAL: &str = "effect probe { write put(xs: List<U8>, n: Int) -> Unit }\n\
type Byte = | Byte(U8)\n\
type Pair<a, b> = | Pair(b, a)\n\
fn pick<a>(xs: List<a>, r: { v: I16, n: Int }) -> Option<a> = match xs { [x, ..] -> Some(x), _ -> None }\n\
fn apply<a>(f: (Int) -> a, x: Int) -> a = f(x)\n\
fn sent(xs: List<U8>) -> Unit / { probe.write } = probe.put(xs, 1)\n";

/// A constructor's fields read over its type's parameters, a root's type variables by number, and
/// an operation's arguments as its declaration types them.
#[test]
fn constructors_variables_and_operations_read_their_published_carries() {
    let front =
        dump::read(&answer(&[("m", NOMINAL)]), &[SourceId(0)]).unwrap_or_else(|e| panic!("{e}"));
    let named = |name: &str| Symbol::new(name);
    assert_eq!(
        front.ctor_carries[&named("m.Byte")],
        [Carry::Width(IntTy::U8)]
    );
    assert_eq!(
        front.ctor_carries[&named("m.Pair")],
        [Carry::Var(1), Carry::Var(0)]
    );
    assert_eq!(front.ctor_carries[&named("Some")], [Carry::Var(0)]);
    let root = |name: &str| {
        front
            .emitter_roots
            .iter()
            .find(|r| r.root.as_str() == name)
            .unwrap_or_else(|| panic!("no root `{name}`"))
    };
    let pick = root("m.pick");
    assert_eq!(
        pick.params,
        [
            Carry::List(Box::new(Carry::Var(0))),
            Carry::Record(vec![
                (named("n"), Carry::Plain),
                (named("v"), Carry::Width(IntTy::I16)),
            ]),
        ]
    );
    assert_eq!(pick.answer, Carry::Sum(vec![Carry::Var(0)]));
    let apply = root("m.apply");
    assert_eq!(
        apply.params,
        [
            Carry::Fn(vec![Carry::Plain], Box::new(Carry::Var(0))),
            Carry::Plain
        ]
    );
    assert_eq!(apply.answer, Carry::Var(0));
    let put = &front.check.effects[&named("m.probe")].ops[&named("put")];
    assert_eq!(
        put.params,
        [Carry::List(Box::new(Carry::Width(IntTy::U8))), Carry::Plain]
    );
}

/// Each `.ply` file in `dir` as `(file name, text)`.
fn ply_files(dir: &std::path::Path) -> Vec<(String, String)> {
    let mut paths: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|entry| entry.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "ply"))
        .collect();
    paths.sort();
    paths
        .into_iter()
        .map(|p| {
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            (name, std::fs::read_to_string(&p).unwrap())
        })
        .collect()
}

/// Every row the front end answers, over the compiler, as the committed builder carries it, and over
/// the examples, reads, and reads to one structure however often it is read.
#[test]
fn an_answer_over_the_examples_and_the_compiler_reads_whole() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let examples = ply_machine::builds::answered(&ply_files(&root.join("examples")))
        .unwrap_or_else(|d| panic!("the builder answers: {}", d.message));
    for (what, bytes) in [
        ("the committed builder", ply_compiler::bootstrap::BUILDER),
        ("the examples", examples.as_slice()),
    ] {
        let answer = ply_machine::runnable::front_value(bytes)
            .unwrap_or_else(|why| panic!("{what}: the answer reads: {why}"));
        let Value::List(files) = field(&answer, "files") else {
            panic!("{what}: the answer lists no files");
        };
        let ids: Vec<SourceId> = (0..files.len()).map(|i| SourceId(i as u32)).collect();
        let dump = field(&answer, "dump");
        let once = dump::read(dump, &ids).unwrap_or_else(|e| panic!("{what}: {e}"));
        assert!(
            !once.has_error(),
            "{what} does not check: {:?}",
            once.diagnostics
        );
        assert!(!once.check.defs.is_empty() && !once.hashes.defs.is_empty());
        assert_eq!(once.check.tests.len(), once.hashes.tests.len());
        let again = dump::read(dump, &ids).unwrap_or_else(|e| panic!("{what}: {e}"));
        assert!(
            format!("{again:?}") == format!("{once:?}"),
            "{what}: one answer read twice reads to two structures"
        );
    }
}

#[test]
fn a_malformed_answer_names_the_path_to_what_is_wrong() {
    let good = answer(&[("m", "pub fn one() -> Int = 1\n")]);
    let ids = [SourceId(0)];
    dump::read(&good, &ids).unwrap_or_else(|e| panic!("{e}"));

    let def = with(&first(field(&good, "defs")), "params", Value::Int(7));
    let err = dump::read(&with(&good, "defs", Value::list(vec![def])), &ids).unwrap_err();
    assert_eq!(err.path, "the front end's answer.defs[0].params", "{err}");
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
    assert!(front.check.defs.is_empty() && front.packages.is_empty());

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
        ("public", Value::Bool(true)),
        ("reuse", Value::Bool(false)),
        ("footprint", footprint.clone()),
        ("performed", footprint),
        ("params", empty()),
        ("at", nowhere()),
    ]);
    let mut tables: Vec<(&str, Value)> = [
        "diags",
        "packages",
        "pins",
        "mod_pkg",
        "modules",
        "types",
        "tests",
        "laws",
        "effects",
        "hashes",
        "keys",
        "emit_roots",
        "emit_ctors",
        "ordinals",
        "bodies",
        "test_bodies",
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
