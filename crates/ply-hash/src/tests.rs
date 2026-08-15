//! The properties in CONTRACTS.md, plus the collision cases a normalizer is
//! most likely to get wrong.
//!
//! Hand-built ASTs cover the shapes that are fiddly to write in surface syntax;
//! every span in them is `DUMMY`, which is safe because normalization erases
//! spans. The parsed programs at the end re-prove the six properties against
//! real source, where spans and formatting genuinely differ.

use super::*;
use ply_span::Span;
use ply_syntax::ast::*;

fn id(name: &str) -> Ident {
    Ident::new(name, Span::DUMMY)
}

fn qn(name: &str) -> QName {
    QName::bare(id(name))
}

fn e(kind: ExprKind) -> Expr {
    Expr {
        kind,
        span: Span::DUMMY,
    }
}

fn var(name: &str) -> Expr {
    e(ExprKind::Var(qn(name)))
}

fn int(v: i64) -> Expr {
    e(ExprKind::Lit(Lit::Int(v)))
}

fn str_lit(v: &str) -> Expr {
    e(ExprKind::Lit(Lit::Str(v.to_string())))
}

fn bytes_lit(v: &[u8]) -> Expr {
    e(ExprKind::Lit(Lit::Bytes(v.to_vec())))
}

fn call(func: Expr, args: Vec<Expr>) -> Expr {
    e(ExprKind::App {
        func: Box::new(func),
        args,
    })
}

fn callv(name: &str, args: Vec<Expr>) -> Expr {
    call(var(name), args)
}

fn add(lhs: Expr, rhs: Expr) -> Expr {
    e(ExprKind::Binary {
        op: BinOp::Add,
        lhs: Box::new(lhs),
        rhs: Box::new(rhs),
    })
}

fn bin(op: BinOp, lhs: Expr, rhs: Expr) -> Expr {
    e(ExprKind::Binary {
        op,
        lhs: Box::new(lhs),
        rhs: Box::new(rhs),
    })
}

fn lambda(params: &[&str], body: Expr) -> Expr {
    e(ExprKind::Lambda {
        params: params.iter().map(|p| param(p)).collect(),
        body: Box::new(body),
    })
}

fn param(name: &str) -> Param {
    Param {
        name: id(name),
        ty: None,
        span: Span::DUMMY,
    }
}

fn typed_param(name: &str, ty: &str) -> Param {
    Param {
        name: id(name),
        ty: Some(ty_con(ty, vec![])),
        span: Span::DUMMY,
    }
}

fn ty_con(name: &str, args: Vec<TypeExpr>) -> TypeExpr {
    TypeExpr::Con {
        name: qn(name),
        args,
        span: Span::DUMMY,
    }
}

fn let_(name: &str, value: Expr) -> Stmt {
    Stmt::Let {
        pat: Pattern {
            kind: PatternKind::Var(id(name)),
            span: Span::DUMMY,
        },
        ty: None,
        value: Box::new(value),
        span: Span::DUMMY,
    }
}

fn pat(kind: PatternKind) -> Pattern {
    Pattern {
        kind,
        span: Span::DUMMY,
    }
}

fn pvar(name: &str) -> Pattern {
    pat(PatternKind::Var(id(name)))
}

fn pctor(name: &str, args: Vec<Pattern>) -> Pattern {
    pat(PatternKind::Ctor {
        name: qn(name),
        args,
    })
}

fn arm(pattern: Pattern, body: Expr) -> MatchArm {
    MatchArm {
        pat: pattern,
        guard: None,
        body,
        span: Span::DUMMY,
    }
}

fn block(stmts: Vec<Stmt>, tail: Option<Expr>) -> Expr {
    e(ExprKind::Block {
        stmts,
        tail: tail.map(Box::new),
    })
}

fn record(fields: Vec<(&str, Expr)>) -> Expr {
    e(ExprKind::Record {
        fields: fields.into_iter().map(|(n, v)| (id(n), v)).collect(),
    })
}

fn list(items: Vec<Expr>) -> Expr {
    e(ExprKind::List { items })
}

fn func(name: &str, params: &[&str], body: Expr) -> Item {
    Item::Fn(Box::new(FnDef {
        vis: Visibility::Private,
        name: id(name),
        generics: Generics::default(),
        params: params.iter().map(|p| param(p)).collect(),
        ret: None,
        effects: None,
        constraints: Vec::new(),
        derived: None,
        spec: Vec::new(),
        body,
        span: Span::DUMMY,
    }))
}

fn test_item(name: &str, body: Expr) -> Item {
    Item::Test(Box::new(TestDef {
        name: name.to_string(),
        name_span: Span::DUMMY,
        nondet: false,
        body,
        span: Span::DUMMY,
    }))
}

fn module(items: Vec<Item>) -> Module {
    Module {
        name: ModuleName::anonymous(),
        source: Span::DUMMY.source,
        imports: Vec::new(),
        items,
    }
}

fn hashes(items: Vec<Item>) -> HashOutput {
    hash_ast(&module(items)).expect("module should hash")
}

fn hash_of(items: Vec<Item>, name: &str) -> DefHash {
    *hashes(items)
        .defs
        .get(&Symbol::new(name))
        .expect("definition should be hashed")
}

/// `a` calls `b` calls `c` calls `d`, with a test on top of `a`. The chain the
/// rename properties are demonstrated against.
fn chain(a: &str, b: &str, c: &str, d: &str) -> Vec<Item> {
    vec![
        func(a, &["x"], callv(b, vec![add(var("x"), int(1))])),
        func(b, &["y"], callv(c, vec![var("y")])),
        func(c, &["z"], callv(d, vec![var("z"), int(2)])),
        func(d, &["p", "q"], add(var("p"), var("q"))),
        test_item(
            "the chain adds up",
            callv("assert_eq", vec![callv(a, vec![int(1)]), int(4)]),
        ),
    ]
}

// ---- property 1: renaming a top-level definition changes no hash ----

#[test]
fn renaming_a_transitively_called_definition_changes_no_hash() {
    let before = hashes(chain("a", "b", "c", "d"));
    let after = hashes(chain("a", "b", "c", "renamed_deep_helper"));

    assert_eq!(
        before.defs[&Symbol::new("a")],
        after.defs[&Symbol::new("a")]
    );
    assert_eq!(
        before.defs[&Symbol::new("b")],
        after.defs[&Symbol::new("b")]
    );
    assert_eq!(
        before.defs[&Symbol::new("c")],
        after.defs[&Symbol::new("c")]
    );
    assert_eq!(
        before.defs[&Symbol::new("d")],
        after.defs[&Symbol::new("renamed_deep_helper")]
    );
    assert_eq!(before.tests, after.tests);
}

#[test]
fn renaming_every_definition_at_once_changes_no_hash() {
    let before = hashes(chain("a", "b", "c", "d"));
    let after = hashes(chain("first", "second", "third", "fourth"));

    let mut b: Vec<DefHash> = before.defs.values().copied().collect();
    let mut a: Vec<DefHash> = after.defs.values().copied().collect();
    b.sort_unstable();
    a.sort_unstable();
    assert_eq!(b, a);
    assert_eq!(before.tests, after.tests);
}

#[test]
fn renaming_a_recursive_definition_changes_no_hash() {
    let fact = |name: &str| {
        vec![func(
            name,
            &["n"],
            e(ExprKind::If {
                cond: Box::new(bin(BinOp::Le, var("n"), int(1))),
                then_branch: Box::new(int(1)),
                else_branch: Box::new(bin(
                    BinOp::Mul,
                    var("n"),
                    callv(name, vec![bin(BinOp::Sub, var("n"), int(1))]),
                )),
            }),
        )]
    };
    assert_eq!(
        hash_of(fact("fact"), "fact"),
        hash_of(fact("factorial"), "factorial")
    );
}

#[test]
fn renaming_a_type_changes_no_hash_of_its_users() {
    let program = |ty: &str| {
        vec![
            Item::Type(Box::new(TypeDef {
                vis: Visibility::Private,
                name: id(ty),
                params: vec![],
                body: TypeDefBody::Sum(vec![
                    VariantDef {
                        name: id("Active"),
                        fields: vec![],
                        span: Span::DUMMY,
                    },
                    VariantDef {
                        name: id("Banned"),
                        fields: vec![ty_con("String", vec![])],
                        span: Span::DUMMY,
                    },
                ]),
                span: Span::DUMMY,
            })),
            Item::Fn(Box::new(FnDef {
                vis: Visibility::Private,
                name: id("describe"),
                generics: Generics::default(),
                params: vec![typed_param("u", ty)],
                ret: Some(ty_con("String", vec![])),
                effects: None,
                constraints: Vec::new(),
                derived: None,
                spec: Vec::new(),
                body: e(ExprKind::Match {
                    scrutinee: Box::new(var("u")),
                    arms: vec![
                        arm(pctor("Active", vec![]), str_lit("active")),
                        arm(pctor("Banned", vec![pvar("why")]), var("why")),
                    ],
                }),
                span: Span::DUMMY,
            })),
        ]
    };
    assert_eq!(
        hash_of(program("User"), "describe"),
        hash_of(program("Account"), "describe")
    );
}

#[test]
fn renaming_an_effect_changes_no_hash_of_its_performers() {
    let program = |eff: &str| {
        vec![
            Item::Effect(Box::new(EffectDef {
                vis: Visibility::Private,
                name: id(eff),
                nondet: false,
                ops: vec![OpDef {
                    name: id("get"),
                    mode: Mode::Read,
                    resource_param: true,
                    params: vec![ty_con("Int", vec![])],
                    ret: ty_con("Int", vec![]),
                    span: Span::DUMMY,
                }],
                span: Span::DUMMY,
            })),
            func(
                "lookup",
                &["k"],
                e(ExprKind::Perform {
                    effect: qn(eff),
                    op: id("get"),
                    resource: Some(id("users")),
                    args: vec![var("k")],
                }),
            ),
        ]
    };
    assert_eq!(
        hash_of(program("db"), "lookup"),
        hash_of(program("store"), "lookup")
    );
}

// ---- property 2: renaming a local changes no hash ----

#[test]
fn renaming_parameters_and_let_bindings_changes_no_hash() {
    let program = |p: &str, q: &str, l: &str| {
        vec![func(
            "f",
            &[p],
            block(
                vec![let_(l, add(var(p), int(1)))],
                Some(lambda(&[q], add(var(l), var(q)))),
            ),
        )]
    };
    assert_eq!(
        hash_of(program("x", "y", "t"), "f"),
        hash_of(program("a", "b", "u"), "f")
    );
}

#[test]
fn renaming_a_match_binder_changes_no_hash() {
    let program = |b: &str| {
        vec![func(
            "f",
            &["xs"],
            e(ExprKind::Match {
                scrutinee: Box::new(var("xs")),
                arms: vec![MatchArm {
                    pat: Pattern {
                        kind: PatternKind::List {
                            items: vec![Pattern {
                                kind: PatternKind::Var(id(b)),
                                span: Span::DUMMY,
                            }],
                            rest: None,
                        },
                        span: Span::DUMMY,
                    },
                    guard: None,
                    body: var(b),
                    span: Span::DUMMY,
                }],
            }),
        )]
    };
    assert_eq!(hash_of(program("head"), "f"), hash_of(program("h"), "f"));
}

#[test]
fn shadowing_resolves_to_the_innermost_binder() {
    let shadowed = hash_of(
        vec![func("f", &[], lambda(&["x"], lambda(&["x"], var("x"))))],
        "f",
    );
    let distinct = hash_of(
        vec![func("f", &[], lambda(&["x"], lambda(&["y"], var("y"))))],
        "f",
    );
    let outer = hash_of(
        vec![func("f", &[], lambda(&["x"], lambda(&["y"], var("x"))))],
        "f",
    );
    assert_eq!(shadowed, distinct);
    assert_ne!(shadowed, outer);
}

#[test]
fn a_local_shadows_a_top_level_definition_of_the_same_name() {
    let out = hashes(vec![
        func("helper", &[], int(0)),
        func("f", &["helper"], var("helper")),
    ]);
    assert_eq!(out.deps[&Symbol::new("f")], Vec::<Symbol>::new());
    assert_eq!(
        out.defs[&Symbol::new("f")],
        hash_of(
            vec![func("helper", &[], int(0)), func("f", &["x"], var("x"))],
            "f"
        ),
    );
}

#[test]
fn renaming_a_cell_binder_changes_no_hash() {
    let program = |binder: &str| {
        vec![func(
            "f",
            &[],
            e(ExprKind::WithCell {
                resource: id("users"),
                init: Box::new(int(0)),
                binder: id(binder),
                body: Box::new(callv("cell_get", vec![var(binder)])),
            }),
        )]
    };
    assert_eq!(hash_of(program("cell"), "f"), hash_of(program("c"), "f"));
}

#[test]
fn de_bruijn_levels_distinguish_which_binder_is_used() {
    let first = hash_of(vec![func("f", &["a", "b"], var("a"))], "f");
    let second = hash_of(vec![func("f", &["a", "b"], var("b"))], "f");
    assert_ne!(first, second);
}

#[test]
fn a_let_binding_is_not_in_scope_in_its_own_right_hand_side() {
    // `let x = x` refers to whatever `x` meant outside, not to itself.
    let inner = hash_of(
        vec![func(
            "f",
            &["x"],
            block(vec![let_("x", var("x"))], Some(var("x"))),
        )],
        "f",
    );
    let renamed = hash_of(
        vec![func(
            "f",
            &["p"],
            block(vec![let_("q", var("p"))], Some(var("q"))),
        )],
        "f",
    );
    assert_eq!(inner, renamed);
}

// ---- property 3: reformatting and reordering change no hash ----

#[test]
fn wrapping_a_body_in_a_block_changes_no_hash() {
    let bare = hash_of(vec![func("f", &["x"], add(var("x"), int(1)))], "f");
    let braced = hash_of(
        vec![func(
            "f",
            &["x"],
            block(vec![], Some(add(var("x"), int(1)))),
        )],
        "f",
    );
    assert_eq!(bare, braced);
}

#[test]
fn reordering_top_level_items_changes_no_hash() {
    let forward = chain("a", "b", "c", "d");
    let mut reversed = forward.clone();
    reversed.reverse();
    let before = hash_ast(&module(forward)).unwrap();
    let after = hash_ast(&module(reversed)).unwrap();
    for name in ["a", "b", "c", "d"] {
        assert_eq!(
            before.defs[&Symbol::new(name)],
            after.defs[&Symbol::new(name)],
            "{name}"
        );
    }
    assert_eq!(before.tests, after.tests);
}

#[test]
fn reordering_mutually_recursive_definitions_changes_no_hash() {
    let parity = |name: &str, other: &str, at_zero: bool| {
        func(
            name,
            &["n"],
            e(ExprKind::If {
                cond: Box::new(bin(BinOp::Eq, var("n"), int(0))),
                then_branch: Box::new(e(ExprKind::Lit(Lit::Bool(at_zero)))),
                else_branch: Box::new(callv(other, vec![bin(BinOp::Sub, var("n"), int(1))])),
            }),
        )
    };
    let even = parity("is_even", "is_odd", true);
    let odd = parity("is_odd", "is_even", false);
    let forward = hashes(vec![even.clone(), odd.clone()]);
    let backward = hashes(vec![odd, even]);
    assert_eq!(
        forward.defs[&Symbol::new("is_even")],
        backward.defs[&Symbol::new("is_even")]
    );
    assert_eq!(
        forward.defs[&Symbol::new("is_odd")],
        backward.defs[&Symbol::new("is_odd")]
    );
    assert_ne!(
        forward.defs[&Symbol::new("is_even")],
        forward.defs[&Symbol::new("is_odd")]
    );
}

/// `f(n) = g(n-1)` and `g(n) = f(n-1)` differ only in which of the two they
/// call, which makes them the same definition twice over: either one can be
/// substituted for the other anywhere without changing what the program
/// computes. They share a hash, and reordering them is therefore free like any
/// other reordering.
#[test]
fn indistinguishable_cycle_members_share_one_hash() {
    let even = func(
        "is_even",
        &["n"],
        callv("is_odd", vec![bin(BinOp::Sub, var("n"), int(1))]),
    );
    let odd = func(
        "is_odd",
        &["n"],
        callv("is_even", vec![bin(BinOp::Sub, var("n"), int(1))]),
    );
    let forward = hashes(vec![even.clone(), odd.clone()]);
    let backward = hashes(vec![odd, even]);
    assert_eq!(
        forward.defs[&Symbol::new("is_even")],
        forward.defs[&Symbol::new("is_odd")]
    );
    assert_eq!(
        forward.defs[&Symbol::new("is_even")],
        backward.defs[&Symbol::new("is_even")]
    );
    assert_eq!(
        forward.defs[&Symbol::new("is_odd")],
        backward.defs[&Symbol::new("is_odd")]
    );
}

/// Refinement has to keep splitting past the first round: `a` and `c` are told
/// apart only by what their callees' callees do, which no single pass sees.
#[test]
fn refinement_separates_members_that_differ_only_deeper_in_the_cycle() {
    let step = |name: &str, next: &str, extra: i64| {
        func(name, &["n"], callv(next, vec![add(var("n"), int(extra))]))
    };
    let out = hashes(vec![
        step("a", "b", 0),
        step("b", "c", 0),
        step("c", "d", 1),
        step("d", "a", 0),
    ]);
    let distinct: BTreeSet<DefHash> = out.defs.values().copied().collect();
    assert_eq!(distinct.len(), 4, "{:?}", out.defs);
}

#[test]
fn reordering_the_atoms_of_an_effect_annotation_changes_no_hash() {
    let annotated = |atoms: Vec<(&str, Mode, &str)>| {
        vec![Item::Fn(Box::new(FnDef {
            vis: Visibility::Private,
            name: id("f"),
            generics: Generics::default(),
            params: vec![],
            ret: None,
            effects: Some(RowExpr {
                atoms: atoms
                    .into_iter()
                    .map(|(eff, mode, res)| AtomExpr {
                        effect: qn(eff),
                        mode,
                        resource: Some(id(res)),
                        span: Span::DUMMY,
                    })
                    .collect(),
                tail: None,
                span: Span::DUMMY,
            }),
            constraints: Vec::new(),
            derived: None,
            spec: Vec::new(),
            body: int(0),
            span: Span::DUMMY,
        }))]
    };
    let a = hash_of(
        annotated(vec![
            ("db", Mode::Read, "users"),
            ("db", Mode::Write, "orders"),
        ]),
        "f",
    );
    let b = hash_of(
        annotated(vec![
            ("db", Mode::Write, "orders"),
            ("db", Mode::Read, "users"),
        ]),
        "f",
    );
    assert_eq!(a, b);

    let different = hash_of(
        annotated(vec![
            ("db", Mode::Read, "users"),
            ("db", Mode::Write, "users"),
        ]),
        "f",
    );
    assert_ne!(a, different);
}

// ---- property 4: editing a body changes it and its dependents ----

#[test]
fn editing_a_body_changes_exactly_its_transitive_dependents() {
    let before = hashes(chain("a", "b", "c", "d"));
    let mut edited = chain("a", "b", "c", "d");
    edited[3] = func("d", &["p", "q"], bin(BinOp::Mul, var("p"), var("q")));
    let after = hashes(edited);

    assert_ne!(
        before.defs[&Symbol::new("d")],
        after.defs[&Symbol::new("d")]
    );
    assert_ne!(
        before.defs[&Symbol::new("c")],
        after.defs[&Symbol::new("c")]
    );
    assert_ne!(
        before.defs[&Symbol::new("b")],
        after.defs[&Symbol::new("b")]
    );
    assert_ne!(
        before.defs[&Symbol::new("a")],
        after.defs[&Symbol::new("a")]
    );
    assert_ne!(before.tests[0], after.tests[0]);
}

#[test]
fn editing_a_body_leaves_unrelated_definitions_alone() {
    let program = |body: Expr| {
        vec![
            func("touched", &["x"], body),
            func("untouched", &["x"], add(var("x"), int(7))),
            test_item("untouched test", callv("untouched", vec![int(1)])),
            test_item("touched test", callv("touched", vec![int(1)])),
        ]
    };
    let before = hashes(program(int(1)));
    let after = hashes(program(int(2)));
    assert_ne!(
        before.defs[&Symbol::new("touched")],
        after.defs[&Symbol::new("touched")]
    );
    assert_eq!(
        before.defs[&Symbol::new("untouched")],
        after.defs[&Symbol::new("untouched")]
    );
    assert_eq!(before.tests[0], after.tests[0]);
    assert_ne!(before.tests[1], after.tests[1]);
}

#[test]
fn changing_a_type_definition_changes_the_hash_of_its_users() {
    let program = |extra: Vec<VariantDef>| {
        let mut variants = vec![VariantDef {
            name: id("Active"),
            fields: vec![],
            span: Span::DUMMY,
        }];
        variants.extend(extra);
        vec![
            Item::Type(Box::new(TypeDef {
                vis: Visibility::Private,
                name: id("Status"),
                params: vec![],
                body: TypeDefBody::Sum(variants),
                span: Span::DUMMY,
            })),
            func("mk", &[], var("Active")),
        ]
    };
    let before = hash_of(program(vec![]), "mk");
    let after = hash_of(
        program(vec![VariantDef {
            name: id("Banned"),
            fields: vec![],
            span: Span::DUMMY,
        }]),
        "mk",
    );
    assert_ne!(before, after);
}

#[test]
fn changing_one_member_of_a_cycle_changes_the_whole_component() {
    let program = |lit: i64| {
        vec![
            func(
                "ping",
                &["n"],
                callv("pong", vec![bin(BinOp::Sub, var("n"), int(lit))]),
            ),
            func("pong", &["n"], callv("ping", vec![var("n")])),
            func("caller", &[], callv("ping", vec![int(3)])),
        ]
    };
    let before = hashes(program(1));
    let after = hashes(program(2));
    assert_ne!(
        before.defs[&Symbol::new("ping")],
        after.defs[&Symbol::new("ping")]
    );
    assert_ne!(
        before.defs[&Symbol::new("pong")],
        after.defs[&Symbol::new("pong")]
    );
    assert_ne!(
        before.defs[&Symbol::new("caller")],
        after.defs[&Symbol::new("caller")]
    );
}

// ---- property 5: structurally identical definitions hash identically ----

#[test]
fn structurally_identical_definitions_share_a_hash() {
    let out = hashes(vec![
        func("increment", &["x"], add(var("x"), int(1))),
        func("succ", &["n"], add(var("n"), int(1))),
        func("plus_two", &["n"], add(var("n"), int(2))),
    ]);
    assert_eq!(
        out.defs[&Symbol::new("increment")],
        out.defs[&Symbol::new("succ")]
    );
    assert_ne!(
        out.defs[&Symbol::new("increment")],
        out.defs[&Symbol::new("plus_two")]
    );
}

#[test]
fn identical_definitions_hash_identically_across_modules() {
    let alone = hash_of(vec![func("f", &["x"], add(var("x"), int(1)))], "f");
    let crowded = hash_of(
        vec![
            func("noise", &[], int(0)),
            func("f", &["x"], add(var("x"), int(1))),
            test_item("noise test", int(0)),
        ],
        "f",
    );
    assert_eq!(alone, crowded);
}

// ---- property 6: swapping fields, arguments or arms changes the hash ----

#[test]
fn swapping_two_arguments_changes_the_hash() {
    let a = hash_of(
        vec![func(
            "caller",
            &["x", "y"],
            callv("f", vec![var("x"), var("y")]),
        )],
        "caller",
    );
    let b = hash_of(
        vec![func(
            "caller",
            &["x", "y"],
            callv("f", vec![var("y"), var("x")]),
        )],
        "caller",
    );
    assert_ne!(a, b);
}

#[test]
fn swapping_two_record_fields_changes_the_hash() {
    let a = hash_of(
        vec![func("f", &[], record(vec![("a", int(1)), ("b", int(2))]))],
        "f",
    );
    let b = hash_of(
        vec![func("f", &[], record(vec![("b", int(2)), ("a", int(1))]))],
        "f",
    );
    let c = hash_of(
        vec![func("f", &[], record(vec![("a", int(2)), ("b", int(1))]))],
        "f",
    );
    assert_ne!(a, b);
    assert_ne!(a, c);
}

#[test]
fn swapping_two_match_arms_changes_the_hash() {
    let arms = |first: i64, second: i64| {
        let arm = |lit: i64| MatchArm {
            pat: Pattern {
                kind: PatternKind::Lit(Lit::Int(lit)),
                span: Span::DUMMY,
            },
            guard: None,
            body: int(lit * 10),
            span: Span::DUMMY,
        };
        vec![func(
            "f",
            &["x"],
            e(ExprKind::Match {
                scrutinee: Box::new(var("x")),
                arms: vec![arm(first), arm(second)],
            }),
        )]
    };
    assert_ne!(hash_of(arms(1, 2), "f"), hash_of(arms(2, 1), "f"));
}

#[test]
fn swapping_the_operands_of_a_binary_operator_changes_the_hash() {
    let a = hash_of(
        vec![func("f", &["x", "y"], bin(BinOp::Sub, var("x"), var("y")))],
        "f",
    );
    let b = hash_of(
        vec![func("f", &["x", "y"], bin(BinOp::Sub, var("y"), var("x")))],
        "f",
    );
    assert_ne!(a, b);
}

#[test]
fn differently_shaped_trees_do_not_collide() {
    let shapes: Vec<(&str, Expr)> = vec![
        ("two element list", list(vec![int(1), int(2)])),
        ("nested list", list(vec![list(vec![int(1), int(2)])])),
        (
            "pair of lists",
            list(vec![list(vec![int(1)]), list(vec![int(2)])]),
        ),
        ("empty list", list(vec![])),
        ("empty record", record(vec![])),
        ("unit literal", e(ExprKind::Lit(Lit::Unit))),
        ("empty block", block(vec![], None)),
        ("int one", int(1)),
        ("string one", str_lit("1")),
        ("bool", e(ExprKind::Lit(Lit::Bool(true)))),
        ("curried call", call(callv("f", vec![int(1)]), vec![int(2)])),
        ("uncurried call", callv("f", vec![int(1), int(2)])),
        ("nullary call", callv("f", vec![])),
        ("bare reference", var("f")),
        ("record with one field", record(vec![("a", int(1))])),
        (
            "record with two fields",
            record(vec![("a", int(1)), ("b", int(1))]),
        ),
        (
            "field access",
            e(ExprKind::Field {
                base: Box::new(var("f")),
                field: id("a"),
            }),
        ),
        (
            "statement then tail",
            block(vec![Stmt::Expr(int(1))], Some(int(2))),
        ),
        (
            "two statements",
            block(vec![Stmt::Expr(int(1)), Stmt::Expr(int(2))], None),
        ),
        (
            "let then tail",
            block(vec![let_("v", int(1))], Some(int(2))),
        ),
    ];
    let mut seen: Vec<(DefHash, &str)> = Vec::new();
    for (label, body) in shapes {
        let h = hash_of(vec![func("f", &[], body)], "f");
        if let Some((_, other)) = seen.iter().find(|(prev, _)| *prev == h) {
            panic!("`{label}` collided with `{other}`");
        }
        seen.push((h, label));
    }
}

#[test]
fn string_literals_cannot_be_confused_with_their_neighbours() {
    let a = hash_of(
        vec![func("f", &[], list(vec![str_lit("ab"), str_lit("c")]))],
        "f",
    );
    let b = hash_of(
        vec![func("f", &[], list(vec![str_lit("a"), str_lit("bc")]))],
        "f",
    );
    assert_ne!(a, b);
}

/// The two have different types and must not share a definition, which is the
/// whole reason `LIT_BYTES` is a tag of its own rather than `LIT_STR` reused.
#[test]
fn a_byte_literal_never_hashes_as_the_string_with_the_same_characters() {
    let s = hash_of(vec![func("f", &[], str_lit("ab"))], "f");
    let b = hash_of(vec![func("f", &[], bytes_lit(b"ab"))], "f");
    assert_ne!(s, b);
}

#[test]
fn byte_literals_cannot_be_confused_with_their_neighbours() {
    let joined = hash_of(
        vec![func(
            "f",
            &[],
            list(vec![bytes_lit(b"ab"), bytes_lit(b"c")]),
        )],
        "f",
    );
    let split = hash_of(
        vec![func(
            "f",
            &[],
            list(vec![bytes_lit(b"a"), bytes_lit(b"bc")]),
        )],
        "f",
    );
    assert_ne!(joined, split);

    let empty = hash_of(vec![func("f", &[], bytes_lit(b""))], "f");
    let nul = hash_of(vec![func("f", &[], bytes_lit(b"\0"))], "f");
    assert_ne!(empty, nul);
}

/// A `b"..."` pattern is a `PatternKind::Lit` like any other, so this is really
/// a check that the pattern path reaches the same tag the expression path does.
#[test]
fn a_byte_pattern_is_distinct_from_a_string_pattern() {
    let arms = |l: Lit| {
        vec![func(
            "f",
            &["v"],
            e(ExprKind::Match {
                scrutinee: Box::new(var("v")),
                arms: vec![
                    arm(pat(PatternKind::Lit(l)), int(1)),
                    arm(pat(PatternKind::Wildcard), int(0)),
                ],
            }),
        )]
    };
    assert_ne!(
        hash_of(arms(Lit::Str("ab".to_string())), "f"),
        hash_of(arms(Lit::Bytes(b"ab".to_vec())), "f")
    );
}

#[test]
fn an_absent_annotation_differs_from_a_written_one() {
    let bare = hash_of(vec![func("f", &["x"], var("x"))], "f");
    let annotated = hash_of(
        vec![Item::Fn(Box::new(FnDef {
            vis: Visibility::Private,
            name: id("f"),
            generics: Generics::default(),
            params: vec![typed_param("x", "Int")],
            ret: None,
            effects: None,
            constraints: Vec::new(),
            derived: None,
            spec: Vec::new(),
            body: var("x"),
            span: Span::DUMMY,
        }))],
        "f",
    );
    assert_ne!(bare, annotated);
}

#[test]
fn a_generic_parameter_is_positional_not_named() {
    let generic = |declared: [&str; 2], used: [&str; 2]| {
        vec![Item::Fn(Box::new(FnDef {
            vis: Visibility::Private,
            name: id("f"),
            generics: Generics {
                types: vec![id(declared[0]), id(declared[1])],
                effects: vec![],
            },
            params: vec![typed_param("x", used[0]), typed_param("y", used[1])],
            ret: None,
            effects: None,
            constraints: Vec::new(),
            derived: None,
            spec: Vec::new(),
            body: var("x"),
            span: Span::DUMMY,
        }))]
    };
    assert_eq!(
        hash_of(generic(["a", "b"], ["a", "b"]), "f"),
        hash_of(generic(["s", "t"], ["s", "t"]), "f"),
    );
    assert_ne!(
        hash_of(generic(["a", "b"], ["a", "b"]), "f"),
        hash_of(generic(["a", "b"], ["b", "a"]), "f"),
    );
}

#[test]
fn a_free_type_name_is_not_a_generic_parameter() {
    let concrete = hash_of(
        vec![Item::Fn(Box::new(FnDef {
            vis: Visibility::Private,
            name: id("f"),
            generics: Generics::default(),
            params: vec![typed_param("x", "Int")],
            ret: None,
            effects: None,
            constraints: Vec::new(),
            derived: None,
            spec: Vec::new(),
            body: var("x"),
            span: Span::DUMMY,
        }))],
        "f",
    );
    let generic = hash_of(
        vec![Item::Fn(Box::new(FnDef {
            vis: Visibility::Private,
            name: id("f"),
            generics: Generics {
                types: vec![id("Int")],
                effects: vec![],
            },
            params: vec![typed_param("x", "Int")],
            ret: None,
            effects: None,
            constraints: Vec::new(),
            derived: None,
            spec: Vec::new(),
            body: var("x"),
            span: Span::DUMMY,
        }))],
        "f",
    );
    assert_ne!(concrete, generic);
}

fn handler_program(resource: &str, clause_body: Expr) -> Vec<Item> {
    vec![func(
        "run",
        &[],
        e(ExprKind::WithCell {
            resource: id(resource),
            init: Box::new(int(0)),
            binder: id("cell"),
            body: Box::new(e(ExprKind::Handle {
                body: Box::new(callv("body", vec![])),
                clauses: vec![HandleClause {
                    effect: qn("db"),
                    op: id("get"),
                    resource: Some(id(resource)),
                    params: vec![id("k")],
                    resume: None,
                    body: clause_body,
                    span: Span::DUMMY,
                }],
                return_clause: Some(Box::new(ReturnClause {
                    binder: id("x"),
                    body: var("x"),
                    span: Span::DUMMY,
                })),
            })),
        }),
    )]
}

#[test]
fn handler_clause_parameters_are_de_bruijn_bound() {
    let a = hash_of(handler_program("users", var("k")), "run");
    let renamed = {
        let mut items = handler_program("users", var("k"));
        if let Item::Fn(d) = &mut items[0]
            && let ExprKind::WithCell { body, .. } = &mut d.body.kind
            && let ExprKind::Handle { clauses, .. } = &mut body.kind
        {
            clauses[0].params = vec![id("key")];
            clauses[0].body = var("key");
        }
        hash_of(items, "run")
    };
    assert_eq!(a, renamed);
}

/// ADR 0005, required tests 24 and 25. Omitting the binder from the hash makes
/// a general clause and a tail-resumptive one — two different semantics — share
/// one cache entry, which is the most expensive defect this system has.
#[test]
fn binding_a_continuation_changes_the_hash_and_renaming_the_binder_does_not() {
    let with_binder = |binder: &str| {
        let mut items = handler_program("users", callv(binder, vec![int(1)]));
        if let Item::Fn(d) = &mut items[0]
            && let ExprKind::WithCell { body, .. } = &mut d.body.kind
            && let ExprKind::Handle { clauses, .. } = &mut body.kind
        {
            clauses[0].resume = Some(id(binder));
        }
        hash_of(items, "run")
    };
    let tail_resumptive = hash_of(handler_program("users", callv("k", vec![int(1)])), "run");

    assert_ne!(with_binder("resume"), tail_resumptive);
    assert_eq!(with_binder("resume"), with_binder("kont"));
}

#[test]
fn the_resource_of_a_cell_or_a_clause_is_part_of_the_hash() {
    let users = hash_of(handler_program("users", var("k")), "run");
    let orders = hash_of(handler_program("orders", var("k")), "run");
    assert_ne!(users, orders);
}

#[test]
fn the_operation_of_a_perform_is_part_of_the_hash() {
    let perform = |op: &str, mode_resource: Option<&str>| {
        vec![func(
            "f",
            &["k"],
            e(ExprKind::Perform {
                effect: qn("db"),
                op: id(op),
                resource: mode_resource.map(id),
                args: vec![var("k")],
            }),
        )]
    };
    let get = hash_of(perform("get", Some("users")), "f");
    let put = hash_of(perform("put", Some("users")), "f");
    let bare = hash_of(perform("get", None), "f");
    assert_ne!(get, put);
    assert_ne!(get, bare);
}

#[test]
fn a_nondet_test_differs_from_a_deterministic_one() {
    let body = callv("assert", vec![e(ExprKind::Lit(Lit::Bool(true)))]);
    let det = hashes(vec![test_item("t", body.clone())]);
    let nondet = hashes(vec![Item::Test(Box::new(TestDef {
        name: "t".to_string(),
        name_span: Span::DUMMY,
        nondet: true,
        body,
        span: Span::DUMMY,
    }))]);
    assert_ne!(det.tests[0], nondet.tests[0]);
}

#[test]
fn a_tests_name_is_not_part_of_its_identity() {
    let body = callv("assert", vec![e(ExprKind::Lit(Lit::Bool(true)))]);
    let a = hashes(vec![test_item("first name", body.clone())]);
    let b = hashes(vec![test_item("second name", body)]);
    assert_eq!(a.tests[0], b.tests[0]);
}

#[test]
fn cycle_members_that_differ_get_distinct_hashes() {
    let out = hashes(vec![
        func("ping", &["n"], callv("pong", vec![var("n")])),
        func(
            "pong",
            &["n"],
            callv("ping", vec![bin(BinOp::Sub, var("n"), int(1))]),
        ),
    ]);
    assert_ne!(
        out.defs[&Symbol::new("ping")],
        out.defs[&Symbol::new("pong")]
    );
}

#[test]
fn a_self_recursive_definition_is_not_hashed_like_a_call_to_a_twin() {
    let recursive = hash_of(
        vec![func(
            "loop_forever",
            &["n"],
            callv("loop_forever", vec![var("n")]),
        )],
        "loop_forever",
    );
    let delegating = hashes(vec![
        func("a", &["n"], callv("b", vec![var("n")])),
        func("b", &["n"], var("n")),
    ]);
    assert_ne!(recursive, delegating.defs[&Symbol::new("a")]);
}

#[test]
fn a_cycle_hash_is_stable_across_runs() {
    let program = || {
        vec![
            func("ping", &["n"], callv("pong", vec![var("n")])),
            func(
                "pong",
                &["n"],
                callv("ping", vec![bin(BinOp::Sub, var("n"), int(1))]),
            ),
        ]
    };
    assert_eq!(hashes(program()).defs, hashes(program()).defs);
}

#[test]
fn a_recursive_type_hashes_and_is_rename_invariant() {
    let program = |name: &str| {
        vec![
            Item::Type(Box::new(TypeDef {
                vis: Visibility::Private,
                name: id(name),
                params: vec![],
                body: TypeDefBody::Sum(vec![
                    VariantDef {
                        name: id("Nil"),
                        fields: vec![],
                        span: Span::DUMMY,
                    },
                    VariantDef {
                        name: id("Cons"),
                        fields: vec![ty_con("Int", vec![]), ty_con(name, vec![])],
                        span: Span::DUMMY,
                    },
                ]),
                span: Span::DUMMY,
            })),
            func("empty", &[], var("Nil")),
        ]
    };
    assert_eq!(
        hash_of(program("IntList"), "empty"),
        hash_of(program("Ints"), "empty")
    );
}

#[test]
fn deps_are_direct_and_closure_is_transitive() {
    let out = hashes(chain("a", "b", "c", "d"));
    assert_eq!(out.deps[&Symbol::new("a")], vec![Symbol::new("b")]);
    assert_eq!(out.deps[&Symbol::new("d")], Vec::<Symbol>::new());

    let closure_of_a: Vec<String> = out.closure[&Symbol::new("a")]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(closure_of_a, vec!["a", "b", "c", "d"]);
    let closure_of_c: Vec<String> = out.closure[&Symbol::new("c")]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(closure_of_c, vec!["c", "d"]);
}

#[test]
fn a_tests_closure_covers_everything_it_reaches() {
    let out = hashes(chain("a", "b", "c", "d"));
    let closure: Vec<String> = out.closure[&Symbol::new("the chain adds up")]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(closure, vec!["a", "b", "c", "d", "the chain adds up"]);
}

#[test]
fn a_cycle_closure_contains_every_member() {
    let out = hashes(vec![
        func("ping", &["n"], callv("pong", vec![var("n")])),
        func("pong", &["n"], callv("ping", vec![var("n")])),
        func("caller", &[], callv("ping", vec![int(0)])),
    ]);
    let ping: Vec<String> = out.closure[&Symbol::new("ping")]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(ping, vec!["ping", "pong"]);
    let caller: Vec<String> = out.closure[&Symbol::new("caller")]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(caller, vec!["caller", "ping", "pong"]);
}

#[test]
fn builtins_and_unknown_names_are_not_dependencies() {
    let out = hashes(vec![func(
        "f",
        &["x"],
        callv("assert_eq", vec![var("x"), int(1)]),
    )]);
    assert_eq!(out.deps[&Symbol::new("f")], Vec::<Symbol>::new());
    let closure: Vec<String> = out.closure[&Symbol::new("f")]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(closure, vec!["f"]);
}

#[test]
fn deps_include_the_types_and_effects_a_definition_mentions() {
    let out = hashes(vec![
        Item::Type(Box::new(TypeDef {
            vis: Visibility::Private,
            name: id("Status"),
            params: vec![],
            body: TypeDefBody::Sum(vec![VariantDef {
                name: id("Active"),
                fields: vec![],
                span: Span::DUMMY,
            }]),
            span: Span::DUMMY,
        })),
        Item::Effect(Box::new(EffectDef {
            vis: Visibility::Private,
            name: id("db"),
            nondet: false,
            ops: vec![OpDef {
                name: id("get"),
                mode: Mode::Read,
                resource_param: true,
                params: vec![],
                ret: ty_con("Int", vec![]),
                span: Span::DUMMY,
            }],
            span: Span::DUMMY,
        })),
        func(
            "f",
            &[],
            block(
                vec![Stmt::Expr(e(ExprKind::Perform {
                    effect: qn("db"),
                    op: id("get"),
                    resource: Some(id("users")),
                    args: vec![],
                }))],
                Some(var("Active")),
            ),
        ),
    ]);
    let deps: Vec<String> = out.deps[&Symbol::new("f")]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(deps, vec!["db", "Status"]);
    assert!(!out.defs.contains_key(&Symbol::new("Status")));
    assert!(out.decls.contains_key(&Symbol::new("Status")));
    assert!(out.decls.contains_key(&Symbol::new("db")));
    assert!(!out.decls.contains_key(&Symbol::new("f")));
}

/// A declaration is content-addressed like anything else, so renaming a type
/// moves nothing and changing its shape moves it.
#[test]
fn declaration_hashes_follow_structure_not_names() {
    let program = |name: &str, extra: Vec<VariantDef>| {
        let mut variants = vec![VariantDef {
            name: id("Active"),
            fields: vec![],
            span: Span::DUMMY,
        }];
        variants.extend(extra);
        vec![Item::Type(Box::new(TypeDef {
            vis: Visibility::Private,
            name: id(name),
            params: vec![],
            body: TypeDefBody::Sum(variants),
            span: Span::DUMMY,
        }))]
    };
    let status = hashes(program("Status", vec![]));
    let renamed = hashes(program("State", vec![]));
    assert_eq!(
        status.decls[&Symbol::new("Status")],
        renamed.decls[&Symbol::new("State")]
    );

    let widened = hashes(program(
        "Status",
        vec![VariantDef {
            name: id("Banned"),
            fields: vec![],
            span: Span::DUMMY,
        }],
    ));
    assert_ne!(
        status.decls[&Symbol::new("Status")],
        widened.decls[&Symbol::new("Status")]
    );
}

#[test]
fn duplicate_definitions_are_reported_rather_than_silently_merged() {
    let err = hash_ast(&module(vec![
        func("f", &[], int(1)),
        func("f", &[], int(2)),
        func("g", &[], int(3)),
    ]))
    .expect_err("a duplicate definition must be an error");
    assert_eq!(err.len(), 1);
    assert_eq!(err[0].code, ply_span::codes::DUPLICATE_DEFINITION);
    assert!(err[0].message.contains('f'));
    assert!(err[0].primary_span().is_some());
}

#[test]
fn duplicate_variants_across_types_are_reported() {
    let sum = |ty: &str, variant: &str| {
        Item::Type(Box::new(TypeDef {
            vis: Visibility::Private,
            name: id(ty),
            params: vec![],
            body: TypeDefBody::Sum(vec![VariantDef {
                name: id(variant),
                fields: vec![],
                span: Span::DUMMY,
            }]),
            span: Span::DUMMY,
        }))
    };
    let err = hash_ast(&module(vec![sum("A", "Same"), sum("B", "Same")])).expect_err("duplicate");
    assert_eq!(err[0].code, ply_span::codes::DUPLICATE_DEFINITION);
    assert!(err[0].message.contains("variant"));
}

#[test]
fn hex_round_trips_and_short_is_a_prefix() {
    let h = hash_of(vec![func("f", &[], int(1))], "f");
    assert_eq!(h.to_hex().len(), 64);
    assert_eq!(h.short().len(), 12);
    assert!(h.to_hex().starts_with(&h.short()));
    assert_eq!(DefHash::from_hex(&h.to_hex()), Some(h));
    assert_eq!(h.to_string(), h.short());
    assert_eq!(DefHash::from_hex("nonsense"), None);
    assert_eq!(DefHash::from_hex(&"z".repeat(64)), None);
}

#[test]
fn a_hash_serializes_as_a_hex_string() {
    let h = hash_of(vec![func("f", &[], int(1))], "f");
    let json = serde_json::to_string(&h).unwrap();
    assert_eq!(json, format!("\"{}\"", h.to_hex()));
    assert_eq!(serde_json::from_str::<DefHash>(&json).unwrap(), h);
    assert!(serde_json::from_str::<DefHash>("\"short\"").is_err());
}

#[test]
fn an_empty_module_hashes_to_nothing() {
    let out = hash_ast(&module(vec![])).unwrap();
    assert!(out.defs.is_empty());
    assert!(out.decls.is_empty());
    assert!(out.tests.is_empty());
    assert!(out.closure.is_empty());
}

// ---- the same properties, over real parsed source ----

fn parsed(source: &str) -> HashOutput {
    let module = match ply_syntax::parse(ply_span::SourceId(0), source) {
        Ok(m) => m,
        Err(diags) => panic!("source did not parse: {diags:#?}"),
    };
    hash_ast(&module).expect("parsed module should hash")
}

const LEDGER: &str = r#"
fn apply_debit(balance: Int, amount: Int) -> Int = balance - amount

fn settle(balance: Int, amount: Int) -> Int = apply_debit(balance, amount)

fn close_out(balance: Int) -> Int = settle(balance, balance)

fn report(balance: Int) -> Int = close_out(balance) + 1

test "the ledger closes out" {
  assert_eq(report(10), 1)
}
"#;

#[test]
fn parsed_rename_through_three_intermediaries_changes_no_hash() {
    let before = parsed(LEDGER);
    let after = parsed(&LEDGER.replace("apply_debit", "post_debit_entry"));
    assert_eq!(
        before.defs[&Symbol::new("apply_debit")],
        after.defs[&Symbol::new("post_debit_entry")]
    );
    for name in ["settle", "close_out", "report"] {
        assert_eq!(
            before.defs[&Symbol::new(name)],
            after.defs[&Symbol::new(name)],
            "{name}"
        );
    }
    assert_eq!(before.tests, after.tests);
}

#[test]
fn parsed_reformatting_and_recommenting_change_no_hash() {
    let reformatted = r#"
// A comment nobody reads.

fn apply_debit(balance: Int, amount: Int) -> Int =
    balance - amount   // trailing note

fn settle(balance: Int, amount: Int) -> Int = {
    apply_debit(balance, amount)
}

fn close_out(balance: Int) -> Int
  = settle(balance,
           balance)

fn report(balance: Int) -> Int = close_out(balance)
    + 1

test "the ledger closes out" { assert_eq(report(10), 1) }
"#;
    assert_eq!(parsed(LEDGER).defs, parsed(reformatted).defs);
    assert_eq!(parsed(LEDGER).tests, parsed(reformatted).tests);
}

#[test]
fn parsed_reordering_items_changes_no_hash() {
    let reordered = r#"
test "the ledger closes out" {
  assert_eq(report(10), 1)
}

fn report(balance: Int) -> Int = close_out(balance) + 1

fn close_out(balance: Int) -> Int = settle(balance, balance)

fn settle(balance: Int, amount: Int) -> Int = apply_debit(balance, amount)

fn apply_debit(balance: Int, amount: Int) -> Int = balance - amount
"#;
    let before = parsed(LEDGER);
    let after = parsed(reordered);
    for name in ["apply_debit", "settle", "close_out", "report"] {
        assert_eq!(
            before.defs[&Symbol::new(name)],
            after.defs[&Symbol::new(name)],
            "{name}"
        );
    }
    assert_eq!(before.tests, after.tests);
}

#[test]
fn parsed_renaming_locals_changes_no_hash() {
    let renamed = LEDGER.replace("balance", "amt").replace("amount", "delta");
    assert_eq!(parsed(LEDGER).defs, parsed(&renamed).defs);
}

#[test]
fn parsed_editing_a_leaf_changes_it_and_every_dependent() {
    let before = parsed(LEDGER);
    let after = parsed(&LEDGER.replace("balance - amount", "balance - amount - 1"));
    for name in ["apply_debit", "settle", "close_out", "report"] {
        assert_ne!(
            before.defs[&Symbol::new(name)],
            after.defs[&Symbol::new(name)],
            "{name}"
        );
    }
    assert_ne!(before.tests[0], after.tests[0]);
}

#[test]
fn parsed_structurally_identical_definitions_share_a_hash() {
    let out = parsed(
        r#"
fn increment(x: Int) -> Int = x + 1
fn succ(n: Int) -> Int = n + 1
fn plus_two(n: Int) -> Int = n + 2
"#,
    );
    assert_eq!(
        out.defs[&Symbol::new("increment")],
        out.defs[&Symbol::new("succ")]
    );
    assert_ne!(
        out.defs[&Symbol::new("increment")],
        out.defs[&Symbol::new("plus_two")]
    );
}

#[test]
fn parsed_swapping_arguments_or_arms_changes_the_hash() {
    let base = parsed(
        "fn f(a: Int, b: Int) -> Int = apply(a, b)\nfn apply(x: Int, y: Int) -> Int = x - y",
    );
    let swapped = parsed(
        "fn f(a: Int, b: Int) -> Int = apply(b, a)\nfn apply(x: Int, y: Int) -> Int = x - y",
    );
    assert_ne!(
        base.defs[&Symbol::new("f")],
        swapped.defs[&Symbol::new("f")]
    );

    let arms = |first: &str, second: &str| {
        parsed(&format!(
            "fn f(n: Int) -> Int = match n {{ {first} -> 1, {second} -> 2, _ -> 3 }}"
        ))
    };
    assert_ne!(
        arms("0", "1").defs[&Symbol::new("f")],
        arms("1", "0").defs[&Symbol::new("f")]
    );
}

#[test]
fn parsed_effects_and_handlers_hash_by_structure() {
    let source = |effect: &str, helper: &str| {
        format!(
            r#"
effect {effect} {{
  read get[r](key: Int) -> Int
  write put[r](key: Int, value: Int) -> Unit
}}

fn {helper}(k: Int) -> Int / {{{effect}.read[users]}} = {effect}.get[users](k)

test "reads are handled" {{
  with_cell[users](0) {{ c ->
    handle assert_eq({helper}(1), 0) with {{
      {effect}.get[users](k) -> cell_get(c),
    }}
  }}
}}
"#
        )
    };
    let before = parsed(&source("db", "lookup"));
    let after = parsed(&source("store", "fetch"));
    assert_eq!(
        before.defs[&Symbol::new("lookup")],
        after.defs[&Symbol::new("fetch")]
    );
    assert_eq!(before.tests, after.tests);
}

// ---- effects are nominal, and `let` order is not always meaningful ----

const LOOK_ALIKES: &str = "effect {a} {\n  write emit[r](v: Int) -> Int\n}\n\
                           effect {b} {\n  write emit[r](v: Int) -> Int\n}\n\
                           fn f(v: Int) -> Int / {{eff}.write[log]} = {eff}.emit[log](v)";

fn look_alikes(a: &str, b: &str, eff: &str) -> HashOutput {
    parsed(
        &LOOK_ALIKES
            .replace("{a}", a)
            .replace("{b}", b)
            .replace("{eff}", eff),
    )
}

/// A definition that performs one of two byte-identical effects and one that
/// performs the other differ by a consistent renaming of the two and by nothing
/// else, so they are one definition. Nothing local can separate them — that is
/// what "byte-identical declaration" means — and the alternatives all cost more
/// than they buy: a rank over the program's names makes adding an unrelated
/// module renumber existing hashes, and a rank over source position makes moving
/// an item do the same.
///
/// It stays sound because the moment anything *pins* one of the two, that thing
/// records which slot it meant; see [`a_handler_records_which_look_alike_it_discharges`].
#[test]
fn performing_either_of_two_identically_declared_effects_is_one_definition() {
    assert_eq!(
        look_alikes("db", "audit", "db").defs[&Symbol::new("f")],
        look_alikes("db", "audit", "audit").defs[&Symbol::new("f")],
    );
}

/// The separating context. `f` performs the first declared effect and `g` the
/// second; a handler for one discharges exactly one of them, and which one it
/// picked is part of its identity.
#[test]
fn a_handler_records_which_look_alike_it_discharges() {
    let source = |handled: &str| {
        format!(
            "effect db {{\n  write emit[r](v: Int) -> Int\n}}\n\
             effect audit {{\n  write emit[r](v: Int) -> Int\n}}\n\
             fn f(v: Int) -> Int / {{db.write[log]}} = db.emit[log](v)\n\
             fn g(v: Int) -> Int / {{audit.write[log]}} = audit.emit[log](v)\n\
             fn caught(v: Int) -> Int / {{audit.write[log], db.write[log]}} =\n\
               handle f(v) + g(v) with {{ {handled}.emit[log](x) -> x, }}\n"
        )
    };
    assert_ne!(
        parsed(&source("db")).defs[&Symbol::new("caught")],
        parsed(&source("audit")).defs[&Symbol::new("caught")],
    );
}

/// Renaming an effect is free even when the program holds a second one declared
/// exactly like it, and free in either direction: the pair below is renamed so
/// that the two swap places in every name ordering.
#[test]
fn renaming_look_alike_effects_changes_no_hash() {
    let f = |a: &str, b: &str, eff: &str| look_alikes(a, b, eff).defs[&Symbol::new("f")];
    assert_eq!(f("audit", "db", "audit"), f("alerts", "cache", "alerts"));
    assert_eq!(f("audit", "db", "audit"), f("zebra", "alpha", "zebra"));
}

#[test]
fn reordering_two_let_bindings_that_perform_changes_the_hash() {
    let program = |first: &str, second: &str| {
        format!(
            "effect db {{\n  read get[r](k: Int) -> Int\n  write put[r](k: Int, v: Int) -> Int\n}}\n\
             fn f() -> Int / {{db.read[users], db.write[users]}} = {{\n\
             \x20 let {first};\n\
             \x20 let {second};\n\
             \x20 a + b\n\
             }}"
        )
    };
    let read = "a = db.get[users](1)";
    let write = "b = db.put[users](1, 2)";
    assert_ne!(
        parsed(&program(read, write)).defs[&Symbol::new("f")],
        parsed(&program(write, read)).defs[&Symbol::new("f")],
    );
}

#[test]
fn a_let_that_shadows_what_the_next_one_reads_is_not_reordered() {
    let program = |first: &str, second: &str| {
        format!("fn f(a: Int) -> Int = {{ let {first}; let {second}; x + a }}")
    };
    assert_ne!(
        parsed(&program("x = a + 1", "a = 7")).defs[&Symbol::new("f")],
        parsed(&program("a = 7", "x = a + 1")).defs[&Symbol::new("f")],
    );
}

#[test]
fn two_let_bindings_of_the_same_name_are_not_reordered() {
    let program = |first: i64, second: i64| {
        format!("fn f() -> Int = {{ let a = {first}; let a = {second}; a }}")
    };
    assert_ne!(
        parsed(&program(1, 2)).defs[&Symbol::new("f")],
        parsed(&program(2, 1)).defs[&Symbol::new("f")]
    );
}

/// A dependency chain far deeper than any call stack Tarjan could afford to use.
#[test]
fn a_very_deep_dependency_chain_does_not_overflow_the_stack() {
    const DEPTH: usize = 2000;
    let mut items = vec![func("d0", &["n"], var("n"))];
    for i in 1..DEPTH {
        items.push(func(
            &format!("d{i}"),
            &["n"],
            callv(&format!("d{}", i - 1), vec![var("n")]),
        ));
    }
    let out = hashes(items);
    assert_eq!(out.defs.len(), DEPTH);
    assert_eq!(
        out.closure[&Symbol::new(format!("d{}", DEPTH - 1))].len(),
        DEPTH
    );
    let unique: BTreeSet<DefHash> = out.defs.values().copied().collect();
    assert_eq!(unique.len(), DEPTH);
}

#[test]
fn tarjan_emits_dependencies_before_dependents() {
    let edges = vec![vec![graph::NodeId(1)], vec![graph::NodeId(2)], vec![]];
    assert_eq!(graph::tarjan(3, &edges), vec![vec![2], vec![1], vec![0]]);
}

#[test]
fn tarjan_groups_cycles_and_orders_them_before_their_dependents() {
    // 0 -> 1 -> 2 -> 0, 3 -> 0, 4 alone.
    let edges = vec![
        vec![graph::NodeId(1)],
        vec![graph::NodeId(2)],
        vec![graph::NodeId(0)],
        vec![graph::NodeId(0)],
        vec![],
    ];
    let components = graph::tarjan(5, &edges);
    let cycle = components
        .iter()
        .position(|c| c.len() == 3)
        .expect("the cycle is one component");
    let dependent = components
        .iter()
        .position(|c| c == &[3])
        .expect("3 stands alone");
    assert!(cycle < dependent);
    let mut members = components[cycle].clone();
    members.sort_unstable();
    assert_eq!(members, vec![0, 1, 2]);
    assert!(components.iter().any(|c| c == &[4]));
    assert!(graph::is_cyclic(&components[cycle], &edges));
    assert!(!graph::is_cyclic(&[3], &edges));
    assert!(graph::is_cyclic(&[0], &[vec![graph::NodeId(0)]]));
}

/// The contract entry point, driven the way the CLI drives it.
#[test]
fn hash_module_agrees_with_hash_ast() {
    let source = r#"
fn double(n: Int) -> Int = n * 2

test "double doubles" {
  assert_eq(double(2), 4)
}
"#;
    let parsed_module = ply_syntax::parse(ply_span::SourceId(0), source).expect("parses");
    let check = match ply_core::check_module(&parsed_module) {
        Ok(check) => check,
        Err(diags) => panic!("source did not typecheck: {diags:#?}"),
    };
    let out = hash_module(&parsed_module, &check).expect("hashes");
    assert_eq!(out.defs.len(), 1);
    assert_eq!(out.tests.len(), check.tests.len());
    assert_eq!(out, hash_ast(&parsed_module).unwrap());
}

/// A spec is a claim *about* a definition, so its key has to move when the
/// definition does. A key that omitted the owner would leave a discharged
/// `ensures` discharged after its body was rewritten — a cached proof of
/// something no longer true, which is the permissive-direction failure M8 must
/// not ship.
#[test]
fn a_spec_key_moves_when_its_definition_does() {
    let owner = DefHash([1; 32]);
    let rewritten = DefHash([2; 32]);
    let clause = b"normalized";
    assert_ne!(
        spec_hash(owner, SpecKind::Ensures, 0, clause),
        spec_hash(rewritten, SpecKind::Ensures, 0, clause),
    );
}

/// The clause's own structure, its position among its siblings, and which kind
/// of clause it is are all part of what is being claimed, so each separates two
/// obligations. Reordering two `ensures` clauses therefore re-runs both, which
/// is correct: the index is what names them.
#[test]
fn a_spec_key_separates_the_clause_from_its_siblings() {
    let owner = DefHash([1; 32]);
    let base = spec_hash(owner, SpecKind::Ensures, 0, b"normalized");
    assert_ne!(base, spec_hash(owner, SpecKind::Ensures, 1, b"normalized"));
    assert_ne!(base, spec_hash(owner, SpecKind::Requires, 0, b"normalized"));
    assert_ne!(base, spec_hash(owner, SpecKind::Ensures, 0, b"other"));
}

/// Domain-tagged, so no clause can ever produce the hash of some definition's
/// own normalized bytes and read a test's result as its own.
#[test]
fn a_spec_key_cannot_collide_with_a_definition_hash() {
    let owner = DefHash([1; 32]);
    assert_ne!(spec_hash(owner, SpecKind::Ensures, 0, b""), owner);
    assert_ne!(
        spec_hash(owner, SpecKind::Ensures, 0, b"x"),
        DefHash::of(b"x")
    );
}

/// A spec is a claim *about* a definition, not part of it, so the normalizer
/// erases it exactly as it erases names, spans and `pub`. Writing one therefore
/// changes no definition hash, moves no test hash, and rebuilds nothing — the
/// same sentence as "renaming a function selects zero tests", and true for the
/// same reason.
#[test]
fn writing_a_spec_or_a_law_changes_no_definition_hash_and_no_test_hash() {
    const BARE: &str = r#"
fn apply_debit(balance: Int, amount: Int) -> Int = balance - amount

fn settle(balance: Int, amount: Int) -> Int = apply_debit(balance, amount)

test "the ledger settles" {
  assert_eq(settle(10, 3), 7)
}
"#;
    let bare = parsed(BARE);
    assert_eq!(bare.defs.len(), 2);
    assert_eq!(bare.tests.len(), 1);

    for variant in [
        // added
        r#"
fn apply_debit(balance: Int, amount: Int) -> Int
  requires amount > 0
= balance - amount

fn settle(balance: Int, amount: Int) -> Int = apply_debit(balance, amount)

test "the ledger settles" {
  assert_eq(settle(10, 3), 7)
}
"#,
        // edited, and a second clause on a second definition
        r#"
fn apply_debit(balance: Int, amount: Int) -> Int
  requires amount > 0
  ensures result <= balance
= balance - amount

fn settle(balance: Int, amount: Int) -> Int
  ensures result <= balance
= apply_debit(balance, amount)

test "the ledger settles" {
  assert_eq(settle(10, 3), 7)
}
"#,
        // a standalone law naming both definitions
        r#"
fn apply_debit(balance: Int, amount: Int) -> Int = balance - amount

fn settle(balance: Int, amount: Int) -> Int = apply_debit(balance, amount)

law "settling is a debit"
  forall (b: Int, n: Int) where n > 0 {
    settle(b, n) == apply_debit(b, n)
  }

test "the ledger settles" {
  assert_eq(settle(10, 3), 7)
}
"#,
    ] {
        let with_spec = parsed(variant);
        assert_eq!(with_spec.defs, bare.defs, "definition hashes moved");
        assert_eq!(with_spec.decls, bare.decls);
        assert_eq!(with_spec.tests, bare.tests, "a test would be re-run");

        // A law is an item of its own, so it brings its own reference entry —
        // which is what `Laws::of` reads to decide the definitions it covers.
        // What must not move is any entry that was already there: a law
        // constrains what it names without becoming part of it.
        for (name, deps) in &bare.deps {
            assert_eq!(with_spec.deps.get(name), Some(deps), "`{name}` moved");
            assert_eq!(with_spec.closure.get(name), bare.closure.get(name));
        }
        let added: Vec<&str> = with_spec
            .deps
            .keys()
            .filter(|name| !bare.deps.contains_key(*name))
            .map(|name| name.as_str())
            .collect();
        let expected: Vec<&str> = if variant.contains("law ") {
            vec!["settling is a debit"]
        } else {
            Vec::new()
        };
        assert_eq!(added, expected);
    }
}

/// The claim gets its own hash, which covers the definition's — so editing an
/// implementation re-opens its obligations, while editing the claim moves
/// nothing at all. That asymmetry is exactly the asymmetry review has.
#[test]
fn an_obligation_key_covers_the_implementation_and_the_clause() {
    const SOURCE: &str = r#"
fn apply_debit(balance: Int, amount: Int) -> Int
  requires amount > 0
  ensures result <= balance
= balance - amount
"#;
    let base = parsed(SOURCE);
    let clauses = base.specs[&Symbol::new("apply_debit")].clone();
    assert_eq!(clauses.len(), 2, "one key per clause, requires included");
    assert_ne!(clauses[0], clauses[1]);

    // The implementation moved: every obligation on it re-opens. A key that
    // omitted the owner's hash would leave a discharged `ensures` discharged
    // after its definition was rewritten.
    let rewritten = parsed(&SOURCE.replace("balance - amount", "balance - (amount + 0)"));
    assert_ne!(
        rewritten.defs[&Symbol::new("apply_debit")],
        base.defs[&Symbol::new("apply_debit")]
    );
    assert_ne!(rewritten.specs[&Symbol::new("apply_debit")], clauses);

    // The claim moved and the implementation did not: the definition's hash is
    // untouched, and only the clause that changed gets a new key.
    let restated = parsed(&SOURCE.replace("result <= balance", "result - balance <= 0"));
    assert_eq!(restated.defs, base.defs);
    assert_eq!(restated.specs[&Symbol::new("apply_debit")][0], clauses[0]);
    assert_ne!(restated.specs[&Symbol::new("apply_debit")][1], clauses[1]);

    // The index is in the key, so reordering two clauses re-opens both.
    let reordered = parsed(
        r#"
fn apply_debit(balance: Int, amount: Int) -> Int
  ensures result <= balance
  requires amount > 0
= balance - amount
"#,
    );
    assert_eq!(reordered.defs, base.defs);
    assert!(
        reordered.specs[&Symbol::new("apply_debit")]
            .iter()
            .all(|k| !clauses.contains(k))
    );
}

/// The complement of the key above, and the whole of what `ply review` asks: a
/// claim's *sentence* moves when the sentence is rewritten and stays put when
/// the implementation under it is.
///
/// Both halves matter and they fail in opposite directions. If a sentence moved
/// with its implementation, ADR 0007 §9.2's *implementation changed · spec
/// unchanged* row — the cheapest review in the system — would be unreachable and
/// every body edit would send a reviewer back to the diff. If it did not move
/// when the claim was rewritten, a rewritten claim would report "spec unchanged"
/// and the reviewer would never read it.
#[test]
fn a_claims_sentence_moves_with_the_claim_and_not_with_the_implementation() {
    const SOURCE: &str = r#"
fn apply_debit(balance: Int, amount: Int) -> Int
  requires amount > 0
  ensures result <= balance
= balance - amount

law "a debit never raises the balance"
  forall (b: Int, n: Int) where n > 0 {
    apply_debit(b, n) <= b
  }
"#;
    let base = parsed(SOURCE);
    let name = Symbol::new("apply_debit");
    let sentences = base.spec_texts[&name].clone();
    assert_eq!(sentences.len(), 2);
    assert_ne!(sentences[0], sentences[1]);
    assert_eq!(base.law_texts.len(), 1);

    // The implementation moved. Every obligation *key* re-opens — that is the
    // test above — and no sentence moves, because neither the clauses nor the
    // law were rewritten.
    let rewritten = parsed(&SOURCE.replace("balance - amount", "balance - (amount + 0)"));
    assert_ne!(rewritten.defs[&name], base.defs[&name]);
    assert_ne!(rewritten.specs[&name], base.specs[&name]);
    assert_ne!(
        rewritten.laws, base.laws,
        "the law's key covers what it names"
    );
    assert_eq!(rewritten.spec_texts[&name], sentences);
    assert_eq!(rewritten.law_texts, base.law_texts);

    // The claim moved: exactly the rewritten clause's sentence moves.
    let restated = parsed(&SOURCE.replace("result <= balance", "result - balance <= 0"));
    assert_eq!(restated.defs, base.defs);
    assert_eq!(restated.spec_texts[&name][0], sentences[0]);
    assert_ne!(restated.spec_texts[&name][1], sentences[1]);
    assert_eq!(restated.law_texts, base.law_texts);

    // The law moved, and nothing else did.
    let relawed = parsed(&SOURCE.replace("where n > 0", "where n > 1"));
    assert_eq!(relawed.defs, base.defs);
    assert_eq!(relawed.spec_texts[&name], sentences);
    assert_ne!(relawed.law_texts, base.law_texts);

    // A sentence is still a normalized encoding: reformatting is not a rewrite.
    let reformatted = parsed(&SOURCE.replace("result <= balance", "result   <=   balance"));
    assert_eq!(reformatted.spec_texts[&name], sentences);
    assert_eq!(reformatted.law_texts, base.law_texts);
}

/// A law's identity is its binders, guard and body against the *hashes* of what
/// it names — never their names, and never its own label.
#[test]
fn a_law_is_hashed_by_what_it_claims_rather_than_by_what_it_is_called() {
    const SOURCE: &str = r#"
fn debit(balance: Int, amount: Int) -> Int = balance - amount

law "a debit lowers a balance"
  forall (b: Int, n: Int) where n > 0 {
    debit(b, n) <= b
  }
"#;
    let base = parsed(SOURCE);
    assert_eq!(base.laws.len(), 1);

    let relabelled = parsed(&SOURCE.replace("a debit lowers a balance", "debits lower balances"));
    assert_eq!(
        relabelled.laws, base.laws,
        "a law is labelled, not named: the label is namespace metadata"
    );

    let renamed = parsed(&SOURCE.replace("debit", "subtract"));
    assert_eq!(
        renamed.laws, base.laws,
        "renaming a definition a law names changes no law hash"
    );

    let edited = parsed(&SOURCE.replace("balance - amount", "balance - amount - 0"));
    assert_ne!(
        edited.laws, base.laws,
        "editing a definition a law names must re-open the law"
    );

    let rebound = parsed(
        &SOURCE
            .replace("forall (b: Int, n: Int)", "forall (x: Int, n: Int)")
            .replace("debit(b, n) <= b", "debit(x, n) <= x"),
    );
    assert_eq!(rebound.laws, base.laws, "a binder is a level, not a name");
}

// ---- `where derivable(D, a)` is part of the published signature ----

fn constrained(param: &str, constraints: &[(Deriver, &str)]) -> Vec<Item> {
    vec![Item::Fn(Box::new(FnDef {
        vis: Visibility::Private,
        name: id("f"),
        generics: Generics {
            types: vec![id(param)],
            effects: Vec::new(),
        },
        params: vec![param_of("x", TypeExpr::Var(id(param)))],
        ret: Some(TypeExpr::Var(id(param))),
        effects: None,
        constraints: constraints
            .iter()
            .map(|(deriver, on)| Constraint {
                deriver: *deriver,
                deriver_span: Span::DUMMY,
                param: id(on),
                span: Span::DUMMY,
            })
            .collect(),
        derived: None,
        spec: Vec::new(),
        body: var("x"),
        span: Span::DUMMY,
    }))]
}

fn param_of(name: &str, ty: TypeExpr) -> Param {
    Param {
        name: id(name),
        ty: Some(ty),
        span: Span::DUMMY,
    }
}

/// Not a taste call. Gate 2 rechecks a definition only when its own hash moved,
/// and a caller's hash moves only when a callee's does — so an erased constraint
/// would leave a caller accepted against a signature that no longer admits it.
#[test]
fn adding_a_constraint_changes_the_definition_hash() {
    let bare = hash_of(constrained("a", &[]), "f");
    let json = hash_of(constrained("a", &[(Deriver::Json, "a")]), "f");
    let ord = hash_of(constrained("a", &[(Deriver::Ord, "a")]), "f");

    assert_ne!(
        bare, json,
        "a constraint is part of the published signature"
    );
    assert_ne!(json, ord, "which deriver is constrained is part of it too");
}

#[test]
fn reordering_or_repeating_constraints_changes_no_hash() {
    let one = hash_of(
        constrained("a", &[(Deriver::Json, "a"), (Deriver::Ord, "a")]),
        "f",
    );
    let other = hash_of(
        constrained("a", &[(Deriver::Ord, "a"), (Deriver::Json, "a")]),
        "f",
    );
    let repeated = hash_of(
        constrained(
            "a",
            &[
                (Deriver::Ord, "a"),
                (Deriver::Json, "a"),
                (Deriver::Ord, "a"),
            ],
        ),
        "f",
    );

    assert_eq!(one, other, "constraints are a set, not a sequence");
    assert_eq!(one, repeated, "a repeated constraint says nothing new");
}

#[test]
fn renaming_a_constrained_type_parameter_changes_no_hash() {
    let a = hash_of(constrained("a", &[(Deriver::Json, "a")]), "f");
    let elem = hash_of(constrained("elem", &[(Deriver::Json, "elem")]), "f");
    assert_eq!(a, elem, "a type parameter is a level, not a name");
}

/// A constraint the signature does not bind is an error the checker reports, and
/// a hash may not depend on a name the definition cannot reach.
#[test]
fn a_constraint_on_an_unbound_parameter_contributes_nothing() {
    let bare = hash_of(constrained("a", &[]), "f");
    let dangling = hash_of(constrained("a", &[(Deriver::Json, "b")]), "f");
    assert_eq!(bare, dangling);
}

/// The decoder's half. A constraint is in the byte stream, so a body that lost
/// one would decode into a definition with a different hash than the key it is
/// filed under — which is the one thing a body may never do.
#[test]
fn a_constraint_survives_a_body_round_trip() {
    let items = constrained("a", &[(Deriver::Ord, "a"), (Deriver::Json, "a")]);
    let (hashes, bodies) = crate::hash_ast_with_bodies(&module(items)).expect("module should hash");
    let rebuilt = crate::body::reconstruct(&bodies).expect("bodies should reconstruct");

    let f = rebuilt
        .program
        .modules
        .iter()
        .flat_map(|m| &m.items)
        .find_map(|item| match item {
            Item::Fn(d) => Some(d),
            _ => None,
        })
        .expect("the definition should come back");
    assert_eq!(
        f.constraints.iter().map(|c| c.deriver).collect::<Vec<_>>(),
        [Deriver::Json, Deriver::Ord],
        "constraints come back sorted, which is how they were written down"
    );

    let (again, _) = crate::hash_ast_with_bodies(&rebuilt.program.modules[0].clone())
        .expect("rebuilt module should hash");
    assert_eq!(
        again.defs.values().collect::<Vec<_>>(),
        hashes.defs.values().collect::<Vec<_>>(),
        "a decoded definition hashes back to its key"
    );
}
