use ply_span::SourceId;
use ply_ty::*;

const SOURCES: [SourceId; 2] = [SourceId(0), SourceId(1)];

/// `<header> <length>\n<body>`: a frame, or a field inside one.
fn unit(header: &str, body: &str) -> String {
    format!("{header} {}\n{body}", body.len())
}

/// A `<kind> <name>` frame of `<key> <text>` fields.
fn frame(kind: &str, name: &str, fields: &[(&str, &str)]) -> String {
    let payload: String = fields.iter().map(|(key, text)| unit(key, text)).collect();
    unit(&format!("{kind} {name}"), &payload)
}

/// Test 0, `m.t`, whole.
fn a_test() -> String {
    frame(
        "test",
        "0",
        &[
            ("key", "m.t"),
            ("name", "t"),
            ("module", "m"),
            ("index", "0"),
            ("nondet", "0"),
            ("footprint", ""),
            ("span", "1 0 9"),
            ("name_span", "1 5 8"),
        ],
    )
}

/// Law 0, `m.l`, whole.
fn a_law() -> String {
    frame(
        "law",
        "0",
        &[
            ("key", "m.l"),
            ("name", "l"),
            ("module", "m"),
            ("index", "0"),
            ("has_guard", "0"),
            ("host", "0"),
            ("footprint", ""),
            ("span", "1 10 20"),
        ],
    )
}

fn test_hash(name: &str) -> String {
    frame(
        "testhash",
        name,
        &[("key", "m.t"), ("hash", &"07".repeat(32))],
    )
}

#[test]
fn an_error_diagnostic_ends_the_dump() {
    let warned = frame(
        "diag",
        "0",
        &[
            ("code", "W0611"),
            ("severity", "warning"),
            ("message", "never used"),
            ("label", "1 0 1 1\nhere"),
        ],
    );
    let failed = frame(
        "diag",
        "1",
        &[
            ("code", "E0201"),
            ("severity", "error"),
            ("message", "expected Int"),
            ("label", "1 2 3 1\nhere"),
        ],
    );
    let text = format!("{warned}{failed}");
    let back = read_front(&text, &SOURCES).unwrap_or_else(|e| panic!("{e}\n{text}"));
    assert_eq!(back.diagnostics.len(), 2);
    assert!(back.has_error());
    assert!(back.check.defs.is_empty());

    let continued = format!("{text}order _ 0\n");
    let err = read_front(&continued, &SOURCES).unwrap_err();
    assert!(err.contains("continues past an error"), "{err}");
}

#[test]
fn the_reader_names_what_it_refuses() {
    let refused = |dump: &str| read_front(dump, &SOURCES).unwrap_err();

    let err = refused("");
    assert!(err.contains("ends after the diagnostics"), "{err}");

    let err = refused(&frame("defn", "m.count", &[]));
    assert!(err.contains("unknown frame kind `defn`"), "{err}");

    let err = refused(&frame("typ3", "m.Shape", &[]));
    assert!(err.contains("unknown frame kind `typ3`"), "{err}");

    let order = frame("order", "_", &[("module", "m")]);
    let err = refused(&order[..order.len() - 3]);
    assert!(err.contains("truncated"), "{err}");

    let def = |fields: &[(&str, &str)]| frame("def", "m.count", fields);
    let err = refused(&def(&[("module", "m"), ("simple_nane", "count")]));
    assert!(
        err.contains("def `m.count`: unknown field `simple_nane`"),
        "{err}"
    );

    let err = refused(&def(&[("public", "2")]));
    assert!(err.contains("`public` is `2`, not 0 or 1"), "{err}");

    let err = refused(&def(&[("public", "1"), ("reuse", "2")]));
    assert!(err.contains("`reuse` is `2`, not 0 or 1"), "{err}");

    let named = [
        ("public", "1"),
        ("reuse", "0"),
        ("module", "m"),
        ("simple_name", "count"),
    ];
    let err = refused(&def(&[&named[..], &[("scheme", "(Int) -) Int")]].concat()));
    assert!(err.contains("def `m.count`: scheme:"), "{err}");

    let err = refused(&def(&[
        &named[..],
        &[
            ("scheme", "(Int) -> Int"),
            ("footprint", ""),
            ("performed", ""),
        ],
    ]
    .concat()));
    assert!(
        err.contains("def `m.count` has no `internally_effectful`"),
        "{err}"
    );

    let err = refused(&def(&[("param", "xs1810abc")]));
    assert!(
        err.contains("def `m.count`: param `xs1810abc` is not `<name> <span>`"),
        "{err}"
    );

    let err = refused(&def(&[("literal", "rat -3")]));
    assert!(
        err.contains("`rat` is not `int`, `str` or `bytes`"),
        "{err}"
    );

    let err = read_front(&frame("module", "m", &[("index", "1")]), &[SourceId(0)]).unwrap_err();
    assert!(err.contains("only 1 sources were handed over"), "{err}");

    let err = refused(&frame(
        "module",
        "m",
        &[("index", "1"), ("effect_set", "io,store std.db.read")],
    ));
    assert!(err.contains("is not `<name> <includes> <atoms>`"), "{err}");

    let test = |fields: &[(&str, &str)]| frame("test", "0", fields);
    let err = refused(&test(&[("key", "m.t"), ("xame_span", "1 5 8")]));
    assert!(err.contains("test `0`: unknown field `xame_span`"), "{err}");

    let placed = [
        ("key", "m.t"),
        ("name", "t"),
        ("module", "m"),
        ("name_span", "1 5 8"),
    ];
    let err = refused(&test(
        &[&placed[..], &[("index", "0"), ("nondet", "2")]].concat(),
    ));
    assert!(
        err.contains("test `0`: `nondet` is `2`, not 0 or 1"),
        "{err}"
    );

    let err = refused(&test(
        &[&placed[..], &[("index", "7"), ("nondet", "0")]].concat(),
    ));
    assert!(
        err.contains("test `0`: `index` is 7, but the frame is numbered 0"),
        "{err}"
    );

    let err = refused(&format!("{}{}", a_test(), test_hash("1")));
    assert!(
        err.contains("testhash `1` names test 1, and only 1 were declared"),
        "{err}"
    );

    let err = refused(&format!("{}{}", a_law(), frame("lawhash", "1", &[])));
    assert!(
        err.contains("lawhash `1` names law 1, and only 1 were declared"),
        "{err}"
    );

    let err = refused(&format!("{}{}{}", a_test(), test_hash("0"), test_hash("0")));
    assert!(err.contains("testhash `0` is written twice"), "{err}");

    let err = refused(&frame("testbody", "1", &[]));
    assert!(
        err.contains("testbody `1` is numbered out of order"),
        "{err}"
    );
}

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
            let stem = p.file_stem().unwrap().to_string_lossy().to_string();
            (stem, std::fs::read_to_string(&p).unwrap())
        })
        .collect()
}

/// Every frame kind the front end writes, over the examples and the compiler: the answer reads,
/// and reads to one structure however often it is read.
#[test]
fn a_real_answer_reads_to_one_structure() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut user = ply_files(&root.join("examples"));
    user.extend(ply_files(&root.join("crates/ply-compiler/ply")));
    let shipped: Vec<(String, String)> = ply_std::sources()
        .map(|(name, text)| (name.to_string(), text.to_string()))
        .collect();
    let pulled = ply_codegen::c::producer::front_pulling_std(&user, &shipped)
        .unwrap_or_else(|e| panic!("the corpus does not answer: {e:#}"));
    let ids: Vec<SourceId> = (0..user.len() + pulled.modules.len())
        .map(|i| SourceId(i as u32))
        .collect();

    let first = read_front(&pulled.dump, &ids).unwrap_or_else(|e| panic!("{e}"));
    assert!(
        !first.has_error(),
        "the corpus does not check: {:?}",
        first.diagnostics
    );
    assert!(!first.check.defs.is_empty() && !first.hash_order.is_empty());
    let second = read_front(&pulled.dump, &ids).unwrap_or_else(|e| panic!("{e}"));
    same(
        "the answer's structure",
        &format!("{second:?}"),
        &format!("{first:?}"),
    );
}

#[track_caller]
fn same(what: &str, got: &str, want: &str) {
    if got == want {
        return;
    }
    let shorter = got.len().min(want.len());
    let at = got
        .bytes()
        .zip(want.bytes())
        .position(|(a, b)| a != b)
        .unwrap_or(shorter);
    let near = |text: &str| {
        let bytes = &text.as_bytes()[at.saturating_sub(200)..(at + 200).min(text.len())];
        String::from_utf8_lossy(bytes).into_owned()
    };
    panic!(
        "{what} departs at byte {at}\n  got:  {:?}\n  want: {:?}",
        near(got),
        near(want)
    );
}
