use crate::pack::SHIPPED;
use ply_privileged::shipped_modules;

fn held(path: &str) -> &'static [u8] {
    SHIPPED
        .iter()
        .find(|(p, _)| *p == path)
        .map(|(_, bytes)| *bytes)
        .expect("the pack holds the file")
}

#[test]
fn a_standard_library_module_in_the_binary_is_listed_by_its_dotted_module_name() {
    let names = shipped_modules::names();
    assert!(names.iter().any(|n| n == "std.hash"), "{names:?}");
    assert!(names.iter().any(|n| n == "std.hash.legacy"), "{names:?}");
    assert_eq!(
        shipped_modules::source("std.hash.legacy").map(str::as_bytes),
        Some(held("std/hash/legacy.ply"))
    );
    assert!(
        !shipped_modules::data_names()
            .iter()
            .any(|n| n.ends_with(".ply")),
        "a module was listed as data"
    );
}

#[test]
fn a_compiler_module_in_the_binary_is_listed_by_its_dotted_module_name() {
    let names = shipped_modules::names();
    assert!(names.iter().any(|n| n == "compiler.front"), "{names:?}");
    assert!(
        names.iter().any(|n| n == "compiler.emit.roots"),
        "{names:?}"
    );
    assert_eq!(
        shipped_modules::source("compiler.emit.roots").map(str::as_bytes),
        Some(held("compiler/emit/roots.ply"))
    );
    assert_eq!(
        ply_privileged::shipped::module_digest("compiler.emit.roots"),
        Some(
            blake3::hash(held("compiler/emit/roots.ply"))
                .to_hex()
                .to_string()
        )
    );
}

#[test]
fn a_data_file_embedded_by_a_shipped_module_is_stored_under_its_path_and_is_not_listed_as_a_module()
{
    let name = "std/oid/names.txt";
    let bytes = held("std/oid/names.txt");
    assert!(shipped_modules::data_names().iter().any(|n| n == name));
    assert_eq!(shipped_modules::data(name), Some(bytes));
    assert_eq!(
        ply_privileged::shipped::module_digest(name),
        Some(blake3::hash(bytes).to_hex().to_string())
    );
    assert!(
        !shipped_modules::names().iter().any(|n| n.contains('/')),
        "a data file was listed as a module"
    );
    for other in [
        "std/oid.ply",
        "std/../../Cargo.toml",
        "oid/names.txt",
        "std/gone.txt",
    ] {
        assert_eq!(shipped_modules::data(other), None, "`{other}`");
    }
    assert_eq!(
        ply_privileged::shipped::module_digest("std.oid"),
        shipped_modules::source("std.oid")
            .map(|text| blake3::hash(text.as_bytes()).to_hex().to_string())
    );
}
