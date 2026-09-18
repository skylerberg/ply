//! Definition bodies: the third element of `Hash -> (Definition, Type, Footprint)`.

use indexmap::IndexMap;
use ply_core::check_program;
use ply_hash::body::{BodySet, ItemKind, reconstruct};
use ply_hash::{DefHash, HashOutput, hash_program_with_bodies};
use ply_span::{SourceId, Symbol, codes};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::resolve::Resolved;
use ply_ty::CheckOutput;
use ply_ty::{EffectAtom, Footprint, Row, RowVar, Scheme, TyVar, Type};
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

const EVERY_OPERATOR: &str = r#"
fn arithmetic(a, b) = a + b - a * b / a % b
fn comparison(a, b) = (a == b) && (a != b) || (a < b) && (a <= b) || (a > b) && (a >= b)
fn concatenation(a, b) = a ++ b
fn bits(a, b) = a & b | a ^ b
fn shifts(a, b) = (a << b) + (a >> b) + (a >>> b)
fn prefixes(a, p) = -a + ~a + (if !p { 1 } else { 0 })
"#;

/// Deliberately does not typecheck: the byte table is under test, not the prelude.
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

/// A rebuilt program has no text, so the C emitter is handed it printed back to source.
fn on_the_tier<'a>(
    program: &'a ply_syntax::ast::Program,
    resolved: &'a ply_syntax::resolve::Resolved,
    check: &'a CheckOutput,
) -> ply_eval::Machine<'a> {
    let texts: std::collections::HashMap<String, String> =
        ply_syntax::print::program(program).into_iter().collect();
    let unit = ply_codegen::Unit::over_with_texts(program, resolved, texts)
        .expect("this host has a C compiler");
    let spec = ply_eval::BackendSpec {
        kind: ply_eval::BackendKind::C,
        ..Default::default()
    };
    let mut machine = ply_eval::Machine::new(program, resolved, check);
    machine.set_compiled(ply_eval::Provider::attach(unit, &spec));
    machine
}

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
    let mut interp = on_the_tier(&rebuilt.program, &resolved, &check);

    assert_eq!(interp.test_count(), 2);
    for index in 0..interp.test_count() {
        interp
            .eval_test(index)
            .unwrap_or_else(|d| panic!("reconstructed test {index} failed: {d}"));
    }
}

#[test]
fn a_reconstructed_program_prints_to_the_source_it_hashes_as() {
    let files = corpus();
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(name, text)| (name.as_str(), text.as_str()))
        .collect();
    let original = compile(&borrowed);
    let mut rebuilt = reconstruct(&original.bodies).expect("bodies should reconstruct");
    let resolved = ply_syntax::resolve(&mut rebuilt.program).expect("it should resolve");
    let (before, _) =
        hash_program_with_bodies(&rebuilt.program, &resolved).expect("it should hash");

    let texts = ply_syntax::print::program(&rebuilt.program);
    let printed: Vec<(&str, &str)> = texts
        .iter()
        .map(|(name, text)| (name.as_str(), text.as_str()))
        .collect();
    let after = compile(&printed).hashes;

    let hex = |h: &IndexMap<Symbol, DefHash>| -> BTreeMap<String, String> {
        h.iter().map(|(n, h)| (n.to_string(), h.to_hex())).collect()
    };
    assert_eq!(
        hex(&after.defs),
        hex(&before.defs),
        "a definition hashes differently once printed"
    );
    assert_eq!(
        after.tests, before.tests,
        "a test hashes differently once printed"
    );
    assert_eq!(
        after.laws, before.laws,
        "a law hashes differently once printed"
    );
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
fn the_examples_reconstruct() {
    let files = corpus();
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(name, text)| (name.as_str(), text.as_str()))
        .collect();
    assert_interfaces_survive(&borrowed);
}

/// Each mutation is re-filed under its own key, so the decoder rather than the self-check sees it.
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
            // Succeeding is allowed: some mutations are still a definition.
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

fn names_of(checked: &Checked) -> Vec<(Symbol, DefHash)> {
    checked
        .hashes
        .defs
        .iter()
        .chain(checked.hashes.decls.iter())
        .map(|(name, hash)| (name.clone(), *hash))
        .collect()
}

fn exact_round_trip(original: &Checked) -> Vec<(String, String)> {
    let names = names_of(original);
    let program = ply_hash::body::reconstruct_exact(&original.bodies, &names, |_| false)
        .unwrap_or_else(|diags| panic!("the names say everything: {diags:#?}"));
    let printed = ply_syntax::print::program(&program);
    let borrowed: Vec<(&str, &str)> = printed
        .iter()
        .map(|(name, text)| (name.as_str(), text.as_str()))
        .collect();
    let again = compile(&borrowed).hashes;
    for (name, hash) in &names {
        let now = again
            .defs
            .get(name)
            .or_else(|| again.decls.get(name))
            .unwrap_or_else(|| panic!("`{name}` did not come back"));
        assert_eq!(now, hash, "`{name}` came back as a different definition");
    }
    let wanted: BTreeSet<&Symbol> = names.iter().map(|(name, _)| name).collect();
    let rebuilt: BTreeSet<&Symbol> = again.defs.keys().chain(again.decls.keys()).collect();
    assert_eq!(rebuilt, wanted, "a name was invented or dropped");
    printed
}

#[test]
fn a_namespace_restores_the_names_and_the_modules() {
    let original = compile(&NAMED.map(|(n, s)| (n, s)));
    let printed = exact_round_trip(&original);
    let modules: Vec<&str> = printed.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(modules, ["app", "store.wire"], "units were not merged");
}

#[test]
fn modules_sharing_a_last_segment_are_imported_under_distinct_binders() {
    let original = compile(&[
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
    let printed = exact_round_trip(&original);
    let app = &printed
        .iter()
        .find(|(name, _)| name == "app")
        .expect("the app comes back")
        .1;
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
    let original = compile(&[("a", a.as_str()), ("b", pair)]);
    let hash = |name: &str| original.hashes.defs[&Symbol::new(name)];
    assert_eq!(hash("a.one"), hash("a.uno"));
    assert_eq!(hash("a.spin"), hash("a.twirl"));
    assert_eq!(hash("a.even"), hash("b.even"));
    exact_round_trip(&original);
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
    let refused =
        ply_hash::body::reconstruct_exact(&original.bodies, &names_of(&original), |_| false)
            .expect_err("the names cannot say which member a call reaches");
    assert_eq!(refused[0].code, codes::ARTIFACT_INVALID);
    assert!(
        refused[0].message.contains("`m.b`") && refused[0].message.contains("`m.c`"),
        "{}",
        refused[0].message
    );
}

#[test]
fn one_effect_declaration_named_twice_is_refused() {
    let original = compile(&[
        ("a", "pub effect one { read at() -> Int }"),
        ("b", "pub effect two { read at() -> Int }"),
    ]);
    let names = names_of(&original);
    let refused = ply_hash::body::reconstruct_exact(&original.bodies, &names, |_| false)
        .expect_err("two names for one declaration");
    assert!(
        refused[0].message.contains("`a.one`") && refused[0].message.contains("`b.two`"),
        "{}",
        refused[0].message
    );

    let once: Vec<(Symbol, DefHash)> = names
        .into_iter()
        .filter(|(name, _)| name.as_str() == "a.one")
        .collect();
    ply_hash::body::reconstruct_exact(&original.bodies, &once, |_| false)
        .expect("named once, the declaration is that name's");
}

#[test]
fn names_that_cannot_be_applied_are_refused() {
    let original = compile(&NAMED.map(|(n, s)| (n, s)));
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
        let refused = ply_hash::body::reconstruct_exact(&original.bodies, &broken, |_| false)
            .expect_err("a namespace that cannot be applied");
        assert_eq!(refused[0].code, codes::ARTIFACT_INVALID);
    }
}

#[test]
fn a_module_named_only_is_imported_and_not_rebuilt() {
    let original = compile(&NAMED.map(|(n, s)| (n, s)));
    let program =
        ply_hash::body::reconstruct_exact(&original.bodies, &names_of(&original), |module| {
            module.as_str() == "store.wire"
        })
        .expect("the names say everything");
    let printed = ply_syntax::print::program(&program);
    assert_eq!(printed.len(), 1);
    assert_eq!(printed[0].0, "app");
    assert!(
        printed[0].1.contains("import store.wire"),
        "{}",
        printed[0].1
    );
}

#[test]
fn the_examples_come_back_under_their_own_names() {
    let files = corpus();
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(name, text)| (name.as_str(), text.as_str()))
        .collect();
    exact_round_trip(&compile(&borrowed));
}

/// Bisection reconstructs without a namespace: a historical set must rebuild without today's names.
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

/// Slots index one component's effect enumeration, and each test is its own component.
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
    let mut interp = on_the_tier(&rebuilt.program, &resolved, &check);
    assert_eq!(interp.test_count(), 2);
    for index in 0..interp.test_count() {
        interp
            .eval_test(index)
            .unwrap_or_else(|d| panic!("reconstructed test {index} failed: {d}"));
    }
}
