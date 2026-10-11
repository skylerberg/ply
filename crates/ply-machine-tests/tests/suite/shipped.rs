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
        Some(held("crates/ply-std/ply/hash/legacy.ply"))
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

#[test]
fn a_data_file_a_shipped_module_embeds_is_held_by_name_and_is_no_module() {
    let name = "std/oid/names.txt";
    let bytes = held("crates/ply-std/ply/oid/names.txt");
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
