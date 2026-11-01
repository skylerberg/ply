pub const SHIPPED: &[(&str, &[u8])] = &[
    ("std/json.ply", b"pub type Json = | Null\n"),
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
