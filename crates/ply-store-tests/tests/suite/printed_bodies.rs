//! Definition bodies printed back to source: the third element of `Hash -> (Definition, Type,
//! Footprint)`.

use ply_codegen::c::producer::print_bodies;
use ply_span::{Diagnostic, SourceId, Symbol, codes};
use ply_store::body::{BodySet, StoredBody};
use ply_ty::{CheckOutput, DefHash, HashOutput};
use ply_ty::{LabelVar, Resource, Row, RowVar, Scheme, TyVar, Type};
use std::collections::{BTreeMap, BTreeSet};

/// `files[i]` is `(module name, text)` for `SourceId(i)`.
#[track_caller]
fn port_front(files: &[(&str, &str)]) -> ply_ty::Front {
    let named: Vec<(String, String)> = files
        .iter()
        .map(|(name, text)| ((*name).to_string(), (*text).to_string()))
        .collect();
    let ids: Vec<SourceId> = (0..files.len()).map(|i| SourceId(i as u32)).collect();
    ply_codegen::c::producer::checked_front(&named, &ids)
        .unwrap_or_else(|e| panic!("the program must typecheck: {e:#}"))
}

struct Checked {
    hashes: HashOutput,
    check: CheckOutput,
    bodies: BodySet,
}

fn compile(files: &[(&str, &str)]) -> Checked {
    let front = port_front(files);
    Checked {
        bodies: ply_store::body::of_front(&front),
        hashes: front.hashes,
        check: front.check,
    }
}

fn names_of(checked: &Checked) -> Vec<(Symbol, DefHash)> {
    checked
        .hashes
        .defs
        .iter()
        .chain(checked.hashes.decls.iter())
        .map(|(name, hash)| (name.clone(), *hash))
        .collect()
}

fn print(
    bodies: &BodySet,
    names: &[(Symbol, DefHash)],
    shipped: &[&str],
) -> Result<Vec<(String, String)>, Diagnostic> {
    let bytes: Vec<&[u8]> = bodies.defs().map(|(_, body)| body.as_bytes()).collect();
    let tests: Vec<&[u8]> = bodies.tests().iter().map(StoredBody::as_bytes).collect();
    let names: Vec<(&str, DefHash)> = names.iter().map(|(n, h)| (n.as_str(), *h)).collect();
    print_bodies(&bytes, &names, &tests, &[], shipped)
}

/// Quantified variables renumbered from zero in traversal order.
fn canonical(scheme: &Scheme) -> Scheme {
    let mut tys: BTreeMap<TyVar, TyVar> = BTreeMap::new();
    let mut rows: BTreeMap<RowVar, RowVar> = BTreeMap::new();
    // Labels first, in the head's order: an atom sorts by the number its label holds.
    let mut labels: BTreeMap<LabelVar, LabelVar> = BTreeMap::new();
    for v in &scheme.label_vars {
        let next = LabelVar(labels.len() as u32);
        labels.entry(*v).or_insert(next);
    }
    let ty = renumber(&scheme.ty, &mut tys, &mut rows, &mut labels);
    Scheme {
        ty_vars: scheme
            .ty_vars
            .iter()
            .filter_map(|v| tys.get(v))
            .copied()
            .collect(),
        row_vars: scheme
            .row_vars
            .iter()
            .filter_map(|v| rows.get(v))
            .copied()
            .collect(),
        label_vars: scheme
            .label_vars
            .iter()
            .filter_map(|v| labels.get(v))
            .copied()
            .collect(),
        ty,
    }
}

fn renumber(
    ty: &Type,
    tys: &mut BTreeMap<TyVar, TyVar>,
    rows: &mut BTreeMap<RowVar, RowVar>,
    labels: &mut BTreeMap<LabelVar, LabelVar>,
) -> Type {
    match ty {
        Type::Var(v) => {
            let next = TyVar(tys.len() as u32);
            Type::Var(*tys.entry(*v).or_insert(next))
        }
        Type::Con(name, args) => Type::Con(
            name.clone(),
            args.iter()
                .map(|a| renumber(a, tys, rows, labels))
                .collect(),
        ),
        Type::Fn {
            params,
            ret,
            effects,
        } => {
            let params = params
                .iter()
                .map(|p| renumber(p, tys, rows, labels))
                .collect();
            let ret = Box::new(renumber(ret, tys, rows, labels));
            let tail = effects.tail.map(|t| {
                let next = RowVar(rows.len() as u32);
                *rows.entry(t).or_insert(next)
            });
            let atoms = effects
                .atoms
                .iter()
                .map(|atom| {
                    let mut out = atom.clone();
                    if let Resource::Var(v) = atom.resource {
                        let next = LabelVar(labels.len() as u32);
                        out.resource = Resource::Var(*labels.entry(v).or_insert(next));
                    }
                    out
                })
                .collect();
            Type::Fn {
                params,
                ret,
                effects: Row { atoms, tail },
            }
        }
        Type::Record(fields) => Type::Record(
            fields
                .iter()
                .map(|(k, v)| (k.clone(), renumber(v, tys, rows, labels)))
                .collect(),
        ),
    }
}

#[track_caller]
fn round_trip(files: &[(&str, &str)]) -> (Checked, Vec<(String, String)>) {
    let original = compile(files);
    let names = names_of(&original);
    let printed = print(&original.bodies, &names, &[])
        .unwrap_or_else(|d| panic!("the names say everything: {d:#?}"));
    let borrowed: Vec<(&str, &str)> = printed
        .iter()
        .map(|(name, text)| (name.as_str(), text.as_str()))
        .collect();
    let again = compile(&borrowed);
    for (name, hash) in &names {
        let now = again
            .hashes
            .defs
            .get(name)
            .or_else(|| again.hashes.decls.get(name))
            .unwrap_or_else(|| panic!("`{name}` did not come back"));
        assert_eq!(now, hash, "`{name}` came back as a different definition");
    }
    let wanted: BTreeSet<&Symbol> = names.iter().map(|(name, _)| name).collect();
    let rebuilt: BTreeSet<&Symbol> = again
        .hashes
        .defs
        .keys()
        .chain(again.hashes.decls.keys())
        .collect();
    assert_eq!(rebuilt, wanted, "a name was invented or dropped");
    assert_eq!(
        again.hashes.tests, original.hashes.tests,
        "a test came back as another"
    );

    for (name, info) in &original.check.defs {
        let after = &again.check.defs[name];
        assert_eq!(
            canonical(&info.scheme),
            canonical(&after.scheme),
            "`{name}` came back with a different type"
        );
        assert_eq!(
            info.footprint, after.footprint,
            "`{name}` came back with a different footprint"
        );
    }
    for (before, after) in original.check.tests.iter().zip(&again.check.tests) {
        assert_eq!(
            before.footprint, after.footprint,
            "test `{}` came back with a different footprint",
            before.key
        );
        assert_eq!(before.nondet, after.nondet);
    }
    (original, printed)
}

fn text_of<'a>(printed: &'a [(String, String)], module: &str) -> &'a str {
    &printed
        .iter()
        .find(|(name, _)| name == module)
        .unwrap_or_else(|| panic!("`{module}` was not printed"))
        .1
}

const EVERY_ITEM_KIND: &str = r#"
effect db {
  read  get[r](key: Int) -> Int
  write put[r](key: Int, value: Int) -> Unit
}

nondet effect clock {
  read now() -> Int
}

type Colour = | Red | Green | Blue(Int)

type Pair<a> = { left: a, right: a }

type Alias = Int

fn identity<a>(x: a) -> a = x

fn describe(c: Colour) -> String = match c {
  Red -> "red",
  Green -> "green",
  Blue(n) -> string_concat("blue ", int_to_string(n)),
}

fn stash(key: Int) -> Unit / {db.write[cache]} = db.put[cache](key, key + 1)

fn read_back(key: Int) -> Int / {db.read[cache]} = db.get[cache](key)

fn swap<a>(p: Pair<a>) -> Pair<a> = { left: p.right, right: p.left }

fn total(xs: List<Int>) -> Int = fold(xs, 0, |acc, x| acc + x)

fn widen(n: Alias) -> Int = n

fn verb(head: Bytes) -> Bytes = match head {
  b"GET" -> b"GET",
  b"\r\n\x00" -> b"",
  _ -> bytes_slice(head, 0, bytes_len(head)),
}

fn head_text(head: Bytes) -> String =
  if bytes_is_utf8(head) { string_of_bytes(head) } else { string_of_bytes_lossy(head) }

test "a byte literal survives the body encoding" {
  assert_eq(verb(b"GET"), b"GET");
  assert_eq(verb(b"\r\n\x00"), b"");
  assert_eq(head_text(bytes_of_string("é")), "é");
  assert_eq(string_len(head_text(b"\xff")), 1)
}

test "handled effects are discharged" {
  with_cell[cache](0) { c ->
    handle {
      stash(1);
      assert_eq(read_back(1), 0)
    } with {
      db.put[cache](k, v) -> cell_set(c, v),
      db.get[cache](k) -> cell_get(c),
      return x -> x,
    }
  }
}

test/nondet "the clock is not deterministic" {
  assert(clock.now() >= 0)
}
"#;

const EVERY_OPERATOR: &str = r#"
pub fn arithmetic(a: Int, b: Int) -> Int = a + b - a * b / a % b
pub fn comparison(a: Int, b: Int) -> Bool = (a == b) && (a != b) || (a < b) && (a <= b) || (a > b) && (a >= b)
pub fn concatenation(a: String, b: String) -> String = a ++ b
pub fn bits(a: Int, b: Int) -> Int = a & b | a ^ b
pub fn shifts(a: Int, b: Int) -> Int = (a << b) + (a >> b) + (a >>> b)
pub fn prefixes(a: Int, p: Bool) -> Int = -a + ~a + (if !p { 1 } else { 0 })
"#;

#[test]
fn every_operator_survives_printing() {
    let (original, _) = round_trip(&[("m", EVERY_OPERATOR)]);
    assert_eq!(
        original.hashes.defs.len(),
        6,
        "the sample lost a definition"
    );
}

#[test]
fn every_item_kind_round_trips() {
    let (_, printed) = round_trip(&[("m", EVERY_ITEM_KIND)]);
    let m = text_of(&printed, "m");
    for form in [
        "pub effect db {",
        "pub nondet effect clock {",
        "pub type Colour = | Red | Green | Blue(Int) ",
        "pub type Pair<_t0> = {left: _t0, right: _t0}",
        "pub type Alias = Int",
        "pub fn identity<_t0>(_l0: _t0) -> _t0 =",
    ] {
        assert!(m.contains(form), "`{form}` is not in:\n{m}");
    }
    let tests = text_of(&printed, "ply_tests");
    assert!(tests.contains("test/nondet \"t2\""), "{tests}");
}

#[test]
fn numeric_literals_and_regions_and_constraints_survive_printing() {
    round_trip(&[(
        "m",
        r#"
        pub fn neg_zero() -> Float = -0.0
        pub fn zero() -> Float = 0.0
        pub fn scaled() -> Decimal = 1.50m
        pub fn tiny() -> Decimal = -0.000000000000000000000000001m
        pub fn huge() -> Float = 1e300
        pub fn tenth() -> Float = 0.1
        pub fn total(a: Decimal, b: Decimal) -> Decimal = a + b * 2m
        pub fn rate() -> Float = 1.5 / 0.0
        pub fn narrow() -> U8 = 255u8
        pub fn pattern() -> Int = 0xFFFFFFFFFFFFFFFF
        pub fn shaped() -> Int = with_cell[r](7) { c -> cell_get(c) }
        pub fn keep<a>(x: a) -> a where derivable(ord, a), derivable(eq, a) = x
        "#,
    )]);
}

#[test]
fn cross_module_references_resolve_once_printed() {
    round_trip(&[
        (
            "store",
            r#"
            pub effect db {
              read get[r](key: Int) -> Int
            }
            pub type Row = | Row(Int)
            pub fn fetch(key: Int) -> Row / {db.read[users]} = Row(db.get[users](key))
            "#,
        ),
        (
            "app",
            r#"
            import store

            fn value(key: Int) -> Int / {store::db.read[users]} =
              match store::fetch(key) { store::Row(n) -> n }

            test "a cross-module call is printable" {
              handle {
                assert_eq(value(1), 7)
              } with { store::db.get[users](k) -> 7 }
            }
            "#,
        ),
    ]);
}

#[test]
fn a_self_recursive_definition_imports_nothing() {
    let (original, printed) = round_trip(&[(
        "m",
        r#"
        pub fn countdown(n: Int) -> Int = if n == 0 { 0 } else { countdown(n - 1) }

        test "self recursion" { assert_eq(countdown(4), 0) }
        "#,
    )]);
    let hash = original.hashes.defs[&Symbol::new("m.countdown")];
    assert!(
        original
            .bodies
            .get(hash)
            .expect("a body for countdown")
            .verify(hash)
    );
    assert!(!text_of(&printed, "m").contains("import"), "{printed:?}");
}

#[test]
fn a_mutually_recursive_component_round_trips_wired_the_way_it_was_written() {
    let src = r#"
        pub fn is_even(n: Int) -> Bool = if n == 0 { true } else { is_odd(n - 1) }
        pub fn is_odd(n: Int) -> Bool = if n == 0 { false } else { is_even(n - 1) }
        "#;
    let (original, _) = round_trip(&[("m", src)]);
    let even = original.hashes.defs[&Symbol::new("m.is_even")];
    let odd = original.hashes.defs[&Symbol::new("m.is_odd")];
    assert_ne!(even, odd, "the two members are not interchangeable");

    let a = original.bodies.get(even).expect("a body for is_even");
    let b = original.bodies.get(odd).expect("a body for is_odd");
    assert_ne!(a, b, "one payload, two class indices");
    assert!(a.verify(even) && b.verify(odd));
}

#[test]
fn two_cycles_wired_in_opposite_directions_do_not_collide() {
    let clockwise = r#"
        pub fn f(n: Int) -> Int = g(n - 1) + 1
        pub fn g(n: Int) -> Int = h(n - 1) + 2
        pub fn h(n: Int) -> Int = f(n - 1) + 3
        "#;
    let widdershins = r#"
        pub fn f(n: Int) -> Int = h(n - 1) + 1
        pub fn h(n: Int) -> Int = g(n - 1) + 3
        pub fn g(n: Int) -> Int = f(n - 1) + 2
        "#;
    let (one, _) = round_trip(&[("m", clockwise)]);
    let (other, _) = round_trip(&[("m", widdershins)]);
    let one: BTreeSet<DefHash> = one.hashes.defs.values().copied().collect();
    let other: BTreeSet<DefHash> = other.hashes.defs.values().copied().collect();
    assert_eq!(one.len(), 3, "three distinguishable members");
    assert!(
        one.is_disjoint(&other),
        "the two wirings are different computations and must not share a hash"
    );
}

#[test]
fn a_body_verifies_only_against_its_own_key() {
    let original = compile(&[(
        "m",
        "pub fn f(x: Int) -> Int = x + 1\npub fn g(x: Int) -> Int = x + 2\n",
    )]);
    let f = original.hashes.defs[&Symbol::new("m.f")];
    let g = original.hashes.defs[&Symbol::new("m.g")];

    let body = original.bodies.get(f).expect("a body for f");
    assert_eq!(body.key(), Some(f));
    assert!(body.verify(f));
    assert!(!body.verify(g));
}

#[test]
fn a_truncated_body_is_refused_rather_than_printed() {
    let original = compile(&[("m", "pub fn f(x: Int) -> Int = x + 1\n")]);
    let hash = original.hashes.defs[&Symbol::new("m.f")];
    let mut bytes = original.bodies.get(hash).unwrap().as_bytes().to_vec();
    bytes.truncate(bytes.len() - 1);
    let body = StoredBody::from_bytes(bytes).expect("still an envelope");
    let key = body.key().expect("a solo body keys itself");

    let refused = print_bodies(&[body.as_bytes()], &[("m.f", key)], &[], &[], &[])
        .expect_err("a truncated body must not print");
    assert_eq!(refused.code, codes::ARTIFACT_INVALID);
}

#[test]
fn a_body_carrying_an_out_of_range_decimal_is_refused() {
    let original = compile(&[("m", "pub fn f() -> Decimal = 1.50m")]);
    let (_, body) = original.bodies.defs().next().expect("one definition");
    let mut bytes = body.as_bytes().to_vec();
    let scale = bytes
        .windows(4)
        .rposition(|w| w == 2u32.to_le_bytes())
        .expect("the scale is in the stream");
    bytes[scale..scale + 4].copy_from_slice(&99u32.to_le_bytes());
    let body = StoredBody::from_bytes(bytes).expect("still a body envelope");
    let key = body.key().expect("a solo body keys itself");

    let refused = print_bodies(&[body.as_bytes()], &[("m.f", key)], &[], &[], &[])
        .expect_err("a scale of 99 is not a `Decimal`");
    assert!(
        refused.message.contains("not a `Decimal`"),
        "{}",
        refused.message
    );
}

#[test]
fn a_name_whose_hash_no_body_carries_is_refused() {
    let original = compile(&[(
        "m",
        "pub fn f(x: Int) -> Int = x + 1\npub fn g(x: Int) -> Int = x + 2\n",
    )]);
    let f = original.hashes.defs[&Symbol::new("m.f")];
    let g = original.hashes.defs[&Symbol::new("m.g")];

    let refused = print_bodies(
        &[original.bodies.get(f).unwrap().as_bytes()],
        &[("m.g", g)],
        &[],
        &[],
        &[],
    )
    .expect_err("a misfiled body must not print");
    assert!(
        refused.message.contains("`m.g` has no body"),
        "{}",
        refused.message
    );
}

#[test]
fn a_reference_with_no_body_is_named_rather_than_guessed() {
    let original = compile(&[(
        "m",
        "fn helper(x: Int) -> Int = x + 1\npub fn caller(x: Int) -> Int = helper(x)\n",
    )]);
    let caller = original.hashes.defs[&Symbol::new("m.caller")];
    let helper = original.hashes.defs[&Symbol::new("m.helper")];

    let refused = print_bodies(
        &[original.bodies.get(caller).unwrap().as_bytes()],
        &[("m.caller", caller)],
        &[],
        &[],
        &[],
    )
    .expect_err("an open set must not print");
    assert!(
        refused.message.contains("not among the bodies"),
        "{}",
        refused.message
    );
    assert!(
        refused.notes.iter().any(|n| n.contains(&helper.short())),
        "{:?}",
        refused.notes
    );
}

#[test]
fn renaming_changes_no_body() {
    let before = compile(&[(
        "m",
        "pub fn f(x: Int) -> Int = x + 1\npub fn g() -> Int = f(1)\n",
    )]);
    let after = compile(&[(
        "m",
        "pub fn renamed(x: Int) -> Int = x + 1\npub fn g() -> Int = renamed(1)\n",
    )]);

    let mut lhs: Vec<_> = before.bodies.defs().map(|(h, b)| (h, b.clone())).collect();
    let mut rhs: Vec<_> = after.bodies.defs().map(|(h, b)| (h, b.clone())).collect();
    lhs.sort_by_key(|(h, _)| *h);
    rhs.sort_by_key(|(h, _)| *h);
    assert_eq!(lhs, rhs);
}

#[test]
fn moving_a_definition_between_modules_changes_no_body() {
    let together = compile(&[(
        "a",
        "pub fn helper(x: Int) -> Int = x + 1\npub fn caller(x: Int) -> Int = helper(x)\n",
    )]);
    let apart = compile(&[
        ("a", "pub fn helper(x: Int) -> Int = x + 1\n"),
        (
            "b",
            "import a\npub fn caller(x: Int) -> Int = a::helper(x)\n",
        ),
    ]);

    let mut lhs: Vec<_> = together
        .bodies
        .defs()
        .map(|(h, b)| (h, b.clone()))
        .collect();
    let mut rhs: Vec<_> = apart.bodies.defs().map(|(h, b)| (h, b.clone())).collect();
    lhs.sort_by_key(|(h, _)| *h);
    rhs.sort_by_key(|(h, _)| *h);
    assert_eq!(lhs, rhs);
}

/// The examples and every shipped module: this harness has no import graph to pull in `std.net`.
fn corpus() -> Vec<(String, String)> {
    let mut files: Vec<(String, String)> = Vec::new();
    for entry in std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples")).unwrap()
    {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "ply") {
            let stem = path.file_stem().unwrap().to_str().unwrap().to_string();
            files.push((stem, std::fs::read_to_string(&path).unwrap()));
        }
    }
    files.sort();
    assert!(!files.is_empty(), "the examples moved");
    files.extend(ply_std::sources().map(|(name, source)| (name.to_string(), source.to_string())));
    files
}

#[test]
fn the_examples_come_back_under_their_own_names() {
    let files = corpus();
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(name, text)| (name.as_str(), text.as_str()))
        .collect();
    round_trip(&borrowed);
}

#[test]
fn no_mutation_of_a_body_can_abort_the_printer() {
    let original = compile(&[("m", EVERY_ITEM_KIND)]);
    let (_, body) = original.bodies.defs().next().expect("at least one body");
    let bytes = body.clone().into_bytes();
    assert!(bytes.len() > 8, "the sample is too small to be a test");

    for at in 0..bytes.len() {
        for mask in [0x01u8, 0x80, 0xff] {
            let mut mutated = bytes.clone();
            mutated[at] ^= mask;
            let Some(stored) = StoredBody::from_bytes(mutated) else {
                continue;
            };
            let Some(key) = stored.key() else { continue };
            let _ = print_bodies(&[stored.as_bytes()], &[("m.x", key)], &[], &[], &[]);
        }
    }
}

#[test]
fn nothing_prints_as_no_module() {
    assert_eq!(
        print_bodies(&[], &[], &[], &[], &[]).expect("an empty set is not an error"),
        Vec::new()
    );
}

#[test]
fn printing_is_deterministic() {
    let original = compile(&[("m", EVERY_ITEM_KIND)]);
    let names = names_of(&original);
    assert_eq!(
        print(&original.bodies, &names, &[]).expect("printable"),
        print(&original.bodies, &names, &[]).expect("printable")
    );
}

const NAMED: [(&str, &str); 2] = [
    (
        "store.wire",
        r#"
        pub effect audit { write emit[t](what: String) -> Unit }
        pub type Note = { what: String }
        pub fn note(what: String) -> Note = { what: what }
        "#,
    ),
    (
        "app",
        r#"
        import store.wire (audit, Note, note)
        pub fn record(what: String) -> Note / {audit.write[log]} = {
          audit.emit[log](what);
          note(what)
        }
        fn main() -> String / {audit.write[log]} = record("x").what
        "#,
    ),
];

#[test]
fn a_namespace_restores_the_names_and_the_modules() {
    let (_, printed) = round_trip(&NAMED);
    let modules: Vec<&str> = printed.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(modules, ["app", "store.wire"], "units were not merged");
}

#[test]
fn modules_sharing_a_last_segment_are_imported_under_distinct_binders() {
    let (_, printed) = round_trip(&[
        (
            "left.util",
            "pub type Tally = { v: Int }\npub fn one() -> Int = 1\n",
        ),
        ("right.util", "pub fn two() -> Int = 2\n"),
        (
            "app",
            r#"
            import left.util as l
            import right.util as r
            fn main() -> Int = {
              let t: l::Tally = { v: l::one() };
              t.v + r::two()
            }
            "#,
        ),
    ]);
    let app = text_of(&printed, "app");
    assert!(app.contains("import left.util as left_util"), "{app}");
    assert!(app.contains("import right.util as right_util"), "{app}");
}

#[test]
fn every_name_of_one_body_comes_back() {
    let pair = r#"
        pub fn even(n: Int) -> Bool = if n == 0 { true } else { odd(n - 1) }
        pub fn odd(n: Int) -> Bool = if n == 0 { false } else { even(n - 1) }
    "#;
    let twins = r#"
        pub fn one() -> Int = 1
        pub fn uno() -> Int = 1
        pub fn spin(n: Int) -> Int = if n == 0 { 0 } else { twirl(n - 1) }
        pub fn twirl(n: Int) -> Int = if n == 0 { 0 } else { spin(n - 1) }
    "#;
    let a = format!("{pair}{twins}");
    let (original, _) = round_trip(&[("a", a.as_str()), ("b", pair)]);
    let hash = |name: &str| original.hashes.defs[&Symbol::new(name)];
    assert_eq!(hash("a.one"), hash("a.uno"));
    assert_eq!(hash("a.spin"), hash("a.twirl"));
    assert_eq!(hash("a.even"), hash("b.even"));
}

#[test]
fn one_body_named_twice_within_a_group_is_refused() {
    let original = compile(&[(
        "m",
        r#"
        pub fn a(n: Int) -> Int = if n == 0 { 0 } else { b(n - 1) + c(n - 1) }
        pub fn b(n: Int) -> Int = if n == 0 { 1 } else { a(n - 1) }
        pub fn c(n: Int) -> Int = if n == 0 { 1 } else { a(n - 1) }
        "#,
    )]);
    let hash = |name: &str| original.hashes.defs[&Symbol::new(name)];
    assert_eq!(hash("m.b"), hash("m.c"));
    let refused = print(&original.bodies, &names_of(&original), &[])
        .expect_err("the names cannot say which member a call reaches");
    assert_eq!(refused.code, codes::ARTIFACT_INVALID);
    assert!(
        refused.message.contains("`m.b`") && refused.message.contains("`m.c`"),
        "{}",
        refused.message
    );
}

#[test]
fn one_effect_declaration_named_twice_is_refused() {
    // An effect is nominal in its simple name and not in its module path, so it takes the same name
    // in two modules to make one declaration that two names claim.
    let original = compile(&[
        ("a", "pub effect one { read at() -> Int }"),
        ("b", "pub effect one { read at() -> Int }"),
    ]);
    let names = names_of(&original);
    let refused = print(&original.bodies, &names, &[]).expect_err("two names for one declaration");
    assert!(
        refused.message.contains("`a.one`") && refused.message.contains("`b.one`"),
        "{}",
        refused.message
    );

    let once: Vec<(Symbol, DefHash)> = names
        .into_iter()
        .filter(|(name, _)| name.as_str() == "a.one")
        .collect();
    print(&original.bodies, &once, &[]).expect("named once, the declaration is that name's");
}

#[test]
fn names_that_cannot_be_applied_are_refused() {
    let original = compile(&NAMED);
    let full = names_of(&original);
    for broken in [
        full[1..].to_vec(),
        full.iter()
            .map(|(_, hash)| (Symbol::new("m.same"), *hash))
            .collect(),
        full.iter()
            .map(|(_, hash)| (Symbol::new("bare"), *hash))
            .collect(),
    ] {
        let refused =
            print(&original.bodies, &broken, &[]).expect_err("a namespace that cannot be applied");
        assert_eq!(refused.code, codes::ARTIFACT_INVALID);
    }
}

#[test]
fn a_shipped_module_is_imported_and_not_printed() {
    let original = compile(&NAMED);
    let printed = print(&original.bodies, &names_of(&original), &["store.wire"])
        .expect("the names say everything");
    assert_eq!(printed.len(), 1);
    assert_eq!(printed[0].0, "app");
    assert!(
        printed[0].1.contains("import store.wire"),
        "{}",
        printed[0].1
    );
}

#[test]
fn two_identical_effect_declarations_are_one_hash() {
    // Identical includes the name an effect is nominal in; the module path it is declared under is
    // no part of its identity, so declaring it in two modules declares it once.
    let original = compile(&[
        ("a", "pub effect one { read at() -> Int }"),
        ("b", "pub effect one { read at() -> Int }"),
    ]);
    let here = original.hashes.decls[&Symbol::new("a.one")];
    let there = original.hashes.decls[&Symbol::new("b.one")];
    assert_eq!(
        here, there,
        "two byte-identical declarations must hash alike, or content addressing is not what it says"
    );
}

#[test]
fn two_sums_that_differ_only_in_their_names_are_two_declarations() {
    let original = compile(&[
        ("a", "pub type One = | Wrap(Int)"),
        ("b", "pub type Two = | Wrap(Int)"),
    ]);
    assert_ne!(
        original.hashes.decls[&Symbol::new("a.One")],
        original.hashes.decls[&Symbol::new("b.Two")],
        "`unify_structural` compares a named type by name, so a hash may not ignore it"
    );
}

#[test]
fn one_sum_declared_in_two_modules_is_one_declaration() {
    let original = compile(&[
        ("a", "pub type One = | Wrap(Int)"),
        ("b", "pub type One = | Wrap(Int)"),
    ]);
    assert_eq!(
        original.hashes.decls[&Symbol::new("a.One")],
        original.hashes.decls[&Symbol::new("b.One")],
        "a module path is no part of an identity, so vendoring a module must move nothing"
    );
}

/// An alias is expanded by `conv_type` before anything unifies, so the checker has no name to
/// compare and a hash that carried one would be finer than the identity it is for.
#[test]
fn an_alias_is_transparent_so_its_name_is_no_part_of_it() {
    let metres = compile(&[(
        "m",
        "pub type Metres = Int\npub fn go(x: Metres) -> Metres = x",
    )]);
    let feet = compile(&[("m", "pub type Feet = Int\npub fn go(x: Feet) -> Feet = x")]);
    assert_eq!(
        metres.hashes.decls[&Symbol::new("m.Metres")],
        feet.hashes.decls[&Symbol::new("m.Feet")]
    );
    assert_eq!(
        metres.hashes.defs[&Symbol::new("m.go")],
        feet.hashes.defs[&Symbol::new("m.go")],
        "the two definitions have one type, so they are one definition"
    );
}

#[test]
fn two_effects_that_differ_only_in_their_names_are_two_declarations() {
    let original = compile(&[
        ("a", "pub effect one { read at() -> Int }"),
        ("b", "pub effect two { read at() -> Int }"),
    ]);
    assert_ne!(
        original.hashes.decls[&Symbol::new("a.one")],
        original.hashes.decls[&Symbol::new("b.two")],
        "an effect is the declaration its name declares, not a shape another declaration matches"
    );
}

/// Slots index one component's effect enumeration, and each test is its own component.
#[test]
fn two_tests_that_number_one_effect_differently_both_come_back() {
    round_trip(&[(
        "m",
        r#"
        effect left  { read one() -> Int }
        effect right { read two() -> Bool }

        fn only_right() -> Bool / {right.read} = right.two()
        fn both() -> Int / {left.read, right.read} =
          if right.two() { left.one() } else { 0 }

        test "the shared effect is alone here" {
          handle { assert_eq(only_right(), true) } with { right.two() -> true }
        }

        test "and beside another one here" {
          handle { assert_eq(both(), 7) } with { left.one() -> 7, right.two() -> true }
        }
        "#,
    )]);
}
