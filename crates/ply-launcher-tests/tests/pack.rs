//! The pack a native program carries after its code, read back as `ply bootstrap` lays it out.

use std::io::Write;

/// `files` laid out as a pack after `code`: their bytes, the table ascending by path, the trailer.
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
fn a_binary_reads_back_each_file_of_the_pack_after_its_code() {
    let pack = read(&binary(
        b"\x7fELF code",
        &[
            (
                "crates/ply-std/ply/option.ply",
                b"pub type Option<a> = Some(a) | None",
            ),
            ("program/front", b"front"),
        ],
    ))
    .expect("the pack reads")
    .expect("the binary carries one");
    assert_eq!(pack.bytes("program/front"), Some(&b"front"[..]));
    assert_eq!(
        pack.text("crates/ply-std/ply/option.ply"),
        Some("pub type Option<a> = Some(a) | None")
    );
    assert_eq!(pack.bytes("crates/ply-std/ply/list.ply"), None);
    assert_eq!(
        pack.digest_of("program/front"),
        Some(*blake3::hash(b"front").as_bytes())
    );
    assert_eq!(
        pack.paths().collect::<Vec<_>>(),
        ["crates/ply-std/ply/option.ply", "program/front"]
    );
}

#[test]
fn a_pack_lists_the_files_directly_in_a_directory_and_the_modules_and_data_below_it() {
    let pack = read(&binary(
        b"",
        &[
            ("crates/ply-std/ply/option.ply", b"a"),
            ("crates/ply-std/ply/hash/legacy.ply", b"b"),
            ("crates/ply-std/ply/oid/names.txt", b"c"),
            ("crates/ply-std/ply.txt", b"d"),
            ("crates/ply-compiler/ply/front.ply", b"e"),
        ],
    ))
    .expect("the pack reads")
    .expect("the binary carries one");
    assert_eq!(
        pack.files_in(ply_pack::STD).collect::<Vec<_>>(),
        ["crates/ply-std/ply/option.ply"]
    );
    assert_eq!(
        pack.modules_below(ply_pack::STD).collect::<Vec<_>>(),
        [
            "crates/ply-std/ply/hash/legacy.ply",
            "crates/ply-std/ply/option.ply"
        ]
    );
    assert_eq!(
        pack.data_below(ply_pack::STD).collect::<Vec<_>>(),
        ["crates/ply-std/ply/oid/names.txt"]
    );
}

#[test]
fn an_answer_is_the_frame_a_range_of_the_answers_file_holds() {
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
        pack.answer("store", 0, one.len()),
        Some(b"one record".to_vec())
    );
    assert_eq!(
        pack.answer("store", one.len(), two.len()),
        Some(b"another".to_vec())
    );
    assert_eq!(pack.answer("index", 0, 0), Some(b"index".to_vec()));
    // Not where a frame starts, past the file, or no file at all.
    assert_eq!(pack.answer("store", 1, one.len()), None);
    assert_eq!(pack.answer("store", one.len(), two.len() + 1), None);
    assert_eq!(pack.answer("records", 0, 0), None);
}

#[test]
fn a_packs_digest_moves_with_any_file_and_with_nothing_else() {
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
fn a_digest_below_directories_moves_with_a_file_there_and_with_none_outside() {
    let below = |files: &[(&str, &[u8])]| {
        read(&binary(b"", files))
            .expect("the pack reads")
            .expect("the binary carries one")
            .digest_below(&[ply_pack::STD, ply_pack::COMPILER])
    };
    let base = below(&[
        ("crates/ply-std/ply/option.ply", b"a"),
        ("crates/ply-compiler/ply/infer.ply", b"p"),
        ("program/front", b"f"),
    ]);
    let moved = |files: &[(&str, &[u8])]| below(files) != base;
    assert!(moved(&[
        ("crates/ply-std/ply/option.ply", b"b"),
        ("crates/ply-compiler/ply/infer.ply", b"p"),
        ("program/front", b"f"),
    ]));
    assert!(moved(&[
        ("crates/ply-std/ply/option.ply", b"a"),
        ("crates/ply-std/ply/oid/names.txt", b"n"),
        ("crates/ply-compiler/ply/infer.ply", b"p"),
        ("program/front", b"f"),
    ]));
    assert!(moved(&[
        ("crates/ply-std/ply/option.ply", b"a"),
        ("crates/ply-compiler/ply/infer.ply", b"q"),
        ("program/front", b"f"),
    ]));
    // The program, and a file whose path only starts as a root's does, are no part of it.
    assert!(!moved(&[
        ("crates/ply-std/ply/option.ply", b"a"),
        ("crates/ply-std/ply.txt", b"d"),
        ("crates/ply-compiler/ply/infer.ply", b"p"),
        ("program/front", b"another"),
    ]));
}

// A stage is named for the program, which reaches only some shipped modules: an edit to another,
// or to a test in one, must still move the directory a rebuilt `ply` reads them from.
#[test]
fn a_stages_shipped_directory_moves_with_any_shipped_file() {
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
fn a_torn_pack_is_refused_rather_than_read() {
    let whole = binary(b"code", &[("a", b"1"), ("b", b"2")]);
    let mut lying = whole.clone();
    let trailer = lying.len() - 24;
    lying[trailer..trailer + 8].copy_from_slice(&(u64::MAX / 2).to_le_bytes());
    assert!(
        read(&lying).is_err(),
        "a trailer naming more than the binary holds"
    );

    // The table claims a file runs past where the table begins.
    let mut overrun = whole.clone();
    let table_len = u64::from_le_bytes(overrun[trailer + 8..trailer + 16].try_into().unwrap());
    let table = trailer - table_len as usize;
    let first_len = table + 4 + 4 + 1 + 8;
    overrun[first_len..first_len + 8].copy_from_slice(&1000u64.to_le_bytes());
    assert!(read(&overrun).is_err(), "a file past the table");
}

#[test]
fn a_table_out_of_order_is_refused() {
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
    let refused = read(&data).err().expect("an unsorted table is refused");
    assert!(refused.contains("ascending"), "{refused}");
}
