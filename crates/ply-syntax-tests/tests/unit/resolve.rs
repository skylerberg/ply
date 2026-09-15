use ply_span::{Diagnostic, SourceId, Span, Symbol, codes};
use ply_syntax::ast::{Ident, ImportDecl, ImportKind, Module, ModuleName, Program, QName};
use ply_syntax::parser::parse_module;
use ply_syntax::resolve::{Namespace, Resolved, resolve};

fn module_of(source: SourceId, name: &str, text: &str) -> Module {
    parse_module(source, ModuleName::from_dotted(name), text).expect("test source parses")
}

fn program(files: &[(&str, &str)]) -> Program {
    Program {
        modules: files
            .iter()
            .enumerate()
            .map(|(i, (name, text))| module_of(SourceId(i as u32), name, text))
            .collect(),
    }
}

fn at(r: &Resolved, name: &str) -> usize {
    r.index_of(&ModuleName::from_dotted(name))
        .expect("module is in the program")
}

fn bare(name: &str) -> QName {
    QName::bare(Ident::new(name, Span::DUMMY))
}

fn qualified(module: &str, name: &str) -> QName {
    QName::qualified(
        Ident::new(module, Span::DUMMY),
        Ident::new(name, Span::DUMMY),
    )
}

fn errors(files: &[(&str, &str)]) -> Vec<Diagnostic> {
    match resolve(&mut program(files)) {
        Ok(_) => panic!("expected resolution to fail"),
        Err(diags) => diags,
    }
}

fn only(diags: &[Diagnostic], code: &str) -> Diagnostic {
    let hits: Vec<&Diagnostic> = diags.iter().filter(|d| d.code == code).collect();
    assert_eq!(hits.len(), 1, "expected one {code}, got {diags:?}");
    hits[0].clone()
}

fn text_of(d: &Diagnostic) -> String {
    let labels: Vec<&str> = d.labels.iter().map(|l| l.message.as_str()).collect();
    format!(
        "{} | {} | {}",
        d.message,
        labels.join(" / "),
        d.notes.join(" / ")
    )
}

#[test]
fn a_name_resolves_across_three_modules() {
    let r = resolve(&mut program(&[
        ("base", "pub fn one() -> Int = 1"),
        (
            "middle",
            "import base (one)\npub fn two() -> Int = one() + one()",
        ),
        (
            "top",
            "import middle\nfn four() -> Int = middle::two() + middle::two()",
        ),
    ]))
    .expect("resolves");

    let middle = at(&r, "middle");
    let top = at(&r, "top");
    assert_eq!(
        r.lookup(middle, Namespace::Value, &bare("one"))
            .unwrap()
            .qualified
            .as_str(),
        "base.one"
    );
    assert_eq!(
        r.lookup(top, Namespace::Value, &qualified("middle", "two"))
            .unwrap()
            .qualified
            .as_str(),
        "middle.two"
    );
    let position = |name: &str| r.order.iter().position(|&i| i == at(&r, name)).unwrap();
    assert!(position("base") < position("middle"));
    assert!(position("middle") < position("top"));
}

#[test]
fn a_diamond_import_visits_the_shared_module_once() {
    let r = resolve(&mut program(&[
        ("base", "pub fn one() -> Int = 1"),
        ("left", "import base (one)\npub fn l() -> Int = one()"),
        ("right", "import base (one)\npub fn r() -> Int = one()"),
        (
            "top",
            "import left\nimport right\nfn t() -> Int = left::l() + right::r()",
        ),
    ]))
    .expect("resolves");

    assert_eq!(r.order.len(), 4);
    let position = |name: &str| r.order.iter().position(|&i| i == at(&r, name)).unwrap();
    assert!(position("base") < position("left"));
    assert!(position("base") < position("right"));
    assert!(position("left") < position("top"));
    assert!(position("right") < position("top"));

    let top = at(&r, "top");
    assert!(
        r.lookup(top, Namespace::Value, &qualified("left", "l"))
            .is_ok()
    );
    // `base` is reachable only through `left` and `right`, never directly.
    let d = r
        .lookup(top, Namespace::Value, &qualified("base", "one"))
        .unwrap_err();
    assert_eq!(d.code, codes::UNKNOWN_MODULE);
}

#[test]
fn a_private_name_cannot_be_reached_from_another_module() {
    let r = resolve(&mut program(&[
        (
            "store",
            "fn secret() -> Int = 1\npub fn public() -> Int = secret()",
        ),
        ("app", "import store\nfn use() -> Int = store::public()"),
    ]))
    .expect("resolves");

    let app = at(&r, "app");
    assert!(
        r.lookup(app, Namespace::Value, &qualified("store", "public"))
            .is_ok()
    );
    let d = r
        .lookup(app, Namespace::Value, &qualified("store", "secret"))
        .unwrap_err();
    assert_eq!(d.code, codes::PRIVATE_NAME);
    let shown = text_of(&d);
    assert!(shown.contains("private to module `store`"), "{shown}");
    assert!(shown.contains("pub fn secret"), "{shown}");
}

#[test]
fn selectively_importing_a_private_name_is_rejected_at_the_import() {
    let diags = errors(&[
        ("store", "fn secret() -> Int = 1"),
        ("app", "import store (secret)\nfn use() -> Int = secret()"),
    ]);
    let d = only(&diags, codes::PRIVATE_NAME);
    let shown = text_of(&d);
    assert!(
        shown.contains("`secret` is private to module `store`"),
        "{shown}"
    );
    assert!(shown.contains("pub fn secret"), "{shown}");
}

#[test]
fn importing_a_name_a_module_does_not_declare_names_what_it_exports() {
    let diags = errors(&[
        ("store", "pub fn place() -> Int = 1"),
        ("app", "import store (plce)"),
    ]);
    let d = only(&diags, codes::UNKNOWN_NAME);
    let shown = text_of(&d);
    assert!(shown.contains("declares no `plce`"), "{shown}");
    assert!(shown.contains("`store` exports: `place`"), "{shown}");
}

#[test]
fn a_two_module_cycle_is_rejected_and_names_the_cycle() {
    let diags = errors(&[
        ("a", "import b\npub fn f() -> Int = 1"),
        ("b", "import a\npub fn g() -> Int = 1"),
    ]);
    let d = only(&diags, codes::MODULE_CYCLE);
    let shown = text_of(&d);
    assert!(shown.contains("`a` -> `b` -> `a`"), "{shown}");
    assert!(shown.contains("closes the cycle"), "{shown}");
}

#[test]
fn a_three_module_cycle_prints_every_module_in_order() {
    let diags = errors(&[
        ("a", "import b\npub fn f() -> Int = 1"),
        ("b", "import c\npub fn g() -> Int = 1"),
        ("c", "import a\npub fn h() -> Int = 1"),
    ]);
    let d = only(&diags, codes::MODULE_CYCLE);
    assert!(
        d.message.contains("`a` -> `b` -> `c` -> `a`"),
        "{}",
        d.message
    );
}

#[test]
fn a_self_import_is_the_length_one_cycle() {
    let diags = errors(&[("a", "import a\npub fn f() -> Int = 1")]);
    let d = only(&diags, codes::MODULE_CYCLE);
    assert!(d.message.contains("`a` imports itself"), "{}", d.message);
}

#[test]
fn an_import_that_collides_with_a_local_definition_is_ambiguous() {
    let diags = errors(&[
        ("store", "pub fn place() -> Int = 1"),
        ("app", "import store (place)\nfn place() -> Int = 2"),
    ]);
    let d = only(&diags, codes::AMBIGUOUS_IMPORT);
    let shown = text_of(&d);
    assert!(shown.contains("both imported and defined"), "{shown}");
    assert!(shown.contains("imported from `store` here"), "{shown}");
    assert!(shown.contains("also defined here"), "{shown}");
}

#[test]
fn qualifying_the_reference_fixes_an_ambiguous_import() {
    let r = resolve(&mut program(&[
        ("store", "pub fn place() -> Int = 1"),
        (
            "app",
            "import store\nfn place() -> Int = store::place() + 1",
        ),
    ]))
    .expect("resolves once the import binds the module rather than the name");

    let app = at(&r, "app");
    assert_eq!(
        r.lookup(app, Namespace::Value, &bare("place"))
            .unwrap()
            .qualified
            .as_str(),
        "app.place"
    );
    assert_eq!(
        r.lookup(app, Namespace::Value, &qualified("store", "place"))
            .unwrap()
            .qualified
            .as_str(),
        "store.place"
    );
}

#[test]
fn two_imports_of_one_name_are_a_duplicate_import() {
    let diags = errors(&[
        ("left", "pub fn place() -> Int = 1"),
        ("right", "pub fn place() -> Int = 2"),
        ("app", "import left (place)\nimport right (place)"),
    ]);
    let d = only(&diags, codes::DUPLICATE_IMPORT);
    let shown = text_of(&d);
    assert!(shown.contains("imported twice"), "{shown}");
    assert!(shown.contains("first imported here"), "{shown}");
}

#[test]
fn two_imports_binding_one_module_name_are_a_duplicate_import() {
    let diags = errors(&[
        ("left", "pub fn f() -> Int = 1"),
        ("right", "pub fn g() -> Int = 2"),
        ("app", "import left\nimport right as left"),
    ]);
    let d = only(&diags, codes::DUPLICATE_IMPORT);
    assert!(
        d.message.contains("bind the module name `left`"),
        "{}",
        d.message
    );
}

#[test]
fn an_unknown_module_is_reported_at_the_import_path() {
    let diags = errors(&[("app", "import store.orders\nfn f() -> Int = 1")]);
    let d = only(&diags, codes::UNKNOWN_MODULE);
    assert!(
        d.message.contains("no module named `store.orders`"),
        "{}",
        d.message
    );
}

#[test]
fn a_module_binder_lives_in_its_own_namespace() {
    let r = resolve(&mut program(&[
        ("orders", "pub fn place() -> Int = 1"),
        (
            "app",
            "import orders\nfn f(orders: Int) -> Int = orders + orders::place()",
        ),
    ]))
    .expect("a local named `orders` does not hide the module binder");

    let app = at(&r, "app");
    assert!(r.scopes[app].modules.contains_key(&Symbol::new("orders")));
    assert_eq!(
        r.lookup(app, Namespace::Value, &qualified("orders", "place"))
            .unwrap()
            .qualified
            .as_str(),
        "orders.place"
    );
}

#[test]
fn an_alias_rebinds_the_module_and_the_default_binder_goes_away() {
    let r = resolve(&mut program(&[
        ("store.orders", "pub fn place() -> Int = 1"),
        (
            "app",
            "import store.orders as ord\nfn f() -> Int = ord::place()",
        ),
    ]))
    .expect("resolves");

    let app = at(&r, "app");
    assert!(
        r.lookup(app, Namespace::Value, &qualified("ord", "place"))
            .is_ok()
    );
    let d = r
        .lookup(app, Namespace::Value, &qualified("orders", "place"))
        .unwrap_err();
    assert_eq!(d.code, codes::UNKNOWN_MODULE);
}

#[test]
fn a_selective_import_binds_no_module_binder() {
    let r = resolve(&mut program(&[
        ("orders", "pub fn place() -> Int = 1"),
        ("app", "import orders (place)\nfn f() -> Int = place()"),
    ]))
    .expect("resolves");

    let app = at(&r, "app");
    assert!(r.scopes[app].modules.is_empty());
    let d = r
        .lookup(app, Namespace::Value, &qualified("orders", "place"))
        .unwrap_err();
    assert_eq!(d.code, codes::UNKNOWN_MODULE);
    let shown = text_of(&d);
    assert!(shown.contains("add `import orders`"), "{shown}");
    assert!(
        shown.contains("a selective import binds no module name"),
        "{shown}"
    );
    assert!(
        shown.contains("this import brings in names from `orders`"),
        "{shown}"
    );
}

#[test]
fn a_public_type_exports_its_constructors_and_a_private_one_does_not() {
    let r = resolve(&mut program(&[
        (
            "shapes",
            "pub type Shape = Circle(Int) | Square(Int)\ntype Hidden = Only(Int)",
        ),
        ("app", "import shapes\nfn f() -> Int = 1"),
    ]))
    .expect("resolves");

    let app = at(&r, "app");
    assert!(
        r.lookup(app, Namespace::Type, &qualified("shapes", "Shape"))
            .is_ok()
    );
    assert!(
        r.lookup(app, Namespace::Value, &qualified("shapes", "Circle"))
            .is_ok()
    );
    assert_eq!(
        r.lookup(app, Namespace::Value, &qualified("shapes", "Only"))
            .unwrap_err()
            .code,
        codes::PRIVATE_NAME
    );
    assert_eq!(
        r.lookup(app, Namespace::Type, &qualified("shapes", "Hidden"))
            .unwrap_err()
            .code,
        codes::PRIVATE_NAME
    );
}

#[test]
fn effects_and_modules_of_the_same_name_coexist() {
    let r = resolve(&mut program(&[
        ("clock", "pub nondet effect clock { read now() -> Int }"),
        ("app", "import clock\nfn f() -> Int = 1"),
    ]))
    .expect("resolves");

    let app = at(&r, "app");
    assert!(
        r.lookup(app, Namespace::Effect, &qualified("clock", "clock"))
            .is_ok()
    );
    let clock = at(&r, "clock");
    assert_eq!(
        r.declarations[clock].effects[&Symbol::new("clock")]
            .qualified
            .as_str(),
        "clock.clock"
    );
}

#[test]
fn a_name_missing_everywhere_points_at_the_module_that_exports_it() {
    let r = resolve(&mut program(&[
        ("store", "pub fn place() -> Int = 1"),
        ("app", "fn f() -> Int = 1"),
    ]))
    .expect("resolves");

    let d = r
        .lookup(at(&r, "app"), Namespace::Value, &bare("place"))
        .unwrap_err();
    assert_eq!(d.code, codes::UNKNOWN_NAME);
    let shown = text_of(&d);
    assert!(shown.contains("import store (place)"), "{shown}");
}

#[test]
fn the_anonymous_module_keeps_its_names_bare() {
    let module =
        parse_module(SourceId(0), ModuleName::anonymous(), "fn f() -> Int = 1").expect("parses");
    let r = resolve(&mut Program::single(module)).expect("resolves");
    assert_eq!(
        r.scopes[0].values[&Symbol::new("f")].qualified.as_str(),
        "f"
    );
    assert_eq!(r.order, vec![0]);
}

#[test]
fn two_items_of_one_name_leave_the_first_binding_for_inference_to_report() {
    let r = resolve(&mut program(&[(
        "app",
        "fn f() -> Int = 1\nfn f() -> Int = 2",
    )]))
    .expect("a duplicate definition is inference's diagnostic, not resolution's");
    let first = &r.scopes[at(&r, "app")].values[&Symbol::new("f")];
    assert_eq!(first.qualified.as_str(), "app.f");
}

/// The graph walk is iterative for the same reason the definition-level SCC pass is: a
/// generated project can be deeper than the native stack.
#[test]
fn a_deep_import_chain_does_not_overflow_the_stack() {
    let depth = 20_000;
    let modules: Vec<Module> = (0..depth)
        .map(|i| {
            let imports = if i + 1 < depth {
                vec![ImportDecl {
                    path: vec![Ident::new(format!("m{}", i + 1), Span::DUMMY)],
                    kind: ImportKind::Module,
                    span: Span::DUMMY,
                }]
            } else {
                Vec::new()
            };
            Module {
                name: ModuleName::from_dotted(format!("m{i}")),
                source: SourceId(i as u32),
                imports,
                items: Vec::new(),
            }
        })
        .collect();

    let r = resolve(&mut Program { modules }).expect("a chain is acyclic");
    assert_eq!(r.order.len(), depth);
    assert_eq!(r.order[0], depth - 1, "the deepest import is checked first");
    assert_eq!(r.order[depth - 1], 0);
}

#[test]
fn two_independent_cycles_are_both_reported() {
    let diags = errors(&[
        ("a", "import b\npub fn f() -> Int = 1"),
        ("b", "import a\npub fn g() -> Int = 1"),
        ("c", "import d\npub fn h() -> Int = 1"),
        ("d", "import c\npub fn i() -> Int = 1"),
    ]);
    let cycles: Vec<&Diagnostic> = diags
        .iter()
        .filter(|d| d.code == codes::MODULE_CYCLE)
        .collect();
    assert_eq!(cycles.len(), 2, "{diags:?}");
}

#[test]
fn importing_one_module_twice_is_not_a_cycle() {
    let r = resolve(&mut program(&[
        ("store", "pub fn place() -> Int = 1"),
        (
            "app",
            "import store\nimport store (place)\nfn f() -> Int = place() + store::place()",
        ),
    ]))
    .expect("resolves");
    let app = at(&r, "app");
    assert!(r.lookup(app, Namespace::Value, &bare("place")).is_ok());
    assert!(
        r.lookup(app, Namespace::Value, &qualified("store", "place"))
            .is_ok()
    );
}
