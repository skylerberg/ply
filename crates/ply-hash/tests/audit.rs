//! Adversarial audit of content addressing.
//!
//! Every test here is written at the source level, because that is the surface a
//! user edits. A failure in the "false negative" section means the cache can
//! return a stale pass for changed code.

use ply_hash::{DefHash, HashOutput, hash_ast};
use ply_span::{SourceId, Symbol};

fn hashes(source: &str) -> HashOutput {
    let module = match ply_syntax::parse(SourceId(0), source) {
        Ok(m) => m,
        Err(diags) => panic!("source did not parse: {diags:#?}\n---\n{source}"),
    };
    hash_ast(&module).expect("module should hash")
}

fn def(source: &str, name: &str) -> DefHash {
    let out = hashes(source);
    *out.defs
        .get(&Symbol::new(name))
        .unwrap_or_else(|| panic!("no definition named `{name}` in\n{source}"))
}

#[track_caller]
fn changed(what: &str, before: &str, after: &str, name: &str) {
    assert_ne!(def(before, name), def(after, name), "{what}: hash did not change");
}

#[track_caller]
fn unchanged(what: &str, before: &str, after: &str, name: &str) {
    assert_eq!(def(before, name), def(after, name), "{what}: hash changed");
}

// False negatives: a real change that must move the hash.

#[test]
fn swapping_two_arguments_at_a_call_site_changes_the_hash() {
    changed(
        "argument swap",
        "fn sub(a: Int, b: Int) -> Int = a - b\nfn f(x: Int, y: Int) -> Int = sub(x, y)",
        "fn sub(a: Int, b: Int) -> Int = a - b\nfn f(x: Int, y: Int) -> Int = sub(y, x)",
        "f",
    );
}

#[test]
fn swapping_two_record_fields_changes_the_hash() {
    changed(
        "record field swap",
        "fn f(a: Int, b: Int) -> {p: Int, q: Int} = {p: a, q: b}",
        "fn f(a: Int, b: Int) -> {p: Int, q: Int} = {p: b, q: a}",
        "f",
    );
}

#[test]
fn swapping_two_match_arms_changes_the_hash() {
    changed(
        "match arm swap",
        "fn f(n: Int) -> Int = match n { 0 -> 10, 1 -> 20, _ -> 30 }",
        "fn f(n: Int) -> Int = match n { 1 -> 20, 0 -> 10, _ -> 30 }",
        "f",
    );
}

#[test]
fn swapping_two_list_elements_changes_the_hash() {
    changed(
        "list element swap",
        "fn f(a: Int, b: Int) -> List<Int> = [a, b]",
        "fn f(a: Int, b: Int) -> List<Int> = [b, a]",
        "f",
    );
}

#[test]
fn changing_an_integer_literal_changes_the_hash() {
    changed("int literal", "fn f() -> Int = 41", "fn f() -> Int = 42", "f");
    changed("int literal sign", "fn f() -> Int = 1", "fn f() -> Int = -1", "f");
    changed(
        "int literal in a pattern",
        "fn f(n: Int) -> Int = match n { 0 -> 1, _ -> 2 }",
        "fn f(n: Int) -> Int = match n { 1 -> 1, _ -> 2 }",
        "f",
    );
}

#[test]
fn changing_a_string_literal_changes_the_hash() {
    changed("string literal", r#"fn f() -> String = "ok""#, r#"fn f() -> String = "no""#, "f");
    changed(
        "string split point",
        r#"fn f() -> String = "ab" ++ "c""#,
        r#"fn f() -> String = "a" ++ "bc""#,
        "f",
    );
    changed(
        "an escape is not its spelling",
        r#"fn f() -> String = "a\nb""#,
        r#"fn f() -> String = "a\tb""#,
        "f",
    );
}

#[test]
fn changing_a_comparison_operator_changes_the_hash() {
    for (a, b) in [("<", "<="), ("<", ">"), ("==", "!="), (">=", ">"), ("<=", ">=")] {
        changed(
            "comparison operator",
            &format!("fn f(x: Int, y: Int) -> Bool = x {a} y"),
            &format!("fn f(x: Int, y: Int) -> Bool = x {b} y"),
            "f",
        );
    }
}

#[test]
fn swapping_the_operands_of_a_comparison_changes_the_hash() {
    changed(
        "operand swap",
        "fn f(x: Int, y: Int) -> Bool = x < y",
        "fn f(x: Int, y: Int) -> Bool = y < x",
        "f",
    );
}

#[test]
fn changing_which_same_typed_parameter_the_body_uses_changes_the_hash() {
    changed(
        "parameter selection",
        "fn f(a: Int, b: Int) -> Int = a",
        "fn f(a: Int, b: Int) -> Int = b",
        "f",
    );
    changed(
        "lambda parameter selection",
        "fn f() -> (Int, Int) -> Int = |a: Int, b: Int| a",
        "fn f() -> (Int, Int) -> Int = |a: Int, b: Int| b",
        "f",
    );
}

#[test]
fn changing_which_match_binder_the_arm_uses_changes_the_hash() {
    let source = |used: &str| {
        format!(
            "type Pair = P(Int, Int)\n\
             fn f(p: Pair) -> Int = match p {{ P(a, b) -> {used} }}"
        )
    };
    changed("match binder selection", &source("a"), &source("b"), "f");
}

#[test]
fn changing_which_handler_clause_parameter_is_used_changes_the_hash() {
    let source = |used: &str| {
        format!(
            "effect db {{\n  write put[r](key: Int, value: Int) -> Int\n}}\n\
             fn body() -> Int / {{db.write[users]}} = db.put[users](1, 2)\n\
             fn f() -> Int = handle body() with {{ db.put[users](k, v) -> {used}, }}"
        )
    };
    changed("handler clause parameter", &source("k"), &source("v"), "f");
}

#[test]
fn adding_an_unused_parameter_changes_the_hash() {
    changed(
        "extra parameter",
        "fn f(a: Int) -> Int = a",
        "fn f(a: Int, b: Int) -> Int = a",
        "f",
    );
    changed(
        "extra lambda parameter",
        "fn f() -> (Int) -> Int = |a: Int| a",
        "fn f() -> (Int, Int) -> Int = |a: Int, b: Int| a",
        "f",
    );
}

#[test]
fn changing_an_effect_annotation_changes_the_hash() {
    let source = |row: &str| {
        format!(
            "effect db {{\n  read get[r](key: Int) -> Int\n  write put[r](key: Int, v: Int) -> Int\n}}\n\
             fn f(k: Int) -> Int / {{{row}}} = db.get[users](k)"
        )
    };
    changed("added atom", &source("db.read[users]"), &source("db.read[users], db.write[orders]"), "f");
    changed("dropped annotation", &source("db.read[users]"), "effect db {\n  read get[r](key: Int) -> Int\n  write put[r](key: Int, v: Int) -> Int\n}\nfn f(k: Int) -> Int = db.get[users](k)", "f");
}

#[test]
fn changing_a_resource_label_changes_the_hash() {
    let source = |resource: &str| {
        format!(
            "effect db {{\n  read get[r](key: Int) -> Int\n}}\n\
             fn f(k: Int) -> Int / {{db.read[{resource}]}} = db.get[{resource}](k)"
        )
    };
    changed("resource label", &source("users"), &source("orders"), "f");
}

#[test]
fn changing_read_to_write_changes_the_hash() {
    let effect = |mode: &str| {
        format!("effect db {{\n  {mode} touch[r](key: Int) -> Int\n}}\n")
    };
    let program =
        |mode: &str, row: &str| format!("{}fn f(k: Int) -> Int / {{{row}}} = db.touch[users](k)", effect(mode));
    changed(
        "operation mode",
        &program("read", "db.read[users]"),
        &program("write", "db.write[users]"),
        "f",
    );

    // The annotation alone, with the declaration held fixed, must also move it.
    let annotated = |row: &str| {
        format!(
            "effect db {{\n  read get[r](key: Int) -> Int\n  write put[r](key: Int, v: Int) -> Int\n}}\n\
             fn f(k: Int) -> Int / {{{row}}} = db.get[users](k)"
        )
    };
    changed("annotated mode", &annotated("db.read[users]"), &annotated("db.write[users]"), "f");
}

#[test]
fn changing_a_constructor_arity_changes_the_hash() {
    changed(
        "constructor arity",
        "type T = A(Int) | B\nfn f() -> T = B",
        "type T = A(Int, Int) | B\nfn f() -> T = B",
        "f",
    );
}

#[test]
fn swapping_two_variants_of_a_sum_type_changes_the_hash() {
    changed(
        "variant order",
        "type T = A(Int) | B(String)\nfn f() -> T = B(\"x\")",
        "type T = B(String) | A(Int)\nfn f() -> T = B(\"x\")",
        "f",
    );
}

#[test]
fn a_nested_pair_is_not_a_flat_list() {
    changed(
        "nesting",
        "fn f(a: Int, b: Int, c: Int) -> List<List<Int>> = [[a], [b, c]]",
        "fn f(a: Int, b: Int, c: Int) -> List<List<Int>> = [[a, b], [c]]",
        "f",
    );
}

#[test]
fn an_empty_record_is_not_unit_and_neither_is_an_empty_list() {
    let empty_record = def("fn f() -> Unit = {}", "f");
    let unit = def("fn f() -> Unit = ()", "f");
    let empty_list = def("fn f() -> List<Int> = []", "f");
    assert_ne!(empty_record, unit, "an empty record hashes like unit");
    assert_ne!(empty_record, empty_list, "an empty record hashes like an empty list");
    assert_ne!(unit, empty_list, "unit hashes like an empty list");
}

#[test]
fn an_inner_binding_that_captures_an_outer_name_changes_the_hash() {
    changed(
        "shadowing a parameter",
        "fn f(x: Int) -> Int = { let y = 1; x + y }",
        "fn f(x: Int) -> Int = { let x = 1; x + x }",
        "f",
    );
}

#[test]
fn capturing_a_top_level_name_with_a_local_changes_the_hash() {
    let before = "fn amount() -> Int = 7\nfn f() -> Int = { let unused = 2; amount() }";
    let after = "fn amount() -> Int = 7\nfn f() -> Int = { let amount = 2; amount }";
    changed("local captures a top-level name", before, after, "f");
}

#[test]
fn a_lambda_parameter_that_captures_an_outer_parameter_changes_the_hash() {
    changed(
        "lambda shadowing",
        "fn f(x: Int) -> (Int) -> Int = |y: Int| x",
        "fn f(x: Int) -> (Int) -> Int = |x: Int| x",
        "f",
    );
}

#[test]
fn a_match_binder_that_captures_a_parameter_changes_the_hash() {
    let source = |binder: &str, used: &str| {
        format!(
            "type Box = B(Int)\n\
             fn f(x: Int, b: Box) -> Int = match b {{ B({binder}) -> {used} }}"
        )
    };
    // `B(y) -> x` reads the parameter; `B(x) -> x` reads the binder.
    changed("match binder capture", &source("y", "x"), &source("x", "x"), "f");
}

#[test]
fn two_effects_with_identical_declarations_are_distinguishable() {
    // `db` and `audit` declare the same operations. Performing one is not
    // performing the other: they yield different atoms, so they conflict
    // differently and are discharged by different handlers.
    let source = |effect: &str| {
        format!(
            "effect db {{\n  write emit[r](key: Int) -> Int\n}}\n\
             effect audit {{\n  write emit[r](key: Int) -> Int\n}}\n\
             fn f(k: Int) -> Int / {{{effect}.write[log]}} = {effect}.emit[log](k)"
        )
    };
    changed("switching which effect is performed", &source("db"), &source("audit"), "f");
}

/// Aliases are expanded by the checker, so `Meters` and `Feet` below are both
/// literally `Int` and swapping one for the other is genuinely a no-op. This is
/// the one place where erasing a declaration's name is safe, and it is safe only
/// because the entity is structural — contrast the effect case above.
#[test]
fn swapping_two_transparent_type_aliases_is_free() {
    let source = |ty: &str| {
        format!(
            "type Meters = Int\n\
             type Feet = Int\n\
             fn f(x: {ty}) -> {ty} = x"
        )
    };
    unchanged("transparent alias swap", &source("Meters"), &source("Feet"), "f");
}

#[test]
fn two_sum_types_with_identical_shapes_are_distinguishable() {
    let source = |ctor: &str| {
        format!(
            "type Left = L1 | L2\n\
             type Right = R1 | R2\n\
             fn f() -> {} = {ctor}",
            if ctor.starts_with('L') { "Left" } else { "Right" }
        )
    };
    changed("switching which sum type is used", &source("L1"), &source("R1"), "f");
}

#[test]
fn a_handler_clause_that_moves_to_a_different_effect_changes_the_hash() {
    let source = |handled: &str| {
        format!(
            "effect db {{\n  read get[r](key: Int) -> Int\n}}\n\
             effect cache {{\n  read get[r](key: Int) -> Int\n}}\n\
             fn body(k: Int) -> Int / {{db.read[users]}} = db.get[users](k)\n\
             fn f() -> Int = handle body(1) with {{ {handled}.get[users](k) -> k, }}"
        )
    };
    changed("handled effect", &source("db"), &source("cache"), "f");
}

#[test]
fn a_nondet_effect_declaration_is_part_of_the_hash() {
    let source = |nondet: &str| {
        format!(
            "{nondet}effect clock {{\n  read now() -> Int\n}}\n\
             fn f() -> Int / {{clock.read}} = clock.now()"
        )
    };
    changed("nondet marker", &source(""), &source("nondet "), "f");
}

#[test]
fn editing_a_body_moves_every_transitive_dependent_and_the_test() {
    let program = |leaf: &str| {
        format!(
            "fn leaf(n: Int) -> Int = {leaf}\n\
             fn mid(n: Int) -> Int = leaf(n)\n\
             fn top(n: Int) -> Int = mid(n)\n\
             test \"t\" {{ assert_eq(top(1), 1) }}"
        )
    };
    let before = hashes(&program("n"));
    let after = hashes(&program("n + 1"));
    for name in ["leaf", "mid", "top"] {
        assert_ne!(before.defs[&Symbol::new(name)], after.defs[&Symbol::new(name)], "{name}");
    }
    assert_ne!(before.tests, after.tests, "the test hash did not move");
}

// False positives: a no-op edit that must not move the hash.

#[test]
fn renaming_a_top_level_definition_through_several_callers_is_free() {
    let program = |leaf: &str| {
        format!(
            "fn {leaf}(n: Int) -> Int = n + 1\n\
             fn mid(n: Int) -> Int = {leaf}(n)\n\
             fn top(n: Int) -> Int = mid(mid(n))\n\
             test \"t\" {{ assert_eq(top(1), 3) }}"
        )
    };
    let before = hashes(&program("leaf"));
    let after = hashes(&program("compute_the_leaf_value"));
    assert_eq!(before.defs[&Symbol::new("leaf")], after.defs[&Symbol::new("compute_the_leaf_value")]);
    for name in ["mid", "top"] {
        assert_eq!(before.defs[&Symbol::new(name)], after.defs[&Symbol::new(name)], "{name}");
    }
    assert_eq!(before.tests, after.tests);
}

#[test]
fn renaming_locals_parameters_and_binders_is_free() {
    let program = |a: &str, b: &str, m: &str, c: &str| {
        format!(
            "type Box = B(Int)\n\
             fn f({a}: Int, {b}: Box) -> Int = {{\n\
             \x20 let {c} = {a} + 1;\n\
             \x20 match {b} {{ B({m}) -> {m} + {c} }}\n\
             }}"
        )
    };
    unchanged(
        "renaming every binder",
        &program("x", "y", "z", "w"),
        &program("first", "second", "third", "fourth"),
        "f",
    );
}

#[test]
fn renaming_a_handler_clause_parameter_and_a_cell_binder_is_free() {
    let program = |k: &str, v: &str, cell: &str| {
        format!(
            "effect db {{\n  write put[r](key: Int, value: Int) -> Int\n}}\n\
             fn body() -> Int / {{db.write[users]}} = db.put[users](1, 2)\n\
             fn f() -> Int = with_cell[users](0) {{ {cell} ->\n\
             \x20 handle body() with {{ db.put[users]({k}, {v}) -> {k} + {v} + cell_get({cell}), }}\n\
             }}"
        )
    };
    unchanged(
        "renaming clause and cell binders",
        &program("k", "v", "c"),
        &program("the_key", "the_value", "the_cell"),
        "f",
    );
}

#[test]
fn renaming_a_type_an_effect_and_a_generic_parameter_is_free() {
    let program = |ty: &str, eff: &str, g: &str| {
        format!(
            "type {ty} = {{ id: Int }}\n\
             effect {eff} {{\n  read get[r](key: Int) -> Int\n}}\n\
             fn pick<{g}>(xs: List<{g}>, fallback: {g}) -> {g} = fallback\n\
             fn f(r: {ty}, k: Int) -> Int / {{{eff}.read[users]}} = r.id + {eff}.get[users](k)"
        )
    };
    let before = hashes(&program("Row", "db", "a"));
    let after = hashes(&program("Record", "store", "elem"));
    assert_eq!(before.defs[&Symbol::new("f")], after.defs[&Symbol::new("f")], "f");
    assert_eq!(before.defs[&Symbol::new("pick")], after.defs[&Symbol::new("pick")], "pick");
}

#[test]
fn reformatting_and_recommenting_is_free() {
    let dense = "fn f(a: Int, b: Int) -> Int = { let c = a + b; c * 2 }";
    let airy = "\n\
        // A leading comment.\n\
        fn f(\n\
        \x20   a: Int,\n\
        \x20   b: Int,\n\
        ) -> Int = {\n\
        \x20   // an interior note\n\
        \x20   let c = a + b;   // trailing\n\
        \x20   c * 2\n\
        }\n";
    unchanged("reformatting", dense, airy, "f");
}

#[test]
fn wrapping_a_body_in_braces_is_free() {
    unchanged(
        "brace wrapping",
        "fn f(a: Int) -> Int = a + 1",
        "fn f(a: Int) -> Int = { a + 1 }",
        "f",
    );
}

#[test]
fn reordering_top_level_items_is_free() {
    let forward = "fn a(n: Int) -> Int = n + 1\n\
                   fn b(n: Int) -> Int = a(n) + 2\n\
                   fn c(n: Int) -> Int = b(n) + 3\n\
                   test \"t\" { assert_eq(c(0), 6) }";
    let reversed = "test \"t\" { assert_eq(c(0), 6) }\n\
                    fn c(n: Int) -> Int = b(n) + 3\n\
                    fn b(n: Int) -> Int = a(n) + 2\n\
                    fn a(n: Int) -> Int = n + 1";
    let before = hashes(forward);
    let after = hashes(reversed);
    for name in ["a", "b", "c"] {
        assert_eq!(before.defs[&Symbol::new(name)], after.defs[&Symbol::new(name)], "{name}");
    }
    assert_eq!(before.tests, after.tests);
}

#[test]
fn reordering_the_atoms_of_an_annotation_is_free() {
    let program = |row: &str| {
        format!(
            "effect db {{\n  read get[r](key: Int) -> Int\n  write put[r](key: Int, v: Int) -> Int\n}}\n\
             fn f(k: Int) -> Int / {{{row}}} = db.get[users](k) + db.put[orders](k, 1)"
        )
    };
    unchanged(
        "atom order",
        &program("db.read[users], db.write[orders]"),
        &program("db.write[orders], db.read[users]"),
        "f",
    );
}

#[test]
fn reordering_independent_let_bindings_is_free() {
    unchanged(
        "independent let order",
        "fn f() -> Int = { let a = 1; let b = 2; a + b }",
        "fn f() -> Int = { let b = 2; let a = 1; a + b }",
        "f",
    );
}

#[test]
fn reordering_the_operations_of_an_effect_is_free() {
    let program = |ops: &str| {
        format!(
            "effect db {{\n{ops}}}\n\
             fn f(k: Int) -> Int / {{db.read[users]}} = db.get[users](k)"
        )
    };
    unchanged(
        "operation order",
        &program("  read get[r](key: Int) -> Int\n  write put[r](key: Int, v: Int) -> Int\n"),
        &program("  write put[r](key: Int, v: Int) -> Int\n  read get[r](key: Int) -> Int\n"),
        "f",
    );
}

#[test]
fn reordering_the_fields_of_a_record_type_is_free() {
    unchanged(
        "record type field order",
        "fn f(r: {a: Int, b: String}) -> Int = r.a",
        "fn f(r: {b: String, a: Int}) -> Int = r.a",
        "f",
    );
}

#[test]
fn reordering_indistinguishable_mutually_recursive_definitions_is_free() {
    let forward = "fn ping(n: Int) -> Int = pong(n - 1)\nfn pong(n: Int) -> Int = ping(n - 1)";
    let backward = "fn pong(n: Int) -> Int = ping(n - 1)\nfn ping(n: Int) -> Int = pong(n - 1)";
    let before = hashes(forward);
    let after = hashes(backward);
    assert_eq!(before.defs[&Symbol::new("ping")], after.defs[&Symbol::new("ping")], "ping");
    assert_eq!(before.defs[&Symbol::new("pong")], after.defs[&Symbol::new("pong")], "pong");
}

#[test]
fn renaming_a_test_is_free_but_its_body_is_not() {
    let program = |name: &str, body: &str| format!("fn g() -> Int = 1\ntest \"{name}\" {{ assert_eq(g(), {body}) }}");
    assert_eq!(hashes(&program("a", "1")).tests, hashes(&program("b", "1")).tests);
    assert_ne!(hashes(&program("a", "1")).tests, hashes(&program("a", "2")).tests);
}

// Determinism and structural sanity.

#[test]
fn hashing_is_stable_across_repeated_runs() {
    let source = include_str!("../../../examples/ledger.ply");
    let first = hashes(source);
    for _ in 0..8 {
        assert_eq!(first, hashes(source));
    }
}

#[test]
fn every_definition_in_a_realistic_module_gets_a_distinct_hash_unless_identical() {
    let out = hashes(include_str!("../../../examples/ledger.ply"));
    let mut by_hash: std::collections::BTreeMap<DefHash, Vec<Symbol>> = Default::default();
    for (name, hash) in &out.defs {
        by_hash.entry(*hash).or_default().push(name.clone());
    }
    let collisions: Vec<_> = by_hash.values().filter(|v| v.len() > 1).collect();
    assert!(collisions.is_empty(), "distinct definitions share a hash: {collisions:?}");
}

/// Normalization walks the expression tree recursively. The parser's own limit
/// does not bound that walk: a left-leaning operator chain is parsed iteratively
/// at constant depth, so an arbitrarily deep tree reaches the normalizer.
#[test]
fn a_long_operator_chain_does_not_overflow_the_stack() {
    let chain = std::iter::repeat_n("a", 20_000).collect::<Vec<_>>().join(" + ");
    let source = format!("fn f(a: Int) -> Int = {chain}");
    let module = ply_syntax::parse(SourceId(0), &source).expect("a long chain parses");
    let out = hash_ast(&module).expect("a long chain hashes");
    assert!(out.defs.contains_key(&Symbol::new("f")));

    // `Expr`'s derived `Drop` recurses through every `Box` and overflows on a
    // tree this deep. That is the AST's own defect and it cannot be fixed from
    // here, so the tree is leaked rather than allowed to abort this binary and
    // hide the result above.
    std::mem::forget(module);
}

/// A cheap deterministic PRNG so the search below is reproducible from its seed.
fn next(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

/// Definitions are hashed in the order Tarjan emits components, and a reference
/// is written as the referent's hash, so a component emitted before one it
/// depends on would silently fall back to an opaque self-marker and collapse
/// distinct references onto the same bytes.
#[test]
fn randomized_acyclic_modules_hash_independently_of_item_order() {
    let mut seed = 0x5eed_1234_u64;
    for case in 0..200 {
        let count = 2 + (next(&mut seed) % 8) as usize;
        let mut items: Vec<String> = Vec::new();
        for i in 0..count {
            let body = if i == 0 || next(&mut seed).is_multiple_of(3) {
                format!("n + {i}")
            } else {
                let callee = (next(&mut seed) as usize) % i;
                format!("d{callee}(n) + {i}")
            };
            items.push(format!("fn d{i}(n: Int) -> Int = {body}"));
        }
        let forward = hashes(&items.join("\n"));

        let mut shuffled = items.clone();
        for i in (1..shuffled.len()).rev() {
            let j = (next(&mut seed) as usize) % (i + 1);
            shuffled.swap(i, j);
        }
        let after = hashes(&shuffled.join("\n"));

        for i in 0..count {
            let name = Symbol::new(format!("d{i}"));
            assert_eq!(forward.defs[&name], after.defs[&name], "case {case}, {name}");
        }
        let distinct: std::collections::BTreeSet<DefHash> = forward.defs.values().copied().collect();
        assert!(
            distinct.len() >= 2,
            "case {case}: every definition collapsed to one hash\n{}",
            items.join("\n")
        );
    }
}

#[test]
fn randomized_graphs_are_emitted_in_reverse_topological_order() {
    use ply_hash::graph::{NodeId, tarjan};
    let mut seed = 0xabcd_ef01_u64;
    for case in 0..300 {
        let n = 1 + (next(&mut seed) % 12) as usize;
        let mut edges: Vec<Vec<NodeId>> = vec![Vec::new(); n];
        for outgoing in &mut edges {
            for _ in 0..(next(&mut seed) % 4) {
                outgoing.push(NodeId((next(&mut seed) as usize) % n));
            }
        }
        let components = tarjan(n, &edges);

        let mut position = vec![usize::MAX; n];
        for (ci, component) in components.iter().enumerate() {
            for &v in component {
                assert_eq!(position[v], usize::MAX, "case {case}: node {v} in two components");
                position[v] = ci;
            }
        }
        assert!(position.iter().all(|&p| p != usize::MAX), "case {case}: a node was dropped");

        for (v, outgoing) in edges.iter().enumerate() {
            for w in outgoing {
                assert!(
                    position[w.0] <= position[v],
                    "case {case}: {v} -> {} is emitted after its dependency\nedges: {edges:?}",
                    w.0
                );
            }
        }
    }
}
