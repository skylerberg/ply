//! A program the builder makes embeds as `ply`'s own load does: each path read beside the module
//! that asks, written into it as a literal, and handed to the C backend with the answer.

use crate::fixture::{backend, loaded, scratch, write};
use ply_machine::testrun::{self, Executor, Hosting};

#[test]
fn a_program_loaded_from_nothing_runs_what_it_embedded() {
    let dir = scratch();
    write(
        dir.path(),
        "sub/m.ply",
        "fn one() -> Bytes = embed(\"../data.txt\")\n\
         fn all() -> List<{ name: String, bytes: Bytes }> = embed_dir(\"files\")\n\n\
         test \"embedded\" {\n  assert_eq(one(), b\"hi\");\n  \
         assert_eq(map(all(), |f: { name: String, bytes: Bytes }| f.name), \
         [\"a.txt\", \"b.txt\", \"deeper/c.txt\"]);\n  \
         assert_eq(map(all(), |f: { name: String, bytes: Bytes }| f.bytes), [b\"A\", b\"B\", b\"C\"])\n}\n",
    );
    write(dir.path(), "data.txt", "hi");
    write(dir.path(), "sub/files/b.txt", "B");
    write(dir.path(), "sub/files/a.txt", "A");
    write(dir.path(), "sub/files/deeper/c.txt", "C");
    write(dir.path(), "sub/files/.cache/d.txt", "not read");
    write(dir.path(), "sub/files/.hidden", "not read");
    let loaded = loaded(dir.path());
    let executor = Executor {
        front: &loaded.front,
        hosting: Hosting::default(),
        provider: backend(&loaded),
    };
    let ran = testrun::executed(&executor, 0, None);
    assert!(ran.failure.is_none(), "the test passes: {:?}", ran.failure);
}

/// `std.oid` embeds `oid/names.txt` from beside it, which a program has no file for: the builder
/// asks the binary for it by name.
#[test]
fn a_program_runs_what_a_shipped_module_it_imports_embedded() {
    let dir = scratch();
    write(
        dir.path(),
        "m.ply",
        "import std.oid\n\n\
         test \"named\" {\n  \
         assert_eq(oid::name(oid::common_name()), Some(\"commonName\"));\n  \
         assert_eq(oid::named(\"id-Ed25519\"), Some(oid::ed25519()))\n}\n",
    );
    let loaded = loaded(dir.path());
    let executor = Executor {
        front: &loaded.front,
        hosting: Hosting::default(),
        provider: backend(&loaded),
    };
    let ran = testrun::executed(&executor, 0, None);
    assert!(ran.failure.is_none(), "the test passes: {:?}", ran.failure);
}

#[test]
fn a_data_file_a_shipped_module_embeds_is_held_by_name_and_is_no_module() {
    use ply_machine::shipped_modules;
    let name = "std/oid/names.txt";
    let on_disk = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../ply-std/ply/oid/names.txt"),
    )
    .expect("the checkout holds the data file");
    assert!(shipped_modules::data_names().iter().any(|n| n == name));
    assert_eq!(shipped_modules::data(name), Some(&on_disk[..]));
    assert_eq!(
        ply_machine::shipped::module_digest(name),
        Some(blake3::hash(&on_disk).to_hex().to_string())
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
    let module = ply_eval::ModuleName::from_dotted("std.oid");
    assert_eq!(
        ply_machine::shipped::module_digest("std.oid"),
        shipped_modules::source(&module)
            .map(|text| blake3::hash(text.as_bytes()).to_hex().to_string())
    );
}

#[test]
fn an_embed_nothing_can_be_read_for_refuses_the_load() {
    let dir = scratch();
    write(
        dir.path(),
        "m.ply",
        "fn gone() -> Bytes = embed(\"missing.txt\")\n",
    );
    let refused = crate::fixture::refusal(dir.path());
    assert_eq!(refused.len(), 1, "{refused:?}");
    assert_eq!(refused[0].code, "E0146");
    assert_eq!(refused[0].message, "`missing.txt` could not be embedded");
}
