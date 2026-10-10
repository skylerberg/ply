//! The pack a `ply` binary carries: what a checkout puts in it, and that a binary reads back exactly
//! what was appended to it.

use ply_pack::Pack;
use std::path::{Path, PathBuf};

/// This test binary's pack is the checkout it was built in, and what each test reads of it is traced.
#[ctor::ctor(unsafe)]
fn pack() {
    ply_machine::tested::installed(
        std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")),
        concat!(env!("CARGO_PKG_NAME"), "::", env!("CARGO_CRATE_NAME")),
    );
}

fn repo() -> PathBuf {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")).to_path_buf()
}

/// A stand-in for the binary the Rust build makes: bytes a pack goes after.
fn runtime(dir: &Path) -> PathBuf {
    let at = dir.join("ply");
    std::fs::write(&at, b"\x7fELF not really a program, only its bytes").expect("written");
    at
}

#[test]
fn a_checkout_packs_the_shipped_modules_the_builder_and_the_program() {
    let pack = Pack::of_checkout(&repo()).expect("the checkout packs");
    let paths: Vec<&str> = pack.paths().collect();
    for wanted in [
        "crates/ply-std/ply/result.ply",
        "crates/ply-compiler/ply/front.ply",
        "crates/ply-compiler/prelude.ply",
        "crates/ply-compiler/bootstrap/build.run",
        "crates/ply-compiler/bootstrap/build.digest",
        "crates/ply-cli/ply/ply.ply",
        "crates/ply-cli/ply/ply.pkg",
    ] {
        assert!(paths.contains(&wanted), "the pack holds no `{wanted}`");
    }
    assert!(
        paths.iter().all(|path| path.ends_with(".ply")
            || path.ends_with("ply.pkg")
            || path.contains("/bootstrap/")
            || path.starts_with("crates/ply-std/ply/")),
        "the pack holds something that is neither a source, a manifest, what `ply bootstrap` wrote nor data a shipped module embeds: {paths:?}"
    );
    assert!(
        !paths.iter().any(|path| path.contains("/.")),
        "a hidden file or directory was packed: {paths:?}"
    );
}

/// What the shipped modules answer rides beside the checkout's files: a binary reads each answer
/// back by its path in the store, and `check` holds the binary to the checkout's files alone.
#[test]
fn a_pack_carries_what_the_shipped_modules_answer_beside_the_checkouts_files() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let store = dir.path().join("answers");
    for (sub, name, bytes) in [
        ("records", "aa", b"a record".as_slice()),
        ("outputs", "bb", b"an answer".as_slice()),
        ("outputs", "cc.tmp", b"half written".as_slice()),
    ] {
        std::fs::create_dir_all(store.join(sub)).expect("the store's directory");
        std::fs::write(store.join(sub).join(name), bytes).expect("written");
    }
    let unclaimed = Pack::of_checkout(&repo())
        .expect("the checkout packs")
        .with_answers(&store, None);
    assert!(
        unclaimed.is_err(),
        "a store no answering finished was packed"
    );
    std::fs::write(store.join("standing"), b"claimed").expect("written");
    let binary = runtime(dir.path());
    let checkout = Pack::of_checkout(&repo()).expect("the checkout packs");
    let pack = Pack::of_checkout(&repo())
        .expect("the checkout packs")
        .with_answers(&store, None)
        .expect("the store is read");
    ply_pack::append(&binary, &pack).expect("the binary is packed");
    let read = Pack::of_binary(&binary)
        .expect("the binary reads")
        .expect("the binary carries a pack");
    assert_eq!(read.answer("records/aa"), Some(b"a record".to_vec()));
    assert_eq!(read.answer("outputs/bb"), Some(b"an answer".to_vec()));
    assert_eq!(read.answer("outputs/cc.tmp"), None);
    assert_eq!(read.answer("standing"), Some(b"claimed".to_vec()));
    assert!(matches!(
        ply_pack::check(&binary, &checkout).expect("the binary reads"),
        ply_pack::Checked::Same
    ));
}

#[test]
fn a_binary_is_current_once_it_carries_the_checkout_and_its_answers() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let store = dir.path().join("answers");
    for sub in ["records", "outputs"] {
        std::fs::create_dir_all(store.join(sub)).expect("the store's directory");
        std::fs::write(store.join(sub).join("aa"), b"kept").expect("written");
    }
    std::fs::write(store.join("standing"), b"claimed").expect("written");
    let binary = runtime(dir.path());
    let checkout = || Pack::of_checkout(&repo()).expect("the checkout packs");
    assert!(!ply_pack::current(&binary, &checkout()).expect("the binary reads"));
    ply_pack::append(&binary, &checkout()).expect("packed");
    assert!(!ply_pack::current(&binary, &checkout()).expect("the binary reads"));
    let answered = checkout()
        .with_answers(&store, None)
        .expect("the store is read");
    ply_pack::append(&binary, &answered).expect("packed with its answers");
    assert!(ply_pack::current(&binary, &checkout()).expect("the binary reads"));
}

/// An answer is named by its digest, so packing again takes the bytes the binary carries under its
/// name rather than deflating the file again: here the file was overwritten, which no store does.
#[test]
fn packing_again_takes_each_answer_the_binary_carries_as_it_carries_it() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let store = dir.path().join("answers");
    for sub in ["records", "outputs"] {
        std::fs::create_dir_all(store.join(sub)).expect("the store's directory");
    }
    std::fs::write(store.join("records/aa"), b"a record").expect("written");
    std::fs::write(store.join("outputs/bb"), b"an answer").expect("written");
    std::fs::write(store.join("standing"), b"claimed").expect("written");
    let binary = runtime(dir.path());
    let checkout = || Pack::of_checkout(&repo()).expect("the checkout packs");
    let once = checkout()
        .with_answers(&store, None)
        .expect("the store is read");
    ply_pack::append(&binary, &once).expect("packed");
    std::fs::write(store.join("outputs/bb"), b"overwritten").expect("written");
    std::fs::write(store.join("outputs/cc"), b"a new answer").expect("written");
    let carried = Pack::of_binary(&binary)
        .expect("the binary reads")
        .expect("the binary carries a pack");
    let again = checkout()
        .with_answers(&store, Some(&carried))
        .expect("the store is read");
    assert_eq!(again.answer("outputs/bb"), Some(b"an answer".to_vec()));
    assert_eq!(again.answer("outputs/cc"), Some(b"a new answer".to_vec()));
    assert_eq!(again.answer("records/aa"), Some(b"a record".to_vec()));
}

#[test]
fn a_binary_reads_back_the_pack_appended_to_it() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let binary = runtime(dir.path());
    let pack = Pack::of_checkout(&repo()).expect("the checkout packs");
    ply_pack::append(&binary, &pack).expect("the binary is packed");
    let read = Pack::of_binary(&binary)
        .expect("the binary reads")
        .expect("the binary carries a pack");
    assert_eq!(read.digest(), pack.digest());
    assert_eq!(
        read.paths().collect::<Vec<_>>(),
        pack.paths().collect::<Vec<_>>()
    );
    for path in pack.paths() {
        assert_eq!(
            read.bytes(path),
            pack.bytes(path),
            "`{path}` came back other bytes"
        );
    }
    let bytes = std::fs::read(&binary).expect("the binary reads");
    assert!(
        bytes.starts_with(b"\x7fELF not really a program"),
        "the runtime's bytes moved"
    );
}

#[test]
fn packing_again_replaces_the_pack_rather_than_adding_one() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let binary = runtime(dir.path());
    let pack = Pack::of_checkout(&repo()).expect("the checkout packs");
    ply_pack::append(&binary, &pack).expect("packed once");
    let once = std::fs::read(&binary).expect("the binary reads");
    ply_pack::append(&binary, &pack).expect("packed twice");
    assert_eq!(std::fs::read(&binary).expect("the binary reads"), once);
}

#[test]
fn a_binary_packed_from_other_sources_names_the_file_they_differ_on() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let other = dir.path().join("checkout");
    let pack = Pack::of_checkout(&repo()).expect("the checkout packs");
    for path in pack.paths() {
        let to = other.join(path);
        std::fs::create_dir_all(to.parent().expect("a parent")).expect("made");
        std::fs::copy(repo().join(path), to).expect("copied");
    }
    let edited = other.join("crates/ply-std/ply/result.ply");
    let mut text = std::fs::read_to_string(&edited).expect("reads");
    text.push_str("\n// edited\n");
    std::fs::write(&edited, text).expect("written");
    let binary = runtime(dir.path());
    ply_pack::append(&binary, &Pack::of_checkout(&other).expect("the copy packs")).expect("packed");
    match ply_pack::check(&binary, &pack).expect("the binary reads") {
        ply_pack::Checked::Differs(path) => assert_eq!(path, "crates/ply-std/ply/result.ply"),
        _ => panic!("a pack of other sources passed as this checkout's"),
    }
    ply_pack::append(&binary, &pack).expect("packed again");
    assert!(matches!(
        ply_pack::check(&binary, &pack).expect("the binary reads"),
        ply_pack::Checked::Same
    ));
}

#[test]
fn a_pack_edited_after_it_was_appended_fails_its_check() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let binary = runtime(dir.path());
    let pack = Pack::of_checkout(&repo()).expect("the checkout packs");
    ply_pack::append(&binary, &pack).expect("packed");
    let mut bytes = std::fs::read(&binary).expect("the binary reads");
    // The first byte of the first file, just after the runtime's bytes.
    let at = b"\x7fELF not really a program, only its bytes".len();
    bytes[at] ^= 1;
    std::fs::write(&binary, bytes).expect("written");
    assert!(ply_pack::check(&binary, &pack).is_err());
}

#[test]
fn a_binary_with_no_pack_carries_none() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let binary = runtime(dir.path());
    assert!(
        Pack::of_binary(&binary)
            .expect("the binary reads")
            .is_none()
    );
}

#[test]
fn a_torn_pack_is_refused_rather_than_read() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let binary = runtime(dir.path());
    let pack = Pack::of_checkout(&repo()).expect("the checkout packs");
    ply_pack::append(&binary, &pack).expect("packed");
    let mut bytes = std::fs::read(&binary).expect("the binary reads");
    // The trailer's pack length claims more than the file holds.
    let at = bytes.len() - 24;
    bytes[at..at + 8].copy_from_slice(&u64::MAX.to_le_bytes());
    std::fs::write(&binary, bytes).expect("written");
    assert!(Pack::of_binary(&binary).is_err());
}

#[test]
fn a_file_ply_bootstrap_does_not_write_is_refused() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let fake = dir.path();
    let copy = |from: &str| {
        let to = fake.join(from);
        std::fs::create_dir_all(to.parent().expect("a parent")).expect("made");
        std::fs::copy(repo().join(from), to).expect("copied");
    };
    for path in Pack::of_checkout(&repo())
        .expect("the checkout packs")
        .paths()
    {
        copy(path);
    }
    assert!(
        Pack::of_checkout(fake).is_ok(),
        "the copy packs as the checkout does"
    );
    std::fs::write(
        fake.join("crates/ply-compiler/bootstrap/notes.txt"),
        b"stray",
    )
    .expect("written");
    let refused = Pack::of_checkout(fake)
        .err()
        .expect("a stray file is refused");
    assert!(refused.contains("notes.txt"), "{refused}");
}

/// What of `listed` was not in a pack whose paths were `before`.
fn added<'a>(listed: impl Iterator<Item = &'a str>, before: &[String]) -> Vec<&'a str> {
    listed
        .filter(|path| !before.iter().any(|was| was == path))
        .collect()
}

/// The checkout's pack laid out under `fake`, as the checkout it was packed from.
fn copied(fake: &Path) {
    for path in Pack::of_checkout(&repo())
        .expect("the checkout packs")
        .paths()
    {
        let to = fake.join(path);
        std::fs::create_dir_all(to.parent().expect("a parent")).expect("made");
        std::fs::copy(repo().join(path), to).expect("copied");
    }
}

#[test]
fn the_modules_and_data_below_the_shipped_modules_are_packed_and_nothing_hidden_there_is() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let fake = dir.path().join("checkout");
    copied(&fake);
    let before: Vec<String> = Pack::of_checkout(&fake)
        .expect("the copy packs")
        .paths()
        .map(str::to_string)
        .collect();
    let std = fake.join(ply_pack::STD);
    let write = |below: &str, bytes: &[u8]| {
        let at = std.join(below);
        std::fs::create_dir_all(at.parent().expect("a parent")).expect("made");
        std::fs::write(at, bytes).expect("written");
    };
    write("words/list.txt", b"one\ntwo\n");
    write("words/deep/more.bin", &[0, 159, 146, 150]);
    write("beside.txt", b"beside the modules");
    write("words/nested.ply", b"pub fn one() -> Int = 1\n");
    write("words/deep/deeper.ply", b"pub fn two() -> Int = 2\n");
    write("words/.cache/kept.txt", b"not shipped");
    write("words/.hidden.ply", b"not shipped");
    write(".hidden.txt", b"not shipped");
    let pack = Pack::of_checkout(&fake).expect("the copy packs");
    assert_eq!(
        added(pack.data_below(ply_pack::STD), &before),
        [
            "crates/ply-std/ply/beside.txt",
            "crates/ply-std/ply/words/deep/more.bin",
            "crates/ply-std/ply/words/list.txt",
        ]
    );
    assert_eq!(
        added(pack.modules_below(ply_pack::STD), &before),
        [
            "crates/ply-std/ply/words/deep/deeper.ply",
            "crates/ply-std/ply/words/nested.ply",
        ]
    );
    assert!(
        pack.modules_below(ply_pack::STD)
            .any(|path| path == "crates/ply-std/ply/hash.ply"),
        "a module directly in the library is below it"
    );
    assert!(
        !pack
            .files_in(ply_pack::STD)
            .any(|path| path.ends_with("nested.ply")),
        "a module below the library was listed as one in it"
    );
    assert!(
        !pack.paths().any(|path| path.contains("/.")),
        "a hidden file was packed"
    );
    let binary = runtime(dir.path());
    ply_pack::append(&binary, &pack).expect("the binary is packed");
    let read = Pack::of_binary(&binary)
        .expect("the binary reads")
        .expect("the binary carries a pack");
    assert_eq!(
        read.bytes("crates/ply-std/ply/words/deep/more.bin"),
        Some(&[0u8, 159, 146, 150][..])
    );
    assert_eq!(
        read.data_below(ply_pack::STD).collect::<Vec<_>>(),
        pack.data_below(ply_pack::STD).collect::<Vec<_>>()
    );
    assert!(matches!(
        ply_pack::check(&binary, &pack).expect("the binary reads"),
        ply_pack::Checked::Same
    ));
    // Edited data is another pack, named by the file that differs.
    write("words/list.txt", b"one\ntwo\nthree\n");
    match ply_pack::check(&binary, &Pack::of_checkout(&fake).expect("the copy packs"))
        .expect("the binary reads")
    {
        ply_pack::Checked::Differs(path) => assert_eq!(path, "crates/ply-std/ply/words/list.txt"),
        _ => panic!("a pack of other data passed as this checkout's"),
    }
}

/// The data and the modules below a directory are two listings: a file that comes moves the one it
/// is in and leaves the other standing.
#[test]
fn a_listing_below_a_directory_is_a_trace_line_that_moves_when_a_file_of_its_kind_comes() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let fake = dir.path().join("checkout");
    copied(&fake);
    let pack = Pack::of_checkout(&fake).expect("the copy packs");
    ply_pack::record();
    let _ = pack.data_below(ply_pack::STD).count();
    let _ = pack.modules_below(ply_pack::STD).count();
    let lines = pack.asked_lines();
    let line = |prefix: &str| {
        lines
            .iter()
            .find(|l| l.starts_with(prefix))
            .cloned()
            .unwrap_or_else(|| panic!("no `{prefix}` line in {lines:?}"))
    };
    let data = line("packed\tcrates/ply-std/ply/**\t");
    let modules = line("packed\tcrates/ply-std/ply/**.ply\t");
    assert_eq!(pack.stands(&data), Some(true));
    assert_eq!(pack.stands(&modules), Some(true));
    let write = |below: &str, bytes: &[u8]| {
        let at = fake.join(ply_pack::STD).join(below);
        std::fs::create_dir_all(at.parent().expect("a parent")).expect("made");
        std::fs::write(at, bytes).expect("written");
    };
    write("words/list.txt", b"one\n");
    let grown = Pack::of_checkout(&fake).expect("the copy packs");
    assert_eq!(grown.stands(&data), Some(false));
    assert_eq!(grown.stands(&modules), Some(true));
    std::fs::remove_dir_all(fake.join(ply_pack::STD).join("words")).expect("removed");
    write("words/fresh.ply", b"pub fn one() -> Int = 1\n");
    let moduled = Pack::of_checkout(&fake).expect("the copy packs");
    assert_eq!(moduled.stands(&data), Some(true));
    assert_eq!(moduled.stands(&modules), Some(false));
}

#[test]
fn the_program_is_its_package_and_every_package_it_reaches_by_path() {
    let pack = Pack::of_checkout(&repo()).expect("the checkout packs");
    let packages = pack.program_packages();
    assert_eq!(
        packages.first().map(String::as_str),
        Some(ply_pack::PROGRAM)
    );
    for reached in [
        "crates/ply-prove/ply",
        "crates/ply-store/ply",
        "crates/ply-test/ply",
    ] {
        assert!(
            packages.iter().any(|p| p == reached),
            "{reached} is not in {packages:?}"
        );
    }
    let mut unique = packages.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        packages.len(),
        "a package was laid out twice: {packages:?}"
    );
}

#[test]
fn a_manifest_names_its_path_dependencies_and_nothing_else() {
    let manifest = r#"fn package() -> Manifest = {
  dependencies: [
    { name: "a", source: Path("../a") },
    { name: "b", source: Registry("b") },
    { name: "c", source: Path("../../x/c") },
  ],
}"#;
    assert_eq!(ply_pack::path_dependencies(manifest), ["../a", "../../x/c"]);
    assert_eq!(
        ply_pack::normalized("crates/ply-cli/ply/../../ply-prove/ply"),
        "crates/ply-prove/ply"
    );
    assert_eq!(ply_pack::normalized("./a//b/./c"), "a/b/c");
}

#[test]
fn what_a_process_asks_of_its_pack_is_a_trace_the_pack_answers() {
    let pack = Pack::of_checkout(&repo()).expect("the checkout packs");
    ply_pack::record();
    pack.bytes("crates/ply-std/ply/option.ply")
        .expect("the pack carries std.option");
    assert_eq!(pack.files_in("crates/ply-std/ply").count(), {
        ply_pack::unrecorded(|| pack.files_in("crates/ply-std/ply").count())
    });
    let lines = pack.asked_lines();
    let read = lines
        .iter()
        .find(|l| l.starts_with("pack\tcrates/ply-std/ply/option.ply\t"))
        .expect("the read is a line");
    let listed = lines
        .iter()
        .find(|l| l.starts_with("packed\tcrates/ply-std/ply\t"))
        .expect("the listing is a line");
    assert_eq!(pack.stands(read), Some(true));
    assert_eq!(pack.stands(listed), Some(true));
    let moved = format!("pack\tcrates/ply-std/ply/option.ply\t{}", "0".repeat(64));
    assert_eq!(pack.stands(&moved), Some(false));
    assert_eq!(pack.stands("file\trepo\tCargo.toml\t\tabc"), None);
}
