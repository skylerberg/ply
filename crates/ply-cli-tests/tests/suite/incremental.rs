use ply_cli::driver;
use ply_cli::load::Loaded;
use ply_store::Store;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// `{:?}`, not a printed signature: `print_scheme` renames variables per item and would hide a numbering divergence.
fn snapshot(loaded: &Loaded) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (name, hash) in &loaded.hashes.defs {
        out.insert(format!("hash {name}"), hash.to_hex());
    }
    for (name, hash) in &loaded.hashes.decls {
        out.insert(format!("decl {name}"), hash.to_hex());
    }
    for (name, def) in &loaded.check.defs {
        out.insert(format!("scheme {name}"), format!("{:?}", def.scheme));
        out.insert(format!("footprint {name}"), def.footprint.to_string());
    }
    for (name, ctor) in &loaded.check.ctors {
        out.insert(
            format!("ctor {name}"),
            format!("{} {:?} {:?}", ctor.index, ctor.fields, ctor.scheme),
        );
    }
    for (name, effect) in &loaded.check.effects {
        let ops: Vec<String> = effect
            .ops
            .values()
            .map(|op| format!("{} {:?} {:?} {:?}", op.name, op.mode, op.params, op.ret))
            .collect();
        out.insert(
            format!("effect {name}"),
            format!("{} {ops:?}", effect.nondet),
        );
    }
    for (i, test) in loaded.check.tests.iter().enumerate() {
        let hash = loaded
            .hashes
            .tests
            .get(i)
            .map(|h| h.to_hex())
            .unwrap_or_default();
        out.insert(
            format!("test {i}"),
            format!("{} {} {} {hash}", test.key, test.nondet, test.footprint),
        );
    }
    out
}

/// The incremental run goes first so it sees the store as an edit-test loop would.
#[track_caller]
fn agree(dir: &Path, what: &str) -> Loaded {
    let mut store = Store::open(dir).expect("the cache directory must be creatable");
    let incremental = driver::load_incremental(dir, &mut store)
        .unwrap_or_else(|e| panic!("{what}: the incremental path failed: {:?}", codes(&e)));
    let full = driver::load_full(dir)
        .unwrap_or_else(|e| panic!("{what}: the full path failed: {:?}", codes(&e)));

    let a = snapshot(&full);
    let b = snapshot(&incremental);
    let mut differences = Vec::new();
    for key in a.keys().chain(b.keys()) {
        if a.get(key) != b.get(key) {
            differences.push(format!(
                "  {key}\n    full        {:?}\n    incremental {:?}",
                a.get(key),
                b.get(key)
            ));
        }
    }
    differences.dedup();
    assert!(
        differences.is_empty(),
        "{what}: the incremental path disagreed with a from-scratch check:\n{}",
        differences.join("\n")
    );
    incremental
}

fn codes(e: &ply_cli::load::LoadError) -> Vec<String> {
    e.diagnostics
        .iter()
        .map(|d| format!("{}: {}", d.code, d.message))
        .collect()
}

fn write(dir: &Path, name: &str, text: &str) {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, text).unwrap();
}

fn edit(dir: &Path, name: &str, from: &str, to: &str) {
    let path = dir.join(name);
    let text = fs::read_to_string(&path).unwrap();
    assert!(
        text.contains(from),
        "`{from}` is not in {name}; the fixture drifted"
    );
    fs::write(path, text.replace(from, to)).unwrap();
}

const CORE: &str = r#"
pub type Money = Int

pub type Item =
  | Book(String, Money)
  | Note(String)

pub effect db {
  read  get[r](key: Int) -> Int
  write put[r](key: Int, value: Int) -> Int
}

pub fn price(i: Item) -> Money =
  match i {
    Book(_, p) -> p,
    Note(_) -> 0,
  }

pub fn twice<a>(x: a, f: (a) -> a) -> a = f(f(x))

pub fn label(i: Item) -> String =
  match i {
    Book(t, _) -> t,
    Note(t) -> t,
  }

test "a note is free" {
  assert_eq(price(Note("n")), 0)
}
"#;

const SHOP: &str = r#"
import core

fn total(items: List<core::Item>) -> Int =
  fold(items, 0, |acc, i: core::Item| acc + core::price(i))

fn doubled(n: Int) -> Int = core::twice(n, |x: Int| x + x)

fn stored(k: Int) -> Int / {core::db.read[cart]} = core::db.get[cart](k)

test "a shelf adds up" {
  assert_eq(total([core::Book("b", 3), core::Note("n")]), 3)
}
"#;

const LEAF: &str = r#"
pub fn one() -> Int = 1
pub fn two() -> Int = one() + one()
"#;

fn corpus() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "core.ply", CORE);
    write(dir.path(), "shop.ply", SHOP);
    write(dir.path(), "leaf.ply", LEAF);
    dir
}

fn examples() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    for entry in fs::read_dir(&root).expect("the example corpus must be present") {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "ply") {
            let name = path.file_name().unwrap().to_str().unwrap().to_string();
            write(dir.path(), &name, &fs::read_to_string(&path).unwrap());
        }
    }
    dir
}

#[test]
fn a_cold_run_and_a_warm_run_agree() {
    let dir = corpus();
    agree(dir.path(), "cold");
    agree(dir.path(), "warm");
}

#[test]
fn the_example_corpus_agrees_cold_and_warm() {
    let dir = examples();
    agree(dir.path(), "cold");
    agree(dir.path(), "warm");
}

#[test]
fn editing_a_body_agrees() {
    let dir = corpus();
    agree(dir.path(), "cold");
    edit(dir.path(), "leaf.ply", "one() + one()", "one() + one() + 0");
    agree(dir.path(), "body edit");
}

#[test]
fn changing_a_signature_agrees() {
    let dir = corpus();
    agree(dir.path(), "cold");
    edit(
        dir.path(),
        "leaf.ply",
        "pub fn one() -> Int = 1",
        "pub fn one() -> Money = 1",
    );
    edit(
        dir.path(),
        "leaf.ply",
        "pub fn two()",
        "pub type Money = Int\npub fn two()",
    );
    agree(dir.path(), "signature change");
}

#[test]
fn renaming_a_definition_agrees() {
    let dir = corpus();
    agree(dir.path(), "cold");
    edit(dir.path(), "core.ply", "pub fn label(", "pub fn title(");
    agree(dir.path(), "rename a function");
}

/// A `type` rename changes no hash, so only the resolution witness can tell the front end.
#[test]
fn renaming_a_type_agrees() {
    let dir = corpus();
    agree(dir.path(), "cold");
    edit(dir.path(), "core.ply", "Money", "Cost");
    edit(dir.path(), "shop.ply", "core::Item", "core::Item");
    agree(dir.path(), "rename a type");
}

#[test]
fn renaming_an_effect_agrees() {
    let dir = corpus();
    agree(dir.path(), "cold");
    edit(dir.path(), "core.ply", "effect db", "effect audit");
    edit(dir.path(), "shop.ply", "core::db.", "core::audit.");
    agree(dir.path(), "rename an effect");
}

#[test]
fn moving_a_definition_between_modules_agrees() {
    let dir = corpus();
    agree(dir.path(), "cold");
    edit(
        dir.path(),
        "core.ply",
        "pub fn twice<a>(x: a, f: (a) -> a) -> a = f(f(x))\n",
        "",
    );
    edit(
        dir.path(),
        "leaf.ply",
        "pub fn one() -> Int = 1",
        "pub fn twice<a>(x: a, f: (a) -> a) -> a = f(f(x))\npub fn one() -> Int = 1",
    );
    edit(
        dir.path(),
        "shop.ply",
        "import core",
        "import core\nimport leaf",
    );
    edit(dir.path(), "shop.ply", "core::twice", "leaf::twice");
    agree(dir.path(), "move between modules");
}

#[test]
fn adding_a_file_agrees() {
    let dir = corpus();
    agree(dir.path(), "cold");
    write(
        dir.path(),
        "extra.ply",
        "import leaf\nfn three() -> Int = leaf::two() + leaf::one()\n",
    );
    agree(dir.path(), "file added");
}

/// The referencing file did not change, so nothing about its bytes says its dependency is gone.
#[test]
fn deleting_a_file_is_reported_rather_than_skipped_past() {
    let dir = corpus();
    write(
        dir.path(),
        "extra.ply",
        "import leaf\npub fn three() -> Int = leaf::two()\n",
    );
    agree(dir.path(), "cold");

    fs::remove_file(dir.path().join("leaf.ply")).unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    let err = driver::load_incremental(dir.path(), &mut store)
        .expect_err("a dangling import must be an error, not a skipped file");
    assert!(
        err.diagnostics
            .iter()
            .any(|d| d.code == ply_span::codes::UNKNOWN_MODULE),
        "expected UNKNOWN_MODULE, got {:?}",
        codes(&err)
    );
}

#[test]
fn deleting_an_unreferenced_file_agrees() {
    let dir = corpus();
    write(dir.path(), "spare.ply", "pub fn spare() -> Int = 9\n");
    agree(dir.path(), "cold");
    fs::remove_file(dir.path().join("spare.ply")).unwrap();
    agree(dir.path(), "file deleted");
}

#[test]
fn adding_and_removing_an_import_agrees() {
    let dir = corpus();
    agree(dir.path(), "cold");
    edit(
        dir.path(),
        "shop.ply",
        "import core",
        "import core\nimport leaf",
    );
    agree(dir.path(), "import added");
    edit(
        dir.path(),
        "shop.ply",
        "import core\nimport leaf",
        "import core",
    );
    agree(dir.path(), "import removed");
}

#[test]
fn reformatting_agrees() {
    let dir = corpus();
    agree(dir.path(), "cold");
    edit(
        dir.path(),
        "leaf.ply",
        "pub fn one()",
        "// a comment\npub fn  one()",
    );
    agree(dir.path(), "reformatted");
}

#[test]
fn a_dependencys_change_reaches_its_dependents() {
    let dir = corpus();
    agree(dir.path(), "cold");
    edit(
        dir.path(),
        "core.ply",
        "pub fn price(i: Item) -> Money =",
        "pub fn price(i: Item) -> Int =",
    );
    agree(dir.path(), "dependency changed");
}

#[test]
fn a_warm_load_answers_as_the_cold_one_did_and_an_edit_is_seen() {
    let dir = corpus();
    let load = || {
        let mut store = Store::open(dir.path()).unwrap();
        driver::load_incremental(dir.path(), &mut store)
            .unwrap_or_else(|e| panic!("the load failed: {:?}", codes(&e)))
    };

    let cold = load();
    let warm = load();
    assert_eq!(snapshot(&warm), snapshot(&cold));

    let three = ply_span::Symbol::new("leaf.three");
    edit(
        dir.path(),
        "leaf.ply",
        "pub fn two()",
        "pub fn three() -> Int = 3\npub fn two()",
    );
    let edited = load();
    assert!(!warm.check.defs.contains_key(&three));
    assert!(
        edited.check.defs.contains_key(&three),
        "the load after an edit does not see it"
    );
    assert_eq!(snapshot(&load()), snapshot(&edited));
}

fn answer(loaded: &Loaded) -> String {
    format!("{:?}", loaded.front)
}

/// Seeding must not change an answer: a load handed what the last one published says exactly what
/// a load handed nothing says, through every kind of edit.
#[test]
fn a_seeded_load_answers_as_a_full_load_through_every_kind_of_edit() {
    let dir = corpus();
    let step = |what: &str| {
        let mut store = Store::open(dir.path()).unwrap();
        let incremental = driver::load_incremental(dir.path(), &mut store)
            .unwrap_or_else(|e| panic!("{what}: the incremental load failed: {:?}", codes(&e)));
        let full = driver::load_full(dir.path())
            .unwrap_or_else(|e| panic!("{what}: the full load failed: {:?}", codes(&e)));
        assert_eq!(answer(&incremental), answer(&full), "{what}");
    };

    step("cold");
    step("unchanged");

    edit(dir.path(), "leaf.ply", "one() + one()", "one() + one() + 0");
    step("a body edit in a leaf");
    edit(
        dir.path(),
        "leaf.ply",
        "pub fn two() -> Int = one() + one() + 0",
        "pub fn two(k: Int) -> Int = one() + k",
    );
    step("a signature edit in a leaf");
    edit(dir.path(), "leaf.ply", "pub fn one()", "fn one()");
    step("`pub` removed in a leaf");

    edit(
        dir.path(),
        "leaf.ply",
        "fn one() -> Int = 1",
        "fn one() -> Int = \"1\"",
    );
    let mut store = Store::open(dir.path()).unwrap();
    let incremental = driver::load_incremental(dir.path(), &mut store).expect_err("a type error");
    let full = driver::load_full(dir.path()).expect_err("a type error");
    assert_eq!(codes(&incremental), codes(&full));
    edit(
        dir.path(),
        "leaf.ply",
        "fn one() -> Int = \"1\"",
        "fn one() -> Int = 1",
    );
    step("the error undone");

    edit(dir.path(), "core.ply", "pub fn label(", "pub fn title(");
    step("a rename in a module another imports");
    edit(
        dir.path(),
        "core.ply",
        "Book(_, p) -> p,",
        "Book(_, p) -> p + 0,",
    );
    step("a body edit in a module another imports");
    edit(
        dir.path(),
        "shop.ply",
        "acc + core::price(i)",
        "acc + core::price(i) + 0",
    );
    step("a body edit in a module that imports another");

    edit(
        dir.path(),
        "shop.ply",
        "import core\n",
        "import core\nimport leaf\n",
    );
    step("an import added");
    edit(
        dir.path(),
        "shop.ply",
        "import core\nimport leaf\n",
        "import core\n",
    );
    step("an import removed");

    write(
        dir.path(),
        "extra.ply",
        "import leaf\npub fn four() -> Int = leaf::two(2) + 2\n",
    );
    step("a file added");
    edit(dir.path(), "extra.ply", "+ 2\n", "+ 3\n");
    step("an edit to the added file, which imports the leaf");
    edit(dir.path(), "leaf.ply", "one() + k", "one() + k + 0");
    step("an edit to the leaf the added file imports");
    edit(
        dir.path(),
        "leaf.ply",
        "fn one() -> Int = 1",
        "import core\n\nfn one() -> Int = core::price(core::Note(\"n\")) + 1",
    );
    edit(
        dir.path(),
        "core.ply",
        "Book(_, p) -> p + 0,",
        "Book(_, p) -> p + 1,",
    );
    step("an import added to a module edited with the one it now imports");
    fs::remove_file(dir.path().join("extra.ply")).unwrap();
    step("the added file deleted");

    step("unchanged again");
}

#[test]
fn prove_asks_for_claims_only_when_something_is_discharged_and_only_where_an_edit_reached() {
    use clap::Parser;
    use ply_cli::cli::{Cli, Command};
    use ply_codegen::c::producer;
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "base.ply",
        "\
pub fn one() -> Int = 1

pub fn bump(x: Int) -> Int
  requires x < 100
  ensures result > x
= x + one()
",
    );
    write(
        dir.path(),
        "app.ply",
        "\
import base

fn more(x: Int) -> Int
  requires x < 50
  ensures result > x
= base::bump(x)
",
    );
    write(
        dir.path(),
        "side.ply",
        "\
fn zero(x: Int) -> Int
  ensures result == 0
= x - x
",
    );
    let path = dir.path().to_str().unwrap().to_string();
    let prove = |what: &str, flags: &[&str], claimed: usize| {
        let mut argv = vec!["ply", "prove"];
        argv.extend_from_slice(flags);
        argv.push(&path);
        let Command::Prove(args) = Cli::parse_from(argv).command else {
            panic!("`ply prove` parsed as another command");
        };
        producer::reset_census();
        let code = ply_cli::commands::prove::execute(&args, ply_cli::style::Style::plain());
        assert_eq!(code, ply_cli::EXIT_OK, "{what}: every claim holds");
        assert_eq!(
            producer::census().claimed,
            claimed,
            "{what}: modules whose claims the port was asked for"
        );
    };

    prove("cold", &[], 3);
    prove("every obligation discharged again", &["--no-cache"], 0);
    edit(dir.path(), "side.ply", "x - x", "x - x + 0");
    prove("an edit to a module nothing imports", &[], 1);
    edit(dir.path(), "base.ply", "x + one()", "x + one() + 0");
    prove("an edit to a module another imports", &[], 2);
    fs::remove_file(dir.path().join(".ply-cache/claims.answer")).unwrap();
    prove("every obligation answered from the cache", &[], 0);
}

#[test]
fn the_full_path_writes_no_front_end_cache() {
    let dir = corpus();
    driver::load_full(dir.path()).unwrap();
    let store = Store::open(dir.path()).unwrap();
    assert!(
        store.frontend_is_empty(),
        "the full path must not populate the front-end cache"
    );
}

#[test]
fn a_corrupt_front_end_cache_degrades_to_the_full_path() {
    let dir = corpus();
    agree(dir.path(), "cold");
    fs::write(
        dir.path().join(".ply-cache/frontend.idx"),
        "not an index at all",
    )
    .unwrap();
    agree(dir.path(), "corrupt cache");
}

#[test]
fn a_definition_removed_outright_agrees() {
    let dir = corpus();
    agree(dir.path(), "cold");
    edit(
        dir.path(),
        "shop.ply",
        "fn doubled(n: Int) -> Int = core::twice(n, |x: Int| x + x)\n",
        "",
    );
    edit(
        dir.path(),
        "core.ply",
        "pub fn twice<a>(x: a, f: (a) -> a) -> a = f(f(x))\n",
        "",
    );
    agree(dir.path(), "definition removed");
}

/// One store for every step: each step's fingerprints are whatever the step before left behind.
#[test]
fn a_whole_editing_session_agrees_at_every_step() {
    let dir = corpus();
    agree(dir.path(), "step 0");

    edit(dir.path(), "leaf.ply", "one() + one()", "one() + 1");
    agree(dir.path(), "step 1: body");

    edit(dir.path(), "core.ply", "pub fn label(", "pub fn title(");
    agree(dir.path(), "step 2: rename");

    edit(dir.path(), "core.ply", "Money", "Cost");
    agree(dir.path(), "step 3: rename a type");

    write(
        dir.path(),
        "extra.ply",
        "import leaf\npub fn four() -> Int = leaf::two() + 2\n",
    );
    agree(dir.path(), "step 4: add a file");

    edit(
        dir.path(),
        "extra.ply",
        "leaf::two() + 2",
        "leaf::two() + 3",
    );
    agree(dir.path(), "step 5: edit the new file");

    fs::remove_file(dir.path().join("extra.ply")).unwrap();
    agree(dir.path(), "step 6: delete it again");

    edit(dir.path(), "core.ply", "effect db", "effect ledger");
    edit(dir.path(), "shop.ply", "core::db.", "core::ledger.");
    agree(dir.path(), "step 7: rename an effect");
}

/// Structurally identical definitions in two modules share a `DefHash` while their schemes name different types.
#[test]
fn two_definitions_that_share_a_hash_each_keep_their_own_interface() {
    let dir = tempfile::tempdir().unwrap();
    let body =
        "pub type Thing = | Wrap(Int)\npub fn peel(t: Thing) -> Int = match t { Wrap(n) -> n }\n";
    write(dir.path(), "a.ply", body);
    write(dir.path(), "b.ply", body);

    let cold = agree(dir.path(), "cold");
    assert_eq!(
        cold.hashes.defs[&ply_span::Symbol::new("a.peel")],
        cold.hashes.defs[&ply_span::Symbol::new("b.peel")],
        "the fixture is only interesting while the two hash alike"
    );
    let warm = agree(dir.path(), "warm");
    assert_ne!(
        format!(
            "{:?}",
            warm.check.defs[&ply_span::Symbol::new("a.peel")].scheme
        ),
        format!(
            "{:?}",
            warm.check.defs[&ply_span::Symbol::new("b.peel")].scheme
        ),
        "each definition's scheme must name its own module's type"
    );
}

/// `x.look` and `y.look` are one definition: they differ only by which of two identical effects they name.
#[test]
fn identically_declared_effects_in_two_modules_agree() {
    let dir = tempfile::tempdir().unwrap();
    let body = "pub effect db { read get[r](key: Int) -> Int }\n\
                pub fn look(k: Int) -> Int / {db.read[t]} = db.get[t](k)\n";
    write(dir.path(), "x.ply", body);
    write(dir.path(), "y.ply", body);
    write(dir.path(), "z.ply", "pub fn plain() -> Int = 7\n");
    let pick = |m: &str| {
        format!(
            "import x\nimport y\n\
             pub fn pick(k: Int) -> Int =\n\
               handle x::look(k) + y::look(k) with {{ {m}::db.get[t](j) -> j, }}\n"
        )
    };
    write(dir.path(), "w.ply", &pick("x"));

    agree(dir.path(), "cold");
    let warm = agree(dir.path(), "warm");
    assert_eq!(
        warm.hashes.defs[&ply_span::Symbol::new("x.look")],
        warm.hashes.defs[&ply_span::Symbol::new("y.look")],
        "two performers that differ only by which look-alike they name are one definition"
    );
    let via_x = warm.hashes.defs[&ply_span::Symbol::new("w.pick")];

    write(dir.path(), "w.ply", &pick("y"));
    let after = agree(dir.path(), "the handler switched to the other capability");
    assert_ne!(
        after.hashes.defs[&ply_span::Symbol::new("w.pick")],
        via_x,
        "two effects of the same shape are still different capabilities"
    );

    write(dir.path(), "z.ply", "pub fn plain() -> Int = 8\n");
    agree(dir.path(), "an unrelated edit");
}

/// A `type` alias has no constructors, so nothing about it survives in a cached declaration beyond its arity.
#[test]
fn a_module_declaring_a_type_alias_agrees() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "alias.ply",
        "pub type Cents = Int\npub fn zero() -> Cents = 0\n",
    );
    write(
        dir.path(),
        "user.ply",
        "import alias\nfn take() -> alias::Cents = alias::zero()\n",
    );
    write(dir.path(), "lone.ply", "fn lone() -> Int = 1\n");

    agree(dir.path(), "cold");
    agree(dir.path(), "warm");

    write(dir.path(), "lone.ply", "fn lone() -> Int = 2\n");
    agree(dir.path(), "an unrelated edit");
}

#[test]
fn a_file_whose_tests_changed_agrees() {
    let dir = tempfile::tempdir().unwrap();
    let base = "fn f() -> Int = 1\ntest \"f is one\" { assert_eq(f(), 1) }\n";
    write(dir.path(), "m.ply", base);
    agree(dir.path(), "cold");

    write(
        dir.path(),
        "m.ply",
        &format!("{base}test \"twice\" {{ assert_eq(f() + f(), 2) }}\n"),
    );
    agree(dir.path(), "a test added");
}

#[test]
fn a_test_added_with_a_body_already_present_agrees() {
    let dir = tempfile::tempdir().unwrap();
    let base = "fn f() -> Int = 1\ntest \"f is one\" { assert_eq(f(), 1) }\n";
    write(dir.path(), "m.ply", base);
    let cold = agree(dir.path(), "cold");

    write(
        dir.path(),
        "m.ply",
        &format!("{base}test \"still one\" {{ assert_eq(f(), 1) }}\n"),
    );
    let after = agree(dir.path(), "a duplicate test added");
    assert_eq!(
        cold.hashes.tests[0], after.hashes.tests[0],
        "the first test's body did not move, so its hash may not either"
    );
    assert_eq!(
        after.hashes.tests[0], after.hashes.tests[1],
        "two tests with one body are one computation"
    );
}

/// Real code exercises handlers, regions, `nondet` effects and cross-module types the synthetic corpus does not.
#[test]
fn the_example_corpus_agrees_across_a_session() {
    let dir = examples();
    agree(dir.path(), "step 0");

    edit(
        dir.path(),
        "report.ply",
        "fn assets() -> List<String>",
        "// a note\nfn assets() -> List<String>",
    );
    agree(dir.path(), "step 1: a comment");

    edit(dir.path(), "ledger.ply", "presented", "presented_value");
    edit(dir.path(), "report.ply", "presented", "presented_value");
    agree(dir.path(), "step 2: rename across modules");

    edit(dir.path(), "report.ply", "type Line = ", "type Row = ");
    edit(dir.path(), "report.ply", "-> Line =", "-> Row =");
    edit(dir.path(), "report.ply", "List<Line>", "List<Row>");
    edit(dir.path(), "report.ply", "l: Line|", "l: Row|");
    agree(dir.path(), "step 3: rename a type");

    fs::write(
        dir.path().join("clock.ply"),
        fs::read_to_string(dir.path().join("clock.ply")).unwrap() + "\npub fn ticks() -> Int = 0\n",
    )
    .unwrap();
    agree(dir.path(), "step 4: add a definition");
}

#[test]
fn a_long_shuffle_of_compiling_states_agrees_at_every_step() {
    // Each file's variants differ in a body, a signature, a name, a declaration's shape, or an import.
    let variants: [(&str, [&str; 3]); 3] = [
        (
            "leaf.ply",
            [
                "pub fn one() -> Int = 1\npub fn two() -> Int = one() + one()\n",
                "// reformatted\npub fn one() -> Int = 1\n\npub fn two() -> Int = one() + one()\n",
                "pub fn one() -> Int = 1\npub fn two() -> Int = one() + 1\n",
            ],
        ),
        (
            "core.ply",
            [
                CORE,
                &const_str_replace(CORE, "Money", "Cost"),
                &const_str_replace(CORE, "pub fn label(", "pub fn title("),
            ],
        ),
        (
            "shop.ply",
            [
                SHOP,
                &const_str_replace(SHOP, "import core\n", "import core\nimport leaf\n"),
                &const_str_replace(SHOP, "acc + core::price(i)", "acc + core::price(i) + 0"),
            ],
        ),
    ];

    let dir = corpus();
    let mut state = [0usize; 3];
    let mut seed: u64 = 0x5eed_1234_9abc_def0;
    for step in 0..60 {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let file = (seed >> 33) as usize % variants.len();
        let pick = (seed >> 17) as usize % 3;
        if state[file] == pick {
            continue;
        }
        state[file] = pick;

        let (name, bodies) = &variants[file];
        write(dir.path(), name, bodies[pick]);
        agree(
            dir.path(),
            &format!("shuffle step {step}: {name} -> {pick}"),
        );
    }
}

/// `str::replace` is not `const`, and the variants are edits of the corpus rather than copies that can drift.
fn const_str_replace(text: &str, from: &str, to: &str) -> String {
    assert!(text.contains(from), "`{from}` is not in the fixture");
    text.replace(from, to)
}

const PALETTE: &str = r#"
pub type Color = Red | Green

pub fn paint(what: String, shade: Color = Red) -> String =
  match shade {
    Red -> string_concat(what, " red"),
    Green -> string_concat(what, " green"),
  }
"#;

const WALL: &str = r#"
import palette

pub fn wall() -> String = palette::paint("wall")
pub fn given() -> String = palette::paint("wall", palette::Green)

test "the default crosses the module boundary" {
  assert_eq(wall(), "wall red")
}
"#;

#[test]
fn editing_a_cross_module_default_agrees_and_moves_the_importer() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "palette.ply", PALETTE);
    write(dir.path(), "wall.ply", WALL);

    let cold = agree(dir.path(), "cold");
    let before = cold.hashes.defs[&ply_span::Symbol::new("wall.wall")];

    // Only the default moves.
    edit(
        dir.path(),
        "palette.ply",
        "shade: Color = Red",
        "shade: Color = Green",
    );

    let after = agree(dir.path(), "default edit");
    assert_ne!(
        before,
        after.hashes.defs[&ply_span::Symbol::new("wall.wall")],
        "`wall` was expanded against a default that changed, so its hash has to move"
    );
}

#[test]
fn the_three_spellings_of_one_call_are_one_definition() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "m.ply",
        r#"
pub fn greet(name: String, greeting: String = "hello") -> String =
  string_concat(greeting, name)

pub fn omitted() -> String = greet("ada")
pub fn written() -> String = greet("ada", "hello")
pub fn by_name() -> String = greet("ada", greeting: "hello")
pub fn different() -> String = greet("ada", "hi")
"#,
    );
    let loaded = agree(dir.path(), "cold");
    let of = |n: &str| loaded.hashes.defs[&ply_span::Symbol::new(format!("m.{n}"))];
    assert_eq!(of("omitted"), of("written"));
    assert_eq!(of("omitted"), of("by_name"));
    assert_ne!(
        of("omitted"),
        of("different"),
        "a call that really does pass another value must not collide with one that does not"
    );
}

/// A `reuse fn` is checked whole-program, so every load must know the module holds one and have its body.
#[test]
fn a_promise_is_known_on_every_run_and_still_refused() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    write(
        dir,
        "grow.ply",
        "reuse fn grow(xs: List<Int>, n: Int) -> List<Int> = {\n\
         \x20 let ys = push(xs, n);\n\
         \x20 if len(xs) < 0 { xs } else { ys }\n\
         }\n",
    );
    let first = agree(dir, "first run");
    assert!(first.promised);

    let second = agree(dir, "second run");
    assert!(second.promised, "the promise was lost on the second run");

    let broken = ply_cli::costs::promises(&second);
    assert_eq!(broken.len(), 1, "{broken:#?}");
    assert_eq!(broken[0].code, ply_span::codes::REUSE_BROKEN);
}
