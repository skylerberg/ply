//! The parser lands in a sibling crate on its own schedule, so these build the
//! AST directly. That also lets a test give an expression a distinctive span and
//! then assert which expression a diagnostic blamed.

use crate::infer::check_module;
use crate::print::print_scheme;
use crate::{CheckOutput, DefInfo};
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
    Expr { kind, span: sp(start) }
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
    ex(ExprKind::App { func: Box::new(func), args })
}

fn call(name: &str, args: Vec<Expr>) -> Expr {
    app(var(name), args)
}

fn lambda(params: &[&str], body: Expr) -> Expr {
    ex(ExprKind::Lambda {
        params: params
            .iter()
            .map(|p| Param { name: id(p), ty: None, span: any() })
            .collect(),
        body: Box::new(body),
    })
}

fn add(l: Expr, r: Expr) -> Expr {
    ex(ExprKind::Binary { op: BinOp::Add, lhs: Box::new(l), rhs: Box::new(r) })
}

fn block(stmts: Vec<Stmt>, tail: Option<Expr>) -> Expr {
    ex(ExprKind::Block { stmts, tail: tail.map(Box::new) })
}

fn let_(name: &str, value: Expr) -> Stmt {
    Stmt::Let {
        pat: Pattern { kind: PatternKind::Var(id(name)), span: any() },
        ty: None,
        value: Box::new(value),
        span: any(),
    }
}

fn perform(effect: &str, op: &str, resource: Option<&str>, args: Vec<Expr>) -> Expr {
    perform_at(effect, op, resource, args, 0)
}

fn perform_at(
    effect: &str,
    op: &str,
    resource: Option<&str>,
    args: Vec<Expr>,
    start: u32,
) -> Expr {
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

fn clause(effect: &str, op: &str, resource: Option<&str>, params: &[&str], body: Expr) -> HandleClause {
    HandleClause {
        effect: id(effect).into(),
        op: id(op),
        resource: resource.map(id),
        params: params.iter().map(|p| id(p)).collect(),
        body,
        span: any(),
    }
}

fn handle(body: Expr, clauses: Vec<HandleClause>) -> Expr {
    ex(ExprKind::Handle { body: Box::new(body), clauses, return_clause: None })
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
    TypeExpr::Con { name: id(name).into(), args, span: any() }
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
                .map(|p| Param { name: id(p), ty: None, span: any() })
                .collect(),
            ret: None,
            effects: None,
            body,
            span: any(),
        },
    }
}

impl FnBuilder {
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
    Item::Effect(Box::new(EffectDef { vis: Visibility::Private, name: id(name), nondet, ops, span: any() }))
}

fn op(name: &str, mode: Mode, resource_param: bool, params: Vec<TypeExpr>, ret: TypeExpr) -> OpDef {
    OpDef { name: id(name), mode, resource_param, params, ret, span: any() }
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
    Module { name: ModuleName::anonymous(), source: SRC, imports: Vec::new(), items }
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
            let labels: Vec<String> =
                d.labels.iter().map(|l| format!("  @{}..{} {}", l.span.start, l.span.end, l.message)).collect();
            format!("[{}] {}\n{}", d.code, d.message, labels.join("\n"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn sig(out: &CheckOutput, name: &str) -> String {
    print_scheme(&def(out, name).scheme)
}

fn def<'a>(out: &'a CheckOutput, name: &str) -> &'a DefInfo {
    out.defs.get(&Symbol::new(name)).unwrap_or_else(|| panic!("no definition `{name}`"))
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
            op("get", Mode::Read, true, vec![con("Int", vec![])], con("Int", vec![])),
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

fn clock_effect() -> Item {
    effect_def(
        "clock",
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
        func("read_one", &[], perform("db", "get", Some("users"), vec![int(1)])).item(),
    ]);
    assert_eq!(footprint(&out, "read_one"), "{db.read[users]}");
    assert_eq!(sig(&out, "read_one"), "() -> Int / {db.read[users]}");
}

#[test]
fn rows_accumulate_through_application_and_composition() {
    let out = check(vec![
        db_effect(),
        func("reader", &[], perform("db", "get", Some("users"), vec![int(1)])).item(),
        func("writer", &[], perform("db", "put", Some("orders"), vec![int(1), int(2)])).item(),
        func(
            "both",
            &[],
            block(vec![Stmt::Expr(call("reader", vec![]))], Some(call("writer", vec![]))),
        )
        .item(),
    ]);
    assert_eq!(footprint(&out, "both"), "{db.write[orders], db.read[users]}");
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
        func("reader", &[], perform("db", "get", Some("users"), vec![int(1)])).item(),
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
            call("map", vec![var("xs"), lambda(&["x"], add(var("x"), var("x")))]),
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
        func("reader", &[], perform("db", "get", Some("users"), vec![int(1)])).item(),
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

#[test]
fn a_handler_only_subtracts_the_resource_it_names() {
    let out = check(vec![
        db_effect(),
        func("reader", &[], perform("db", "get", Some("orders"), vec![int(1)])).item(),
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
        func("reader", &[], perform("db", "get", Some("users"), vec![int(1)])).item(),
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
        func("escaped", &["x"], with_cell("users", var("x"), "c", var("c"))).item(),
    ]);
    let d = only(&diags, codes::TYPE_MISMATCH);
    assert_eq!(d.message, "the cell escapes its `with_cell[users]` region");
    assert!(d.labels[0].message.contains("Cell[users]"), "{}", d.labels[0].message);
}

#[test]
fn a_region_escape_is_caught_however_the_cell_is_wrapped() {
    let in_a_list = with_cell("users", int(0), "c", ex(ExprKind::List { items: vec![var("c")] }));
    assert!(
        has_code(&check_err(vec![func("f", &[], in_a_list).item()]), codes::TYPE_MISMATCH),
    );

    let in_a_record = with_cell(
        "users",
        int(0),
        "c",
        ex(ExprKind::Record { fields: vec![(id("held"), var("c"))] }),
    );
    assert!(
        has_code(&check_err(vec![func("g", &[], in_a_record).item()]), codes::TYPE_MISMATCH),
    );

    let in_a_closure = with_cell("users", int(0), "c", lambda(&["_ignored"], var("c")));
    assert!(
        has_code(&check_err(vec![func("h", &[], in_a_closure).item()]), codes::TYPE_MISMATCH),
    );
}

#[test]
fn a_region_that_returns_a_plain_value_is_accepted() {
    let out = check(vec![
        func("f", &[], with_cell("users", int(1), "c", call("cell_get", vec![var("c")]))).item(),
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
                vec![Stmt::Expr(call("cell_set", vec![var("d"), bool_lit(false)]))],
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
        with_cell("inner", bool_lit(true), "d", call("cell_set", vec![var("c"), bool_lit(false)])),
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
    assert!(primary.message.contains("sensors.read[north]"), "{}", primary.message);
    assert!(
        d.notes.iter().any(|n| n.contains("sensors.read_at[north]()")),
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
    assert!(has_code(&diags, codes::EFFECT_NOT_PERMITTED), "{}", render(&diags));
}

#[test]
fn a_handled_atom_stops_being_evidence_for_the_determinism_check() {
    let handled = handle(
        block(vec![], Some(perform_at("clock", "now", None, vec![], 10))),
        vec![clause("clock", "now", None, &[], int(0))],
    );
    let body = block(
        vec![Stmt::Expr(handled)],
        Some(perform_at("clock", "now", None, vec![], 20)),
    );
    let diags = check_err(vec![clock_effect(), test_def("one handled one not", false, body)]);
    let d = only(&diags, codes::NONDET_IN_DET_TEST);
    let primary = d.labels.iter().find(|l| l.primary).unwrap();
    assert_eq!(primary.span.start, 20, "the blamed perform must be the surviving one");
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
        vec![clause("db", "get", Some("users"), &["k"], call("cell_get", vec![var("c")]))],
    );
    let out = check(vec![
        db_effect(),
        func("reader", &[], perform("db", "get", Some("users"), vec![int(1)])).item(),
        func("isolated", &[], with_cell("users", int(0), "c", handled)).item(),
    ]);
    assert_eq!(footprint(&out, "isolated"), "{}");
}

#[test]
fn an_annotation_is_an_upper_bound_and_is_what_gets_published() {
    let def = func("reads_only", &[], perform("db", "get", Some("users"), vec![int(1)]))
        .ret(con("Int", vec![]))
        .effects(row(
            &[("db", Mode::Read, Some("users")), ("db", Mode::Write, Some("audit"))],
            None,
        ))
        .item();
    let out = check(vec![db_effect(), def]);
    assert_eq!(footprint(&out, "reads_only"), "{db.write[audit], db.read[users]}");
}

#[test]
fn an_annotation_that_omits_an_atom_names_the_atom_and_the_perform() {
    let def = func(
        "sneaky",
        &[],
        block(
            vec![Stmt::Expr(perform_at("db", "put", Some("orders"), vec![int(1), int(2)], 40))],
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
    assert!(d.notes.iter().any(|n| n.contains("db.write[orders]")), "{:?}", d.notes);
}

#[test]
fn an_undeclared_effect_variable_in_an_annotation_is_an_unbound_row_var() {
    let def = func("f", &[], int(1))
        .ret(con("Int", vec![]))
        .effects(row(&[], Some("e")))
        .item();
    let diags = check_err(vec![def]);
    assert!(has_code(&diags, codes::UNBOUND_ROW_VAR), "{}", render(&diags));
}

#[test]
fn a_nondet_effect_surviving_in_a_det_test_is_e0412() {
    let body = block(
        vec![let_("now", perform_at("clock", "now", None, vec![], 77))],
        Some(call("assert", vec![bool_lit(true)])),
    );
    let diags = check_err(vec![clock_effect(), test_def("uses the clock", false, body)]);
    let d = only(&diags, codes::NONDET_IN_DET_TEST);

    assert_eq!(d.message, "nondeterministic effect in a deterministic test");
    let primary = d.labels.iter().find(|l| l.primary).unwrap();
    assert_eq!(primary.span.start, 77);
    assert!(primary.message.contains("clock.read"), "{}", primary.message);
    assert!(primary.message.contains("nondet"), "{}", primary.message);
    assert!(
        d.notes.iter().any(|n| n.contains("clock.now()")),
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
        block(vec![Stmt::Expr(perform("clock", "now", None, vec![]))], Some(int(0))),
        vec![clause("clock", "now", None, &[], int(1234))],
    );
    let out = check(vec![clock_effect(), test_def("frozen clock", false, body)]);
    assert_eq!(out.tests[0].footprint.to_string(), "{}");
    assert!(!out.tests[0].nondet);
}

#[test]
fn test_nondet_opts_out_of_the_determinism_check() {
    let body = block(vec![Stmt::Expr(perform("clock", "now", None, vec![]))], Some(int(0)));
    let out = check(vec![clock_effect(), test_def("wall clock", true, body)]);
    assert_eq!(out.tests[0].footprint.to_string(), "{clock.read}");
    assert!(out.tests[0].nondet);
}

#[test]
fn e0412_points_through_a_call_when_the_perform_is_indirect() {
    let helper = func("stamp", &[], perform("clock", "now", None, vec![])).item();
    let body = block(vec![], Some(ex_at(ExprKind::App {
        func: Box::new(var("stamp")),
        args: vec![],
    }, 55)));
    let diags = check_err(vec![clock_effect(), helper, test_def("indirect", false, body)]);
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
    assert!(d.notes.iter().any(|n| n.contains("db.get[")), "{:?}", d.notes);
}

#[test]
fn a_resource_label_on_a_plain_operation_is_rejected() {
    let def = func("f", &[], perform("clock", "now", Some("wall"), vec![])).item();
    let diags = check_err(vec![clock_effect(), def]);
    assert!(has_code(&diags, codes::RESOURCE_REQUIRED), "{}", render(&diags));
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
    assert_eq!(only(&diags, codes::UNKNOWN_EFFECT).primary_span().unwrap().start, 30);
    assert_eq!(only(&diags, codes::UNKNOWN_OPERATION).primary_span().unwrap().start, 60);
}

#[test]
fn an_operation_call_with_the_wrong_arity_is_reported() {
    let def = func("f", &[], perform("db", "get", Some("users"), vec![int(1), int(2)])).item();
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
    assert_eq!(diags.iter().filter(|d| d.code == codes::UNKNOWN_NAME).count(), 2);
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
            else_branch: Box::new(call("is_odd", vec![ex(ExprKind::Binary {
                op: BinOp::Sub,
                lhs: Box::new(var("n")),
                rhs: Box::new(int(1)),
            })])),
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
            else_branch: Box::new(call("is_even", vec![ex(ExprKind::Binary {
                op: BinOp::Sub,
                lhs: Box::new(var("n")),
                rhs: Box::new(int(1)),
            })])),
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
    let option = Item::Type(Box::new(TypeDef {
        vis: Visibility::Private,
        name: id("Option"),
        params: vec![id("a")],
        body: TypeDefBody::Sum(vec![
            VariantDef { name: id("None"), fields: vec![], span: any() },
            VariantDef { name: id("Some"), fields: vec![tvar("a")], span: any() },
        ]),
        span: any(),
    }));
    let arm = |kind: PatternKind, body: Expr| MatchArm {
        pat: Pattern { kind, span: any() },
        guard: None,
        body,
        span: any(),
    };
    let full = ex(ExprKind::Match {
        scrutinee: Box::new(var("o")),
        arms: vec![
            arm(PatternKind::Ctor { name: id("None").into(), args: vec![] }, int(0)),
            arm(
                PatternKind::Ctor {
                    name: id("Some").into(),
                    args: vec![Pattern { kind: PatternKind::Var(id("v")), span: any() }],
                },
                var("v"),
            ),
        ],
    });
    let out = check(vec![
        option.clone(),
        func("unwrap_or_zero", &["o"], full).item(),
        func("wrap", &[], call("Some", vec![int(3)])).item(),
    ]);
    assert_eq!(sig(&out, "unwrap_or_zero"), "(Option<Int>) -> Int");
    assert_eq!(sig(&out, "wrap"), "() -> Option<Int>");
    assert_eq!(out.ctors[&Symbol::new("Some")].arity, 1);
    assert_eq!(out.ctors[&Symbol::new("None")].index, 0);

    let partial = ex(ExprKind::Match {
        scrutinee: Box::new(var("o")),
        arms: vec![arm(PatternKind::Ctor { name: id("None").into(), args: vec![] }, int(0))],
    });
    let diags = check_err(vec![option, func("partial", &["o"], partial).item()]);
    let d = only(&diags, codes::NON_EXHAUSTIVE_MATCH);
    assert!(d.labels[0].message.contains("Some"), "{}", d.labels[0].message);
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
    assert!(has_code(&diags, codes::NOT_A_FUNCTION), "{}", render(&diags));
}

#[test]
fn duplicate_definitions_are_reported_once_and_the_first_wins() {
    let items = vec![
        func("f", &[], int(1)).item(),
        func("f", &[], bool_lit(true)).item(),
    ];
    let diags = check_err(items);
    assert_eq!(diags.iter().filter(|d| d.code == codes::DUPLICATE_DEFINITION).count(), 1);
}

#[test]
fn a_user_effect_may_not_be_called_cell() {
    let diags = check_err(vec![effect_def(
        "cell",
        false,
        vec![op("peek", Mode::Read, true, vec![], con("Int", vec![]))],
    )]);
    assert!(has_code(&diags, codes::DUPLICATE_DEFINITION), "{}", render(&diags));
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
        func("f", &["x"], var("x")).param_types(vec![Some(con("Loop", vec![]))]).item(),
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
        func("as_int", &[], perform("store", "echo", Some("k"), vec![int(1)])).item(),
        func("as_bool", &[], perform("store", "echo", Some("k"), vec![bool_lit(true)])).item(),
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
        func("reader", &[], perform("db", "get", Some("users"), vec![int(1)])).item(),
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
        block(vec![], Some(perform_at("clock", "now", None, vec![], 10))),
        vec![clause(
            "clock",
            "now",
            None,
            &[],
            perform_at("clock", "now", None, vec![], 30),
        )],
    );
    let out = check(vec![clock_effect(), func("f", &[], body.clone()).item()]);
    assert_eq!(footprint(&out, "f"), "{clock.read}");

    let diags = check_err(vec![clock_effect(), test_def("forwarding", false, body)]);
    let d = only(&diags, codes::NONDET_IN_DET_TEST);
    let primary = d.labels.iter().find(|l| l.primary).unwrap();
    assert_eq!(primary.span.start, 30, "the clause body is what still performs it");
}

#[test]
fn a_cell_builtin_cannot_escape_as_a_value_or_be_redefined() {
    let as_value = func("f", &[], block(vec![let_("g", var("cell_get"))], Some(int(1)))).item();
    let diags = check_err(vec![as_value]);
    assert!(has_code(&diags, codes::RESOURCE_REQUIRED), "{}", render(&diags));

    let redefined = func("cell_get", &["c"], int(0)).item();
    let diags = check_err(vec![redefined]);
    assert!(has_code(&diags, codes::DUPLICATE_DEFINITION), "{}", render(&diags));
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
    pat(PatternKind::List { items, rest: rest.map(Box::new) })
}

fn match_on(scrutinee: Expr, arms: Vec<(Pattern, Expr)>) -> Expr {
    ex(ExprKind::Match {
        scrutinee: Box::new(scrutinee),
        arms: arms
            .into_iter()
            .map(|(p, body)| MatchArm { pat: p, guard: None, body, span: any() })
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
    check(vec![on_int_list(vec![(plist(vec![], Some(pvar("r"))), int(0))]).item()]);
}

#[test]
fn a_cons_arm_alone_leaves_the_empty_list_uncovered() {
    let diags =
        check_err(vec![
            on_int_list(vec![(plist(vec![pvar("x")], Some(pvar("r"))), var("x"))]).item()
        ]);
    let d = only(&diags, codes::NON_EXHAUSTIVE_MATCH);
    assert!(d.labels[0].message.contains("the empty list"), "{}", d.labels[0].message);
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
            (plist(vec![pvar("x"), pvar("y"), pvar("z")], Some(pvar("r"))), var("x")),
        ])
        .item(),
    ]);
    let d = only(&diags, codes::NON_EXHAUSTIVE_MATCH);
    assert_eq!(d.labels[0].message, "not covered: lists of 1 element, lists of 2 elements");
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
            (plist(vec![pat(PatternKind::Lit(Lit::Int(1)))], Some(pvar("r"))), int(1)),
        ])
        .item(),
    ]);
    assert!(has_code(&diags, codes::NON_EXHAUSTIVE_MATCH), "{}", render(&diags));
}

fn pair_record(fields: Vec<(&str, Pattern)>, rest: bool) -> Pattern {
    pat(PatternKind::Record {
        fields: fields.into_iter().map(|(n, p)| (id(n), p)).collect(),
        rest,
    })
}

fn pair_ty() -> TypeExpr {
    TypeExpr::Record {
        fields: vec![(id("first"), con("Int", vec![])), (id("second"), con("Int", vec![]))],
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
        on_pair(vec![(pair_record(vec![("first", pvar("a"))], true), var("a"))]).item(),
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
    assert!(d.labels[0].message.contains("`second`"), "{}", d.labels[0].message);
}

#[test]
fn a_refutable_record_field_does_not_make_the_arm_irrefutable() {
    let diags = check_err(vec![
        on_pair(vec![(
            pair_record(
                vec![("first", pat(PatternKind::Lit(Lit::Int(1)))), ("second", pvar("b"))],
                false,
            ),
            var("b"),
        )])
        .item(),
    ]);
    assert!(has_code(&diags, codes::NON_EXHAUSTIVE_MATCH), "{}", render(&diags));
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
        ex(ExprKind::Record { fields: vec![(id("run"), var(name))] })
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
                lhs: Box::new(ex(ExprKind::List { items: vec![var("a")] })),
                rhs: Box::new(ex(ExprKind::List { items: vec![var("b")] })),
            }),
        )
        .item(),
    ]);
    assert_eq!(sig(&out, "cmp"), "<a>(a, a) -> Bool");
}

// Cross-module checking. These parse real source rather than building the AST,
// because what is under test is how imports, `pub` and `::` reach inference.

fn parse_program(files: &[(&str, &str)]) -> Program {
    Program {
        modules: files
            .iter()
            .enumerate()
            .map(|(i, (name, text))| {
                ply_syntax::parse_module(
                    SourceId(i as u32),
                    ModuleName::from_dotted(name),
                    text,
                )
                .unwrap_or_else(|d| panic!("`{name}` should parse: {}", render(&d)))
            })
            .collect(),
    }
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
        ("middle", "import base (one)\npub fn two() -> Int = one() + one()"),
        ("top", "import middle\nfn four() -> Int = middle::two() + middle::two()"),
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
        ("top", "import left\nimport right\nfn t() -> Int = left::l() + right::r()"),
    ]);

    assert_eq!(sig(&out, "top.t"), "() -> Int");
    assert_eq!(out.defs.len(), 4);
    assert_eq!(out.modules.len(), 4);
    assert_eq!(
        out.modules[&Symbol::new("top")].imports,
        vec![ModuleName::from_dotted("left"), ModuleName::from_dotted("right")]
    );
}

#[test]
fn a_private_definition_cannot_be_called_from_another_module() {
    let diags = check_files_err(&[
        ("store", "fn secret() -> Int = 1\npub fn place() -> Int = secret()"),
        ("app", "import store\nfn f() -> Int = store::secret()"),
    ]);
    let d = only(&diags, codes::PRIVATE_NAME);
    assert!(d.message.contains("private to module `store`"), "{}", d.message);
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
    assert!(has_code(&diags, codes::AMBIGUOUS_IMPORT), "{}", render(&diags));
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
        ("app", "import orders\nfn f(orders: Int) -> Int = orders + orders::place()"),
    ]);
    assert_eq!(sig(&out, "app.f"), "(Int) -> Int");
}

#[test]
fn two_modules_may_declare_the_same_effect_without_contending() {
    let out = check_files(&[
        ("left", "pub effect db { read get[r](key: Int) -> Int }\npub fn read_one() -> Int = db.get[users](1)"),
        ("right", "pub effect db { read get[r](key: Int) -> Int }\npub fn read_one() -> Int = db.get[users](1)"),
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
        ("store", "pub effect db { read get[r](key: Int) -> Int\n  write put[r](key: Int, value: Int) -> Unit }"),
        ("reader", "import store (db)\npub fn r() -> Int = db.get[users](1)"),
        ("writer", "import store (db)\npub fn w() -> Unit = db.put[users](1, 2)"),
    ]);

    assert_eq!(footprint(&out, "reader.r"), "{store.db.read[users]}");
    assert_eq!(footprint(&out, "writer.w"), "{store.db.write[users]}");
    assert!(
        def(&out, "reader.r").footprint.conflicts_with(&def(&out, "writer.w").footprint),
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
        ("app", "import shapes\nfn f(s: Int) -> Int = match s { shapes::Circle(r) -> r, _ -> 0 }"),
    ]);
    assert!(has_code(&diags, codes::PRIVATE_NAME), "{}", render(&diags));
}

#[test]
fn a_public_alias_expands_in_the_module_that_wrote_it() {
    let out = check_files(&[
        ("money", "type Cents = Int\npub type Money = Cents"),
        ("app", "import money\nfn total(m: money::Money) -> Int = m + 1"),
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
    assert!(d.notes.iter().any(|n| n.contains("import store")), "{:?}", d.notes);
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
        ("app", "import clock\ntest \"reads the clock\" { assert(clock::clock.now() > 0) }"),
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
        ("app", "import timing (clock)\ntest \"reads\" { assert(clock.now() > 0) }"),
    ]);
    let d = only(&diags, codes::NONDET_IN_DET_TEST);
    assert!(
        d.notes.iter().any(|n| n.contains("{ clock.now() -> <value> }")),
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
    assert_eq!(def(&out, "right.twice").module, ModuleName::from_dotted("right"));
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
    assert!(d.message.contains("has no definition `Shape`"), "{}", d.message);
    assert!(d.notes.iter().any(|n| n.contains("`Circle`")), "{:?}", d.notes);
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
            let name = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
            let text = std::fs::read_to_string(path).expect("example is readable");
            (name, text)
        })
        .collect();
    let files: Vec<(&str, &str)> =
        sources.iter().map(|(name, text)| (name.as_str(), text.as_str())).collect();

    let out = check_files(&files);
    assert_eq!(out.modules.len(), files.len());
    assert!(
        out.defs.keys().all(|k| k.as_str().contains('.')),
        "every definition is keyed by its program-wide name"
    );
    assert!(
        out.tests.iter().all(|t| t.key.as_str().starts_with(t.module.as_str())),
        "a test key is `<module>.<label>`, which is what keeps two labels apart"
    );
}
