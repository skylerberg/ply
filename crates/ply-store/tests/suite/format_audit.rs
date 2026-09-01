//! An adversarial audit of the front-end cache's binary format.
//!
//! The format traded a loud failure mode for a silent one: a JSON schema drift
//! was a parse error, a binary one can decode into a plausible `Footprint` — and
//! a footprint decides which tests may run concurrently. Every test here damages
//! the files the way a crash, a partial write, a restored backup or a build with
//! a different schema would, and demands the same three things each time: no
//! crash, no value, and a warning.
//!
//! These sit beside the crate's own tests rather than inside them because they
//! only ever reach for the public surface — a caller with a damaged cache has no
//! more than that either.

use ply_core::{EffectAtom, Footprint, Resource, Row, RowVar, Scheme, TyVar, Type};
use ply_hash::DefHash;
use ply_span::{Symbol, codes};
use ply_store::{
    BODY_ENCODING, CachedCtor, CachedDecl, CachedDef, CachedOp, CachedTest, ContentHash, DeclBody,
    DefBody, DefEntry, DefKind, FileSpan, NameRef, Outcome, SourceFingerprint, Store,
};
use ply_syntax::ast::Mode;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// The layout, restated
// ---------------------------------------------------------------------------
//
// Restated rather than imported: a test that damages byte 96 has to fail when
// byte 96 stops meaning what it meant, and a constant shared with the code being
// audited would move with it.

const DATA_HEADER: usize = 56;
const FRAME_HEADER: usize = 13;
const INDEX_HEADER: usize = 132;

mod at {
    pub(super) const MAGIC: usize = 0;
    pub(super) const FORMAT: usize = 8;
    pub(super) const SCHEMA: usize = 16;
    pub(super) const NONCE: usize = 48;
    pub(super) const DATA_LEN: usize = 56;
    pub(super) const SECTIONS: usize = 96;
    pub(super) const CHECKSUM: usize = 100;
}

const KIND_DEF: u8 = 1;
const KIND_DECL: u8 = 2;
const KIND_BODY: u8 = 3;
const KIND_SOURCE: u8 = 4;

/// Descriptor positions: defs, decls, bodies, sources, paths.
const SECTION_DEFS: usize = 0;
const DESCRIPTOR: usize = 24;
const HASH_RECORD: usize = 48;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(name: &str) -> TempRoot {
        let dir = std::env::temp_dir().join(format!(
            "ply-format-audit-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temp root");
        TempRoot(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn open(&self) -> Store {
        Store::open(&self.0).expect("the store should open")
    }

    fn index_file(&self) -> PathBuf {
        self.0.join(".ply-cache").join("frontend.idx")
    }

    fn data_file(&self) -> PathBuf {
        self.0.join(".ply-cache").join("frontend.dat")
    }

    fn source_file(&self) -> PathBuf {
        self.0.join("src/user.ply")
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn hash(n: u8) -> DefHash {
    DefHash([n; 32])
}

fn footprint() -> Footprint {
    Footprint::from_atoms([
        EffectAtom::new("db", Resource::Named(Symbol::new("users")), Mode::Read),
        EffectAtom::new("clock", Resource::Singleton, Mode::Write),
    ])
}

fn scheme() -> Scheme {
    Scheme {
        ty_vars: vec![TyVar(0)],
        row_vars: vec![RowVar(0)],
        ty: Type::Fn {
            params: vec![Type::Var(TyVar(0))],
            ret: Box::new(Type::int()),
            effects: Row::open(RowVar(0)),
        },
    }
}

fn def() -> CachedDef {
    CachedDef::new(scheme(), footprint()).witnessed_by(vec![NameRef::new("user.User", hash(9))])
}

fn decl() -> CachedDecl {
    CachedDecl::new(DeclBody::Effect {
        nondet: true,
        ops: vec![CachedOp {
            name: Symbol::new("get"),
            mode: Mode::Read,
            resource_param: true,
            params: vec![Type::int()],
            ret: Type::unit(),
        }],
    })
}

fn fingerprint() -> SourceFingerprint {
    SourceFingerprint {
        content_hash: ContentHash::of(b"fn active_users() -> Int = 1\n"),
        imports: vec![ply_store::ImportEdge {
            module: Symbol::new("store.db"),
            exports: ContentHash::of(b"exports"),
        }],
        deps: vec![NameRef::new("store.db.get", hash(7))],
        defs: vec![DefEntry {
            name: Symbol::new("user.active_users"),
            hash: hash(1),
            span: FileSpan { start: 10, end: 42 },
            kind: DefKind::Fn,
            members: vec![ply_store::Member {
                name: Symbol::new("user.Active"),
                span: FileSpan { start: 12, end: 18 },
            }],
            deps: vec![Symbol::new("store.db.get")],
        }],
        tests: vec![CachedTest {
            name: "active_users excludes inactive".to_string(),
            hash: hash(5),
            nondet: false,
            footprint: footprint(),
            span: FileSpan {
                start: 50,
                end: 120,
            },
            name_span: FileSpan { start: 55, end: 60 },
            deps: vec![Symbol::new("user.active_users")],
        }],
    }
}

/// A cache holding one of everything, plus a result so that a test can show the
/// result cache came through a front-end failure untouched.
fn seeded(name: &str) -> TempRoot {
    let root = TempRoot::new(name);
    let mut store = root.open();
    store.put(hash(200), Outcome::Pass);
    store.put_source(&root.source_file(), fingerprint());
    store.put_def(hash(1), def());
    store.put_def(hash(2), CachedDef::new(scheme(), Footprint::empty()));
    store.put_decl(hash(9), decl());
    store.put_body(hash(1), DefBody::new(BODY_ENCODING, vec![0x20, 0xca, 0xfe]));
    store.flush().expect("the seed should flush");
    let check = root.open();
    assert!(check.warnings().is_empty(), "the seed must be clean");
    assert_eq!(check.defs_len(), 2);
    root
}

fn assert_seed_is_whole(store: &Store, root: &TempRoot, what: &str) {
    assert!(store.def(hash(1)).is_some(), "{what}");
    assert!(store.decl(hash(9)).is_some(), "{what}");
    assert!(store.body(hash(1)).is_some(), "{what}");
    assert!(store.fingerprint(&root.source_file()).is_some(), "{what}");
}

fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path).expect("the cache file should be readable")
}

fn write(path: &Path, bytes: &[u8]) {
    std::fs::write(path, bytes).expect("the cache file should be writable");
}

fn patch(path: &Path, offset: usize, bytes: &[u8]) {
    let mut file = read(path);
    file[offset..offset + bytes.len()].copy_from_slice(bytes);
    write(path, &file);
}

/// The whole-index checksum, recomputed. Repairing it is what turns a damaged
/// index into a *plausible* one, which is the only way to reach the checks that
/// live below the checksum.
fn repair_index(path: &Path) {
    let mut bytes = read(path);
    let checksum = *blake3::hash(&bytes[INDEX_HEADER..]).as_bytes();
    bytes[at::CHECKSUM..at::CHECKSUM + 32].copy_from_slice(&checksum);
    write(path, &bytes);
}

fn frame_checksum(kind: u8, payload: &[u8]) -> [u8; 8] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&[kind]);
    hasher.update(&(payload.len() as u32).to_le_bytes());
    hasher.update(payload);
    let mut out = [0u8; 8];
    out.copy_from_slice(&hasher.finalize().as_bytes()[..8]);
    out
}

/// Every frame in a data file as `(offset, kind, payload length)`, found by
/// walking it — which nothing in the store ever does, because a frame is only
/// ever reached through an index record that already claims where it is.
fn frames(path: &Path) -> Vec<(usize, u8, usize)> {
    let bytes = read(path);
    let mut out = Vec::new();
    let mut at = DATA_HEADER;
    while at + FRAME_HEADER <= bytes.len() {
        let len = u32::from_le_bytes(bytes[at + 1..at + 5].try_into().unwrap()) as usize;
        if at + FRAME_HEADER + len > bytes.len() {
            break;
        }
        out.push((at, bytes[at], len));
        at += FRAME_HEADER + len;
    }
    out
}

fn frame_of(path: &Path, kind: u8) -> (usize, usize) {
    let (offset, _, len) = frames(path)
        .into_iter()
        .find(|(_, k, _)| *k == kind)
        .unwrap_or_else(|| panic!("no frame of kind {kind}"));
    (offset, len)
}

fn payload_of(path: &Path, kind: u8) -> Vec<u8> {
    let (offset, len) = frame_of(path, kind);
    read(path)[offset + FRAME_HEADER..offset + FRAME_HEADER + len].to_vec()
}

/// Overwrites a frame's payload in place and repairs its checksum: bytes that
/// verify but no longer say what the decoder expects, which is what a build with
/// a different schema looks like from the outside.
fn rewrite_payload(path: &Path, kind: u8, payload: &[u8]) {
    let (offset, len) = frame_of(path, kind);
    assert_eq!(payload.len(), len, "this helper cannot resize a frame");
    let mut bytes = read(path);
    bytes[offset + 5..offset + FRAME_HEADER].copy_from_slice(&frame_checksum(kind, payload));
    bytes[offset + FRAME_HEADER..offset + FRAME_HEADER + len].copy_from_slice(payload);
    write(path, &bytes);
}

/// `nth` is a position in the `defs` section, which is sorted by hash.
fn def_record(index: &Path, nth: usize) -> usize {
    let bytes = read(index);
    let descriptor = INDEX_HEADER + SECTION_DEFS * DESCRIPTOR;
    let start = u64::from_le_bytes(bytes[descriptor + 8..descriptor + 16].try_into().unwrap());
    start as usize + nth * HASH_RECORD
}

fn located(index: &Path, record: usize) -> (u64, u32) {
    let bytes = read(index);
    (
        u64::from_le_bytes(bytes[record + 32..record + 40].try_into().unwrap()),
        u32::from_le_bytes(bytes[record + 40..record + 44].try_into().unwrap()),
    )
}

fn set_located(index: &Path, record: usize, offset: u64, len: u32) {
    patch(index, record + 32, &offset.to_le_bytes());
    patch(index, record + 40, &len.to_le_bytes());
    repair_index(index);
}

/// The shape every damaged cache must present: nothing cached, one warning, and
/// a result cache that never noticed.
fn assert_degraded(store: &Store, root: &TempRoot, what: &str) {
    assert!(
        store.frontend_is_empty(),
        "{what}: the front-end cache should have degraded to empty"
    );
    assert!(store.def(hash(1)).is_none(), "{what}");
    assert!(store.decl(hash(9)).is_none(), "{what}");
    assert!(store.body(hash(1)).is_none(), "{what}");
    assert!(store.fingerprint(&root.source_file()).is_none(), "{what}");
    let warnings = store.warnings();
    assert_eq!(
        warnings.len(),
        1,
        "{what}: exactly one warning, got {warnings:?}"
    );
    assert!(
        matches!(
            warnings[0].code,
            codes::CACHE_CORRUPT | codes::CACHE_VERSION_CHANGED | codes::CACHE_UNREADABLE
        ),
        "{what}: unexpected code {}",
        warnings[0].code
    );
    assert!(
        !warnings[0].notes.is_empty(),
        "{what}: must say what happens next"
    );
    assert_eq!(
        store.len(),
        1,
        "{what}: the result cache is a separate file"
    );
}

/// A damaged cache is not a broken project: the next run must be able to write a
/// healthy one over it.
fn assert_repairs_itself(root: &TempRoot, what: &str) {
    let mut store = root.open();
    store.put_def(hash(42), def());
    store
        .flush()
        .expect("a damaged cache must still be writable");
    let repaired = root.open();
    assert!(
        repaired.warnings().is_empty(),
        "{what}: it must repair itself, got {:?}",
        repaired.warnings()
    );
    assert!(repaired.def(hash(42)).is_some(), "{what}");
}

// ---------------------------------------------------------------------------
// The data file: truncation
// ---------------------------------------------------------------------------

/// A killed writer that got half a frame out, and then a *later* index that
/// vouches for the whole of it. The index is the authority on how long the data
/// file is, so a file shorter than that is never partly believed.
#[test]
fn a_data_file_truncated_inside_an_entry_degrades_to_an_empty_cache() {
    let root = seeded("dat-truncated-mid-entry");
    let bytes = read(&root.data_file());
    let (offset, _, len) = *frames(&root.data_file()).last().unwrap();
    let cut = offset + FRAME_HEADER + len / 2;
    assert!(cut < bytes.len(), "the cut must land inside the last frame");
    write(&root.data_file(), &bytes[..cut]);

    let store = root.open();
    assert_degraded(&store, &root, "a data file cut inside an entry");
    drop(store);
    assert_repairs_itself(&root, "a data file cut inside an entry");
}

/// The same cut, landing exactly on a frame boundary. Every byte below it is
/// intact and every index record below it would verify — and it is still refused,
/// because the two files are one cache and one of them is short.
#[test]
fn a_data_file_truncated_between_entries_degrades_to_an_empty_cache() {
    let root = seeded("dat-truncated-between-entries");
    let (offset, _, _) = *frames(&root.data_file()).last().unwrap();
    let bytes = read(&root.data_file());
    write(&root.data_file(), &bytes[..offset]);

    let store = root.open();
    assert_degraded(&store, &root, "a data file cut between entries");
    drop(store);
    assert_repairs_itself(&root, "a data file cut between entries");
}

#[test]
fn a_data_file_cut_to_nothing_degrades_to_an_empty_cache() {
    let root = seeded("dat-empty");
    write(&root.data_file(), &[]);

    let store = root.open();
    assert_degraded(&store, &root, "an empty data file");
    drop(store);
    assert_repairs_itself(&root, "an empty data file");
}

#[test]
fn a_data_file_cut_inside_its_own_header_degrades_to_an_empty_cache() {
    let root = seeded("dat-header-cut");
    let bytes = read(&root.data_file());
    write(&root.data_file(), &bytes[..DATA_HEADER / 2]);

    let store = root.open();
    assert_degraded(&store, &root, "a data file shorter than its header");
}

/// Random bytes of exactly the right length, so nothing about the file's size
/// gives it away. The magic number is the first thing that has to disagree.
#[test]
fn a_data_file_of_random_bytes_degrades_to_an_empty_cache() {
    let root = seeded("dat-random");
    let len = read(&root.data_file()).len();
    let mut noise = Vec::with_capacity(len);
    let mut state = 0x243f_6a88_85a3_08d3u64;
    for _ in 0..len {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        noise.push((state >> 33) as u8);
    }
    write(&root.data_file(), &noise);

    let store = root.open();
    assert_degraded(&store, &root, "a data file of random bytes");
    drop(store);
    assert_repairs_itself(&root, "a data file of random bytes");
}

/// Every field of the data file's header, one at a time. A cache whose *data*
/// file was written by another build must be refused as loudly as one whose
/// index was.
#[test]
fn every_field_of_the_data_header_is_checked() {
    for (what, offset, bytes) in [
        ("magic", at::MAGIC, vec![b'N'; 8]),
        ("format", at::FORMAT, 0xffff_u32.to_le_bytes().to_vec()),
        ("schema", at::SCHEMA, vec![0x5a; 32]),
        ("nonce", at::NONCE, vec![0x11; 8]),
    ] {
        let root = seeded(&format!("dat-header-{what}"));
        patch(&root.data_file(), offset, &bytes);

        let store = root.open();
        assert_degraded(&store, &root, &format!("a damaged data-file {what}"));
    }
}

// ---------------------------------------------------------------------------
// The frame header
// ---------------------------------------------------------------------------

/// A frame is reached through an index record that already claims its kind, its
/// length and — through the checksum — its contents. Damaging any one of the
/// three must cost that entry and nothing else.
#[test]
fn every_field_of_a_frame_header_is_checked_before_its_payload_is_believed() {
    for (what, field, expected) in [
        ("kind", 0usize, "filed under a different kind"),
        ("length", 1, "length does not match"),
        ("checksum", 5, "checksum does not match"),
    ] {
        let root = seeded(&format!("frame-header-{what}"));
        let (offset, _) = frame_of(&root.data_file(), KIND_DEF);
        let mut bytes = read(&root.data_file());
        bytes[offset + field] ^= 0x40;
        write(&root.data_file(), &bytes);

        let mut store = root.open();
        assert!(
            store.warnings().is_empty(),
            "{what}: the damage is below the index, so opening cannot see it"
        );
        assert!(
            store.def(hash(1)).is_none() || store.def(hash(2)).is_none(),
            "{what}: a frame whose header disagrees with the index must not answer"
        );
        // The other entries are untouched: one bad frame is one missing entry.
        assert!(store.decl(hash(9)).is_some(), "{what}");
        assert!(store.body(hash(1)).is_some(), "{what}");
        assert!(store.fingerprint(&root.source_file()).is_some(), "{what}");

        let warnings = store.take_warnings();
        assert_eq!(warnings.len(), 1, "{what}: {warnings:?}");
        assert_eq!(warnings[0].code, codes::CACHE_CORRUPT, "{what}");
        assert!(
            warnings[0].message.contains(expected),
            "{what}: expected `{expected}`, got `{}`",
            warnings[0].message
        );
    }
}

/// A single flipped bit anywhere in a payload. The checksum is what makes this a
/// miss rather than a plausible wrong `Scheme`.
#[test]
fn a_single_damaged_byte_in_a_payload_costs_that_entry_and_no_other() {
    let root = seeded("payload-bitflip");
    let (offset, len) = frame_of(&root.data_file(), KIND_DEF);
    for i in [0usize, len / 2, len - 1] {
        let mut bytes = read(&root.data_file());
        bytes[offset + FRAME_HEADER + i] ^= 0x01;
        write(&root.data_file(), &bytes);

        let mut store = root.open();
        assert!(store.warnings().is_empty());
        let answered = [store.def(hash(1)), store.def(hash(2))]
            .into_iter()
            .flatten()
            .count();
        assert_eq!(
            answered, 1,
            "byte {i}: the damaged interface must not answer"
        );
        let warnings = store.take_warnings();
        assert_eq!(warnings.len(), 1, "byte {i}");
        assert!(warnings[0].message.contains("checksum"), "byte {i}");

        let mut bytes = read(&root.data_file());
        bytes[offset + FRAME_HEADER + i] ^= 0x01;
        write(&root.data_file(), &bytes);
    }
}

// ---------------------------------------------------------------------------
// The index
// ---------------------------------------------------------------------------

/// The whole-index checksum is what protects the records, which are the only
/// thing binding a `DefHash` to a byte range. One damaged byte in a record must
/// cost the cache, not produce a different answer.
#[test]
fn a_damaged_index_record_is_refused_rather_than_followed() {
    for field in [0usize, 32, 40] {
        let root = seeded(&format!("index-record-{field}"));
        let record = def_record(&root.index_file(), 0);
        let mut bytes = read(&root.index_file());
        bytes[record + field] ^= 0x01;
        write(&root.index_file(), &bytes);

        let store = root.open();
        assert_degraded(
            &store,
            &root,
            &format!("a damaged index record byte {field}"),
        );
    }
}

/// With the checksum repaired the record is *plausible*, which is the only way
/// to reach the checks below it. A misaligned offset lands in the interior of a
/// frame, where the kind byte, the length and the checksum cannot all agree.
#[test]
fn an_index_offset_moved_into_the_interior_of_a_frame_is_refused() {
    for slide in [1i64, -1, 5, 7, 64] {
        let root = seeded(&format!("index-interior-{slide}"));
        let record = def_record(&root.index_file(), 0);
        let (offset, len) = located(&root.index_file(), record);
        set_located(
            &root.index_file(),
            record,
            (offset as i64 + slide) as u64,
            len,
        );

        let mut store = root.open();
        let answers: Vec<CachedDef> = [store.def(hash(1)), store.def(hash(2))]
            .into_iter()
            .flatten()
            .map(|d| (*d).clone())
            .collect();
        for answer in &answers {
            assert!(
                *answer == def().canonicalized()
                    || *answer == CachedDef::new(scheme(), Footprint::empty()).canonicalized(),
                "slide {slide}: the store answered with a value nobody stored"
            );
        }
        assert!(
            answers.len() <= 1,
            "slide {slide}: the moved record must not answer"
        );
        assert!(
            !store.take_warnings().is_empty(),
            "slide {slide}: the refusal must be reported"
        );
    }
}

/// An offset below the data file's own header, which no frame can ever occupy.
/// This one is caught while the index is read, before anything asks for an entry.
#[test]
fn an_index_offset_inside_the_data_header_is_refused_at_open() {
    let root = seeded("index-offset-in-header");
    let record = def_record(&root.index_file(), 0);
    let (_, len) = located(&root.index_file(), record);
    set_located(&root.index_file(), record, 8, len);

    let store = root.open();
    assert_degraded(&store, &root, "an offset inside the data header");
}

/// An index that names bytes the data file does not have. This is the crash
/// window in the *forbidden* order — the index made durable before the data it
/// vouches for — and it must never be read as though the offsets meant something.
#[test]
fn an_index_that_claims_more_data_than_exists_is_refused() {
    let root = seeded("index-claims-too-much");
    let bytes = read(&root.index_file());
    let data_len = u64::from_le_bytes(bytes[at::DATA_LEN..at::DATA_LEN + 8].try_into().unwrap());
    patch(
        &root.index_file(),
        at::DATA_LEN,
        &(data_len + 4096).to_le_bytes(),
    );
    repair_index(&root.index_file());

    let store = root.open();
    assert_degraded(&store, &root, "an index claiming more data than exists");
    drop(store);
    assert_repairs_itself(&root, "an index claiming more data than exists");
}

/// The other side of the same crash: the data was appended and synced, and the
/// index that would have named it never landed. The frames are simply invisible,
/// which is not a corrupt cache — and the next flush truncates them away.
#[test]
fn data_appended_past_the_index_is_invisible_rather_than_corrupt() {
    let root = seeded("index-behind-data");
    let donor = seeded("index-behind-data-donor");
    let extra = read(&donor.data_file())[DATA_HEADER..].to_vec();
    let mut bytes = read(&root.data_file());
    let committed = bytes.len();
    bytes.extend_from_slice(&extra);
    write(&root.data_file(), &bytes);

    let mut store = root.open();
    assert!(store.warnings().is_empty(), "{:?}", store.warnings());
    assert_seed_is_whole(&store, &root, "an index behind its data file");
    assert_eq!(
        store.defs_len(),
        2,
        "nothing above the index may be counted"
    );

    store.put_def(hash(42), def());
    store.flush().expect("it should flush");
    assert!(
        read(&root.data_file()).len() < committed + extra.len(),
        "the unindexed tail must be truncated, not appended after"
    );
}

/// The index header is not covered by the index checksum, so the fields in it
/// have to defend themselves. Each of these is a single damaged field.
#[test]
fn every_field_of_the_index_header_is_checked() {
    for (what, offset, bytes) in [
        ("magic", at::MAGIC, vec![b'N'; 8]),
        ("format", at::FORMAT, 0xffff_u32.to_le_bytes().to_vec()),
        ("schema", at::SCHEMA, vec![0x5a; 32]),
        ("nonce", at::NONCE, vec![0x11; 8]),
        ("data length", at::DATA_LEN, 0u64.to_le_bytes().to_vec()),
    ] {
        let root = seeded(&format!("index-header-{what}"));
        patch(&root.index_file(), offset, &bytes);

        let store = root.open();
        assert_degraded(&store, &root, &format!("a damaged index {what}"));
    }
}

/// A section table that describes more sections than the index holds. The
/// descriptors it would have to read lie in the records, which are not
/// descriptors.
#[test]
fn a_section_count_larger_than_the_table_is_refused() {
    for count in [6u32, 7, 64, u32::MAX] {
        let root = seeded(&format!("index-sections-{count}"));
        patch(&root.index_file(), at::SECTIONS, &count.to_le_bytes());

        let store = root.open();
        assert_degraded(&store, &root, &format!("a section count of {count}"));
    }
}

// ---------------------------------------------------------------------------
// Pairing the two files
// ---------------------------------------------------------------------------

/// An append never moves a byte, so an index from before an append still names
/// exactly what it named — this is the property that lets a reader take no lock,
/// and it has to keep holding.
#[test]
fn an_older_index_over_an_appended_data_file_still_reads_its_own_entries() {
    let root = seeded("older-index");
    let old = read(&root.index_file());

    let mut store = root.open();
    store.put_def(hash(42), def());
    store.put_source(&root.path().join("src/other.ply"), fingerprint());
    store.flush().expect("it should flush");
    drop(store);

    write(&root.index_file(), &old);
    let store = root.open();
    assert!(
        store.warnings().is_empty(),
        "an older index is a valid view, not a corrupt one: {:?}",
        store.warnings()
    );
    assert_seed_is_whole(&store, &root, "an older index over an appended data file");
    assert!(
        store.def(hash(42)).is_none(),
        "an entry the older index never named must not appear"
    );
}

/// Compaction is the one thing that moves bytes, so it takes a fresh nonce. An
/// index from before it names offsets that now hold something else, and following
/// them would be the format's one route to a *wrong* answer.
#[test]
fn an_index_from_before_a_compaction_is_refused_rather_than_followed() {
    let root = seeded("pre-compaction-index");
    let stale = read(&root.index_file());

    let mut store = root.open();
    store.forget_source(&root.source_file());
    store.compact(&[]).expect("compaction should succeed");
    drop(store);

    let moved = frames(&root.data_file());
    assert!(!moved.is_empty(), "compaction should have kept something");

    write(&root.index_file(), &stale);
    let store = root.open();
    assert!(
        store.frontend_is_empty(),
        "a pre-compaction index must not be followed into a rewritten data file"
    );
    assert_eq!(store.warnings().len(), 1, "{:?}", store.warnings());
}

/// Two projects' files, mixed. Neither the index nor the data file is damaged;
/// they simply were not written together.
#[test]
fn an_index_and_a_data_file_from_different_projects_are_refused() {
    let root = seeded("mixed-index");
    let other = seeded("mixed-data");
    write(&root.data_file(), &read(&other.data_file()));

    let store = root.open();
    assert_degraded(&store, &root, "a data file from another project");
}

// ---------------------------------------------------------------------------
// Shape drift
// ---------------------------------------------------------------------------

/// The case a checksum cannot catch: bytes that verify but were written to a
/// different shape. Here a definition's frame is given a declaration's payload,
/// which is what a build that renumbered its entry kinds would produce.
#[test]
fn a_frame_carrying_another_shapes_payload_is_refused_rather_than_misread() {
    let root = seeded("shape-swap");
    let decl_payload = payload_of(&root.data_file(), KIND_DECL);
    let (def_offset, def_len) = frame_of(&root.data_file(), KIND_DEF);

    // Same length or the index would disagree before the tag ever mattered, so
    // pad the declaration payload out with the definition's trailing bytes.
    let mut forged = decl_payload;
    forged.resize(def_len, 0xee);
    let mut bytes = read(&root.data_file());
    bytes[def_offset + 5..def_offset + FRAME_HEADER]
        .copy_from_slice(&frame_checksum(KIND_DEF, &forged));
    bytes[def_offset + FRAME_HEADER..def_offset + FRAME_HEADER + def_len].copy_from_slice(&forged);
    write(&root.data_file(), &bytes);

    let mut store = root.open();
    let answered = [store.def(hash(1)), store.def(hash(2))]
        .into_iter()
        .flatten()
        .count();
    assert_eq!(
        answered, 1,
        "a declaration's bytes must not become a definition"
    );
    let warnings = store.take_warnings();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(warnings[0].code, codes::CACHE_CORRUPT);
}

/// An index record pointed at a frame of another kind. The kind byte in the
/// frame header is what stops a renumbering of the sections from handing a
/// declaration's bytes to the definition decoder.
#[test]
fn an_index_record_pointed_at_a_frame_of_another_kind_is_refused() {
    let root = seeded("kind-crossed");
    let (decl_offset, decl_len) = frame_of(&root.data_file(), KIND_DECL);
    let record = def_record(&root.index_file(), 0);
    set_located(
        &root.index_file(),
        record,
        decl_offset as u64,
        decl_len as u32,
    );

    let mut store = root.open();
    let answered = [store.def(hash(1)), store.def(hash(2))]
        .into_iter()
        .flatten()
        .count();
    assert_eq!(
        answered, 1,
        "a declaration's frame must not answer as a definition"
    );
    assert!(
        !store.take_warnings().is_empty(),
        "the refusal must be reported"
    );
    let store = root.open();
    assert!(
        store.decl(hash(9)).is_some(),
        "the declaration itself is fine"
    );
}

/// Every byte of a stored fingerprint, mutated one at a time with the frame
/// checksum repaired — a drifted encoder, simulated exhaustively. The decoder
/// may refuse, and it may decode; what it may not do is decode into a value that
/// is not what those bytes say. Re-storing whatever came back and comparing the
/// frame it produces is how "not what those bytes say" is measured.
///
/// A fingerprint rather than an interface because nothing canonicalizes one, so
/// a difference in the bytes is a difference in the value and not a renumbering.
#[test]
fn no_mutation_of_a_fingerprint_decodes_into_something_it_does_not_say() {
    let root = TempRoot::new("fingerprint-mutations");
    let mut store = root.open();
    store.put_source(&root.source_file(), fingerprint());
    store.flush().expect("it should flush");
    drop(store);

    let pristine_index = read(&root.index_file());
    let pristine_data = read(&root.data_file());
    let (offset, len) = frame_of(&root.data_file(), KIND_SOURCE);

    let scratch = TempRoot::new("fingerprint-mutations-scratch");
    let mut decoded = 0usize;

    for i in 0..len {
        for mask in [0x01u8, 0x80, 0xff] {
            write(&root.index_file(), &pristine_index);
            let mut bytes = pristine_data.clone();
            bytes[offset + FRAME_HEADER + i] ^= mask;
            let payload = bytes[offset + FRAME_HEADER..offset + FRAME_HEADER + len].to_vec();
            bytes[offset + 5..offset + FRAME_HEADER]
                .copy_from_slice(&frame_checksum(KIND_SOURCE, &payload));
            write(&root.data_file(), &bytes);

            let store = root.open();
            let Some(back) = store.fingerprint(&root.source_file()) else {
                continue;
            };
            decoded += 1;

            let mut fresh = scratch.open();
            fresh.clear().expect("the scratch cache should clear");
            fresh.put_source(&scratch.source_file(), (*back).clone());
            fresh.flush().expect("the scratch cache should flush");
            assert_eq!(
                payload_of(&scratch.data_file(), KIND_SOURCE),
                payload,
                "byte {i} ^ {mask:#04x} decoded into a value that re-encodes differently"
            );
        }
    }

    assert!(
        decoded > 0,
        "the mutations must at least sometimes be accepted, or this proves nothing"
    );
}

// ---------------------------------------------------------------------------
// Shapes the schema exemplars never reach
// ---------------------------------------------------------------------------

/// `schema_fingerprint`'s exemplars are the definition of what "the schema"
/// means, and what they do not encode, they do not pin. These are the shapes a
/// real project produces that no exemplar contains: a closed effect row, an
/// empty footprint, a type with several constructors, a record with several
/// fields, an effect with several operations, a fingerprint with nothing in it.
/// They must round-trip byte for byte or the pin is guarding less than it claims.
#[test]
fn shapes_no_schema_exemplar_reaches_still_round_trip() {
    let root = TempRoot::new("uncovered-shapes");
    let closed = Scheme {
        ty_vars: vec![],
        row_vars: vec![],
        // `Row::empty` — a closed row. Every exemplar's row carries a tail.
        ty: Type::Fn {
            params: vec![],
            ret: Box::new(Type::Record(BTreeMap::from([
                (Symbol::new("id"), Type::int()),
                (Symbol::new("name"), Type::string()),
                (Symbol::new("tags"), Type::list(Type::string())),
            ]))),
            effects: Row::empty(),
        },
    };
    let many_ctors = CachedDecl::new(DeclBody::Type {
        arity: 2,
        ctors: vec![
            CachedCtor {
                fields: vec![],
                scheme: closed.clone(),
            },
            CachedCtor {
                fields: vec![Type::Var(TyVar(0)), Type::Var(TyVar(1))],
                scheme: closed.clone(),
            },
            CachedCtor {
                fields: vec![Type::int()],
                scheme: closed.clone(),
            },
        ],
    });
    let many_ops = CachedDecl::new(DeclBody::Effect {
        nondet: false,
        ops: vec![
            CachedOp {
                name: Symbol::new("get"),
                mode: Mode::Read,
                resource_param: false,
                params: vec![],
                ret: Type::unit(),
            },
            CachedOp {
                name: Symbol::new("put"),
                mode: Mode::Write,
                resource_param: true,
                params: vec![Type::int(), Type::string()],
                ret: Type::unit(),
            },
        ],
    });
    let bare = SourceFingerprint::new(ContentHash::of(b""));
    let empty_footprint = CachedDef::new(closed.clone(), Footprint::empty());

    let mut store = root.open();
    store.put_def(hash(1), empty_footprint.clone());
    store.put_decl(hash(2), many_ctors.clone());
    store.put_decl(hash(3), many_ops.clone());
    store.put_source(&root.source_file(), bare.clone());
    store.flush().expect("it should flush");

    let reopened = root.open();
    assert!(reopened.warnings().is_empty(), "{:?}", reopened.warnings());
    assert_eq!(
        reopened.def(hash(1)).as_deref(),
        Some(&empty_footprint.canonicalized())
    );
    assert_eq!(
        reopened.decl(hash(2)).as_deref(),
        Some(&many_ctors.canonicalized()),
        "constructors are matched by position, so their order must survive"
    );
    assert_eq!(
        reopened.decl(hash(3)).as_deref(),
        Some(&many_ops.canonicalized())
    );
    assert_eq!(
        reopened.fingerprint(&root.source_file()).as_deref(),
        Some(&bare)
    );
}

/// A symbol is length-prefixed UTF-8, and a span is a pair of `u32`s. Neither
/// has a value a real project cannot reach, so neither may have a value the
/// encoding cannot carry.
#[test]
fn the_edges_of_every_scalar_field_survive_a_round_trip() {
    let root = TempRoot::new("scalar-edges");
    let long = "m.".to_string() + &"λ🜂ünïcode".repeat(64);
    let mut fp = SourceFingerprint::new(ContentHash([0xff; 32]));
    fp.defs.push(DefEntry {
        name: Symbol::new(&long),
        hash: DefHash([0xff; 32]),
        span: FileSpan {
            start: u32::MAX - 1,
            end: u32::MAX,
        },
        kind: DefKind::Effect,
        members: vec![],
        deps: vec![Symbol::new("")],
    });
    fp.tests.push(CachedTest {
        name: String::new(),
        hash: DefHash([0; 32]),
        nondet: true,
        footprint: Footprint::empty(),
        span: FileSpan { start: 0, end: 0 },
        name_span: FileSpan { start: 0, end: 0 },
        deps: vec![],
    });

    let mut store = root.open();
    store.put_source(&root.source_file(), fp.clone());
    store.flush().expect("it should flush");

    let reopened = root.open();
    assert!(reopened.warnings().is_empty(), "{:?}", reopened.warnings());
    assert_eq!(
        reopened.fingerprint(&root.source_file()).as_deref(),
        Some(&fp)
    );
}

/// Whatever the encoder will write, the decoder must read back. The two once
/// disagreed — the decoder refused past a constant the encoder did not know
/// about — and the result was a healthy cache calling itself corrupt on every
/// run for the affected definitions, with `compact` copying the offending frame
/// verbatim so the complaint never went away. Depths well past that old constant
/// are exercised on purpose.
#[test]
fn a_deeply_nested_type_round_trips_rather_than_reporting_a_healthy_cache_corrupt() {
    for depth in [1usize, 8, 64, 122, 132, 400, 800] {
        let root = TempRoot::new(&format!("deep-type-{depth}"));
        let mut ty = Type::int();
        for _ in 0..depth {
            ty = Type::list(ty);
        }
        let deep = CachedDef::new(
            Scheme {
                ty_vars: vec![],
                row_vars: vec![],
                ty,
            },
            Footprint::empty(),
        );

        let mut store = root.open();
        store.put_def(hash(1), deep.clone());
        store.flush().expect("it should flush");

        let reopened = root.open();
        let back = reopened
            .def(hash(1))
            .unwrap_or_else(|| panic!("depth {depth} was written and must read back"));
        assert_eq!(&*back, &deep.canonicalized(), "depth {depth}");
        assert!(
            reopened.warnings().is_empty(),
            "depth {depth}: {:?}",
            reopened.warnings()
        );
    }
}

// ---------------------------------------------------------------------------
// Bodies
// ---------------------------------------------------------------------------

/// A body is keyed by a hash of itself, so a damaged one has two independent
/// gates: the frame checksum, and the key check `body_set` applies. Neither may
/// let a body through as a definition it is not.
#[test]
fn a_damaged_body_never_becomes_a_definition() {
    let root = seeded("body-damage");
    let (offset, len) = frame_of(&root.data_file(), KIND_BODY);
    let mut bytes = read(&root.data_file());
    bytes[offset + FRAME_HEADER + len - 1] ^= 0xff;
    write(&root.data_file(), &bytes);

    let mut store = root.open();
    assert!(store.body(hash(1)).is_none());
    assert!(!store.has_body(hash(1)));
    let warnings = store.take_warnings();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(warnings[0].code, codes::CACHE_CORRUPT);

    let (set, missing) = store.body_set([hash(1)]);
    assert!(set.is_empty());
    assert_eq!(missing, vec![hash(1)]);
}

/// A body whose payload was rewritten to bytes that verify against the frame but
/// are not the definition the key names. `body_set` is the gate that has to hold,
/// because nothing about a body is a matter of opinion.
#[test]
fn a_body_rewritten_to_other_bytes_is_not_rebuilt_into_a_definition() {
    let root = seeded("body-substituted");
    let payload = payload_of(&root.data_file(), KIND_BODY);
    let mut forged = payload.clone();
    // The tail of the envelope is the body's own bytes; changing them changes
    // what the body *is* without changing the frame it lives in.
    let last = forged.len() - 1;
    forged[last] ^= 0xff;
    rewrite_payload(&root.data_file(), KIND_BODY, &forged);

    let store = root.open();
    let (set, missing) = store.body_set([hash(1)]);
    assert!(
        set.is_empty(),
        "a body that is not the one its key names must not be rebuilt"
    );
    assert_eq!(missing, vec![hash(1)]);
}

// ---------------------------------------------------------------------------
// Concurrency
// ---------------------------------------------------------------------------

/// Readers take no lock. A reader that arrives at any moment during a run of
/// appends must see a whole cache — never a torn one, and never a warning.
#[test]
fn a_reader_arriving_mid_append_never_sees_a_torn_cache() {
    let root = seeded("concurrent-append");
    let root = &root;
    let done = std::sync::atomic::AtomicBool::new(false);
    let done = &done;

    std::thread::scope(|scope| {
        scope.spawn(move || {
            for n in 20u8..60 {
                let mut store = root.open();
                store.put_def(hash(n), def());
                store.put_source(&root.path().join(format!("src/f{n}.ply")), fingerprint());
                store.put_body(hash(n), DefBody::new(BODY_ENCODING, vec![n; 32]));
                store.flush().expect("a writer should flush");
            }
            done.store(true, std::sync::atomic::Ordering::Release);
        });
        scope.spawn(move || {
            while !done.load(std::sync::atomic::Ordering::Acquire) {
                let store = root.open();
                assert_seed_is_whole(&store, root, "a reader running beside a writer");
                assert!(
                    store.warnings().is_empty(),
                    "a reader saw a torn cache: {:?}",
                    store.warnings()
                );
            }
        });
    });

    let store = root.open();
    assert!(store.warnings().is_empty());
    assert_seed_is_whole(&store, root, "after the writers finished");
}

/// Two writers, each flushing repeatedly. The lock is what stops their frames
/// from interleaving; the property is that the cache is readable afterwards and
/// that neither writer's entries became the other's.
#[test]
fn concurrent_writers_never_interleave_their_frames() {
    let root = seeded("concurrent-writers");
    let root = &root;

    std::thread::scope(|scope| {
        for w in 0u8..3 {
            scope.spawn(move || {
                for i in 0u8..10 {
                    let n = 60 + w * 10 + i;
                    let mut store = root.open();
                    store.put_def(hash(n), CachedDef::new(scheme(), footprint()));
                    store.put_body(hash(n), DefBody::new(BODY_ENCODING, vec![n; 8]));
                    store.flush().expect("a writer should flush");
                }
            });
        }
    });

    let store = root.open();
    assert!(store.warnings().is_empty(), "{:?}", store.warnings());
    assert_seed_is_whole(&store, root, "after concurrent writers");
    for n in 60u8..90 {
        if let Some(body) = store.body(hash(n)) {
            assert_eq!(
                body.as_bytes(),
                &[n; 8],
                "a body came back as another writer's"
            );
        }
    }
}

/// A section count is the one index-header field with no independent check, and
/// the header is not covered by the index checksum — so a damaged one is read as
/// written. Losing a section is safe, because a missing entry is a recheck; what
/// must never happen is a section read as another one's records.
#[test]
fn a_damaged_section_count_loses_entries_but_never_invents_one() {
    for count in [0u32, 1, 2, 3, 4, 5] {
        let root = seeded(&format!("index-sections-short-{count}"));
        patch(&root.index_file(), at::SECTIONS, &count.to_le_bytes());

        let store = root.open();
        assert!(store.defs_len() <= 2, "count {count}");
        assert!(store.decls_len() <= 1, "count {count}");
        assert!(store.bodies_len() <= 1, "count {count}");
        assert!(store.sources_len() <= 1, "count {count}");
        if let Some(found) = store.def(hash(1)) {
            assert_eq!(&*found, &def().canonicalized(), "count {count}");
        }
        if let Some(found) = store.decl(hash(9)) {
            assert_eq!(&*found, &decl().canonicalized(), "count {count}");
        }
        if let Some(found) = store.fingerprint(&root.source_file()) {
            assert_eq!(&*found, &fingerprint(), "count {count}");
        }
        assert_eq!(
            store.len(),
            1,
            "count {count}: the result cache is a separate file"
        );
    }
}
