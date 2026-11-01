use std::io::Write;

fn binary(code: &[u8], files: &[(&str, &[u8])]) -> Vec<u8> {
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
    let mut out = code.to_vec();
    out.extend_from_slice(&data);
    out.extend_from_slice(&table);
    out.extend_from_slice(&((data.len() + table.len()) as u64).to_le_bytes());
    out.extend_from_slice(&(table.len() as u64).to_le_bytes());
    out.extend_from_slice(b"PLYPACK1");
    out
}

fn written(bytes: &[u8]) -> tempfile::NamedTempFile {
    let mut file = tempfile::NamedTempFile::new().expect("a scratch file");
    file.write_all(bytes).expect("written");
    file
}

fn read(bytes: &[u8]) -> Result<Option<ply_pack::Pack>, String> {
    let file = written(bytes);
    ply_pack::Pack::of_binary(file.path())
}

fn framed(bytes: &[u8]) -> Vec<u8> {
    zstd::stream::encode_all(bytes, 3).expect("compressed")
}

#[test]
fn a_binary_with_a_bundle_of_files_appended_after_its_code_lets_each_bundled_file_be_read_back() {
    let pack = read(&binary(
        b"\x7fELF code",
        &[
            ("std/option.ply", b"pub type Option<a> = Some(a) | None"),
            ("program/front", b"front"),
        ],
    ))
    .expect("the pack reads")
    .expect("the binary carries one");
    assert_eq!(pack.bytes("program/front"), Some(&b"front"[..]));
    assert_eq!(
        pack.text("std/option.ply"),
        Some("pub type Option<a> = Some(a) | None")
    );
    assert_eq!(pack.bytes("std/list.ply"), None);
    assert_eq!(
        pack.digest_of("program/front"),
        Some(*blake3::hash(b"front").as_bytes())
    );
    assert_eq!(
        pack.paths().collect::<Vec<_>>(),
        ["program/front", "std/option.ply"]
    );
}

#[test]
fn a_bundle_of_files_lists_the_module_files_and_the_data_files_under_a_given_directory() {
    let pack = read(&binary(
        b"",
        &[
            ("std/option.ply", b"a"),
            ("std/hash/legacy.ply", b"b"),
            ("std/oid/names.txt", b"c"),
            ("stdlib.txt", b"d"),
            ("compiler/front.ply", b"e"),
            ("compiler/emit/roots.ply", b"f"),
        ],
    ))
    .expect("the pack reads")
    .expect("the binary carries one");
    assert_eq!(
        pack.modules_below(ply_pack::STD).collect::<Vec<_>>(),
        ["std/hash/legacy.ply", "std/option.ply"]
    );
    assert_eq!(
        pack.data_below(ply_pack::STD).collect::<Vec<_>>(),
        ["std/oid/names.txt"]
    );
    assert_eq!(
        pack.modules_below(ply_pack::COMPILER).collect::<Vec<_>>(),
        ["compiler/emit/roots.ply", "compiler/front.ply"]
    );
}

#[test]
fn a_cached_result_is_the_length_prefixed_record_at_an_offset_and_length_within_a_bundled_cache_file()
 {
    let one = framed(b"one record");
    let two = framed(b"another");
    let store = [one.clone(), two.clone()].concat();
    let pack = read(&binary(
        b"",
        &[
            ("answers/store", &store),
            ("answers/index", &framed(b"index")),
        ],
    ))
    .expect("the pack reads")
    .expect("the binary carries one");
    assert_eq!(
        pack.result("store", 0, one.len()),
        Some(b"one record".to_vec())
    );
    assert_eq!(
        pack.result("store", one.len(), two.len()),
        Some(b"another".to_vec())
    );
    assert_eq!(pack.result("index", 0, 0), Some(b"index".to_vec()));
    assert_eq!(pack.result("store", 1, one.len()), None);
    assert_eq!(pack.result("store", one.len(), two.len() + 1), None);
    assert_eq!(pack.result("records", 0, 0), None);
}

#[test]
fn a_bundle_digest_changes_when_a_bundled_file_changes_and_does_not_change_when_the_binary_code_changes()
 {
    let digest = |code: &[u8], files: &[(&str, &[u8])]| {
        read(&binary(code, files))
            .expect("the pack reads")
            .expect("the binary carries one")
            .digest()
    };
    let base = digest(b"one", &[("a", b"1"), ("b", b"2")]);
    assert_eq!(digest(b"other code", &[("a", b"1"), ("b", b"2")]), base);
    assert_ne!(digest(b"one", &[("a", b"1"), ("b", b"3")]), base);
    assert_ne!(digest(b"one", &[("a", b"1"), ("c", b"2")]), base);
}

#[test]
fn a_bundle_digest_over_the_std_and_compiler_directories_changes_with_files_in_them_but_not_with_files_elsewhere()
 {
    let below = |files: &[(&str, &[u8])]| {
        read(&binary(b"", files))
            .expect("the pack reads")
            .expect("the binary carries one")
            .digest_below(&[ply_pack::STD, ply_pack::COMPILER])
    };
    let base = below(&[
        ("std/option.ply", b"a"),
        ("compiler/infer.ply", b"p"),
        ("program/front", b"f"),
    ]);
    let moved = |files: &[(&str, &[u8])]| below(files) != base;
    assert!(moved(&[
        ("std/option.ply", b"b"),
        ("compiler/infer.ply", b"p"),
        ("program/front", b"f"),
    ]));
    assert!(moved(&[
        ("std/option.ply", b"a"),
        ("std/oid/names.txt", b"n"),
        ("compiler/infer.ply", b"p"),
        ("program/front", b"f"),
    ]));
    assert!(moved(&[
        ("std/option.ply", b"a"),
        ("compiler/infer.ply", b"q"),
        ("program/front", b"f"),
    ]));
    assert!(!moved(&[
        ("std/option.ply", b"a"),
        ("stdlib.txt", b"d"),
        ("compiler/infer.ply", b"p"),
        ("program/front", b"another"),
    ]));
}

#[test]
fn the_shipped_directory_name_changes_with_the_build_stage_the_file_set_name_and_the_digest() {
    let name = ply_launcher::shipped_dir_name;
    let base = name("stamps", "definitions", &[1; 32]);
    assert_eq!(name("stamps", "definitions", &[1; 32]), base);
    assert_ne!(name("stamps", "definitions", &[2; 32]), base);
    assert_ne!(name("stamps", "other", &[1; 32]), base);
    assert_ne!(name("other", "definitions", &[1; 32]), base);
    assert!(base.starts_with("shipped-"));
}

#[test]
fn a_binary_with_no_pack_carries_none() {
    assert!(read(b"just code").expect("reads").is_none());
    assert!(read(b"").expect("reads").is_none());
}

#[test]
fn a_torn_pack_is_rejected_rather_than_read() {
    let whole = binary(b"code", &[("a", b"1"), ("b", b"2")]);
    let mut lying = whole.clone();
    let trailer = lying.len() - 24;
    lying[trailer..trailer + 8].copy_from_slice(&(u64::MAX / 2).to_le_bytes());
    assert!(
        read(&lying).is_err(),
        "a trailer naming more than the binary holds"
    );

    let mut overrun = whole.clone();
    let table_len = u64::from_le_bytes(overrun[trailer + 8..trailer + 16].try_into().unwrap());
    let table = trailer - table_len as usize;
    let first_len = table + 4 + 4 + 1 + 8;
    overrun[first_len..first_len + 8].copy_from_slice(&1000u64.to_le_bytes());
    assert!(read(&overrun).is_err(), "a file past the table");
}

#[test]
fn reading_a_bundle_whose_file_list_is_not_sorted_by_path_fails_with_an_error() {
    let mut data = b"12".to_vec();
    let mut table = 2u32.to_le_bytes().to_vec();
    for (path, at) in [("b", 0u64), ("a", 1u64)] {
        table.extend_from_slice(&1u32.to_le_bytes());
        table.extend_from_slice(path.as_bytes());
        table.extend_from_slice(&at.to_le_bytes());
        table.extend_from_slice(&1u64.to_le_bytes());
        table.extend_from_slice(&[0; 32]);
    }
    let pack_len = (data.len() + table.len()) as u64;
    let table_len = table.len() as u64;
    data.extend_from_slice(&table);
    data.extend_from_slice(&pack_len.to_le_bytes());
    data.extend_from_slice(&table_len.to_le_bytes());
    data.extend_from_slice(b"PLYPACK1");
    let rejected = read(&data).err().expect("an unsorted table is rejected");
    assert!(rejected.contains("ascending"), "{rejected}");
}
