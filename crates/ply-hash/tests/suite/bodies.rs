//! Definition bodies: the third element of `Hash -> (Definition, Type, Footprint)`.

use indexmap::IndexMap;
use ply_core::ty::{EffectAtom, Footprint, Row, RowVar, Scheme, TyVar, Type};
use ply_core::{CheckOutput, check_program};
use ply_hash::body::{BodySet, ItemKind, reconstruct};
use ply_hash::{DefHash, HashOutput, hash_program_with_bodies};
use ply_span::{SourceId, Symbol};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::resolve::Resolved;
use std::collections::{BTreeMap, BTreeSet};

struct Checked {
    hashes: HashOutput,
    check: CheckOutput,
    bodies: BodySet,
}

fn parse(files: &[(&str, &str)]) -> (Program, Resolved) {
    let inputs = files
        .iter()
        .enumerate()
        .map(|(i, (name, source))| (SourceId(i as u32), ModuleName::from_dotted(name), *source));
    let mut program = match ply_syntax::parse_program(inputs) {
        Ok(program) => program,
        Err(diags) => panic!("program did not parse: {diags:#?}"),
    };
    let diags = ply_derive::expand_program(&mut program);
    if !diags.is_empty() {
        panic!("program did not expand: {diags:#?}");
    }
    let resolved = match ply_syntax::resolve(&mut program) {
        Ok(resolved) => resolved,
        Err(diags) => panic!("program did not resolve: {diags:#?}"),
    };
    (program, resolved)
}

fn compile(files: &[(&str, &str)]) -> Checked {
    let (program, resolved) = parse(files);
    let check = match check_program(&program, &resolved) {
        Ok(check) => check,
        Err(diags) => panic!("program did not typecheck: {diags:#?}"),
    };
    let (hashes, bodies) =
        hash_program_with_bodies(&program, &resolved).expect("program should hash");
    Checked {
        hashes,
        check,
        bodies,
    }
}

/// Reconstructs, then checks and re-hashes what came back.
fn rebuild(original: &Checked) -> (Checked, IndexMap<DefHash, Symbol>) {
    let mut rebuilt = reconstruct(&original.bodies).expect("bodies should reconstruct");
    let resolved = match ply_syntax::resolve(&mut rebuilt.program) {
        Ok(resolved) => resolved,
        Err(diags) => panic!("reconstructed program did not resolve: {diags:#?}"),
    };
    let check = match check_program(&rebuilt.program, &resolved) {
        Ok(check) => check,
        Err(diags) => panic!("reconstructed program did not typecheck: {diags:#?}"),
    };
    let (hashes, bodies) =
        hash_program_with_bodies(&rebuilt.program, &resolved).expect("rebuilt program should hash");

    for (hash, name) in &rebuilt.names {
        let again = hashes
            .defs
            .get(name)
            .or_else(|| hashes.decls.get(name))
            .unwrap_or_else(|| panic!("`{name}` is missing from the rebuilt program"));
        assert_eq!(
            again, hash,
            "`{name}` was rebuilt from {hash} and hashes to {again}"
        );
    }
    assert_eq!(
        hashes.tests, original.hashes.tests,
        "rebuilt tests hash differently"
    );

    (
        Checked {
            hashes,
            check,
            bodies,
        },
        rebuilt.names,
    )
}

/// Original program-wide name -> the name the reconstruction invented for it.
fn translation(original: &Checked, names: &IndexMap<DefHash, Symbol>) -> BTreeMap<Symbol, Symbol> {
    original
        .hashes
        .defs
        .iter()
        .chain(original.hashes.decls.iter())
        .filter_map(|(name, hash)| names.get(hash).map(|to| (name.clone(), to.clone())))
        .collect()
}

fn rename_type(ty: &Type, map: &BTreeMap<Symbol, Symbol>) -> Type {
    match ty {
        Type::Var(v) => Type::Var(*v),
        Type::Con(name, args) => Type::Con(
            map.get(name).cloned().unwrap_or_else(|| name.clone()),
            args.iter().map(|a| rename_type(a, map)).collect(),
        ),
        Type::Fn {
            params,
            ret,
            effects,
        } => Type::Fn {
            params: params.iter().map(|p| rename_type(p, map)).collect(),
            ret: Box::new(rename_type(ret, map)),
            effects: Row {
                atoms: effects.atoms.iter().map(|a| rename_atom(a, map)).collect(),
                tail: effects.tail,
            },
        },
        Type::Record(fields) => Type::Record(
            fields
                .iter()
                .map(|(k, v)| (k.clone(), rename_type(v, map)))
                .collect(),
        ),
    }
}

fn rename_atom(atom: &EffectAtom, map: &BTreeMap<Symbol, Symbol>) -> EffectAtom {
    EffectAtom {
        effect: map
            .get(&atom.effect)
            .cloned()
            .unwrap_or_else(|| atom.effect.clone()),
        resource: atom.resource.clone(),
        mode: atom.mode,
    }
}

fn rename_footprint(f: &Footprint, map: &BTreeMap<Symbol, Symbol>) -> Footprint {
    Footprint(f.0.iter().map(|a| rename_atom(a, map)).collect())
}

/// Quantified variables renumbered from zero in traversal order.
fn canonical(scheme: &Scheme) -> Scheme {
    let mut tys: BTreeMap<TyVar, TyVar> = BTreeMap::new();
    let mut rows: BTreeMap<RowVar, RowVar> = BTreeMap::new();
    let ty = renumber(&scheme.ty, &mut tys, &mut rows);
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
        ty,
    }
}

fn renumber(
    ty: &Type,
    tys: &mut BTreeMap<TyVar, TyVar>,
    rows: &mut BTreeMap<RowVar, RowVar>,
) -> Type {
    match ty {
        Type::Var(v) => {
            let next = TyVar(tys.len() as u32);
            Type::Var(*tys.entry(*v).or_insert(next))
        }
        Type::Con(name, args) => Type::Con(
            name.clone(),
            args.iter().map(|a| renumber(a, tys, rows)).collect(),
        ),
        Type::Fn {
            params,
            ret,
            effects,
        } => {
            let params = params.iter().map(|p| renumber(p, tys, rows)).collect();
            let ret = Box::new(renumber(ret, tys, rows));
            let tail = effects.tail.map(|t| {
                let next = RowVar(rows.len() as u32);
                *rows.entry(t).or_insert(next)
            });
            Type::Fn {
                params,
                ret,
                effects: Row {
                    atoms: effects.atoms.clone(),
                    tail,
                },
            }
        }
        Type::Record(fields) => Type::Record(
            fields
                .iter()
                .map(|(k, v)| (k.clone(), renumber(v, tys, rows)))
                .collect(),
        ),
    }
}

/// Every definition must come back with the interface it went in with, modulo the names the
/// reconstruction invented.
fn assert_interfaces_survive(files: &[(&str, &str)]) -> Checked {
    let original = compile(files);
    let (rebuilt, names) = rebuild(&original);
    let map = translation(&original, &names);
    assert!(
        !map.is_empty(),
        "nothing was reconstructed, so nothing was proved"
    );

    for (name, info) in &original.check.defs {
        let Some(to) = map.get(name) else { continue };
        let after = rebuilt
            .check
            .defs
            .get(to)
            .unwrap_or_else(|| panic!("`{name}` came back as `{to}`, which did not check"));
        assert_eq!(
            canonical(&Scheme {
                ty_vars: info.scheme.ty_vars.clone(),
                row_vars: info.scheme.row_vars.clone(),
                ty: rename_type(&info.scheme.ty, &map),
            }),
            canonical(&after.scheme),
            "`{name}` came back with a different type"
        );
        assert_eq!(
            rename_footprint(&info.footprint, &map),
            after.footprint,
            "`{name}` came back with a different footprint"
        );
    }

    for (index, test) in original.check.tests.iter().enumerate() {
        let after = &rebuilt.check.tests[index];
        assert_eq!(
            rename_footprint(&test.footprint, &map),
            after.footprint,
            "test `{}` came back with a different footprint",
            test.key
        );
        assert_eq!(test.nondet, after.nondet);
    }
    original
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

/// Every operator the language has, so the byte table and its inverse are exercised over all of
/// it rather than the handful an example uses.
const EVERY_OPERATOR: &str = r#"
fn arithmetic(a, b) = a + b - a * b / a % b
fn comparison(a, b) = (a == b) && (a != b) || (a < b) && (a <= b) || (a > b) && (a >= b)
fn concatenation(a, b) = a ++ b
fn bits(a, b) = a & b | a ^ b
fn shifts(a, b) = (a << b) + (a >> b) + (a >>> b)
fn prefixes(a, p) = -a + ~a + (if !p { 1 } else { 0 })
"#;

/// The comment above `body::binop_of` called its table "pinned by a round-trip
/// test over every operator" and no such test existed; ADR 0033 added six
/// operators to `binop_byte` and one to `unop_byte`, and the inverse is a
/// second hand-written list, so an omission there is not a compile error — it
/// is `W0602` at decode time, on a body that hashed and stored perfectly well.
/// This is that test. It deliberately does not typecheck: what is under test is
/// the byte table, not the prelude.
#[test]
fn every_operator_survives_the_byte_table_and_its_inverse() {
    let (program, resolved) = parse(&[("m", EVERY_OPERATOR)]);
    let (before, bodies) =
        hash_program_with_bodies(&program, &resolved).expect("program should hash");
    assert_eq!(before.defs.len(), 6, "the sample lost a definition");

    let mut rebuilt = reconstruct(&bodies).expect("bodies should reconstruct");
    let resolved = ply_syntax::resolve(&mut rebuilt.program).expect("it should resolve");
    let (after, _) =
        hash_program_with_bodies(&rebuilt.program, &resolved).expect("it should hash again");

    let keys = |out: &HashOutput| {
        let mut v: Vec<DefHash> = out.defs.values().copied().collect();
        v.sort();
        v
    };
    assert_eq!(
        keys(&before),
        keys(&after),
        "a body carrying an operator did not survive the round trip"
    );
}

#[test]
fn every_item_kind_round_trips() {
    let original = assert_interfaces_survive(&[("m", EVERY_ITEM_KIND)]);
    let rebuilt = reconstruct(&original.bodies).expect("bodies should reconstruct");

    let kinds: Vec<ItemKind> = [
        "m.db",
        "m.clock",
        "m.Colour",
        "m.Pair",
        "m.Alias",
        "m.identity",
    ]
    .iter()
    .map(|name| {
        let hash = original
            .hashes
            .defs
            .get(&Symbol::new(*name))
            .or_else(|| original.hashes.decls.get(&Symbol::new(*name)))
            .unwrap_or_else(|| panic!("`{name}` was not hashed"));
        rebuilt.kind_of(*hash).expect("a kind for every definition")
    })
    .collect();
    assert_eq!(
        kinds,
        vec![
            ItemKind::Effect,
            ItemKind::Effect,
            ItemKind::Type,
            ItemKind::Type,
            ItemKind::Type,
            ItemKind::Fn,
        ]
    );
    assert_eq!(rebuilt.test_keys.len(), 3);
}

#[test]
fn cross_module_references_resolve_after_reconstruction() {
    assert_interfaces_survive(&[
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

            test "a cross-module call is reconstructable" {
              handle {
                assert_eq(value(1), 7)
              } with { store::db.get[users](k) -> 7 }
            }
            "#,
        ),
    ]);
}

/// Self-recursion is one member in its own component, so its intra-component reference names the
/// only class there is and comes back unambiguously.
#[test]
fn self_recursion_round_trips() {
    let original = assert_interfaces_survive(&[(
        "m",
        r#"
        fn countdown(n: Int) -> Int = if n == 0 { 0 } else { countdown(n - 1) }

        test "self recursion" { assert_eq(countdown(4), 0) }
        "#,
    )]);

    let hash = original.hashes.defs[&Symbol::new("m.countdown")];
    let body = original.bodies.get(hash).expect("a body for countdown");
    assert!(body.verify(hash));

    let rebuilt = reconstruct(&original.bodies).expect("bodies should reconstruct");
    let module = rebuilt
        .program
        .modules
        .iter()
        .find(|m| m.items.len() == 1)
        .expect("one module for the component");
    assert!(module.imports.is_empty());
}

/// A mutually recursive component's bytes label each intra-component reference with the class
/// refinement assigned, and refinement runs to a *labelled* fixed point — so the label a reference
/// mentions is the label its referent is filed under, and which member calls which is recoverable.
#[test]
fn a_mutually_recursive_component_round_trips_wired_the_way_it_was_written() {
    let original = compile(&[(
        "m",
        r#"
        fn is_even(n: Int) -> Bool = if n == 0 { true } else { is_odd(n - 1) }
        fn is_odd(n: Int) -> Bool = if n == 0 { false } else { is_even(n - 1) }
        "#,
    )]);

    let even = original.hashes.defs[&Symbol::new("m.is_even")];
    let odd = original.hashes.defs[&Symbol::new("m.is_odd")];
    assert_ne!(even, odd, "the two members are not interchangeable");

    let a = original.bodies.get(even).expect("a body for is_even");
    let b = original.bodies.get(odd).expect("a body for is_odd");
    assert_ne!(a, b, "one payload, two class indices");
    assert!(a.verify(even) && b.verify(odd));

    let (rebuilt, names) = rebuild(&original);
    assert_eq!(rebuilt.hashes.defs.len(), 2);
    assert_eq!(names.len(), 2);
}

/// Two three-cycles that differ only in the direction they are wired.
#[test]
fn two_cycles_wired_in_opposite_directions_do_not_collide() {
    let clockwise = compile(&[(
        "m",
        r#"
        fn f(n: Int) -> Int = g(n - 1) + 1
        fn g(n: Int) -> Int = h(n - 1) + 2
        fn h(n: Int) -> Int = f(n - 1) + 3
        "#,
    )]);
    let widdershins = compile(&[(
        "m",
        r#"
        fn f(n: Int) -> Int = h(n - 1) + 1
        fn h(n: Int) -> Int = g(n - 1) + 3
        fn g(n: Int) -> Int = f(n - 1) + 2
        "#,
    )]);

    let one: BTreeSet<DefHash> = clockwise.hashes.defs.values().copied().collect();
    let other: BTreeSet<DefHash> = widdershins.hashes.defs.values().copied().collect();
    assert_eq!(one.len(), 3, "three distinguishable members");
    assert!(
        one.is_disjoint(&other),
        "the two wirings are different computations and must not share a hash"
    );

    rebuild(&clockwise);
    rebuild(&widdershins);
}

#[test]
fn a_body_verifies_only_against_its_own_key() {
    let original = compile(&[(
        "m",
        "fn f(x: Int) -> Int = x + 1\nfn g(x: Int) -> Int = x + 2\n",
    )]);
    let f = original.hashes.defs[&Symbol::new("m.f")];
    let g = original.hashes.defs[&Symbol::new("m.g")];

    let body = original.bodies.get(f).expect("a body for f");
    assert_eq!(body.key(), Some(f));
    assert!(body.verify(f));
    assert!(!body.verify(g));
}

#[test]
fn a_truncated_body_is_refused_rather_than_decoded() {
    let original = compile(&[("m", "fn f(x: Int) -> Int = x + 1\n")]);
    let hash = original.hashes.defs[&Symbol::new("m.f")];
    let mut bytes = original.bodies.get(hash).unwrap().as_bytes().to_vec();
    bytes.truncate(bytes.len() - 1);

    let mut set = BodySet::default();
    set.insert(
        hash,
        ply_hash::body::StoredBody::from_bytes(bytes).expect("still an envelope"),
    );
    let diags = reconstruct(&set).expect_err("a truncated body must not decode");
    assert!(
        diags
            .iter()
            .any(|d| d.code == ply_span::codes::CACHE_CORRUPT)
    );
}

#[test]
fn a_body_filed_under_the_wrong_key_is_refused() {
    let original = compile(&[(
        "m",
        "fn f(x: Int) -> Int = x + 1\nfn g(x: Int) -> Int = x + 2\n",
    )]);
    let f = original.hashes.defs[&Symbol::new("m.f")];
    let g = original.hashes.defs[&Symbol::new("m.g")];

    let mut set = BodySet::default();
    set.insert(g, original.bodies.get(f).unwrap().clone());
    let diags = reconstruct(&set).expect_err("a misfiled body must not decode");
    assert!(
        diags
            .iter()
            .any(|d| d.code == ply_span::codes::CACHE_CORRUPT)
    );
}

#[test]
fn a_reference_with_no_body_is_named_rather_than_guessed() {
    let original = compile(&[(
        "m",
        "fn helper(x: Int) -> Int = x + 1\nfn caller(x: Int) -> Int = helper(x)\n",
    )]);
    let caller = original.hashes.defs[&Symbol::new("m.caller")];

    let mut set = BodySet::default();
    set.insert(caller, original.bodies.get(caller).unwrap().clone());
    let diags = reconstruct(&set).expect_err("an open set must not reconstruct");
    assert!(
        diags
            .iter()
            .any(|d| d.code == ply_span::codes::CACHE_UNREADABLE)
    );
}

/// Renaming is free for a hash, so it must be free for a body: the bytes a definition is stored
/// under cannot move when its name does.
#[test]
fn renaming_changes_no_body() {
    let before = compile(&[("m", "fn f(x: Int) -> Int = x + 1\nfn g() -> Int = f(1)\n")]);
    let after = compile(&[(
        "m",
        "fn renamed(x: Int) -> Int = x + 1\nfn g() -> Int = renamed(1)\n",
    )]);

    let mut lhs: Vec<_> = before.bodies.defs().map(|(h, b)| (h, b.clone())).collect();
    let mut rhs: Vec<_> = after.bodies.defs().map(|(h, b)| (h, b.clone())).collect();
    lhs.sort_by_key(|(h, _)| *h);
    rhs.sort_by_key(|(h, _)| *h);
    assert_eq!(lhs, rhs);
}

/// Moving a definition between modules changes no hash, so it must change no body either — this is
/// the property a reconstruction of a historical set depends on, because the modules moved and the
/// hashes did not.
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

/// Checking is not the bar — M5 has to *evaluate* a historical definition set — so the
/// reconstructed tests are run, in the reconstructed program, against the reconstructed
/// definitions.
#[test]
fn reconstructed_tests_evaluate() {
    let original = compile(&[(
        "m",
        r#"
        effect db { read get[r](key: Int) -> Int }

        type Colour = | Red | Blue(Int)

        fn shade(c: Colour) -> Int = match c { Red -> 0, Blue(n) -> n }

        fn lookup(key: Int) -> Int / {db.read[users]} = db.get[users](key) + shade(Blue(1))

        test "a handler discharges the effect" {
          handle {
            assert_eq(lookup(3), 8)
          } with { db.get[users](k) -> 7 }
        }

        test "a pure definition still runs" { assert_eq(shade(Red), 0) }
        "#,
    )]);

    let mut rebuilt = reconstruct(&original.bodies).expect("bodies should reconstruct");
    let resolved = ply_syntax::resolve(&mut rebuilt.program).expect("it should resolve");
    let check = check_program(&rebuilt.program, &resolved).expect("it should check");
    let mut interp = ply_eval::Machine::new(&rebuilt.program, &resolved, &check);

    assert_eq!(interp.test_count(), 2);
    for index in 0..interp.test_count() {
        interp
            .eval_test(index)
            .unwrap_or_else(|d| panic!("reconstructed test {index} failed: {d}"));
    }
}

/// The corpus a person actually edits, rather than a snippet written to pass.
#[test]
fn the_examples_reconstruct() {
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

    let mut borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(name, text)| (name.as_str(), text.as_str()))
        .collect();
    // The corpus imports `std.net`, which `ply` pulls in on demand; this harness has no import
    // graph to walk, so it loads the shipped set.
    for (name, source) in ply_std::sources() {
        borrowed.push((name, source));
    }
    assert_interfaces_survive(&borrowed);
}

/// Every mutation is re-filed under *its own* key, so the self-check passes and the decoder is
/// handed real garbage rather than being let off at the door.
#[test]
fn no_mutation_of_a_body_can_abort_the_decoder() {
    let original = compile(&[("m", EVERY_ITEM_KIND)]);
    let (hash, body) = original.bodies.defs().next().expect("at least one body");
    let bytes = body.clone().into_bytes();
    assert!(bytes.len() > 8, "the sample is too small to be a test");

    for at in 0..bytes.len() {
        for mask in [0x01u8, 0x80, 0xff] {
            let mut mutated = bytes.clone();
            mutated[at] ^= mask;
            let Some(stored) = ply_hash::body::StoredBody::from_bytes(mutated) else {
                continue;
            };
            let Some(key) = stored.key() else { continue };
            let mut set = BodySet::default();
            set.insert(key, stored);
            // Succeeding is allowed — some mutations are still a definition — and only returning
            // without aborting is being asserted.
            let _ = reconstruct(&set);
        }
    }
    assert!(original.bodies.contains(hash));
}

#[test]
fn nothing_reconstructs_into_an_empty_program() {
    let rebuilt = reconstruct(&BodySet::default()).expect("an empty set is not an error");
    assert!(rebuilt.program.modules.is_empty());
    assert!(rebuilt.names.is_empty());
    assert!(rebuilt.test_keys.is_empty());
}

/// The reconstruction is an artifact something else will diff, so it may not vary run to run.
#[test]
fn reconstruction_is_deterministic() {
    let original = compile(&[("m", EVERY_ITEM_KIND)]);
    let first = reconstruct(&original.bodies).expect("bodies should reconstruct");
    let second = reconstruct(&original.bodies).expect("bodies should reconstruct");
    assert_eq!(
        format!(
            "{:?}",
            first
                .program
                .modules
                .iter()
                .map(|m| (&m.name, m.items.len()))
                .collect::<Vec<_>>()
        ),
        format!(
            "{:?}",
            second
                .program
                .modules
                .iter()
                .map(|m| (&m.name, m.items.len()))
                .collect::<Vec<_>>()
        )
    );
    assert_eq!(first.names, second.names);
}

// --- reconstructing under the names a definition was written with ------------

/// Everything the `Namespace` is for, in one program: two definitions in one module (so units have
/// to be *merged* rather than each given a module of its own), a cross-module reference (so a
/// dotted import path has to be rebuilt), and an effect (whose program-wide name is the thing a
/// host handler is registered against, and the whole reason this exists).
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

fn namespace(checked: &Checked) -> BTreeMap<DefHash, Symbol> {
    checked
        .hashes
        .defs
        .iter()
        .chain(checked.hashes.decls.iter())
        .map(|(name, hash)| (*hash, name.clone()))
        .collect()
}

#[test]
fn a_namespace_restores_the_names_and_the_modules() {
    let original = compile(&NAMED.map(|(n, s)| (n, s)));
    let names = namespace(&original);
    let mut rebuilt = ply_hash::body::reconstruct_named(&original.bodies, &names)
        .expect("bodies should reconstruct");

    for (hash, given) in &rebuilt.names {
        assert_eq!(
            Some(given),
            names.get(hash),
            "`{hash}` came back under a name the namespace did not give it"
        );
    }
    // Module *order* is the units' — hash order, so a reconstruction is byte-identical run to run —
    // but the set is the program's, which is the claim: five definitions came back as two modules
    // rather than five.
    let mut modules: Vec<String> = rebuilt
        .program
        .modules
        .iter()
        .map(|m| m.name.to_string())
        .collect();
    modules.sort();
    assert_eq!(modules, ["app", "store.wire"], "units were not merged");

    // And it is a program, not just a naming: it resolves, it typechecks, and every definition
    // hashes back to the key its body was filed under.
    let resolved = ply_syntax::resolve(&mut rebuilt.program).expect("it should resolve");
    let check = check_program(&rebuilt.program, &resolved).expect("it should typecheck");
    assert!(check.defs.contains_key(&Symbol::new("app.main")));
    assert!(
        check
            .effects
            .values()
            .any(|e| e.name.as_str() == "store.wire.audit")
    );

    let (again, _) = hash_program_with_bodies(&rebuilt.program, &resolved).expect("it should hash");
    for (name, hash) in original
        .hashes
        .defs
        .iter()
        .chain(original.hashes.decls.iter())
    {
        let now = again
            .defs
            .get(name)
            .or_else(|| again.decls.get(name))
            .unwrap_or_else(|| panic!("`{name}` is missing from the rebuilt program"));
        assert_eq!(
            now, hash,
            "`{name}` was rebuilt into a different definition"
        );
    }
}

/// A namespace that cannot be applied consistently is not applied at all.
#[test]
fn a_partial_or_colliding_namespace_falls_back_to_synthesized_names() {
    let original = compile(&NAMED.map(|(n, s)| (n, s)));
    let full = namespace(&original);

    for broken in [
        // One definition missing.
        {
            let mut names = full.clone();
            let victim = *names.keys().next().unwrap();
            names.remove(&victim);
            names
        },
        // Two definitions claiming one name.
        full.keys().map(|h| (*h, Symbol::new("m.same"))).collect(),
        // A name with no module to put it in.
        full.keys().map(|h| (*h, Symbol::new("bare"))).collect(),
    ] {
        let mut rebuilt = ply_hash::body::reconstruct_named(&original.bodies, &broken)
            .expect("a namespace that cannot be used is not a broken artifact");
        assert!(
            rebuilt
                .names
                .values()
                .all(|n| n.as_str().starts_with('m') && n.as_str().contains(".d")),
            "a mixture was produced: {:?}",
            rebuilt.names.values().take(4).collect::<Vec<_>>()
        );
        ply_syntax::resolve(&mut rebuilt.program).expect("the fallback still resolves");
    }
}

/// Bisection's route is untouched: it deliberately reconstructs without a namespace, because a
/// historical definition set has to be rebuildable without knowing what anything is called now.
#[test]
fn reconstruct_without_a_namespace_is_unchanged() {
    let original = compile(&NAMED.map(|(n, s)| (n, s)));
    let bare = reconstruct(&original.bodies).expect("bodies should reconstruct");
    assert!(
        bare.names.values().all(|n| n.as_str().contains(".d")),
        "{:?}",
        bare.names.values().take(4).collect::<Vec<_>>()
    );
    assert_eq!(bare.program.modules.len(), original.bodies.len());
}

/// Two effect declarations that normalize identically are one hash, so a reconstruction cannot tell
/// them apart — and the encoding only records that a definition *did* tell them apart when one
/// component reached both.
#[test]
fn two_identical_effect_declarations_are_one_hash() {
    let original = compile(&[
        ("a", "pub effect one { read at() -> Int }"),
        ("b", "pub effect two { read at() -> Int }"),
    ]);
    let one = original.hashes.decls[&Symbol::new("a.one")];
    let two = original.hashes.decls[&Symbol::new("b.two")];
    assert_eq!(
        one, two,
        "two byte-identical declarations must hash alike, or content addressing is not what it says"
    );
}

/// A slot is a de Bruijn level into **one component's** effect enumeration, and each test is its
/// own component.
#[test]
fn two_tests_that_number_one_effect_differently_both_reconstruct() {
    let original = compile(&[(
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

    let mut rebuilt = reconstruct(&original.bodies).expect("bodies should reconstruct");
    let resolved = ply_syntax::resolve(&mut rebuilt.program).expect("it should resolve");
    let check = check_program(&rebuilt.program, &resolved).expect("it should typecheck");
    let mut interp = ply_eval::Machine::new(&rebuilt.program, &resolved, &check);
    assert_eq!(interp.test_count(), 2);
    for index in 0..interp.test_count() {
        interp
            .eval_test(index)
            .unwrap_or_else(|d| panic!("reconstructed test {index} failed: {d}"));
    }
}
