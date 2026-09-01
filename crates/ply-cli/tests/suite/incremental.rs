//! The equivalence property, which is the whole safety argument for the incremental front end.

use ply_cli::driver;
use ply_cli::load::Loaded;
use ply_store::Store;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// `{:?}` rather than a printed signature: `print_scheme` renames variables per item, which would
/// paper over exactly the numbering divergence that made canonicalization necessary in the first
/// place.
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

/// The incremental run goes first so it sees the store as an edit-test loop would, and its result
/// is returned so a caller can assert that a gate actually fired — an equivalence that holds
/// because nothing was skipped proves nothing.
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
fn a_cold_run_and_a_warm_run_agree_and_the_warm_one_skips() {
    let dir = corpus();
    agree(dir.path(), "cold");
    let warm = agree(dir.path(), "warm");
    assert!(
        warm.frontend.skipped() > 0,
        "gate 1 never fired, so the equivalence proves nothing: {:?}",
        warm.frontend.files
    );
    assert!(warm.frontend.cached() > 0, "gate 2 never fired");
}

#[test]
fn the_example_corpus_agrees_cold_and_warm() {
    let dir = examples();
    agree(dir.path(), "cold");
    let warm = agree(dir.path(), "warm");
    assert!(
        warm.frontend.skipped() > 0,
        "gate 1 never fired on the examples"
    );
}

#[test]
fn editing_a_body_agrees() {
    let dir = corpus();
    agree(dir.path(), "cold");
    edit(dir.path(), "leaf.ply", "one() + one()", "one() + one() + 0");
    let after = agree(dir.path(), "body edit");
    assert!(after.frontend.parsed() > 0);
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

/// The case the resolution witness exists for: a `type` rename changes no hash at all, so nothing
/// but the witness can tell the front end that every scheme mentioning it is now written in a
/// different name.
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

/// The case a content-only gate would get wrong: the *referencing* file did not change, so nothing
/// about its bytes says its dependency is gone.
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

/// Gate 1 is conservative about formatting and gate 2 is exact, so a reformat must cost a parse and
/// no inference at all.
#[test]
fn reformatting_costs_a_parse_and_no_recheck() {
    let dir = corpus();
    agree(dir.path(), "cold");
    edit(
        dir.path(),
        "leaf.ply",
        "pub fn one()",
        "// a comment\npub fn  one()",
    );
    let after = agree(dir.path(), "reformatted");

    let leaf = after
        .frontend
        .files
        .iter()
        .find(|f| f.path.ends_with("leaf.ply"))
        .expect("leaf.ply must be reported");
    assert!(leaf.parsed, "a changed file must be parsed");
    assert!(
        !leaf.rechecked,
        "an unchanged set of hashes must not be rechecked"
    );
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
    let after = agree(dir.path(), "dependency changed");
    let shop = after
        .frontend
        .files
        .iter()
        .find(|f| f.path.ends_with("shop.ply"))
        .expect("shop.ply must be reported");
    assert!(
        shop.parsed,
        "an importer of a changed module must be parsed"
    );
}

/// `--no-incremental` must neither read nor write the front-end cache, so a run under it can never
/// be the reason a later run skips something.
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

/// A cache the run cannot believe is a slower run, never a wrong one.
#[test]
fn a_corrupt_front_end_cache_degrades_to_the_full_path() {
    let dir = corpus();
    agree(dir.path(), "cold");
    fs::write(
        dir.path().join(".ply-cache/frontend.idx"),
        "not an index at all",
    )
    .unwrap();
    let after = agree(dir.path(), "corrupt cache");
    assert_eq!(
        after.frontend.skipped(),
        0,
        "nothing may be skipped on the evidence of a corrupt cache"
    );
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

/// Every mutation in sequence against one store, which is what an editing session actually looks
/// like: each step's fingerprints are whatever the step before it left behind.
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

/// Two structurally identical definitions in different modules share a `DefHash` while their
/// schemes name different types.
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
    for name in ["a.ply", "b.ply"] {
        let file = warm
            .frontend
            .files
            .iter()
            .find(|f| f.path.ends_with(name))
            .unwrap();
        assert!(
            !file.parsed,
            "{name} was refused ({}); a shared hash must not cost a recheck",
            file.refusal.describe()
        );
    }
}

/// Two byte-identical effect declarations are two capabilities, and `x.look` and `y.look` are still
/// one definition: they differ only by which of the two they name, which no context can observe.
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

/// A `type` alias has no constructors, so nothing about it survives in a cached declaration beyond
/// its arity.
#[test]
fn a_module_declaring_a_type_alias_can_still_be_skipped() {
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
    let warm = agree(dir.path(), "warm");
    assert!(
        warm.frontend
            .files
            .iter()
            .any(|f| f.path.ends_with("alias.ply") && !f.parsed)
    );

    write(dir.path(), "lone.ply", "fn lone() -> Int = 2\n");
    let after = agree(dir.path(), "an unrelated edit");
    assert!(
        after
            .frontend
            .files
            .iter()
            .any(|f| f.path.ends_with("alias.ply") && !f.parsed),
        "an edit elsewhere must not disturb a module nothing parsed imports"
    );
}

/// A test's footprint is written in effect names that its hash erases, and a `CachedTest` carries
/// no witness, so a test whose hash the fingerprint does not already hold has to be checked rather
/// than restored.
#[test]
fn a_file_whose_tests_changed_is_checked_rather_than_restored() {
    let dir = tempfile::tempdir().unwrap();
    let base = "fn f() -> Int = 1\ntest \"f is one\" { assert_eq(f(), 1) }\n";
    write(dir.path(), "m.ply", base);
    agree(dir.path(), "cold");

    write(
        dir.path(),
        "m.ply",
        &format!("{base}test \"twice\" {{ assert_eq(f() + f(), 2) }}\n"),
    );
    let after = agree(dir.path(), "a test added");
    let m = after
        .frontend
        .files
        .iter()
        .find(|f| f.path.ends_with("m.ply"))
        .unwrap();
    assert!(m.rechecked, "a file with a new test must be checked");
}

/// A test's footprint is a function of its body, so a test added with a body the file already holds
/// is a test whose footprint the file already holds.
#[test]
fn a_test_added_with_a_body_already_present_costs_no_recheck() {
    let dir = tempfile::tempdir().unwrap();
    let base = "fn f() -> Int = 1\ntest \"f is one\" { assert_eq(f(), 1) }\n";
    write(dir.path(), "m.ply", base);
    agree(dir.path(), "cold");

    write(
        dir.path(),
        "m.ply",
        &format!("{base}test \"still one\" {{ assert_eq(f(), 1) }}\n"),
    );
    let after = agree(dir.path(), "a duplicate test added");
    let m = after
        .frontend
        .files
        .iter()
        .find(|f| f.path.ends_with("m.ply"))
        .unwrap();
    assert!(!m.rechecked, "the added test's body was already checked");
}

/// The mutations again on real code, which exercises handlers, regions, `nondet` effects and
/// cross-module types that the synthetic corpus does not.
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

/// A long shuffle through states that all compile, because the failures worth finding are the ones
/// nobody thought to write a case for: an invalidation is wrong only in some *sequence* of edits,
/// and a hand-written case only ever exercises the sequence its author imagined.
#[test]
fn a_long_shuffle_of_compiling_states_agrees_at_every_step() {
    // Each file's variants differ in a way the gates must notice: a body, a signature, a name, a
    // declaration's shape, an import.
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

/// `str::replace` is not `const`, and the variants above want to be written as edits of the corpus
/// rather than as three copies that can drift apart.
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

/// **The stale-expansion hazard record update refused record update over, checked where defaults do
/// cross the boundary.**
#[test]
fn editing_a_cross_module_default_agrees_and_refuses_the_importer() {
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
    assert!(
        after.frontend.parsed() > 1,
        "only {} file(s) were parsed: the importer skipped, and its call still \
         carries the old default",
        after.frontend.parsed()
    );
    assert_ne!(
        before,
        after.hashes.defs[&ply_span::Symbol::new("wall.wall")],
        "`wall` was expanded against a default that changed, so its hash has to move"
    );
}

/// The identity the whole design rests on, at the level a user sees: three spellings of one call
/// are one definition with one hash, so adopting a default or a name re-runs nothing.
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
