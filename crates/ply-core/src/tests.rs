//! The parser lands in a sibling crate on its own schedule, so these build the
//! AST directly. That also lets a test give an expression a distinctive span and
//! then assert which expression a diagnostic blamed.

use crate::infer::check_module;
use crate::print::print_scheme;
use crate::{CheckOutput, DefInfo, Footprint, Known, KnownDef};
use ply_span::{Diagnostic, SourceId, Span, Symbol, codes};
use ply_syntax::ast::*;

const SRC: SourceId = SourceId(0);

fn sp(start: u32) -> Span {
    Span::new(SRC, start, start + 1)
}

fn any() -> Span {
    sp(0)
}

fn id(name: &str) -> Ident {
    Ident::new(name, any())
}

fn id_at(name: &str, start: u32) -> Ident {
    Ident::new(name, sp(start))
}

fn ex(kind: ExprKind) -> Expr {
    Expr { kind, span: any() }
}

fn ex_at(kind: ExprKind, start: u32) -> Expr {
    Expr {
        kind,
        span: sp(start),
    }
}

fn var(name: &str) -> Expr {
    ex(ExprKind::Var(id(name).into()))
}

fn int(v: i64) -> Expr {
    ex(ExprKind::Lit(Lit::Int(v)))
}

fn bool_lit(v: bool) -> Expr {
    ex(ExprKind::Lit(Lit::Bool(v)))
}

fn app(func: Expr, args: Vec<Expr>) -> Expr {
    ex(ExprKind::App {
        func: Box::new(func),
        args,
    })
}

fn call(name: &str, args: Vec<Expr>) -> Expr {
    app(var(name), args)
}

fn lambda(params: &[&str], body: Expr) -> Expr {
    ex(ExprKind::Lambda {
        params: params
            .iter()
            .map(|p| Param {
                name: id(p),
                ty: None,
                span: any(),
            })
            .collect(),
        body: Box::new(body),
    })
}

fn add(l: Expr, r: Expr) -> Expr {
    ex(ExprKind::Binary {
        op: BinOp::Add,
        lhs: Box::new(l),
        rhs: Box::new(r),
    })
}

fn block(stmts: Vec<Stmt>, tail: Option<Expr>) -> Expr {
    ex(ExprKind::Block {
        stmts,
        tail: tail.map(Box::new),
    })
}

fn let_(name: &str, value: Expr) -> Stmt {
    Stmt::Let {
        pat: Pattern {
            kind: PatternKind::Var(id(name)),
            span: any(),
        },
        ty: None,
        value: Box::new(value),
        span: any(),
    }
}

fn perform(effect: &str, op: &str, resource: Option<&str>, args: Vec<Expr>) -> Expr {
    perform_at(effect, op, resource, args, 0)
}

fn perform_at(effect: &str, op: &str, resource: Option<&str>, args: Vec<Expr>, start: u32) -> Expr {
    ex_at(
        ExprKind::Perform {
            effect: id(effect).into(),
            op: id(op),
            resource: resource.map(id),
            args,
        },
        start,
    )
}

fn clause(
    effect: &str,
    op: &str,
    resource: Option<&str>,
    params: &[&str],
    body: Expr,
) -> HandleClause {
    HandleClause {
        effect: id(effect).into(),
        op: id(op),
        resource: resource.map(id),
        params: params.iter().map(|p| id(p)).collect(),
        resume: None,
        body,
        span: any(),
    }
}

/// `op(x̄) resume κ -> body`: the general form, whose body has the whole
/// `handle`'s type rather than the operation's.
fn general_clause(
    effect: &str,
    op: &str,
    resource: Option<&str>,
    params: &[&str],
    binder: &str,
    body: Expr,
) -> HandleClause {
    HandleClause {
        resume: Some(id(binder)),
        ..clause(effect, op, resource, params, body)
    }
}

fn handle(body: Expr, clauses: Vec<HandleClause>) -> Expr {
    ex(ExprKind::Handle {
        body: Box::new(body),
        clauses,
        return_clause: None,
    })
}

fn handle_ret(body: Expr, clauses: Vec<HandleClause>, binder: &str, ret: Expr) -> Expr {
    ex(ExprKind::Handle {
        body: Box::new(body),
        clauses,
        return_clause: Some(Box::new(ReturnClause {
            binder: id(binder),
            body: ret,
            span: any(),
        })),
    })
}

fn str_lit(v: &str) -> Expr {
    ex(ExprKind::Lit(Lit::Str(v.to_string())))
}

fn bytes_lit(v: &[u8]) -> Expr {
    ex(ExprKind::Lit(Lit::Bytes(v.to_vec())))
}

fn net_effect() -> Item {
    effect_def(
        "net",
        false,
        vec![op("fetch", Mode::Read, false, vec![], con("Int", vec![]))],
    )
}

fn simulate(body: Expr) -> Expr {
    simulate_at(body, 0)
}

fn simulate_at(body: Expr, start: u32) -> Expr {
    ex_at(
        ExprKind::Simulate {
            body: Box::new(body),
        },
        start,
    )
}

fn with_cell(resource: &str, init: Expr, binder: &str, body: Expr) -> Expr {
    ex(ExprKind::WithCell {
        resource: id(resource),
        init: Box::new(init),
        binder: id(binder),
        body: Box::new(body),
    })
}

fn con(name: &str, args: Vec<TypeExpr>) -> TypeExpr {
    TypeExpr::Con {
        name: id(name).into(),
        args,
        span: any(),
    }
}

fn tvar(name: &str) -> TypeExpr {
    TypeExpr::Var(id(name))
}

fn row(atoms: &[(&str, Mode, Option<&str>)], tail: Option<&str>) -> RowExpr {
    RowExpr {
        atoms: atoms
            .iter()
            .map(|(e, m, r)| AtomExpr {
                effect: id(e).into(),
                mode: *m,
                resource: r.map(id),
                span: any(),
            })
            .collect(),
        tail: tail.map(id),
        span: any(),
    }
}

struct FnBuilder {
    def: FnDef,
}

fn func(name: &str, params: &[&str], body: Expr) -> FnBuilder {
    FnBuilder {
        def: FnDef {
            vis: Visibility::Private,
            name: id(name),
            generics: Generics::default(),
            params: params
                .iter()
                .map(|p| Param {
                    name: id(p),
                    ty: None,
                    span: any(),
                })
                .collect(),
            ret: None,
            effects: None,
            constraints: Vec::new(),
            derived: None,
            spec: Vec::new(),
            body,
            span: any(),
        },
    }
}

impl FnBuilder {
    /// Moves the *name*'s span, which is what a redefinition label points at.
    fn named_at(mut self, start: u32) -> Self {
        self.def.name.span = sp(start);
        self
    }

    fn generics(mut self, types: &[&str], effects: &[&str]) -> Self {
        self.def.generics = Generics {
            types: types.iter().map(|t| id(t)).collect(),
            effects: effects.iter().map(|e| id(e)).collect(),
        };
        self
    }

    fn param_types(mut self, tys: Vec<Option<TypeExpr>>) -> Self {
        for (p, t) in self.def.params.iter_mut().zip(tys) {
            p.ty = t;
        }
        self
    }

    fn ret(mut self, t: TypeExpr) -> Self {
        self.def.ret = Some(t);
        self
    }

    fn effects(mut self, r: RowExpr) -> Self {
        self.def.effects = Some(r);
        self
    }

    fn item(self) -> Item {
        Item::Fn(Box::new(self.def))
    }
}

fn effect_def(name: &str, nondet: bool, ops: Vec<OpDef>) -> Item {
    Item::Effect(Box::new(EffectDef {
        vis: Visibility::Private,
        name: id(name),
        nondet,
        ops,
        span: any(),
    }))
}

fn op(name: &str, mode: Mode, resource_param: bool, params: Vec<TypeExpr>, ret: TypeExpr) -> OpDef {
    OpDef {
        name: id(name),
        mode,
        resource_param,
        params,
        ret,
        span: any(),
    }
}

fn test_def(name: &str, nondet: bool, body: Expr) -> Item {
    Item::Test(Box::new(TestDef {
        name: name.to_string(),
        name_span: any(),
        nondet,
        body,
        span: any(),
    }))
}

fn module(items: Vec<Item>) -> Module {
    Module {
        name: ModuleName::anonymous(),
        source: SRC,
        imports: Vec::new(),
        items,
    }
}

fn check(items: Vec<Item>) -> CheckOutput {
    match check_module(&module(items)) {
        Ok(out) => out,
        Err(diags) => panic!("expected success, got: {}", render(&diags)),
    }
}

fn check_err(items: Vec<Item>) -> Vec<Diagnostic> {
    match check_module(&module(items)) {
        Ok(_) => panic!("expected failure, but the module checked"),
        Err(diags) => diags,
    }
}

fn render(diags: &[Diagnostic]) -> String {
    diags
        .iter()
        .map(|d| {
            let labels: Vec<String> = d
                .labels
                .iter()
                .map(|l| format!("  @{}..{} {}", l.span.start, l.span.end, l.message))
                .collect();
            format!("[{}] {}\n{}", d.code, d.message, labels.join("\n"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn sig(out: &CheckOutput, name: &str) -> String {
    print_scheme(&def(out, name).scheme)
}

fn def<'a>(out: &'a CheckOutput, name: &str) -> &'a DefInfo {
    out.defs
        .get(&Symbol::new(name))
        .unwrap_or_else(|| panic!("no definition `{name}`"))
}

fn footprint(out: &CheckOutput, name: &str) -> String {
    def(out, name).footprint.to_string()
}

fn has_code(diags: &[Diagnostic], code: &str) -> bool {
    diags.iter().any(|d| d.code == code)
}

fn only<'a>(diags: &'a [Diagnostic], code: &str) -> &'a Diagnostic {
    diags
        .iter()
        .find(|d| d.code == code)
        .unwrap_or_else(|| panic!("no `{code}` diagnostic in:\n{}", render(diags)))
}

fn db_effect() -> Item {
    effect_def(
        "db",
        false,
        vec![
            op(
                "get",
                Mode::Read,
                true,
                vec![con("Int", vec![])],
                con("Int", vec![]),
            ),
            op(
                "put",
                Mode::Write,
                true,
                vec![con("Int", vec![]), con("Int", vec![])],
                con("Unit", vec![]),
            ),
        ],
    )
}

fn wall_effect() -> Item {
    effect_def(
        "wall",
        true,
        vec![op("now", Mode::Read, false, vec![], con("Int", vec![]))],
    )
}

#[test]
fn identity_gets_its_principal_type() {
    let out = check(vec![func("id", &["x"], var("x")).item()]);
    assert_eq!(sig(&out, "id"), "<a>(a) -> a");
    assert_eq!(footprint(&out, "id"), "{}");
}

#[test]
fn let_bound_polymorphism_allows_two_instantiations() {
    let body = block(
        vec![
            let_("f", lambda(&["x"], var("x"))),
            Stmt::Expr(call("f", vec![int(1)])),
        ],
        Some(call("f", vec![ex(ExprKind::Lit(Lit::Bool(true)))])),
    );
    let out = check(vec![func("main", &[], body).item()]);
    assert_eq!(sig(&out, "main"), "() -> Bool");
}

#[test]
fn a_monomorphic_lambda_parameter_is_not_generalized() {
    let body = block(
        vec![Stmt::Expr(call("f", vec![int(1)]))],
        Some(call("f", vec![bool_lit(true)])),
    );
    let diags = check_err(vec![func("twice", &["f"], body).item()]);
    assert!(has_code(&diags, codes::TYPE_MISMATCH), "{}", render(&diags));
}

#[test]
fn a_declared_type_variable_cannot_be_pinned_to_a_concrete_type() {
    let def = func("bad", &["x"], add(var("x"), int(1)))
        .generics(&["a"], &[])
        .param_types(vec![Some(tvar("a"))])
        .ret(con("Int", vec![]))
        .item();
    let diags = check_err(vec![def]);
    assert!(has_code(&diags, codes::TYPE_MISMATCH), "{}", render(&diags));
}

#[test]
fn a_perform_contributes_its_atom_to_the_row() {
    let out = check(vec![
        db_effect(),
        func(
            "read_one",
            &[],
            perform("db", "get", Some("users"), vec![int(1)]),
        )
        .item(),
    ]);
    assert_eq!(footprint(&out, "read_one"), "{db.read[users]}");
    assert_eq!(sig(&out, "read_one"), "() -> Int / {db.read[users]}");
}

#[test]
fn rows_accumulate_through_application_and_composition() {
    let out = check(vec![
        db_effect(),
        func(
            "reader",
            &[],
            perform("db", "get", Some("users"), vec![int(1)]),
        )
        .item(),
        func(
            "writer",
            &[],
            perform("db", "put", Some("orders"), vec![int(1), int(2)]),
        )
        .item(),
        func(
            "both",
            &[],
            block(
                vec![Stmt::Expr(call("reader", vec![]))],
                Some(call("writer", vec![])),
            ),
        )
        .item(),
    ]);
    assert_eq!(
        footprint(&out, "both"),
        "{db.write[orders], db.read[users]}"
    );
}

#[test]
fn effect_polymorphism_threads_a_row_variable_through_a_higher_order_function() {
    let apply = func("apply", &["f"], call("f", vec![]))
        .generics(&["a"], &["e"])
        .param_types(vec![Some(TypeExpr::Fn {
            params: vec![],
            ret: Box::new(tvar("a")),
            effects: Some(row(&[], Some("e"))),
            span: any(),
        })])
        .ret(tvar("a"))
        .effects(row(&[], Some("e")))
        .item();
    let out = check(vec![
        db_effect(),
        apply,
        func(
            "reader",
            &[],
            perform("db", "get", Some("users"), vec![int(1)]),
        )
        .item(),
        func("use_it", &[], call("apply", vec![var("reader")])).item(),
    ]);
    assert_eq!(sig(&out, "apply"), "<a | e>(() -> a / e) -> a / e");
    assert_eq!(footprint(&out, "apply"), "{}");
    assert_eq!(footprint(&out, "use_it"), "{db.read[users]}");
}

#[test]
fn the_prelude_map_stays_effect_polymorphic_and_pure_at_a_pure_call() {
    let out = check(vec![
        func(
            "doubled",
            &["xs"],
            call(
                "map",
                vec![var("xs"), lambda(&["x"], add(var("x"), var("x")))],
            ),
        )
        .item(),
    ]);
    assert_eq!(sig(&out, "doubled"), "(List<Int>) -> List<Int>");
    assert_eq!(footprint(&out, "doubled"), "{}");
}

#[test]
fn a_handler_subtracts_the_atoms_it_discharges() {
    let out = check(vec![
        db_effect(),
        func(
            "reader",
            &[],
            perform("db", "get", Some("users"), vec![int(1)]),
        )
        .item(),
        func(
            "isolated",
            &[],
            handle(
                call("reader", vec![]),
                vec![clause("db", "get", Some("users"), &["k"], int(7))],
            ),
        )
        .item(),
    ]);
    assert_eq!(footprint(&out, "reader"), "{db.read[users]}");
    assert_eq!(footprint(&out, "isolated"), "{}");
}

/// ADR 0005 §4.1. `ρ_κ := ρ_h` is self-referential, and it is the one row
/// variable a self-occurrence is sound for; unification's occurs check must not
/// see it and must not be relaxed for it.
#[test]
fn a_clause_that_calls_its_own_continuation_infers_a_closed_row() {
    let out = check(vec![
        db_effect(),
        func(
            "reader",
            &[],
            perform("db", "get", Some("users"), vec![int(1)]),
        )
        .item(),
        func(
            "resumed",
            &[],
            handle(
                call("reader", vec![]),
                vec![general_clause(
                    "db",
                    "get",
                    Some("users"),
                    &["k"],
                    "kont",
                    app(var("kont"), vec![int(7)]),
                )],
            ),
        )
        .item(),
    ]);
    assert_eq!(footprint(&out, "resumed"), "{}");
    assert_eq!(sig(&out, "resumed"), "() -> Int");
}

/// ADR 0005 §4.2. A row is a set, so the number of resumptions cannot move a
/// footprint — which is what keeps the conflict graph invariant under
/// multi-shot.
#[test]
fn resuming_zero_once_or_twice_gives_one_footprint() {
    let program = |body: Expr| {
        vec![
            db_effect(),
            net_effect(),
            func(
                "reader",
                &[],
                perform("db", "get", Some("users"), vec![int(1)]),
            )
            .item(),
            func(
                "handled",
                &[],
                handle(
                    call("reader", vec![]),
                    vec![general_clause(
                        "db",
                        "get",
                        Some("users"),
                        &["k"],
                        "kont",
                        body,
                    )],
                ),
            )
            .item(),
        ]
    };
    let never = perform("net", "fetch", None, vec![]);
    let once = app(var("kont"), vec![int(7)]);
    let twice = add(
        app(var("kont"), vec![int(7)]),
        app(var("kont"), vec![int(8)]),
    );
    let of = |body| footprint(&check(program(body)), "handled");
    assert_eq!(of(never), "{net.read}");
    assert_eq!(of(once), "{}");
    assert_eq!(of(twice), "{}");
}

/// A tail-resumptive clause's body is the *operation's* result; a general
/// clause's body is the whole `handle`'s. Confusing the two is silent, because
/// the two types coincide whenever the handled body has the operation's type.
#[test]
fn a_general_clause_returns_the_handles_result_not_the_operations() {
    let items = |body: Expr| {
        vec![
            db_effect(),
            func(
                "reader",
                &[],
                perform("db", "get", Some("users"), vec![int(1)]),
            )
            .item(),
            func(
                "handled",
                &[],
                handle_ret(
                    call("reader", vec![]),
                    vec![general_clause(
                        "db",
                        "get",
                        Some("users"),
                        &["k"],
                        "kont",
                        body,
                    )],
                    "x",
                    call("int_to_string", vec![var("x")]),
                ),
            )
            .item(),
        ]
    };
    assert_eq!(
        sig(&check(items(str_lit("aborted"))), "handled"),
        "() -> String"
    );
    let diags = check_err(items(int(0)));
    assert!(has_code(&diags, codes::TYPE_MISMATCH), "{}", render(&diags));
}

#[test]
fn a_handler_only_subtracts_the_resource_it_names() {
    let out = check(vec![
        db_effect(),
        func(
            "reader",
            &[],
            perform("db", "get", Some("orders"), vec![int(1)]),
        )
        .item(),
        func(
            "still_dirty",
            &[],
            handle(
                call("reader", vec![]),
                vec![clause("db", "get", Some("users"), &["k"], int(7))],
            ),
        )
        .item(),
    ]);
    assert_eq!(footprint(&out, "still_dirty"), "{db.read[orders]}");
}

#[test]
fn a_handler_clause_adds_its_own_effects_to_the_result() {
    let out = check(vec![
        db_effect(),
        effect_def(
            "net",
            false,
            vec![op("fetch", Mode::Read, false, vec![], con("Int", vec![]))],
        ),
        func(
            "reader",
            &[],
            perform("db", "get", Some("users"), vec![int(1)]),
        )
        .item(),
        func(
            "backed_by_socket",
            &[],
            handle(
                call("reader", vec![]),
                vec![clause(
                    "db",
                    "get",
                    Some("users"),
                    &["k"],
                    perform("net", "fetch", None, vec![]),
                )],
            ),
        )
        .item(),
    ]);
    assert_eq!(footprint(&out, "backed_by_socket"), "{net.read}");
}

#[test]
fn with_cell_discharges_the_cell_atoms_of_its_own_region() {
    let body = block(
        vec![Stmt::Expr(call("cell_set", vec![var("c"), int(3)]))],
        Some(call("cell_get", vec![var("c")])),
    );
    let out = check(vec![
        func("counted", &[], with_cell("users", int(0), "c", body)).item(),
    ]);
    assert_eq!(footprint(&out, "counted"), "{}");
    assert_eq!(sig(&out, "counted"), "() -> Int");
}

#[test]
fn a_cell_may_not_outlive_the_region_that_discharges_its_atoms() {
    let diags = check_err(vec![
        func(
            "escaped",
            &["x"],
            with_cell("users", var("x"), "c", var("c")),
        )
        .item(),
    ]);
    let d = only(&diags, codes::TYPE_MISMATCH);
    assert_eq!(d.message, "the cell escapes its `with_cell[users]` region");
    assert!(
        d.labels[0].message.contains("Cell[users]"),
        "{}",
        d.labels[0].message
    );
}

#[test]
fn a_region_escape_is_caught_however_the_cell_is_wrapped() {
    let in_a_list = with_cell(
        "users",
        int(0),
        "c",
        ex(ExprKind::List {
            items: vec![var("c")],
        }),
    );
    assert!(has_code(
        &check_err(vec![func("f", &[], in_a_list).item()]),
        codes::TYPE_MISMATCH
    ),);

    let in_a_record = with_cell(
        "users",
        int(0),
        "c",
        ex(ExprKind::Record {
            fields: vec![(id("held"), var("c"))],
        }),
    );
    assert!(has_code(
        &check_err(vec![func("g", &[], in_a_record).item()]),
        codes::TYPE_MISMATCH
    ),);

    let in_a_closure = with_cell("users", int(0), "c", lambda(&["_ignored"], var("c")));
    assert!(has_code(
        &check_err(vec![func("h", &[], in_a_closure).item()]),
        codes::TYPE_MISMATCH
    ),);
}

#[test]
fn a_region_that_returns_a_plain_value_is_accepted() {
    let out = check(vec![
        func(
            "f",
            &[],
            with_cell("users", int(1), "c", call("cell_get", vec![var("c")])),
        )
        .item(),
    ]);
    assert_eq!(sig(&out, "f"), "() -> Int");
    assert_eq!(footprint(&out, "f"), "{}");
}

#[test]
fn nested_regions_keep_their_cells_apart() {
    let good = with_cell(
        "outer",
        int(0),
        "c",
        with_cell(
            "inner",
            bool_lit(true),
            "d",
            block(
                vec![Stmt::Expr(call(
                    "cell_set",
                    vec![var("d"), bool_lit(false)],
                ))],
                Some(call("cell_get", vec![var("c")])),
            ),
        ),
    );
    let out = check(vec![func("nested", &[], good).item()]);
    assert_eq!(footprint(&out, "nested"), "{}");
    assert_eq!(sig(&out, "nested"), "() -> Int");

    let bad = with_cell(
        "outer",
        int(0),
        "c",
        with_cell(
            "inner",
            bool_lit(true),
            "d",
            call("cell_set", vec![var("c"), bool_lit(false)]),
        ),
    );
    let diags = check_err(vec![func("confused", &[], bad).item()]);
    assert!(has_code(&diags, codes::TYPE_MISMATCH), "{}", render(&diags));
}

#[test]
fn a_nondet_effect_handled_at_another_resource_still_fails_the_test() {
    let sensors = effect_def(
        "sensors",
        true,
        vec![op("read_at", Mode::Read, true, vec![], con("Int", vec![]))],
    );
    let body = handle(
        block(
            vec![],
            Some(perform_at("sensors", "read_at", Some("north"), vec![], 91)),
        ),
        vec![clause("sensors", "read_at", Some("south"), &[], int(0))],
    );
    let diags = check_err(vec![sensors, test_def("wrong resource", false, body)]);
    let d = only(&diags, codes::NONDET_IN_DET_TEST);
    let primary = d.labels.iter().find(|l| l.primary).unwrap();
    assert_eq!(primary.span.start, 91);
    assert!(
        primary.message.contains("sensors.read[north]"),
        "{}",
        primary.message
    );
    assert!(
        d.notes
            .iter()
            .any(|n| n.contains("sensors.read_at[north]()")),
        "{:?}",
        d.notes
    );
}

#[test]
fn two_distinct_effect_variables_cannot_be_merged_by_one_annotation() {
    let sig_ty = |tail: &str| {
        Some(TypeExpr::Fn {
            params: vec![],
            ret: Box::new(con("Int", vec![])),
            effects: Some(row(&[], Some(tail))),
            span: any(),
        })
    };
    let def = func(
        "both",
        &["g", "h"],
        block(vec![Stmt::Expr(call("g", vec![]))], Some(call("h", vec![]))),
    )
    .generics(&[], &["e", "f"])
    .param_types(vec![sig_ty("e"), sig_ty("f")])
    .ret(con("Int", vec![]))
    .effects(row(&[], Some("e")))
    .item();
    let diags = check_err(vec![def]);
    assert!(
        has_code(&diags, codes::EFFECT_NOT_PERMITTED),
        "{}",
        render(&diags)
    );
}

#[test]
fn a_handled_atom_stops_being_evidence_for_the_determinism_check() {
    let handled = handle(
        block(vec![], Some(perform_at("wall", "now", None, vec![], 10))),
        vec![clause("wall", "now", None, &[], int(0))],
    );
    let body = block(
        vec![Stmt::Expr(handled)],
        Some(perform_at("wall", "now", None, vec![], 20)),
    );
    let diags = check_err(vec![
        wall_effect(),
        test_def("one handled one not", false, body),
    ]);
    let d = only(&diags, codes::NONDET_IN_DET_TEST);
    let primary = d.labels.iter().find(|l| l.primary).unwrap();
    assert_eq!(
        primary.span.start, 20,
        "the blamed perform must be the surviving one"
    );
}

#[test]
fn a_cell_whose_region_is_unknown_is_a_resource_error() {
    let def = func("peek", &["c"], call("cell_get", vec![var("c")]))
        .param_types(vec![Some(con("Cell", vec![con("Int", vec![])]))])
        .item();
    let diags = check_err(vec![def]);
    let d = only(&diags, codes::RESOURCE_REQUIRED);
    assert!(d.message.contains("cell_get"), "{}", d.message);
}

#[test]
fn a_handler_inside_a_cell_region_keeps_the_test_isolated() {
    let handled = handle(
        call("reader", vec![]),
        vec![clause(
            "db",
            "get",
            Some("users"),
            &["k"],
            call("cell_get", vec![var("c")]),
        )],
    );
    let out = check(vec![
        db_effect(),
        func(
            "reader",
            &[],
            perform("db", "get", Some("users"), vec![int(1)]),
        )
        .item(),
        func("isolated", &[], with_cell("users", int(0), "c", handled)).item(),
    ]);
    assert_eq!(footprint(&out, "isolated"), "{}");
}

#[test]
fn an_annotation_is_an_upper_bound_and_is_what_gets_published() {
    let def = func(
        "reads_only",
        &[],
        perform("db", "get", Some("users"), vec![int(1)]),
    )
    .ret(con("Int", vec![]))
    .effects(row(
        &[
            ("db", Mode::Read, Some("users")),
            ("db", Mode::Write, Some("audit")),
        ],
        None,
    ))
    .item();
    let out = check(vec![db_effect(), def]);
    assert_eq!(
        footprint(&out, "reads_only"),
        "{db.write[audit], db.read[users]}"
    );
}

#[test]
fn an_annotation_that_omits_an_atom_names_the_atom_and_the_perform() {
    let def = func(
        "sneaky",
        &[],
        block(
            vec![Stmt::Expr(perform_at(
                "db",
                "put",
                Some("orders"),
                vec![int(1), int(2)],
                40,
            ))],
            Some(perform("db", "get", Some("users"), vec![int(1)])),
        ),
    )
    .ret(con("Int", vec![]))
    .effects(row(&[("db", Mode::Read, Some("users"))], None))
    .item();

    let diags = check_err(vec![db_effect(), def]);
    let d = only(&diags, codes::EFFECT_NOT_PERMITTED);
    assert!(d.message.contains("db.write[orders]"), "{}", d.message);
    assert_eq!(d.primary_span().unwrap().start, 40, "{}", render(&diags));
    assert!(
        d.notes.iter().any(|n| n.contains("db.write[orders]")),
        "{:?}",
        d.notes
    );
}

#[test]
fn an_undeclared_effect_variable_in_an_annotation_is_an_unbound_row_var() {
    let def = func("f", &[], int(1))
        .ret(con("Int", vec![]))
        .effects(row(&[], Some("e")))
        .item();
    let diags = check_err(vec![def]);
    assert!(
        has_code(&diags, codes::UNBOUND_ROW_VAR),
        "{}",
        render(&diags)
    );
}

#[test]
fn a_nondet_effect_surviving_in_a_det_test_is_e0412() {
    let body = block(
        vec![let_("now", perform_at("wall", "now", None, vec![], 77))],
        Some(call("assert", vec![bool_lit(true)])),
    );
    let diags = check_err(vec![wall_effect(), test_def("uses the clock", false, body)]);
    let d = only(&diags, codes::NONDET_IN_DET_TEST);

    assert_eq!(d.message, "nondeterministic effect in a deterministic test");
    let primary = d.labels.iter().find(|l| l.primary).unwrap();
    assert_eq!(primary.span.start, 77);
    assert!(primary.message.contains("wall.read"), "{}", primary.message);
    assert!(primary.message.contains("nondet"), "{}", primary.message);
    assert!(
        d.notes.iter().any(|n| n.contains("wall.now()")),
        "expected a handler suggestion, got {:?}",
        d.notes
    );
    assert!(
        d.notes.iter().any(|n| n.contains("test/nondet")),
        "expected the opt-out suggestion, got {:?}",
        d.notes
    );
}

#[test]
fn handling_the_nondet_effect_makes_the_test_deterministic() {
    let body = handle(
        block(
            vec![Stmt::Expr(perform("wall", "now", None, vec![]))],
            Some(int(0)),
        ),
        vec![clause("wall", "now", None, &[], int(1234))],
    );
    let out = check(vec![wall_effect(), test_def("frozen clock", false, body)]);
    assert_eq!(out.tests[0].footprint.to_string(), "{}");
    assert!(!out.tests[0].nondet);
}

#[test]
fn test_nondet_opts_out_of_the_determinism_check() {
    let body = block(
        vec![Stmt::Expr(perform("wall", "now", None, vec![]))],
        Some(int(0)),
    );
    let out = check(vec![wall_effect(), test_def("wall clock", true, body)]);
    assert_eq!(out.tests[0].footprint.to_string(), "{wall.read}");
    assert!(out.tests[0].nondet);
}

#[test]
fn e0412_points_through_a_call_when_the_perform_is_indirect() {
    let helper = func("stamp", &[], perform("wall", "now", None, vec![])).item();
    let body = block(
        vec![],
        Some(ex_at(
            ExprKind::App {
                func: Box::new(var("stamp")),
                args: vec![],
            },
            55,
        )),
    );
    let diags = check_err(vec![
        wall_effect(),
        helper,
        test_def("indirect", false, body),
    ]);
    let d = only(&diags, codes::NONDET_IN_DET_TEST);
    let primary = d.labels.iter().find(|l| l.primary).unwrap();
    assert_eq!(primary.span.start, 55);
    assert!(d.notes.iter().any(|n| n.contains("calls")), "{:?}", d.notes);
}

#[test]
fn a_missing_resource_label_is_reported_at_the_perform() {
    let def = func("f", &[], perform_at("db", "get", None, vec![int(1)], 12)).item();
    let diags = check_err(vec![db_effect(), def]);
    let d = only(&diags, codes::RESOURCE_REQUIRED);
    assert_eq!(d.primary_span().unwrap().start, 12);
    assert!(
        d.notes.iter().any(|n| n.contains("db.get[")),
        "{:?}",
        d.notes
    );
}

#[test]
fn a_resource_label_on_a_plain_operation_is_rejected() {
    let def = func("f", &[], perform("wall", "now", Some("wall"), vec![])).item();
    let diags = check_err(vec![wall_effect(), def]);
    assert!(
        has_code(&diags, codes::RESOURCE_REQUIRED),
        "{}",
        render(&diags)
    );
}

#[test]
fn unknown_effects_and_operations_are_reported_at_their_own_idents() {
    let bad_effect = func(
        "a",
        &[],
        ex(ExprKind::Perform {
            effect: id_at("nope", 30).into(),
            op: id("x"),
            resource: None,
            args: vec![],
        }),
    )
    .item();
    let bad_op = func(
        "b",
        &[],
        ex(ExprKind::Perform {
            effect: id("db").into(),
            op: id_at("nope", 60),
            resource: Some(id("users")),
            args: vec![],
        }),
    )
    .item();
    let diags = check_err(vec![db_effect(), bad_effect, bad_op]);
    assert_eq!(
        only(&diags, codes::UNKNOWN_EFFECT)
            .primary_span()
            .unwrap()
            .start,
        30
    );
    assert_eq!(
        only(&diags, codes::UNKNOWN_OPERATION)
            .primary_span()
            .unwrap()
            .start,
        60
    );
}

#[test]
fn an_operation_call_with_the_wrong_arity_is_reported() {
    let def = func(
        "f",
        &[],
        perform("db", "get", Some("users"), vec![int(1), int(2)]),
    )
    .item();
    let diags = check_err(vec![db_effect(), def]);
    let d = only(&diags, codes::ARITY_MISMATCH);
    assert!(d.message.contains("db.get"), "{}", d.message);
}

#[test]
fn several_independent_errors_are_reported_in_one_run() {
    let items = vec![
        db_effect(),
        func("a", &[], var("missing_one")).item(),
        func("b", &[], var("missing_two")).item(),
        func("c", &[], add(bool_lit(true), int(1))).item(),
    ];
    let diags = check_err(items);
    assert_eq!(
        diags
            .iter()
            .filter(|d| d.code == codes::UNKNOWN_NAME)
            .count(),
        2
    );
    assert!(has_code(&diags, codes::TYPE_MISMATCH));
}

#[test]
fn mutual_recursion_is_generalized_as_one_component() {
    let is_even = func(
        "is_even",
        &["n"],
        ex(ExprKind::If {
            cond: Box::new(ex(ExprKind::Binary {
                op: BinOp::Eq,
                lhs: Box::new(var("n")),
                rhs: Box::new(int(0)),
            })),
            then_branch: Box::new(bool_lit(true)),
            else_branch: Box::new(call(
                "is_odd",
                vec![ex(ExprKind::Binary {
                    op: BinOp::Sub,
                    lhs: Box::new(var("n")),
                    rhs: Box::new(int(1)),
                })],
            )),
        }),
    )
    .item();
    let is_odd = func(
        "is_odd",
        &["n"],
        ex(ExprKind::If {
            cond: Box::new(ex(ExprKind::Binary {
                op: BinOp::Eq,
                lhs: Box::new(var("n")),
                rhs: Box::new(int(0)),
            })),
            then_branch: Box::new(bool_lit(false)),
            else_branch: Box::new(call(
                "is_even",
                vec![ex(ExprKind::Binary {
                    op: BinOp::Sub,
                    lhs: Box::new(var("n")),
                    rhs: Box::new(int(1)),
                })],
            )),
        }),
    )
    .item();
    let out = check(vec![is_even, is_odd]);
    assert_eq!(sig(&out, "is_even"), "(Int) -> Bool");
    assert_eq!(sig(&out, "is_odd"), "(Int) -> Bool");
}

#[test]
fn a_recursive_function_keeps_the_atoms_it_performs() {
    let body = ex(ExprKind::If {
        cond: Box::new(bool_lit(true)),
        then_branch: Box::new(perform("db", "get", Some("users"), vec![int(0)])),
        else_branch: Box::new(call("looper", vec![])),
    });
    let out = check(vec![db_effect(), func("looper", &[], body).item()]);
    assert_eq!(footprint(&out, "looper"), "{db.read[users]}");
}

#[test]
fn a_definition_used_before_it_is_written_still_generalizes() {
    let out = check(vec![
        func("caller", &[], call("later", vec![int(1)])).item(),
        func("later", &["x"], var("x")).item(),
    ]);
    assert_eq!(sig(&out, "later"), "<a>(a) -> a");
    assert_eq!(sig(&out, "caller"), "() -> Int");
}

#[test]
fn sum_types_give_constructors_and_exhaustiveness() {
    // `Option` is the prelude's: `map_get` returns one and `decimal_of_string`
    // returns one, so a builtin's type would otherwise mention a type the user
    // has to declare. Redeclaring it here would be `E0105`.
    let arm = |kind: PatternKind, body: Expr| MatchArm {
        pat: Pattern { kind, span: any() },
        guard: None,
        body,
        span: any(),
    };
    let full = ex(ExprKind::Match {
        scrutinee: Box::new(var("o")),
        arms: vec![
            arm(
                PatternKind::Ctor {
                    name: id("None").into(),
                    args: vec![],
                },
                int(0),
            ),
            arm(
                PatternKind::Ctor {
                    name: id("Some").into(),
                    args: vec![Pattern {
                        kind: PatternKind::Var(id("v")),
                        span: any(),
                    }],
                },
                var("v"),
            ),
        ],
    });
    let out = check(vec![
        func("unwrap_or_zero", &["o"], full).item(),
        func("wrap", &[], call("Some", vec![int(3)])).item(),
    ]);
    assert_eq!(sig(&out, "unwrap_or_zero"), "(Option<Int>) -> Int");
    assert_eq!(sig(&out, "wrap"), "() -> Option<Int>");
    assert_eq!(out.ctors[&Symbol::new("Some")].arity, 1);
    assert_eq!(out.ctors[&Symbol::new("None")].index, 0);

    let partial = ex(ExprKind::Match {
        scrutinee: Box::new(var("o")),
        arms: vec![arm(
            PatternKind::Ctor {
                name: id("None").into(),
                args: vec![],
            },
            int(0),
        )],
    });
    let diags = check_err(vec![func("partial", &["o"], partial).item()]);
    let d = only(&diags, codes::NON_EXHAUSTIVE_MATCH);
    assert!(
        d.labels[0].message.contains("Some"),
        "{}",
        d.labels[0].message
    );
}

#[test]
fn the_occurs_check_rejects_self_application() {
    let def = func("omega", &["x"], call("x", vec![var("x")])).item();
    let diags = check_err(vec![def]);
    assert!(has_code(&diags, codes::OCCURS_CHECK), "{}", render(&diags));
}

#[test]
fn calling_a_non_function_is_reported_as_such() {
    let def = func("f", &[], app(int(1), vec![int(2)])).item();
    let diags = check_err(vec![def]);
    assert!(
        has_code(&diags, codes::NOT_A_FUNCTION),
        "{}",
        render(&diags)
    );
}

#[test]
fn duplicate_definitions_are_reported_once_and_the_first_wins() {
    let items = vec![
        func("f", &[], int(1)).item(),
        func("f", &[], bool_lit(true)).item(),
    ];
    let diags = check_err(items);
    assert_eq!(
        diags
            .iter()
            .filter(|d| d.code == codes::DUPLICATE_DEFINITION)
            .count(),
        1
    );
}

/// Constructors and functions are one namespace, so declaring both under a name
/// makes the second unreachable. Accepting it silently is the worse half: the
/// call site keeps compiling and quietly means the other thing.
#[test]
fn a_function_and_a_constructor_may_not_share_a_name() {
    let variant = |name: &str, start: u32| VariantDef {
        name: id_at(name, start),
        fields: vec![con("Int", vec![])],
        span: sp(start),
    };
    let sum = Item::Type(Box::new(TypeDef {
        vis: Visibility::Private,
        name: id("Wrapper"),
        params: vec![],
        body: TypeDefBody::Sum(vec![variant("Tag", 40)]),
        span: any(),
    }));
    let items = vec![func("Tag", &["x"], var("x")).named_at(10).item(), sum];

    let diags = check_err(items);
    let d = only(&diags, codes::DUPLICATE_DEFINITION);
    assert!(
        d.message.contains("`Tag` is defined twice"),
        "{}",
        render(&diags)
    );
    assert_eq!(
        d.primary_span().expect("a real span").start,
        40,
        "{}",
        render(&diags)
    );
    assert!(
        d.labels.iter().any(|l| !l.primary && l.span.start == 10),
        "the first declaration has to be pointed at too: {}",
        render(&diags)
    );
    assert!(
        d.notes.iter().any(|n| n.contains("rename")),
        "{:?}",
        d.notes
    );
}

#[test]
fn a_constructor_and_a_later_function_are_reported_against_the_function() {
    let sum = Item::Type(Box::new(TypeDef {
        vis: Visibility::Private,
        name: id("Wrapper"),
        params: vec![],
        body: TypeDefBody::Sum(vec![VariantDef {
            name: id_at("Tag", 10),
            fields: vec![con("Int", vec![])],
            span: sp(10),
        }]),
        span: any(),
    }));
    let items = vec![sum, func("Tag", &["x"], var("x")).named_at(40).item()];

    let diags = check_err(items);
    let d = only(&diags, codes::DUPLICATE_DEFINITION);
    assert_eq!(
        d.primary_span().expect("a real span").start,
        40,
        "{}",
        render(&diags)
    );
    assert!(
        d.labels.iter().any(|l| l.message.contains("as a function")),
        "{}",
        render(&diags)
    );
}

/// The check may not fire between namespaces that really are separate, nor
/// report a same-kind collision a second time.
#[test]
fn a_type_an_effect_and_a_function_may_all_share_one_name() {
    let out = check(vec![
        Item::Type(Box::new(TypeDef {
            vis: Visibility::Private,
            name: id("thing"),
            params: vec![],
            body: TypeDefBody::Alias(con("Int", vec![])),
            span: any(),
        })),
        effect_def(
            "thing",
            false,
            vec![op("get", Mode::Read, true, vec![], con("Int", vec![]))],
        ),
        func("thing", &[], int(1)).item(),
    ]);
    assert_eq!(sig(&out, "thing"), "() -> Int");
}

#[test]
fn a_user_effect_may_not_be_called_cell() {
    let diags = check_err(vec![effect_def(
        "cell",
        false,
        vec![op("peek", Mode::Read, true, vec![], con("Int", vec![]))],
    )]);
    assert!(
        has_code(&diags, codes::DUPLICATE_DEFINITION),
        "{}",
        render(&diags)
    );
}

#[test]
fn a_type_alias_is_expanded_and_a_cyclic_one_is_caught() {
    let alias = Item::Type(Box::new(TypeDef {
        vis: Visibility::Private,
        name: id("Count"),
        params: vec![],
        body: TypeDefBody::Alias(con("Int", vec![])),
        span: any(),
    }));
    let out = check(vec![
        alias,
        func("bump", &["n"], add(var("n"), int(1)))
            .param_types(vec![Some(con("Count", vec![]))])
            .item(),
    ]);
    assert_eq!(sig(&out, "bump"), "(Int) -> Int");

    let cyclic = Item::Type(Box::new(TypeDef {
        vis: Visibility::Private,
        name: id("Loop"),
        params: vec![],
        body: TypeDefBody::Alias(con("Loop", vec![])),
        span: any(),
    }));
    let diags = check_err(vec![
        cyclic,
        func("f", &["x"], var("x"))
            .param_types(vec![Some(con("Loop", vec![]))])
            .item(),
    ]);
    assert!(has_code(&diags, codes::UNKNOWN_TYPE), "{}", render(&diags));
}

#[test]
fn a_generic_effect_operation_is_instantiated_per_call_site() {
    let store = effect_def(
        "store",
        false,
        vec![op("echo", Mode::Read, true, vec![tvar("a")], tvar("a"))],
    );
    let out = check(vec![
        store,
        func(
            "as_int",
            &[],
            perform("store", "echo", Some("k"), vec![int(1)]),
        )
        .item(),
        func(
            "as_bool",
            &[],
            perform("store", "echo", Some("k"), vec![bool_lit(true)]),
        )
        .item(),
    ]);
    assert_eq!(sig(&out, "as_int"), "() -> Int / {store.read[k]}");
    assert_eq!(sig(&out, "as_bool"), "() -> Bool / {store.read[k]}");
}

#[test]
fn a_handler_clause_must_return_the_operations_result_type() {
    let def = func(
        "f",
        &[],
        handle(
            perform("db", "get", Some("users"), vec![int(1)]),
            vec![clause("db", "get", Some("users"), &["k"], bool_lit(true))],
        ),
    )
    .item();
    let diags = check_err(vec![db_effect(), def]);
    assert!(has_code(&diags, codes::TYPE_MISMATCH), "{}", render(&diags));
}

#[test]
fn a_return_clause_changes_the_result_type_of_the_handle() {
    let handled = ex(ExprKind::Handle {
        body: Box::new(int(1)),
        clauses: vec![],
        return_clause: Some(Box::new(ReturnClause {
            binder: id("x"),
            body: call("int_to_string", vec![var("x")]),
            span: any(),
        })),
    });
    let out = check(vec![func("f", &[], handled).item()]);
    assert_eq!(sig(&out, "f"), "() -> String");
}

#[test]
fn a_test_footprint_reaches_through_the_functions_it_calls() {
    let out = check(vec![
        db_effect(),
        func(
            "reader",
            &[],
            perform("db", "get", Some("users"), vec![int(1)]),
        )
        .item(),
        test_def(
            "reads users",
            false,
            block(vec![], Some(call("reader", vec![]))),
        ),
    ]);
    assert_eq!(out.tests[0].footprint.to_string(), "{db.read[users]}");
    assert_eq!(out.tests[0].index, 0);
    assert_eq!(out.tests[0].name, "reads users");
}

#[test]
fn an_unconstrained_row_variable_in_a_test_closes_to_empty() {
    let out = check(vec![
        func("apply", &["f"], call("f", vec![])).item(),
        test_def(
            "pure",
            false,
            block(vec![], Some(call("apply", vec![lambda(&[], int(1))]))),
        ),
    ]);
    assert_eq!(out.tests[0].footprint.to_string(), "{}");
}

#[test]
fn a_handler_that_forwards_to_the_real_effect_stays_honest() {
    let body = handle(
        block(vec![], Some(perform_at("wall", "now", None, vec![], 10))),
        vec![clause(
            "wall",
            "now",
            None,
            &[],
            perform_at("wall", "now", None, vec![], 30),
        )],
    );
    let out = check(vec![wall_effect(), func("f", &[], body.clone()).item()]);
    assert_eq!(footprint(&out, "f"), "{wall.read}");

    let diags = check_err(vec![wall_effect(), test_def("forwarding", false, body)]);
    let d = only(&diags, codes::NONDET_IN_DET_TEST);
    let primary = d.labels.iter().find(|l| l.primary).unwrap();
    assert_eq!(
        primary.span.start, 30,
        "the clause body is what still performs it"
    );
}

#[test]
fn a_cell_builtin_cannot_escape_as_a_value_or_be_redefined() {
    let as_value = func(
        "f",
        &[],
        block(vec![let_("g", var("cell_get"))], Some(int(1))),
    )
    .item();
    let diags = check_err(vec![as_value]);
    assert!(
        has_code(&diags, codes::RESOURCE_REQUIRED),
        "{}",
        render(&diags)
    );

    let redefined = func("cell_get", &["c"], int(0)).item();
    let diags = check_err(vec![redefined]);
    assert!(
        has_code(&diags, codes::DUPLICATE_DEFINITION),
        "{}",
        render(&diags)
    );
}

#[test]
fn a_shadowing_local_takes_precedence_over_a_prelude_name() {
    let body = block(
        vec![let_("len", lambda(&["x"], var("x")))],
        Some(call("len", vec![bool_lit(true)])),
    );
    let out = check(vec![func("f", &[], body).item()]);
    assert_eq!(sig(&out, "f"), "() -> Bool");
}

fn pat(kind: PatternKind) -> Pattern {
    Pattern { kind, span: any() }
}

fn pvar(name: &str) -> Pattern {
    pat(PatternKind::Var(id(name)))
}

fn plist(items: Vec<Pattern>, rest: Option<Pattern>) -> Pattern {
    pat(PatternKind::List {
        items,
        rest: rest.map(Box::new),
    })
}

fn match_on(scrutinee: Expr, arms: Vec<(Pattern, Expr)>) -> Expr {
    ex(ExprKind::Match {
        scrutinee: Box::new(scrutinee),
        arms: arms
            .into_iter()
            .map(|(p, body)| MatchArm {
                pat: p,
                guard: None,
                body,
                span: any(),
            })
            .collect(),
    })
}

fn list_ty() -> TypeExpr {
    con("List", vec![con("Int", vec![])])
}

fn on_int_list(arms: Vec<(Pattern, Expr)>) -> FnBuilder {
    func("f", &["xs"], match_on(var("xs"), arms))
        .param_types(vec![Some(list_ty())])
        .ret(con("Int", vec![]))
}

#[test]
fn an_empty_and_a_cons_arm_cover_every_list() {
    let out = check(vec![
        on_int_list(vec![
            (plist(vec![], None), int(0)),
            (plist(vec![pvar("x")], Some(pvar("r"))), var("x")),
        ])
        .item(),
    ]);
    assert_eq!(sig(&out, "f"), "(List<Int>) -> Int");
}

#[test]
fn a_bare_rest_pattern_is_irrefutable() {
    check(vec![
        on_int_list(vec![(plist(vec![], Some(pvar("r"))), int(0))]).item(),
    ]);
}

#[test]
fn a_cons_arm_alone_leaves_the_empty_list_uncovered() {
    let diags = check_err(vec![
        on_int_list(vec![(plist(vec![pvar("x")], Some(pvar("r"))), var("x"))]).item(),
    ]);
    let d = only(&diags, codes::NON_EXHAUSTIVE_MATCH);
    assert!(
        d.labels[0].message.contains("the empty list"),
        "{}",
        d.labels[0].message
    );
}

#[test]
fn fixed_length_arms_never_cover_the_longer_lists() {
    let diags = check_err(vec![
        on_int_list(vec![
            (plist(vec![], None), int(0)),
            (plist(vec![pvar("x")], None), var("x")),
        ])
        .item(),
    ]);
    let d = only(&diags, codes::NON_EXHAUSTIVE_MATCH);
    assert_eq!(d.labels[0].message, "not covered: lists longer than 1");
}

#[test]
fn a_gap_below_the_open_arm_is_reported_by_length() {
    let diags = check_err(vec![
        on_int_list(vec![
            (plist(vec![], None), int(0)),
            (
                plist(vec![pvar("x"), pvar("y"), pvar("z")], Some(pvar("r"))),
                var("x"),
            ),
        ])
        .item(),
    ]);
    let d = only(&diags, codes::NON_EXHAUSTIVE_MATCH);
    assert_eq!(
        d.labels[0].message,
        "not covered: lists of 1 element, lists of 2 elements"
    );
}

#[test]
fn the_shortest_open_arm_decides_what_the_exact_arms_must_fill() {
    check(vec![
        on_int_list(vec![
            (plist(vec![], None), int(0)),
            (plist(vec![pvar("a"), pvar("b")], None), var("a")),
            (plist(vec![pvar("x")], Some(pvar("r"))), var("x")),
        ])
        .item(),
    ]);
}

#[test]
fn a_refutable_element_does_not_make_an_arm_cover_that_length() {
    let diags = check_err(vec![
        on_int_list(vec![
            (plist(vec![], None), int(0)),
            (
                plist(vec![pat(PatternKind::Lit(Lit::Int(1)))], Some(pvar("r"))),
                int(1),
            ),
        ])
        .item(),
    ]);
    assert!(
        has_code(&diags, codes::NON_EXHAUSTIVE_MATCH),
        "{}",
        render(&diags)
    );
}

fn pair_record(fields: Vec<(&str, Pattern)>, rest: bool) -> Pattern {
    pat(PatternKind::Record {
        fields: fields.into_iter().map(|(n, p)| (id(n), p)).collect(),
        rest,
    })
}

fn pair_ty() -> TypeExpr {
    TypeExpr::Record {
        fields: vec![
            (id("first"), con("Int", vec![])),
            (id("second"), con("Int", vec![])),
        ],
        span: any(),
    }
}

fn on_pair(arms: Vec<(Pattern, Expr)>) -> FnBuilder {
    func("f", &["p"], match_on(var("p"), arms))
        .param_types(vec![Some(pair_ty())])
        .ret(con("Int", vec![]))
}

#[test]
fn one_record_arm_naming_every_field_is_exhaustive() {
    let out = check(vec![
        on_pair(vec![(
            pair_record(vec![("first", pvar("a")), ("second", pvar("b"))], false),
            add(var("a"), var("b")),
        )])
        .item(),
    ]);
    assert_eq!(sig(&out, "f"), "({first: Int, second: Int}) -> Int");
}

#[test]
fn a_record_arm_with_rest_is_exhaustive_without_naming_every_field() {
    check(vec![
        on_pair(vec![(
            pair_record(vec![("first", pvar("a"))], true),
            var("a"),
        )])
        .item(),
    ]);
}

/// The evaluator only matches a `..`-less record pattern against a record with
/// exactly those fields, so accepting a subset here would compile to an arm
/// that silently never fires.
#[test]
fn a_record_pattern_without_rest_must_name_every_field() {
    let diags = check_err(vec![
        on_pair(vec![
            (pair_record(vec![("first", pvar("a"))], false), var("a")),
            (pat(PatternKind::Wildcard), int(0)),
        ])
        .item(),
    ]);
    let d = only(&diags, codes::TYPE_MISMATCH);
    assert_eq!(d.message, "record pattern does not name every field");
    assert!(
        d.labels[0].message.contains("`second`"),
        "{}",
        d.labels[0].message
    );
}

#[test]
fn a_refutable_record_field_does_not_make_the_arm_irrefutable() {
    let diags = check_err(vec![
        on_pair(vec![(
            pair_record(
                vec![
                    ("first", pat(PatternKind::Lit(Lit::Int(1)))),
                    ("second", pvar("b")),
                ],
                false,
            ),
            var("b"),
        )])
        .item(),
    ]);
    assert!(
        has_code(&diags, codes::NON_EXHAUSTIVE_MATCH),
        "{}",
        render(&diags)
    );
}

#[test]
fn comparing_two_functions_is_rejected_before_it_can_run() {
    let diags = check_err(vec![
        func("f", &["x"], var("x")).item(),
        func(
            "cmp",
            &[],
            ex(ExprKind::Binary {
                op: BinOp::Eq,
                lhs: Box::new(var("f")),
                rhs: Box::new(var("f")),
            }),
        )
        .item(),
    ]);
    let d = only(&diags, codes::TYPE_MISMATCH);
    assert_eq!(d.message, "functions cannot be compared for equality");
}

#[test]
fn a_function_buried_in_a_compared_value_is_still_rejected() {
    let boxed = |name: &str| {
        ex(ExprKind::Record {
            fields: vec![(id("run"), var(name))],
        })
    };
    let diags = check_err(vec![
        func("f", &["x"], var("x")).item(),
        func(
            "cmp",
            &[],
            ex(ExprKind::Binary {
                op: BinOp::Ne,
                lhs: Box::new(boxed("f")),
                rhs: Box::new(boxed("f")),
            }),
        )
        .item(),
    ]);
    assert!(has_code(&diags, codes::TYPE_MISMATCH), "{}", render(&diags));
}

#[test]
fn comparing_ordinary_values_stays_legal() {
    let out = check(vec![
        func(
            "cmp",
            &["a", "b"],
            ex(ExprKind::Binary {
                op: BinOp::Eq,
                lhs: Box::new(ex(ExprKind::List {
                    items: vec![var("a")],
                })),
                rhs: Box::new(ex(ExprKind::List {
                    items: vec![var("b")],
                })),
            }),
        )
        .item(),
    ]);
    assert_eq!(sig(&out, "cmp"), "<a>(a, a) -> Bool");
}

// Cross-module checking. These parse real source rather than building the AST,
// because what is under test is how imports, `pub` and `::` reach inference.

/// Parsed and expanded, in that order and never one without the other: the
/// driver expands a `derive` before it resolves anything, so a harness that
/// skipped it would check a program the compiler never sees and would report a
/// generated definition as an unknown name.
///
/// Expansion failing is a defect in a fixture rather than a case under test —
/// the derivers' own negative cases live in `ply-core/tests/derivation.rs`,
/// against `ply_derive::expand_program` directly.
fn parse_program(files: &[(&str, &str)]) -> Program {
    let mut program = Program {
        modules: files
            .iter()
            .enumerate()
            .map(|(i, (name, text))| {
                ply_syntax::parse_module(SourceId(i as u32), ModuleName::from_dotted(name), text)
                    .unwrap_or_else(|d| panic!("`{name}` should parse: {}", render(&d)))
            })
            .collect(),
    };
    let diags = ply_derive::expand_program(&mut program);
    assert!(
        diags.is_empty(),
        "expected every `derive` to expand: {}",
        render(&diags)
    );
    program
}

fn check_files(files: &[(&str, &str)]) -> CheckOutput {
    let program = parse_program(files);
    let resolved = ply_syntax::resolve(&program)
        .unwrap_or_else(|d| panic!("expected resolution to succeed: {}", render(&d)));
    match crate::check_program(&program, &resolved) {
        Ok(out) => out,
        Err(diags) => panic!("expected success, got: {}", render(&diags)),
    }
}

fn check_files_err(files: &[(&str, &str)]) -> Vec<Diagnostic> {
    let program = parse_program(files);
    match ply_syntax::resolve(&program) {
        Err(diags) => diags,
        Ok(resolved) => match crate::check_program(&program, &resolved) {
            Ok(_) => panic!("expected failure, but the program checked"),
            Err(diags) => diags,
        },
    }
}

#[test]
fn a_definition_resolves_through_three_modules() {
    let out = check_files(&[
        ("base", "pub fn one() -> Int = 1"),
        (
            "middle",
            "import base (one)\npub fn two() -> Int = one() + one()",
        ),
        (
            "top",
            "import middle\nfn four() -> Int = middle::two() + middle::two()",
        ),
    ]);

    assert_eq!(sig(&out, "base.one"), "() -> Int");
    assert_eq!(sig(&out, "middle.two"), "() -> Int");
    assert_eq!(sig(&out, "top.four"), "() -> Int");
    assert!(out.defs.contains_key(&Symbol::new("top.four")));
    assert!(!out.defs.contains_key(&Symbol::new("four")));
}

#[test]
fn a_diamond_import_reaches_one_definition_by_both_paths() {
    let out = check_files(&[
        ("base", "pub fn one() -> Int = 1"),
        ("left", "import base (one)\npub fn l() -> Int = one()"),
        ("right", "import base\npub fn r() -> Int = base::one()"),
        (
            "top",
            "import left\nimport right\nfn t() -> Int = left::l() + right::r()",
        ),
    ]);

    assert_eq!(sig(&out, "top.t"), "() -> Int");
    assert_eq!(out.defs.len(), 4);
    assert_eq!(out.modules.len(), 4);
    assert_eq!(
        out.modules[&Symbol::new("top")].imports,
        vec![
            ModuleName::from_dotted("left"),
            ModuleName::from_dotted("right")
        ]
    );
}

#[test]
fn a_private_definition_cannot_be_called_from_another_module() {
    let diags = check_files_err(&[
        (
            "store",
            "fn secret() -> Int = 1\npub fn place() -> Int = secret()",
        ),
        ("app", "import store\nfn f() -> Int = store::secret()"),
    ]);
    let d = only(&diags, codes::PRIVATE_NAME);
    assert!(
        d.message.contains("private to module `store`"),
        "{}",
        d.message
    );
    assert!(
        d.notes.iter().any(|n| n.contains("pub fn secret")),
        "the fix must name the module that would have to export it: {:?}",
        d.notes
    );
}

#[test]
fn a_module_cycle_is_rejected_before_anything_is_inferred() {
    let diags = check_files_err(&[
        ("a", "import b\npub fn f() -> Int = b::g()"),
        ("b", "import a\npub fn g() -> Int = a::f()"),
    ]);
    let d = only(&diags, codes::MODULE_CYCLE);
    assert!(d.message.contains("`a` -> `b` -> `a`"), "{}", d.message);
}

#[test]
fn an_imported_name_colliding_with_a_local_one_is_ambiguous() {
    let diags = check_files_err(&[
        ("store", "pub fn place() -> Int = 1"),
        ("app", "import store (place)\nfn place() -> Int = 2"),
    ]);
    assert!(
        has_code(&diags, codes::AMBIGUOUS_IMPORT),
        "{}",
        render(&diags)
    );
}

#[test]
fn a_local_binder_beats_a_module_item_which_beats_the_prelude() {
    let out = check_files(&[(
        "app",
        "fn len(x: Int) -> Int = x + 1\n\
         fn shadowed(len: Int) -> Int = len\n\
         fn module_item() -> Int = len(1)",
    )]);

    // The parameter wins over the module's own `len`...
    assert_eq!(sig(&out, "app.shadowed"), "(Int) -> Int");
    // ...and the module's own `len` wins over the prelude's `List<a> -> Int`.
    assert_eq!(sig(&out, "app.module_item"), "() -> Int");
}

#[test]
fn the_prelude_is_still_reachable_where_no_module_item_shadows_it() {
    let out = check_files(&[("app", "fn count(xs: List<Int>) -> Int = len(xs)")]);
    assert_eq!(sig(&out, "app.count"), "(List<Int>) -> Int");
}

#[test]
fn a_local_named_like_a_module_binder_does_not_hide_it() {
    let out = check_files(&[
        ("orders", "pub fn place() -> Int = 7"),
        (
            "app",
            "import orders\nfn f(orders: Int) -> Int = orders + orders::place()",
        ),
    ]);
    assert_eq!(sig(&out, "app.f"), "(Int) -> Int");
}

#[test]
fn two_modules_may_declare_the_same_effect_without_contending() {
    let out = check_files(&[
        (
            "left",
            "pub effect db { read get[r](key: Int) -> Int }\npub fn read_one() -> Int = db.get[users](1)",
        ),
        (
            "right",
            "pub effect db { read get[r](key: Int) -> Int }\npub fn read_one() -> Int = db.get[users](1)",
        ),
    ]);

    assert_eq!(footprint(&out, "left.read_one"), "{left.db.read[users]}");
    assert_eq!(footprint(&out, "right.read_one"), "{right.db.read[users]}");
    assert!(
        !def(&out, "left.read_one")
            .footprint
            .conflicts_with(&def(&out, "right.read_one").footprint)
    );
}

#[test]
fn one_effect_shared_by_two_modules_keeps_its_resource_labels_contending() {
    let out = check_files(&[
        (
            "store",
            "pub effect db { read get[r](key: Int) -> Int\n  write put[r](key: Int, value: Int) -> Unit }",
        ),
        (
            "reader",
            "import store (db)\npub fn r() -> Int = db.get[users](1)",
        ),
        (
            "writer",
            "import store (db)\npub fn w() -> Unit = db.put[users](1, 2)",
        ),
    ]);

    assert_eq!(footprint(&out, "reader.r"), "{store.db.read[users]}");
    assert_eq!(footprint(&out, "writer.w"), "{store.db.write[users]}");
    assert!(
        def(&out, "reader.r")
            .footprint
            .conflicts_with(&def(&out, "writer.w").footprint),
        "a resource label is a claim about the world, not about a file"
    );
}

#[test]
fn a_qualified_effect_can_be_performed_and_handled() {
    let out = check_files(&[
        ("store", "pub effect db { read get[r](key: Int) -> Int }"),
        (
            "app",
            "import store\n\
             fn reads() -> Int = store::db.get[users](1)\n\
             fn handled() -> Int = handle reads() with { store::db.get[users](k) -> k }",
        ),
    ]);

    assert_eq!(footprint(&out, "app.reads"), "{store.db.read[users]}");
    assert_eq!(footprint(&out, "app.handled"), "{}");
}

#[test]
fn a_constructor_crosses_a_module_boundary_in_expressions_and_patterns() {
    let out = check_files(&[
        ("shapes", "pub type Shape = Circle(Int) | Square(Int)"),
        (
            "app",
            "import shapes\n\
             fn make() -> shapes::Shape = shapes::Circle(2)\n\
             fn area(s: shapes::Shape) -> Int = match s { shapes::Circle(r) -> r * r, shapes::Square(w) -> w * w }",
        ),
    ]);

    assert_eq!(sig(&out, "app.make"), "() -> shapes.Shape");
    assert_eq!(sig(&out, "app.area"), "(shapes.Shape) -> Int");
    assert!(out.ctors.contains_key(&Symbol::new("shapes.Circle")));
}

#[test]
fn a_private_constructor_is_rejected_at_the_pattern() {
    let diags = check_files_err(&[
        ("shapes", "type Shape = Circle(Int)"),
        (
            "app",
            "import shapes\nfn f(s: Int) -> Int = match s { shapes::Circle(r) -> r, _ -> 0 }",
        ),
    ]);
    assert!(has_code(&diags, codes::PRIVATE_NAME), "{}", render(&diags));
}

#[test]
fn a_public_alias_expands_in_the_module_that_wrote_it() {
    let out = check_files(&[
        ("money", "type Cents = Int\npub type Money = Cents"),
        (
            "app",
            "import money\nfn total(m: money::Money) -> Int = m + 1",
        ),
    ]);

    // `Cents` is private to `money`, so only expanding the alias in its own
    // module's scope can give `Money` a meaning here.
    assert_eq!(sig(&out, "app.total"), "(Int) -> Int");
}

#[test]
fn an_unimported_module_binder_is_an_unknown_module() {
    let diags = check_files_err(&[
        ("store", "pub fn place() -> Int = 1"),
        ("app", "fn f() -> Int = store::place()"),
    ]);
    let d = only(&diags, codes::UNKNOWN_MODULE);
    assert!(
        d.notes.iter().any(|n| n.contains("import store")),
        "{:?}",
        d.notes
    );
}

#[test]
fn a_name_that_is_exported_elsewhere_says_which_import_would_fix_it() {
    let diags = check_files_err(&[
        ("store", "pub fn place() -> Int = 1"),
        ("app", "fn f() -> Int = place()"),
    ]);
    let d = only(&diags, codes::UNKNOWN_NAME);
    assert!(
        d.notes.iter().any(|n| n.contains("import store (place)")),
        "{:?}",
        d.notes
    );
}

#[test]
fn tests_are_keyed_by_module_so_two_labels_may_repeat() {
    let out = check_files(&[
        ("left", "test \"it works\" { assert(true) }"),
        ("right", "test \"it works\" { assert(true) }"),
    ]);

    assert_eq!(out.tests.len(), 2);
    let keys: Vec<&str> = out.tests.iter().map(|t| t.key.as_str()).collect();
    assert_eq!(keys, vec!["left.it works", "right.it works"]);
    assert_eq!(out.tests[0].index, 0);
    assert_eq!(out.tests[1].index, 1);
}

#[test]
fn a_nondet_effect_stays_nondet_across_a_module_boundary() {
    let diags = check_files_err(&[
        ("clock", "pub nondet effect clock { read now() -> Int }"),
        (
            "app",
            "import clock\ntest \"reads the clock\" { assert(clock::clock.now() > 0) }",
        ),
    ]);
    let d = only(&diags, codes::NONDET_IN_DET_TEST);
    // The suggested handler has to be writable in the file it is suggested to:
    // `clock.clock` is the program-wide name and is not syntax.
    assert!(
        d.notes.iter().any(|n| n.contains("clock::clock.now()")),
        "{:?}",
        d.notes
    );
}

#[test]
fn a_suggestion_for_a_selectively_imported_effect_stays_unqualified() {
    let diags = check_files_err(&[
        ("timing", "pub nondet effect clock { read now() -> Int }"),
        (
            "app",
            "import timing (clock)\ntest \"reads\" { assert(clock.now() > 0) }",
        ),
    ]);
    let d = only(&diags, codes::NONDET_IN_DET_TEST);
    assert!(
        d.notes
            .iter()
            .any(|n| n.contains("{ clock.now() -> <value> }")),
        "{:?}",
        d.notes
    );
}

#[test]
fn identical_definitions_in_two_modules_are_kept_apart_by_key_alone() {
    let out = check_files(&[
        ("left", "pub fn twice(x: Int) -> Int = x + x"),
        ("right", "pub fn twice(x: Int) -> Int = x + x"),
    ]);

    assert_eq!(sig(&out, "left.twice"), "(Int) -> Int");
    assert_eq!(sig(&out, "right.twice"), "(Int) -> Int");
    assert_eq!(def(&out, "left.twice").simple_name.as_str(), "twice");
    assert_eq!(
        def(&out, "right.twice").module,
        ModuleName::from_dotted("right")
    );
}

#[test]
fn a_single_module_check_still_leaves_every_name_bare() {
    let out = check(vec![func("f", &[], int(1)).item()]);
    assert!(out.defs.contains_key(&Symbol::new("f")));
    assert_eq!(out.modules.len(), 1);
    assert!(out.modules[&Symbol::new("")].name.is_anonymous());
}

#[test]
fn a_selective_import_shadows_the_prelude_without_being_ambiguous() {
    let out = check_files(&[
        ("mine", "pub fn len(x: Int) -> Int = x"),
        ("app", "import mine (len)\nfn f() -> Int = len(3)"),
    ]);
    assert_eq!(sig(&out, "app.f"), "() -> Int");
}

#[test]
fn a_qualified_name_in_the_wrong_namespace_says_what_the_module_exports() {
    let diags = check_files_err(&[
        ("shapes", "pub type Shape = Circle(Int)"),
        ("app", "import shapes\nfn f() -> Int = shapes::Shape"),
    ]);
    let d = only(&diags, codes::UNKNOWN_NAME);
    assert!(
        d.message.contains("has no definition `Shape`"),
        "{}",
        d.message
    );
    assert!(
        d.notes.iter().any(|n| n.contains("`Circle`")),
        "{:?}",
        d.notes
    );
}

/// The example corpus is the one place cross-module resolution meets real code:
/// `tests/fixtures/` holds the programs that are meant to fail, so anything
/// here failing to check is a regression rather than a fixture.
#[test]
fn the_example_corpus_checks_as_one_program() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let mut paths: Vec<std::path::PathBuf> = std::fs::read_dir(&root)
        .expect("the example corpus is part of the repository")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|e| e == "ply"))
        .collect();
    paths.sort();

    let sources: Vec<(String, String)> = paths
        .iter()
        .map(|path| {
            let name = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let text = std::fs::read_to_string(path).expect("example is readable");
            (name, text)
        })
        .collect();
    let mut files: Vec<(&str, &str)> = sources
        .iter()
        .map(|(name, text)| (name.as_str(), text.as_str()))
        .collect();
    // The example corpus imports `std.net`. `ply` pulls a shipped module in on
    // demand; this harness has no import graph to walk, so it loads the set.
    for (name, source) in ply_std::sources() {
        files.push((name, source));
    }

    let out = check_files(&files);
    assert_eq!(out.modules.len(), files.len());
    assert!(
        out.defs.keys().all(|k| k.as_str().contains('.')),
        "every definition is keyed by its program-wide name"
    );
    assert!(
        out.tests
            .iter()
            .all(|t| t.key.as_str().starts_with(t.module.as_str())),
        "a test key is `<module>.<label>`, which is what keeps two labels apart"
    );
}

// ---------------------------------------------------------------------------
// The prelude effects and `simulate`.
// ---------------------------------------------------------------------------

#[test]
fn task_spawn_answers_a_task_and_join_unwraps_it() {
    let body = block(
        vec![let_(
            "t",
            perform("task", "spawn", None, vec![lambda(&[], int(1))]),
        )],
        Some(perform("task", "join", None, vec![var("t")])),
    );
    let out = check(vec![func("f", &[], simulate(body)).item()]);
    assert_eq!(sig(&out, "f"), "() -> Int / {sim.read}");
}

#[test]
fn joining_something_that_is_not_a_task_is_a_type_error() {
    let def = func("f", &[], perform("task", "join", None, vec![int(1)])).item();
    let diags = check_err(vec![def]);
    assert!(has_code(&diags, codes::TYPE_MISMATCH), "{}", render(&diags));
}

#[test]
fn every_prelude_operation_types_at_its_declared_signature() {
    let out = check(vec![
        func("y", &[], perform("task", "yield", None, vec![])).item(),
        func("n", &[], perform("clock", "now", None, vec![])).item(),
        func("s", &[], perform("clock", "sleep", None, vec![int(5)])).item(),
        func("r", &[], perform("random", "next", None, vec![])).item(),
        func("b", &[], perform("random", "below", None, vec![int(6)])).item(),
        func("d", &[], perform("sim", "seed", None, vec![])).item(),
    ]);
    assert_eq!(sig(&out, "y"), "() -> Unit / {task.write}");
    assert_eq!(sig(&out, "n"), "() -> Int / {clock.read}");
    assert_eq!(sig(&out, "s"), "() -> Unit / {clock.write}");
    assert_eq!(sig(&out, "r"), "() -> Int / {random.write}");
    assert_eq!(sig(&out, "b"), "() -> Int / {random.write}");
    assert_eq!(sig(&out, "d"), "() -> Int / {sim.read}");
}

/// Without `e` on `spawn`'s own row a test that spawns a writer of `orders`
/// would report an empty footprint, and the cross-test conflict graph would run
/// it beside a reader of `orders`.
#[test]
fn the_effects_of_a_spawned_body_land_in_the_spawners_row() {
    let spawned = lambda(
        &[],
        perform("db", "put", Some("orders"), vec![int(1), int(2)]),
    );
    let body = block(
        vec![let_("t", perform("task", "spawn", None, vec![spawned]))],
        Some(perform("task", "join", None, vec![var("t")])),
    );
    let out = check(vec![db_effect(), func("f", &[], body).item()]);
    assert_eq!(footprint(&out, "f"), "{db.write[orders], task.write}");
}

#[test]
fn two_spawns_of_different_resources_union_rather_than_unify() {
    let north = lambda(
        &[],
        perform("db", "put", Some("north"), vec![int(1), int(2)]),
    );
    let south = lambda(
        &[],
        perform("db", "put", Some("south"), vec![int(1), int(2)]),
    );
    let body = block(
        vec![
            let_("a", perform("task", "spawn", None, vec![north])),
            let_("b", perform("task", "spawn", None, vec![south])),
        ],
        Some(perform("task", "join", None, vec![var("a")])),
    );
    let out = check(vec![db_effect(), func("f", &[], body).item()]);
    assert_eq!(
        footprint(&out, "f"),
        "{db.write[north], db.write[south], task.write}"
    );
}

#[test]
fn a_region_discharges_task_clock_and_random_and_publishes_the_seed() {
    let body = block(
        vec![
            Stmt::Expr(perform("task", "yield", None, vec![])),
            Stmt::Expr(perform("clock", "now", None, vec![])),
            Stmt::Expr(perform("clock", "sleep", None, vec![int(1)])),
            Stmt::Expr(perform("random", "next", None, vec![])),
        ],
        Some(int(0)),
    );
    let out = check(vec![func("f", &[], simulate(body)).item()]);
    assert_eq!(footprint(&out, "f"), "{sim.read}");
}

/// The language does not get to claim it simulated an effect it has never heard
/// of, and that is the safety property that survives M7.
#[test]
fn a_region_discharges_nothing_a_user_declared() {
    let body = block(
        vec![Stmt::Expr(perform("task", "yield", None, vec![]))],
        Some(perform("wall", "now", None, vec![])),
    );
    let out = check(vec![wall_effect(), func("f", &[], simulate(body)).item()]);
    assert_eq!(footprint(&out, "f"), "{sim.read, wall.read}");
}

/// Cells are world state. A `with_cell` outside a region holding state the tasks
/// inside share is exactly how two tasks share memory, so the region must leave
/// those atoms to the region boundary that owns them.
#[test]
fn a_region_discharges_the_five_simulated_atoms_and_no_cell() {
    let handled: Vec<String> = crate::prelude::simulated_atoms()
        .iter()
        .map(|a| a.to_string())
        .collect();
    assert_eq!(
        handled,
        ["clock.read", "clock.write", "random.write", "task.write",]
    );

    let body = with_cell(
        "users",
        int(0),
        "c",
        simulate(block(
            vec![Stmt::Expr(perform("task", "yield", None, vec![]))],
            Some(call("cell_get", vec![var("c")])),
        )),
    );
    let out = check(vec![func("f", &[], body).item()]);
    assert_eq!(footprint(&out, "f"), "{sim.read}");
}

#[test]
fn a_det_test_that_spawns_with_no_scheduler_is_e0412() {
    let body = block(
        vec![let_(
            "t",
            perform_at("task", "spawn", None, vec![lambda(&[], int(1))], 31),
        )],
        Some(perform("task", "join", None, vec![var("t")])),
    );
    let diags = check_err(vec![test_def("unscheduled", false, body)]);
    let d = only(&diags, codes::NONDET_IN_DET_TEST);
    let primary = d.labels.iter().find(|l| l.primary).unwrap();
    assert_eq!(primary.span.start, 31);
    assert!(
        primary.message.contains("task.write"),
        "{}",
        primary.message
    );
    assert!(
        d.notes.iter().any(|n| n.contains("simulate { <body> }")),
        "the language ships the handler, so that is what to point at: {:?}",
        d.notes
    );
}

#[test]
fn clock_now_outside_any_region_in_a_det_test_is_still_e0412() {
    let body = block(vec![], Some(perform_at("clock", "now", None, vec![], 44)));
    let diags = check_err(vec![test_def("reads the wall", false, body)]);
    let d = only(&diags, codes::NONDET_IN_DET_TEST);
    let primary = d.labels.iter().find(|l| l.primary).unwrap();
    assert_eq!(primary.span.start, 44);
    assert!(
        primary.message.contains("clock.read"),
        "{}",
        primary.message
    );
}

/// The other direction of the same rule: `simulate` is a handler, handlers
/// discharge, and `sim` is not `nondet` — so a time-dependent, concurrent test
/// is an ordinary `det`, cacheable one.
#[test]
fn a_simulated_test_is_deterministic_and_carries_only_the_seed() {
    let spawned = lambda(&[], perform("clock", "now", None, vec![]));
    let body = block(
        vec![
            let_("t", perform("task", "spawn", None, vec![spawned])),
            Stmt::Expr(perform("random", "next", None, vec![])),
        ],
        Some(perform("task", "join", None, vec![var("t")])),
    );
    let out = check(vec![test_def("simulated", false, simulate(body))]);
    assert_eq!(out.tests[0].footprint.to_string(), "{sim.read}");
    assert!(!out.tests[0].nondet);
}

#[test]
fn a_users_own_nondet_effect_inside_a_region_is_still_e0412() {
    let body = block(
        vec![Stmt::Expr(perform("task", "yield", None, vec![]))],
        Some(perform_at("wall", "now", None, vec![], 66)),
    );
    let diags = check_err(vec![
        wall_effect(),
        test_def("half simulated", false, simulate(body)),
    ]);
    let d = only(&diags, codes::NONDET_IN_DET_TEST);
    let primary = d.labels.iter().find(|l| l.primary).unwrap();
    assert_eq!(primary.span.start, 66);
    assert!(primary.message.contains("wall.read"), "{}", primary.message);
}

/// A `Task` is a key into the region's scheduler, and the scheduler dies with
/// the region. This is the same result-type check `with_cell` uses.
#[test]
fn a_task_in_a_regions_result_type_is_e0413() {
    let body = block(
        vec![],
        Some(perform("task", "spawn", None, vec![lambda(&[], int(1))])),
    );
    let diags = check_err(vec![func("f", &[], simulate_at(body, 12)).item()]);
    let d = only(&diags, codes::TASK_ESCAPES_SCOPE);
    assert!(
        d.labels
            .iter()
            .any(|l| l.primary && l.message.contains("Task")),
        "{:?}",
        d.labels
    );
}

#[test]
fn a_task_nested_inside_the_result_value_is_also_e0413() {
    let spawn = perform("task", "spawn", None, vec![lambda(&[], int(1))]);
    let body = block(vec![], Some(ex(ExprKind::List { items: vec![spawn] })));
    let diags = check_err(vec![func("f", &[], simulate(body)).item()]);
    assert!(
        has_code(&diags, codes::TASK_ESCAPES_SCOPE),
        "{}",
        render(&diags)
    );
}

#[test]
fn a_region_inside_a_region_is_e0416_lexically() {
    let inner = simulate_at(block(vec![], Some(int(1))), 40);
    let diags = check_err(vec![func("f", &[], simulate_at(inner, 10)).item()]);
    let d = only(&diags, codes::NESTED_SIMULATION);
    assert_eq!(d.primary_span().unwrap().start, 10);
    assert!(
        d.labels.iter().any(|l| !l.primary && l.span.start == 40),
        "the nested region is named too: {:?}",
        d.labels
    );
}

#[test]
fn a_region_that_reaches_one_through_a_call_is_e0416_as_well() {
    let inner = func("inner", &[], simulate(block(vec![], Some(int(1))))).item();
    let reached = ex_at(
        ExprKind::App {
            func: Box::new(var("inner")),
            args: vec![],
        },
        71,
    );
    let outer = func("outer", &[], simulate_at(reached, 70)).item();
    let diags = check_err(vec![inner, outer]);
    let d = only(&diags, codes::NESTED_SIMULATION);
    assert_eq!(d.primary_span().unwrap().start, 70);
    assert!(
        d.notes.iter().any(|n| n.contains("calls")),
        "the transitive case says so: {:?}",
        d.notes
    );
}

/// A handler answering `sim.seed()` with a constant pins one known-interesting
/// seed as an ordinary regression test, whose outcome is a function of the
/// definition set alone.
#[test]
fn a_handler_answering_sim_seed_closes_the_seed_out_of_the_row() {
    let region = simulate(block(
        vec![Stmt::Expr(perform("task", "yield", None, vec![]))],
        Some(int(0)),
    ));
    let body = handle(region, vec![clause("sim", "seed", None, &[], int(7))]);
    let out = check(vec![test_def("pinned seed", false, body)]);
    assert_eq!(out.tests[0].footprint.to_string(), "{}");
}

/// Every `task` operation performs the one atom `task.write`, so a handler that
/// covers any of them discharges the effect for the whole body. That is what
/// lets a sequential scheduler written in Ply stand where the seeded one does.
#[test]
fn a_user_written_task_handler_discharges_the_effect() {
    let spawned = block(
        vec![let_(
            "t",
            perform("task", "spawn", None, vec![lambda(&[], int(1))]),
        )],
        Some(perform("task", "join", None, vec![var("t")])),
    );
    let body = handle(
        spawned,
        vec![clause(
            "task",
            "yield",
            None,
            &[],
            ex(ExprKind::Lit(Lit::Unit)),
        )],
    );
    let out = check(vec![test_def("sequential scheduler", false, body)]);
    assert_eq!(out.tests[0].footprint.to_string(), "{}");
    assert!(!out.tests[0].nondet);
}

#[test]
fn an_effect_claiming_a_prelude_name_is_a_duplicate_definition() {
    for name in ["task", "clock", "random", "sim"] {
        let diags = check_err(vec![effect_def(
            name,
            true,
            vec![op("go", Mode::Read, false, vec![], con("Int", vec![]))],
        )]);
        let d = only(&diags, codes::DUPLICATE_DEFINITION);
        assert!(
            d.message.contains(name) && d.message.contains("prelude"),
            "{}",
            d.message
        );
    }
}

/// The prelude is consulted last, so a module's own declaration wins — which is
/// what leaves `examples/clock.ply` uninvolved.
#[test]
fn a_modules_own_clock_shadows_the_prelude() {
    let out = check_files(&[(
        "clock",
        "nondet effect clock { read now() -> Int }\n\
         fn f() -> Int = clock.now()\n",
    )]);
    assert_eq!(footprint(&out, "clock.f"), "{clock.clock.read}");
}

#[test]
fn a_byte_literal_has_its_own_type_and_never_unifies_with_a_string() {
    let out = check(vec![func("m", &[], bytes_lit(b"GET")).item()]);
    assert_eq!(sig(&out, "m"), "() -> Bytes");

    let mixed = check_err(vec![
        func(
            "m",
            &[],
            call("string_concat", vec![bytes_lit(b"a"), str_lit("b")]),
        )
        .item(),
    ]);
    only(&mixed, codes::TYPE_MISMATCH);
}

/// The whole surface at once: a signature that moved would otherwise be caught
/// only by whichever downstream test happened to use it.
#[test]
fn the_bytes_and_string_builtins_have_the_types_the_contract_states() {
    let expected = [
        ("bytes_len", "(Bytes) -> Int"),
        ("bytes_at", "(Bytes, Int) -> Int"),
        ("bytes_slice", "(Bytes, Int, Int) -> Bytes"),
        ("bytes_concat", "(Bytes, Bytes) -> Bytes"),
        ("bytes_of_string", "(String) -> Bytes"),
        ("bytes_is_utf8", "(Bytes) -> Bool"),
        ("bytes_index_of", "(Bytes, Bytes) -> Option<Int>"),
        ("bytes_index_of_from", "(Bytes, Bytes, Int) -> Option<Int>"),
        ("bytes_index_of_byte", "(Bytes, Int) -> Option<Int>"),
        ("bytes_starts_with", "(Bytes, Bytes) -> Bool"),
        ("bytes_ends_with", "(Bytes, Bytes) -> Bool"),
        ("bytes_split", "(Bytes, Bytes) -> List<Bytes>"),
        ("bytes_scan", "(Bytes, Int, Bytes, Int) -> Int"),
        ("bytes_scan_until", "(Bytes, Int, Bytes, Int) -> Int"),
        ("string_of_bytes", "(Bytes) -> String"),
        ("string_of_bytes_lossy", "(Bytes) -> String"),
        ("string_len", "(String) -> Int"),
        ("string_slice", "(String, Int, Int) -> String"),
        ("string_split", "(String, String) -> List<String>"),
        ("string_trim", "(String) -> String"),
        ("string_lower", "(String) -> String"),
        ("string_upper", "(String) -> String"),
        ("string_starts_with", "(String, String) -> Bool"),
        ("string_ends_with", "(String, String) -> Bool"),
        ("string_contains", "(String, String) -> Bool"),
        ("string_find", "(String, String) -> Int"),
    ];
    // `fn probe_f() = f` returns the builtin itself, so the printed signature
    // of the probe carries the builtin's whole type.
    let items: Vec<Item> = expected
        .iter()
        .map(|(name, _)| func(&format!("probe_{name}"), &[], var(name)).item())
        .collect();
    let out = check(items);
    for (name, ty) in expected {
        assert_eq!(
            sig(&out, &format!("probe_{name}")),
            format!("() -> {ty}"),
            "{name}"
        );
    }
}

#[test]
fn the_bytes_type_is_a_builtin_and_cannot_be_redefined() {
    let diags = check_err(vec![Item::Type(Box::new(TypeDef {
        vis: Visibility::Private,
        name: id("Bytes"),
        params: vec![],
        body: TypeDefBody::Alias(con("Int", vec![])),
        span: any(),
    }))]);
    let d = only(&diags, codes::DUPLICATE_DEFINITION);
    assert!(d.message.contains("builtin type"), "{}", d.message);
}

#[test]
fn the_task_type_is_a_builtin_and_cannot_be_redefined() {
    let diags = check_err(vec![Item::Type(Box::new(TypeDef {
        vis: Visibility::Private,
        name: id("Task"),
        params: vec![],
        body: TypeDefBody::Alias(con("Int", vec![])),
        span: any(),
    }))]);
    let d = only(&diags, codes::DUPLICATE_DEFINITION);
    assert!(d.message.contains("builtin type"), "{}", d.message);
}

#[test]
fn a_written_task_type_annotation_agrees_with_what_spawn_answers() {
    let body = block(
        vec![Stmt::Let {
            pat: Pattern {
                kind: PatternKind::Var(id("t")),
                span: any(),
            },
            ty: Some(con("Task", vec![con("Int", vec![])])),
            value: Box::new(perform("task", "spawn", None, vec![lambda(&[], int(1))])),
            span: any(),
        }],
        Some(perform("task", "join", None, vec![var("t")])),
    );
    let out = check(vec![func("f", &[], simulate(body)).item()]);
    assert_eq!(sig(&out, "f"), "() -> Int / {sim.read}");
}

#[test]
fn a_task_of_the_wrong_element_type_is_rejected() {
    let body = block(
        vec![Stmt::Let {
            pat: Pattern {
                kind: PatternKind::Var(id("t")),
                span: any(),
            },
            ty: Some(con("Task", vec![con("Bool", vec![])])),
            value: Box::new(perform("task", "spawn", None, vec![lambda(&[], int(1))])),
            span: any(),
        }],
        Some(perform("task", "join", None, vec![var("t")])),
    );
    let diags = check_err(vec![func("f", &[], simulate(body)).item()]);
    assert!(has_code(&diags, codes::TYPE_MISMATCH), "{}", render(&diags));
}

/// The atom propagates through calls by the ordinary row rules, so a test whose
/// closure reaches a region carries the seed with no new analysis.
#[test]
fn the_seed_atom_propagates_through_an_ordinary_call() {
    let helper = func("run", &[], simulate(block(vec![], Some(int(1))))).item();
    let out = check(vec![
        helper,
        func("caller", &[], call("run", vec![])).item(),
        test_def("through a call", false, call("run", vec![])),
    ]);
    assert_eq!(footprint(&out, "caller"), "{sim.read}");
    assert_eq!(out.tests[0].footprint.to_string(), "{sim.read}");
    assert!(!out.tests[0].nondet);
}

/// The three simulation fixtures that fail in the front end. `tests/fixtures/`
/// owes one program per code, and a fixture that stopped producing its code
/// would otherwise sit there looking like documentation.
#[test]
fn the_simulation_fixtures_produce_the_codes_they_are_named_for() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
    for (file, code, expected) in [
        ("unscheduled_task.ply", codes::NONDET_IN_DET_TEST, 1),
        ("task_escapes_scope.ply", codes::TASK_ESCAPES_SCOPE, 1),
        // Lexically, and through a call: one row-membership question answers
        // both, so the fixture writes both and both must fire.
        ("nested_simulation.ply", codes::NESTED_SIMULATION, 2),
    ] {
        let text = std::fs::read_to_string(root.join(file))
            .unwrap_or_else(|e| panic!("`{file}` is part of the repository: {e}"));
        let name = file.trim_end_matches(".ply");
        let diags = check_files_err(&[(name, &text)]);
        let found = diags.iter().filter(|d| d.code == code).count();
        assert_eq!(
            found,
            expected,
            "`{file}` should produce {expected} × {code}, got:\n{}",
            render(&diags)
        );
    }
}

// Specs and laws. These parse real source: what is under test is the whole path
// from the clause a person writes to the obligation the prover is handed.

fn parsed(src: &str) -> Module {
    ply_syntax::parse(SRC, src).unwrap_or_else(|d| panic!("should parse: {}", render(&d)))
}

fn check_src(src: &str) -> CheckOutput {
    match check_module(&parsed(src)) {
        Ok(out) => out,
        Err(diags) => panic!("expected success, got: {}", render(&diags)),
    }
}

fn check_src_err(src: &str) -> Vec<Diagnostic> {
    match check_module(&parsed(src)) {
        Ok(_) => panic!("expected failure, but the module checked"),
        Err(diags) => diags,
    }
}

const DB: &str =
    "effect db {\n  read get[r](k: Int) -> Int\n  write put[r](k: Int, v: Int) -> Unit\n}\n";

#[test]
fn every_clause_is_its_own_obligation_in_source_order() {
    let out = check_src(
        "fn withdraw(a: Int, n: Int) -> Int\n\
           requires n > 0\n\
           ensures result == a - n\n\
           requires n <= a\n\
           ensures result <= a\n\
         = a - n",
    );
    let spec = &def(&out, "withdraw").spec;
    let shape: Vec<(&str, usize)> = spec.iter().map(|s| (s.kind.as_str(), s.index)).collect();
    assert_eq!(
        shape,
        [
            ("requires", 0),
            ("ensures", 1),
            ("requires", 2),
            ("ensures", 3)
        ]
    );
    assert!(spec.iter().all(|s| s.footprint.is_empty()));
    assert!(spec.iter().all(|s| !s.span.is_dummy()));
}

#[test]
fn attaching_a_spec_changes_no_footprint() {
    let bare = format!("{DB}fn store(k: Int) -> Unit = db.put[users](k, 1)");
    let specified =
        format!("{DB}fn store(k: Int) -> Unit requires k > 0 ensures k > 0 = db.put[users](k, 1)");
    let bare = check_src(&bare);
    let specified = check_src(&specified);
    assert_eq!(footprint(&bare, "store"), "{db.write[users]}");
    assert_eq!(footprint(&specified, "store"), footprint(&bare, "store"));
    assert_eq!(def(&specified, "store").spec.len(), 2);
    assert!(
        def(&specified, "store")
            .spec
            .iter()
            .all(|s| s.footprint.is_empty())
    );
}

#[test]
fn a_test_that_calls_a_specified_definition_keeps_its_footprint() {
    let out = check_src(&format!(
        "{DB}fn store(k: Int) -> Unit requires k > 0 = db.put[users](k, 1)\n\
         test \"stores\" {{ handle {{ store(1) }} with {{ db.put[users](k, v) -> () }} }}"
    ));
    assert_eq!(out.tests[0].footprint.to_string(), "{}");
}

#[test]
fn a_clause_that_performs_an_effect_is_rejected() {
    for (clause, phrase) in [
        ("requires", "a `requires` clause"),
        ("ensures", "an `ensures` clause"),
    ] {
        let diags = check_src_err(&format!(
            "{DB}fn f(k: Int) -> Int {clause} db.get[users](k) > 0 = k"
        ));
        let d = only(&diags, codes::EFFECT_IN_SPEC);
        assert!(d.message.starts_with(phrase), "{}", render(&diags));
        assert!(
            d.labels
                .iter()
                .any(|l| l.message.contains("db.read[users]")),
            "{}",
            render(&diags)
        );
        assert!(
            d.notes.iter().any(|n| n.contains("post-state it caused")),
            "{}",
            render(&diags)
        );
    }
}

#[test]
fn a_simulate_region_in_a_clause_is_rejected() {
    for clause in ["requires", "ensures"] {
        let diags = check_src_err(&format!("fn f() -> Int {clause} (simulate {{ true }}) = 1"));
        let d = only(&diags, codes::EFFECT_IN_SPEC);
        assert!(
            d.labels.iter().any(|l| l.message.contains("sim.read")),
            "{}",
            render(&diags)
        );
    }
}

#[test]
fn a_clause_whose_row_is_an_unsolved_tail_is_rejected() {
    let diags = check_src_err("fn g<a | e>(f: () -> Bool / e) -> Int requires f() = 1");
    let d = only(&diags, codes::EFFECT_IN_SPEC);
    assert!(d.message.contains("not known to be empty"), "{}", d.message);
}

#[test]
fn a_clause_that_is_not_bool_is_a_type_mismatch() {
    let diags = check_src_err("fn f(x: Int) -> Int requires x = x");
    only(&diags, codes::TYPE_MISMATCH);
}

#[test]
fn result_is_bound_only_in_an_ensures() {
    check_src("fn f(x: Int) -> Int ensures result >= x = x + 1");

    let diags = check_src_err("fn f(x: Int) -> Int requires result > 0 = x");
    let d = only(&diags, codes::UNKNOWN_NAME);
    assert!(
        d.notes
            .iter()
            .any(|n| n.contains("bound only in an `ensures`")),
        "{}",
        render(&diags)
    );
}

#[test]
fn a_parameter_named_result_collides_with_an_ensures() {
    check_src("fn f(result: Int) -> Int = result");

    let diags = check_src_err("fn f(result: Int) -> Int ensures result > 0 = result");
    let d = only(&diags, codes::DUPLICATE_DEFINITION);
    assert_eq!(d.labels.len(), 2, "{}", render(&diags));
}

#[test]
fn a_law_carries_its_binders_its_guard_and_a_pure_footprint() {
    let out = check_src(
        "fn credited(a: Int, n: Int) -> Int = a + n\n\
         fn debited(a: Int, n: Int) -> Int = a - n\n\
         law \"credit and debit cancel\" forall (a: Int, n: Int) where n > 0 && n <= a {\n\
           credited(debited(a, n), n) == a\n\
         }",
    );
    assert_eq!(out.laws.len(), 1);
    let law = &out.laws[0];
    assert_eq!(law.name, "credit and debit cancel");
    assert_eq!(law.key.as_str(), "credit and debit cancel");
    assert_eq!(law.index, 0);
    assert!(law.has_guard);
    assert_eq!(law.footprint.to_string(), "{}");
    let binders: Vec<String> = law
        .binders
        .iter()
        .map(|b| format!("{}: {}", b.name, crate::print::print_type(&b.ty)))
        .collect();
    assert_eq!(binders, ["a: Int", "n: Int"]);
}

#[test]
fn a_ground_law_has_no_binders_and_no_guard() {
    let out = check_src("law \"one is one\" { 1 == 1 }");
    let law = &out.laws[0];
    assert!(law.binders.is_empty());
    assert!(!law.has_guard);
}

#[test]
fn a_law_quantifies_over_function_values() {
    let out =
        check_src("law \"f is a function\" forall (f: (Int) -> Int, x: Int) { f(x) == f(x) }");
    let tys: Vec<String> = out.laws[0]
        .binders
        .iter()
        .map(|b| crate::print::print_type(&b.ty))
        .collect();
    assert_eq!(tys, ["(Int) -> Int", "Int"]);
}

#[test]
fn a_law_binder_may_carry_a_type_variable() {
    let out = check_src("law \"length agrees\" forall (xs: List<a>) { len(xs) == len(xs) }");
    assert_eq!(
        crate::print::print_type(&out.laws[0].binders[0].ty),
        "List<a>"
    );
}

/// The prover reads a type variable as an uninterpreted sort, so a proof over it
/// holds for every instantiation — which is only true if the body cannot pick one.
#[test]
fn a_law_body_cannot_instantiate_a_binders_type_variable() {
    let diags = check_src_err("law \"pins a\" forall (x: a) { x == 1 }");
    only(&diags, codes::TYPE_MISMATCH);
}

#[test]
fn a_law_body_may_be_a_simulate_region() {
    let out = check_src("law \"conserves\" forall (n: Int) { simulate { n == n } }");
    assert_eq!(out.laws[0].footprint.to_string(), "{sim.read}");
}

/// A guard decides which values the law is a claim about, so a domain that
/// depends on a seed would be a different domain per run.
#[test]
fn a_where_guard_may_not_carry_the_seed() {
    let diags =
        check_src_err("law \"seeded guard\" forall (n: Int) where (simulate { n > 0 }) { n == n }");
    let d = only(&diags, codes::EFFECT_IN_SPEC);
    assert!(d.message.contains("`where`"), "{}", d.message);
}

#[test]
fn a_law_body_that_performs_an_ordinary_effect_is_rejected() {
    let diags = check_src_err(&format!(
        "{DB}law \"reads\" forall (n: Int) {{ db.get[users](n) == n }}"
    ));
    let d = only(&diags, codes::EFFECT_IN_SPEC);
    assert!(d.message.contains("law body"), "{}", d.message);
    assert!(
        d.notes.iter().any(|n| n.contains("sim.read")),
        "{}",
        render(&diags)
    );
}

#[test]
fn a_law_cannot_quantify_over_a_handler() {
    for src in [
        "law \"any handler\" forall (h: Handler<Int>) { true }",
        "law \"any handler\" forall (h: handler) { true }",
    ] {
        let diags = check_src_err(src);
        let d = only(&diags, codes::UNQUANTIFIABLE_TYPE);
        assert!(d.message.contains("a handler"), "{}", d.message);
        assert!(
            d.notes.iter().any(|n| n.contains("0007-specs.md §3.2")),
            "{}",
            render(&diags)
        );
        assert!(!has_code(&diags, codes::UNKNOWN_TYPE), "{}", render(&diags));
    }
}

#[test]
fn a_law_cannot_quantify_over_a_type_the_generator_cannot_inhabit() {
    for (src, expected) in [
        ("law \"c\" forall (c: Cell<Int>) { true }", "with_cell"),
        ("law \"t\" forall (t: Task<Int>) { true }", "scheduler"),
        (
            "type Box = Wrap(Cell<Int>)\nlaw \"b\" forall (b: Box) { true }",
            "with_cell",
        ),
    ] {
        let diags = check_src_err(src);
        let d = only(&diags, codes::UNQUANTIFIABLE_TYPE);
        assert!(
            d.labels.iter().any(|l| l.message.contains(expected)),
            "{src}: {}",
            render(&diags)
        );
    }
}

#[test]
fn a_law_binder_may_not_carry_an_effect_row() {
    let diags = check_src_err(&format!(
        "{DB}law \"f\" forall (f: (Int) -> Int / {{db.read[users]}}) {{ true }}"
    ));
    let d = only(&diags, codes::UNQUANTIFIABLE_TYPE);
    assert!(
        d.labels.iter().any(|l| l.message.contains("db")),
        "{}",
        render(&diags)
    );

    let diags = check_src_err("law \"f\" forall (f: (Int) -> Int / e) { true }");
    let d = only(&diags, codes::UNQUANTIFIABLE_TYPE);
    assert!(
        d.labels
            .iter()
            .any(|l| l.message.contains("effect variable")),
        "{}",
        render(&diags)
    );
    assert!(
        !has_code(&diags, codes::UNBOUND_ROW_VAR),
        "{}",
        render(&diags)
    );
}

#[test]
fn two_laws_with_one_label_are_a_duplicate() {
    let diags = check_src_err("law \"same\" { true }\nlaw \"same\" { false }");
    let d = only(&diags, codes::DUPLICATE_DEFINITION);
    assert_eq!(d.labels.len(), 2, "{}", render(&diags));
}

#[test]
fn laws_are_indexed_in_program_order() {
    let out = check_files(&[
        ("a", "law \"first\" { true }\nlaw \"second\" { true }"),
        ("b", "law \"third\" { true }"),
    ]);
    let keys: Vec<&str> = out.laws.iter().map(|l| l.key.as_str()).collect();
    assert_eq!(keys, ["a.first", "a.second", "b.third"]);
    assert_eq!(
        out.laws.iter().map(|l| l.index).collect::<Vec<_>>(),
        [0, 1, 2]
    );
}

/// Gate 2 skips re-inferring a body whose hash is unchanged, and a spec is
/// erased from that hash — so a clause must be typed against the restored
/// interface every run in which its file was parsed.
#[test]
fn a_restored_definition_still_has_its_clauses_typed() {
    let src = "fn withdraw(a: Int, n: Int) -> Int requires n > 0 ensures result == a - n = a - n";
    let program = parse_program(&[("ledger", src)]);
    let resolved = ply_syntax::resolve(&program).expect("resolves");
    let first = crate::check_program(&program, &resolved).expect("checks");

    let known = Known {
        defs: first
            .defs
            .iter()
            .map(|(name, info)| {
                (
                    name.clone(),
                    KnownDef {
                        scheme: info.scheme.clone(),
                        footprint: info.footprint.clone(),
                    },
                )
            })
            .collect(),
        tests: Default::default(),
    };
    let restored =
        crate::check_program_with(&program, &resolved, &known).expect("checks from interfaces");
    let spec = &def(&restored, "ledger.withdraw").spec;
    assert_eq!(spec.len(), 2);
    assert!(spec.iter().all(|s| s.footprint.is_empty()));

    // And the restored path still judges the clause, rather than accepting it
    // because nothing constrained the types it was checked against.
    let broken = "fn withdraw(a: Int, n: Int) -> Int ensures result == \"x\" = a - n";
    let program = parse_program(&[("ledger", broken)]);
    let resolved = ply_syntax::resolve(&program).expect("resolves");
    let diags = crate::check_program_with(&program, &resolved, &known)
        .expect_err("a clause comparing Int to String is a mismatch");
    assert!(has_code(&diags, codes::TYPE_MISMATCH), "{}", render(&diags));
}

/// The corpus is where the two purity rules meet real code: a clause's row is
/// empty, and a law body's is empty or exactly the seed. Asserted against the
/// values rather than against the comments that say so.
#[test]
fn every_law_and_clause_in_the_example_corpus_is_pure() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let mut paths: Vec<std::path::PathBuf> = std::fs::read_dir(&root)
        .expect("the example corpus is part of the repository")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|e| e == "ply"))
        .collect();
    paths.sort();

    let sources: Vec<(String, String)> = paths
        .iter()
        .map(|path| {
            let name = path.file_stem().unwrap_or_default().to_string_lossy();
            let text = std::fs::read_to_string(path).expect("example is readable");
            (name.to_string(), text)
        })
        .collect();
    let mut files: Vec<(&str, &str)> = sources
        .iter()
        .map(|(name, text)| (name.as_str(), text.as_str()))
        .collect();
    for (name, source) in ply_std::sources() {
        files.push((name, source));
    }
    let out = check_files(&files);

    let clauses: usize = out.defs.values().map(|d| d.spec.len()).sum();
    assert!(clauses > 0, "the corpus carries no `requires` or `ensures`");
    assert!(!out.laws.is_empty(), "the corpus carries no law");

    for info in out.defs.values() {
        for clause in &info.spec {
            assert!(
                clause.footprint.is_empty(),
                "`{}` clause #{} has footprint {}",
                info.name,
                clause.index,
                clause.footprint
            );
        }
    }
    let seed = Footprint::from_atoms([crate::prelude::seed_atom()]);
    for law in &out.laws {
        assert!(
            law.footprint.is_empty() || law.footprint == seed,
            "law {:?} has footprint {}",
            law.name,
            law.footprint
        );
    }
    assert!(
        out.laws.iter().any(|l| l.footprint == seed),
        "the corpus demonstrates no concurrency law"
    );
}

#[test]
fn a_clause_sees_the_types_the_body_forced_on_an_unannotated_signature() {
    let out = check_src("fn f(x) ensures result > x = x + 1");
    assert_eq!(sig(&out, "f"), "(Int) -> Int");
    assert_eq!(def(&out, "f").spec.len(), 1);

    let diags = check_src_err("fn f(x) ensures result == \"grown\" = x + 1");
    only(&diags, codes::TYPE_MISMATCH);
}

#[test]
fn a_clause_on_a_mutually_recursive_definition_is_typed() {
    let out = check_src(
        "fn even(n: Int) -> Bool requires n >= 0 = if n == 0 { true } else { odd(n - 1) }\n\
         fn odd(n: Int) -> Bool ensures result == !even(n) = if n == 0 { false } else { even(n - 1) }",
    );
    assert_eq!(def(&out, "even").spec.len(), 1);
    assert_eq!(def(&out, "odd").spec.len(), 1);
}

#[test]
fn a_clause_and_a_law_may_name_an_imported_definition() {
    let out = check_files(&[
        ("base", "pub fn twice(n: Int) -> Int = n * 2"),
        (
            "app",
            "import base (twice)\n\
             fn quad(n: Int) -> Int ensures result == twice(twice(n)) = twice(twice(n))\n\
             law \"quadrupling is doubling twice\" forall (n: Int) {\n\
               quad(n) == twice(twice(n))\n\
             }",
        ),
    ]);
    assert_eq!(def(&out, "app.quad").spec.len(), 1);
    assert_eq!(
        out.laws[0].key.as_str(),
        "app.quadrupling is doubling twice"
    );
}
