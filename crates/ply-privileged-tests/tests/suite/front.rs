//! Front ends written out by hand, as the CLI hands one to a machine: each file's place, module and
//! text, and the front end's answer.

use ply_core::Value;
use ply_privileged::payload::record;

/// The text of `std.json`, a module this binary's pack ships.
pub const JSON: &str = "pub type Json = | Null\n";

/// The files this binary's pack holds, by their paths in it.
pub const SHIPPED: &[(&str, &[u8])] = &[
    ("std/json.ply", JSON.as_bytes()),
    ("std/hash.ply", b"pub fn sha256(b: Bytes) -> Bytes = b\n"),
    (
        "std/hash/legacy.ply",
        b"pub fn sha1(b: Bytes) -> Bytes = b\n",
    ),
    (
        "std/oid.ply",
        b"pub fn names() -> Bytes = embed(\"oid/names.txt\")\n",
    ),
    ("std/oid/names.txt", b"2.5.4.3 commonName\n"),
    ("compiler/front.ply", b"pub fn one() -> Int = 1\n"),
    (
        "compiler/emit/roots.ply",
        b"import front\npub fn two() -> Int = front::one() + 1\n",
    ),
];

/// `files` laid out as a pack: their bytes, the table ascending by path, the trailer.
pub fn pack(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut sorted = files.to_vec();
    sorted.sort_by_key(|(path, _)| *path);
    let mut data = Vec::new();
    let mut table = (sorted.len() as u32).to_le_bytes().to_vec();
    for (path, bytes) in sorted {
        table.extend_from_slice(&(path.len() as u32).to_le_bytes());
        table.extend_from_slice(path.as_bytes());
        table.extend_from_slice(&(data.len() as u64).to_le_bytes());
        table.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        table.extend_from_slice(blake3::hash(bytes).as_bytes());
        data.extend_from_slice(bytes);
    }
    let pack_len = (data.len() + table.len()) as u64;
    let table_len = table.len() as u64;
    data.extend_from_slice(&table);
    data.extend_from_slice(&pack_len.to_le_bytes());
    data.extend_from_slice(&table_len.to_le_bytes());
    data.extend_from_slice(b"PLYPACK1");
    data
}

/// The front end's answer for a program that declares nothing a machine reads.
pub fn empty_dump() -> Value {
    let none = || Value::list(Vec::new());
    record(vec![
        ("diags", none()),
        ("modules", none()),
        ("defs", none()),
        ("types", none()),
        ("tests", none()),
        ("laws", none()),
        ("effects", none()),
        ("hashes", none()),
        ("hashes_digest", Value::bytes(vec![0u8; 32])),
        ("emitter_roots", none()),
        ("emitter_ctors", none()),
        ("ordinals", none()),
    ])
}

/// Each of `files` as `(path, module, text)`, as a front end lists them.
pub fn files(files: &[(&str, &str, &str)]) -> Value {
    Value::list(
        files
            .iter()
            .map(|(path, name, text)| {
                record(vec![
                    ("path", Value::str(*path)),
                    ("name", Value::str(*name)),
                    ("text", Value::bytes(text.as_bytes())),
                ])
            })
            .collect(),
    )
}

/// The front end the CLI hands a machine over `files` and `empty_dump`.
pub fn handed(listed: &[(&str, &str, &str)]) -> Value {
    record(vec![
        ("dump", empty_dump()),
        ("files", files(listed)),
        ("read_ms", Value::Int(0)),
        ("front_ms", Value::Int(0)),
        ("file_ms", Value::Int(0)),
        ("cached", Value::Bool(false)),
    ])
}
