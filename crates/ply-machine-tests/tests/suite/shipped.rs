//! The shipped modules as a binary's pack holds them: each `.ply` file below the library a module
//! named by its place, and every other file data a module embeds, held by name.

use crate::front::SHIPPED;
use ply_eval::ModuleName;
use ply_machine::shipped_modules;

fn held(path: &str) -> &'static [u8] {
    SHIPPED
        .iter()
        .find(|(p, _)| *p == path)
        .map(|(_, bytes)| *bytes)
        .expect("the pack holds the file")
}

/// `std.hash.legacy` is `hash/legacy.ply` below the library, beside `std.hash`'s `hash.ply`.
#[test]
fn a_shipped_module_below_the_library_is_named_by_its_place() {
    let names = shipped_modules::names();
    assert!(names.iter().any(|n| n == "std.hash"), "{names:?}");
    assert!(names.iter().any(|n| n == "std.hash.legacy"), "{names:?}");
    let module = ModuleName::from_dotted("std.hash.legacy");
    assert_eq!(
        shipped_modules::source(&module).map(str::as_bytes),
        Some(held("std/hash/legacy.ply"))
    );
    assert_eq!(
        shipped_modules::pseudo_path(&module),
        std::path::PathBuf::from("<std>/hash/legacy.ply")
    );
    assert!(
        !shipped_modules::data_names()
            .iter()
            .any(|n| n.ends_with(".ply")),
        "a module was listed as data"
    );
}

/// `compiler.emit.roots` is `emit/roots.ply` below the compiler, beside `compiler.front`.
#[test]
fn a_compiler_module_below_the_compiler_is_named_by_its_place() {
    let names = shipped_modules::names();
    assert!(names.iter().any(|n| n == "compiler.front"), "{names:?}");
    assert!(
        names.iter().any(|n| n == "compiler.emit.roots"),
        "{names:?}"
    );
    let module = ModuleName::from_dotted("compiler.emit.roots");
    assert_eq!(
        shipped_modules::source(&module).map(str::as_bytes),
        Some(held("compiler/emit/roots.ply"))
    );
    assert_eq!(
        shipped_modules::pseudo_path(&module),
        std::path::PathBuf::from("<compiler>/emit/roots.ply")
    );
    assert_eq!(
        ply_machine::shipped::module_digest("compiler.emit.roots"),
        Some(
            blake3::hash(held("compiler/emit/roots.ply"))
                .to_hex()
                .to_string()
        )
    );
}

#[test]
fn a_data_file_a_shipped_module_embeds_is_held_by_name_and_is_no_module() {
    let name = "std/oid/names.txt";
    let bytes = held("std/oid/names.txt");
    assert!(shipped_modules::data_names().iter().any(|n| n == name));
    assert_eq!(shipped_modules::data(name), Some(bytes));
    assert_eq!(
        ply_machine::shipped::module_digest(name),
        Some(blake3::hash(bytes).to_hex().to_string())
    );
    assert!(
        !shipped_modules::names().iter().any(|n| n.contains('/')),
        "a data file was listed as a module"
    );
    // A module is no data file, and nothing outside the shipped modules is held.
    for other in [
        "std/oid.ply",
        "std/../../Cargo.toml",
        "oid/names.txt",
        "std/gone.txt",
    ] {
        assert_eq!(shipped_modules::data(other), None, "`{other}`");
    }
    let module = ModuleName::from_dotted("std.oid");
    assert_eq!(
        ply_machine::shipped::module_digest("std.oid"),
        shipped_modules::source(&module)
            .map(|text| blake3::hash(text.as_bytes()).to_hex().to_string())
    );
}

/// What a runtime's library describes itself as carries each native under the fields
/// `compiler.native.runtime_of` reads, and the natives' names alone, which a `ply` that reads no
/// `natives` builds against.
#[test]
fn the_runtime_describes_each_native_under_the_fields_a_build_reads() {
    let described: serde_json::Value =
        serde_json::from_str(&ply_machine::shipped::runtime_json()).expect("the description reads");
    let natives = described["natives"].as_array().expect("natives");
    let named = |name: &str| {
        natives
            .iter()
            .find(|n| n["name"] == name)
            .unwrap_or_else(|| panic!("no native `{name}`"))
    };
    let push = named("List::push");
    assert_eq!(
        (&push["words"], &push["answers"], &push["raises"]),
        (&2.into(), &true.into(), &false.into())
    );
    let range = named("Map::range");
    assert_eq!(
        (&range["words"], &range["raises"]),
        (&6.into(), &false.into())
    );
    let decode = named("Secret::decode");
    assert_eq!(
        (&decode["words"], &decode["raises"]),
        (&2.into(), &true.into())
    );
    let names: Vec<&str> = described["builtins"]
        .as_array()
        .expect("builtins")
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();
    let reported: Vec<&str> = natives.iter().filter_map(|n| n["name"].as_str()).collect();
    assert_eq!(names, reported);
}

/// What a runtime's library describes itself as carries how it lays out a list's and a map's parts,
/// under the fields `compiler.native.runtime_of` reads: each a number but the header fields, named.
#[test]
fn the_runtime_describes_its_layouts_under_the_fields_a_build_reads() {
    let described: serde_json::Value =
        serde_json::from_str(&ply_machine::shipped::runtime_json()).expect("the description reads");
    let layouts = &described["layouts"];
    let fields = |object: &str| -> Vec<String> {
        let mut names: Vec<String> = layouts[object]
            .as_object()
            .unwrap_or_else(|| panic!("no layout of `{object}`"))
            .keys()
            .cloned()
            .collect();
        names.sort();
        names
    };
    assert_eq!(
        fields("list"),
        ["room", "root", "start", "tail", "tail_len"]
    );
    assert_eq!(fields("trie"), ["branch", "leaf", "width"]);
    assert_eq!(fields("map"), ["height", "root"]);
    assert_eq!(fields("tree"), ["branch", "keys", "leaf", "sizes", "width"]);
    for (object, field) in [
        ("list", "start"),
        ("list", "tail_len"),
        ("list", "room"),
        ("map", "height"),
    ] {
        assert!(
            ["flags", "aux", "layout"].contains(&layouts[object][field].as_str().unwrap_or("")),
            "{object}'s {field} names a header field"
        );
    }
}
