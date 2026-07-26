//! Definition bodies: the third element of `Hash -> (Definition, Type,
//! Footprint)`.
//!
//! Two properties carry everything here. A body must be a *function of its
//! hash*, so that storing one under a key can never be wrong; and decoding one
//! must yield a definition that checks and evaluates to what the original did,
//! so that a historical definition set can be rebuilt without knowing what
//! anything is called now. The second is tested the only way it can be honestly
//! tested: reconstruct, re-check, and re-hash — a reconstruction that hashes back
//! to the keys it was rebuilt from is one nothing downstream can tell from the
//! original.

use indexmap::IndexMap;
use ply_core::ty::{EffectAtom, Footprint, Row, RowVar, Scheme, TyVar, Type};
use ply_core::{CheckOutput, check_program};
use ply_hash::body::{BodySet, ItemKind, reconstruct};
use ply_hash::{DefHash, HashOutput, hash_program_with_bodies};
use ply_span::{SourceId, Symbol};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::resolve::Resolved;
use std::collections::BTreeMap;

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
    let program = match ply_syntax::parse_program(inputs) {
        Ok(program) => program,
        Err(diags) => panic!("program did not parse: {diags:#?}"),
    };
    let resolved = match ply_syntax::resolve(&program) {
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

/// Reconstructs, then checks and re-hashes what came back. Every definition must
/// hash to the key its body was filed under — the round trip, stated as the only
/// thing that actually matters.
fn rebuild(original: &Checked) -> (Checked, IndexMap<DefHash, Symbol>) {
    let rebuilt = reconstruct(&original.bodies).expect("bodies should reconstruct");
    let resolved = match ply_syntax::resolve(&rebuilt.program) {
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
/// Built through hashes, because that is the only thing the two programs share.
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

/// Quantified variables renumbered from zero in traversal order. Inference emits
/// whatever its global counter reached, and two programs never reach the same
/// numbers, so schemes are only comparable canonically.
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

/// Every definition must come back with the interface it went in with, modulo
/// the names the reconstruction invented. Anything weaker would let a body that
/// decodes into a *different* program pass.
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
    assert_eq!(rebuilt.test_keys.len(), 2);
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

/// Self-recursion is one member in its own component, so its intra-component
/// reference names the only class there is and comes back unambiguously.
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

/// A mutually recursive component's bytes label each intra-component reference
/// with the class the *previous* refinement round assigned, and a round that put
/// every member in one class labels every reference `0`. Which member calls
/// which is therefore not in the bytes, and cannot be, so reconstruction refuses
/// rather than wiring the cycle to whichever member happens to sort first.
///
/// The bodies are still stored, still verify against their keys, and still make
/// every definition that *references* the cycle reconstructable up to the point
/// the cycle is reached.
#[test]
fn a_mutually_recursive_component_is_refused_rather_than_miswired() {
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

    let diags =
        reconstruct(&original.bodies).expect_err("a miswired cycle must not be handed back");
    assert!(
        diags
            .iter()
            .any(|d| d.code == ply_span::codes::CACHE_CORRUPT)
    );
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

/// Renaming is free for a hash, so it must be free for a body: the bytes a
/// definition is stored under cannot move when its name does.
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

/// Moving a definition between modules changes no hash, so it must change no
/// body either — this is the property a reconstruction of a historical set
/// depends on, because the modules moved and the hashes did not.
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

/// Checking is not the bar — M5 has to *evaluate* a historical definition set —
/// so the reconstructed tests are run, in the reconstructed program, against the
/// reconstructed definitions.
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

    let rebuilt = reconstruct(&original.bodies).expect("bodies should reconstruct");
    let resolved = ply_syntax::resolve(&rebuilt.program).expect("it should resolve");
    let check = check_program(&rebuilt.program, &resolved).expect("it should check");
    let mut interp = ply_eval::Interp::new(&rebuilt.program, &resolved, &check);

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

    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(name, text)| (name.as_str(), text.as_str()))
        .collect();
    assert_interfaces_survive(&borrowed);
}

/// Every mutation is re-filed under *its own* key, so the self-check passes and
/// the decoder is handed real garbage rather than being let off at the door.
/// Corrupt bytes have to come back as a diagnostic; an abort takes the run down
/// and a wild length prefix is one allocation away from one.
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
            // Succeeding is allowed — some mutations are still a definition —
            // and only returning without aborting is being asserted.
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

/// The reconstruction is an artifact something else will diff, so it may not
/// vary run to run.
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
