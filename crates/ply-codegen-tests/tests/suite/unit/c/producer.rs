use ply_codegen::c::producer::{imported_closure, module_of_root};
use ply_eval::{Front, ModuleInfo, ModuleName, SourceId, Symbol};
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
    let mut front = Front::default();
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
