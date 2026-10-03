use ply_codegen::c::producer::{imported_closure, module_of_root, stubbed};
use ply_eval::{Analysis, Cut, ModuleInfo, ModuleName, SourceId, Symbol};
use std::collections::HashSet;

/// The emitter reads roots the same way: `emit.module_of_root`'s test holds the same cases.
#[test]
fn a_root_is_in_the_module_its_owner_is_in() {
    assert_eq!(module_of_root("app.cli.main"), "app.cli");
    assert_eq!(module_of_root("app.cli.main#requires#0"), "app.cli");
    assert_eq!(module_of_root("app.test#3"), "app");
    assert_eq!(module_of_root("app.law#2.body"), "app");
    assert_eq!(module_of_root("main"), "");
}

#[test]
fn the_emitter_is_handed_the_lowered_modules_and_what_they_import_transitively() {
    let mut front = Analysis::default();
    for (at, (name, imports)) in [
        ("a", vec!["b"]),
        ("b", vec!["c"]),
        ("c", vec![]),
        ("d", vec!["a"]),
    ]
    .into_iter()
    .enumerate()
    {
        front.check.modules.insert(
            Symbol::new(name),
            ModuleInfo {
                name: ModuleName::from_dotted(name),
                source: SourceId(at as u32),
                items: Vec::new(),
                imports: imports.into_iter().map(ModuleName::from_dotted).collect(),
                cuts: Vec::new(),
            },
        );
    }
    let names = |s: &[&str]| s.iter().map(|n| (*n).to_string()).collect::<HashSet<_>>();
    assert_eq!(imported_closure(&front, ["a"]), names(&["a", "b", "c"]));
    assert_eq!(imported_closure(&front, ["c"]), names(&["c"]));
    assert_eq!(
        imported_closure(&front, ["d"]),
        names(&["a", "b", "c", "d"])
    );
}

/// The front end's `stub_module` blanks a compiled package's modules the same way.
#[test]
fn a_stub_keeps_every_offset_and_a_body_s_braces() {
    let text = "fn f() -> Int = {\n  1 + 2\n}\ntest \"t\" { f() }\nfn g() -> Int = 3\n";
    let body = text.find('{').unwrap();
    let test = text.find("test").unwrap();
    let stub = stubbed(
        text,
        &[
            Cut {
                start: body,
                end: text.find("}\n").unwrap() + 1,
                braced: true,
            },
            Cut {
                start: test,
                end: text.find("g()").unwrap() - 4,
                braced: false,
            },
        ],
    );
    assert_eq!(
        stub,
        "fn f() -> Int = {\n       \n}\n                \nfn g() -> Int = 3\n"
    );
    assert_eq!(stub.len(), text.len());
}
