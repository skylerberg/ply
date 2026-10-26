//! The shipped modules as a binary's pack holds them: each `.ply` file below the library a module
//! named by its place, and every other file data a module embeds, held by name.

use crate::pack::SHIPPED;
use ply_privileged::shipped_modules;

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

/// `compiler.emit.roots` is `emit/roots.ply` below the compiler, beside `compiler.front`.
#[test]
fn a_compiler_module_below_the_compiler_is_named_by_its_place() {
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
fn a_data_file_a_shipped_module_embeds_is_held_by_name_and_is_no_module() {
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
    // A module is no data file, and nothing outside the shipped modules is held.
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

/// What a runtime's library describes itself as carries each native under the fields
/// `compiler.native.runtime_of` reads, and the natives' names alone, which a `ply` that reads no
/// `natives` builds against.
#[test]
fn the_runtime_describes_each_native_under_the_fields_a_build_reads() {
    let described: serde_json::Value =
        serde_json::from_str(&ply_privileged::shipped::runtime_json())
            .expect("the description reads");
    let natives = described["natives"].as_array().expect("natives");
    let named = |name: &str| {
        natives
            .iter()
            .find(|n| n["name"] == name)
            .unwrap_or_else(|| panic!("no native `{name}`"))
    };
    let concat = named("Secret::concat");
    assert_eq!(
        (&concat["words"], &concat["answers"], &concat["raises"]),
        (&2.into(), &true.into(), &false.into())
    );
    let div = named("Decimal::div");
    assert_eq!((&div["words"], &div["raises"]), (&4.into(), &true.into()));
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

/// A field's name, offset and bytes.
type Field<'a> = (&'a str, u64, u64);

/// Each field a description lists, in its order.
fn described_fields(fields: &serde_json::Value) -> Vec<Field<'_>> {
    fields
        .as_array()
        .expect("a list of fields")
        .iter()
        .map(|f| {
            (
                f["name"].as_str().expect("a field's name"),
                f["at"].as_u64().expect("a field's offset"),
                f["bytes"].as_u64().expect("a field's bytes"),
            )
        })
        .collect()
}

/// Past what a released build reads, the description says what emitted code lays out against: each
/// native's machine kinds, the context's fields compiled code reads, the structs it lays out, and
/// how deeply drop roots nest.
#[test]
fn the_runtime_describes_what_emitted_code_lays_out_against() {
    let described: serde_json::Value =
        serde_json::from_str(&ply_privileged::shipped::runtime_json())
            .expect("the description reads");
    let native = |name: &str| {
        described["natives"]
            .as_array()
            .expect("natives")
            .iter()
            .find(|n| n["name"] == name)
            .unwrap_or_else(|| panic!("no native `{name}`"))
            .clone()
    };
    let concat = native("Secret::concat");
    assert_eq!(concat["params"], serde_json::json!(["Word", "Word"]));
    assert_eq!(concat["answer"], "Word");
    let out = native("?out_str");
    assert_eq!(
        (&out["params"], &out["answer"], &out["words"]),
        (
            &serde_json::json!(["Addr", "I64"]),
            &serde_json::Value::Null,
            &2.into()
        )
    );
    // A native that answers nothing may still raise.
    let refused = native("?refused");
    assert_eq!(
        (&refused["params"], &refused["answer"], &refused["raises"]),
        (
            &serde_json::json!(["I64", "I64", "I64", "I64"]),
            &serde_json::Value::Null,
            &true.into()
        )
    );
    let text = native("?refused_text");
    assert_eq!(
        (&text["params"], &text["raises"]),
        (
            &serde_json::json!(["I64", "Addr", "I64", "Addr", "I64"]),
            &true.into()
        )
    );
    let ctx = described_fields(&described["ctx"]);
    let names: Vec<&str> = ctx.iter().map(|f| f.0).collect();
    assert_eq!(
        names,
        [
            "failed",
            "fuel",
            "stack_floor",
            "site_root",
            "site_start",
            "site_end",
            "ticks",
            "next_tick",
            "frames",
            "release_depth",
            "release_waiting"
        ],
        "the release's eight, then those emitted code reads past them"
    );
    for (i, (name, at, bytes)) in ctx.iter().enumerate() {
        assert_eq!((*at, *bytes), (8 * i as u64, 8), "`{name}` is not word {i}");
    }
    assert_eq!(described["release_depth"], 32);
}

/// `drop#Hold<a>` reads what a hold holds where the description says, as
/// `compiler.emit.layout`'s `HoldLaid` names it; the release's `layouts` stay as they were.
#[test]
fn the_runtime_describes_where_a_hold_keeps_what_it_holds() {
    let described: serde_json::Value =
        serde_json::from_str(&ply_privileged::shipped::runtime_json())
            .expect("the description reads");
    assert_eq!(described["hold"], serde_json::json!({ "held": 1 }));
    assert!(described["layouts"].get("hold").is_none());
}

/// Each struct emitted code lays out and the runtime reads is described as
/// `compiler.emit.layout.machine_structs` lists it: in its order, each field at its offset and of
/// its bytes, which is what the head declares and the runtime's Rust reads.
#[test]
fn the_runtime_describes_the_structs_emitted_code_lays_out_as_the_compiler_states_them() {
    let described: serde_json::Value =
        serde_json::from_str(&ply_privileged::shipped::runtime_json())
            .expect("the description reads");
    let structs: Vec<(&str, u64, Vec<Field>)> = described["descriptors"]
        .as_array()
        .expect("a list of structs")
        .iter()
        .map(|s| {
            (
                s["name"].as_str().expect("a struct's name"),
                s["bytes"].as_u64().expect("a struct's bytes"),
                described_fields(&s["fields"]),
            )
        })
        .collect();
    let word = |name: &'static str, at: u64| (name, at, 8);
    assert_eq!(
        structs,
        [
            ("PlyFrame", 16, vec![word("prev", 0), word("each", 8)]),
            (
                "PlyType",
                56,
                vec![
                    word("identity", 0),
                    word("spelled", 8),
                    ("spelled_len", 16, 4),
                    word("inc", 24),
                    word("dec", 32),
                    word("lower", 40),
                    word("lift", 48),
                ]
            ),
            (
                "PlyOp",
                64,
                vec![
                    word("effect", 0),
                    word("op", 8),
                    word("answer", 16),
                    word("own", 24),
                    word("params_len", 32),
                    word("params", 40),
                    word("vars_len", 48),
                    word("vars", 56),
                ]
            ),
            (
                "PlyFnDesc",
                48,
                vec![
                    word("uniform", 0),
                    word("typed", 8),
                    word("convention", 16),
                    word("release", 24),
                    word("share", 32),
                    word("keep", 40),
                ]
            ),
        ]
    );
}
