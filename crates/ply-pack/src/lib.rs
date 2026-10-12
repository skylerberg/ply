//! What a native Ply program carries after its code: the shipped modules a load of another program
//! compiles against, what they answer below [`ANSWERS`], and the program's own front end, each a
//! file keyed by its path. `ply bootstrap` appends one to every program it links; the program maps
//! it back at startup.
//!
//! A pack is the files' bytes, then a table of `(path, offset, length, digest)` ascending by path,
//! then a trailer of the pack's and the table's lengths and [`MAGIC`].

use std::path::Path;
use std::sync::OnceLock;

/// The shipped modules: each `.ply` file at or below it is one, `hash/legacy.ply` being
/// `std.hash.legacy`. Every other file at or below it is data those modules embed.
pub const STD: &str = "crates/ply-std/ply";

const MODULE: &str = ".ply";

/// The compiler's own modules, shipped as `compiler.<stem>`, and the builtins it embeds.
pub const COMPILER: &str = "crates/ply-compiler/ply";
pub const PRELUDE: &str = "crates/ply-compiler/prelude.ply";

/// The memo store of what every shipped module answers, as `ply std --answers` writes it, each file
/// gzipped.
pub const ANSWERS: &str = "answers";

const MAGIC: &[u8; 8] = b"PLYPACK1";
const TRAILER: usize = 8 + 8 + MAGIC.len();

pub type Digest = [u8; 32];

pub struct Pack {
    /// Ascending by path.
    entries: Vec<Entry>,
    mapped: memmap2::Mmap,
}

struct Entry {
    path: String,
    at: usize,
    len: usize,
    digest: Digest,
}

impl Pack {
    /// The pack appended to the binary at `binary`, or `None` when it carries none.
    pub fn of_binary(binary: &Path) -> Result<Option<Pack>, String> {
        let file = std::fs::File::open(binary)
            .map_err(|e| format!("`{}` could not be opened: {e}", binary.display()))?;
        // SAFETY: a binary is not written while it runs; `ply bootstrap` lands a new one by a rename.
        let mapped = unsafe { memmap2::Mmap::map(&file) }
            .map_err(|e| format!("`{}` could not be mapped: {e}", binary.display()))?;
        Pack::of_mapped(mapped)
    }

    fn of_mapped(mapped: memmap2::Mmap) -> Result<Option<Pack>, String> {
        let Some((start, table)) = located(&mapped)? else {
            return Ok(None);
        };
        let entries = read_table(&mapped[table..mapped.len() - TRAILER], start, table)?;
        Ok(Some(Pack { entries, mapped }))
    }

    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|entry| entry.path.as_str())
    }

    /// The paths of the files directly in `dir`, ascending.
    pub fn files_in<'a>(&'a self, dir: &str) -> impl Iterator<Item = &'a str> + 'a {
        let prefix = format!("{dir}/");
        self.paths().filter(move |path| {
            path.strip_prefix(prefix.as_str())
                .is_some_and(|name| !name.contains('/'))
        })
    }

    /// The paths of the data files at or below `dir`, ascending: every file there that is no
    /// module.
    pub fn data_below<'a>(&'a self, dir: &str) -> impl Iterator<Item = &'a str> + 'a {
        self.below_in(dir, false)
    }

    /// The paths of the modules at or below `dir`, ascending.
    pub fn modules_below<'a>(&'a self, dir: &str) -> impl Iterator<Item = &'a str> + 'a {
        self.below_in(dir, true)
    }

    fn below_in<'a>(&'a self, dir: &str, modules: bool) -> impl Iterator<Item = &'a str> + 'a {
        let prefix = format!("{dir}/");
        self.paths().filter(move |path| {
            path.starts_with(prefix.as_str()) && path.ends_with(MODULE) == modules
        })
    }

    pub fn bytes(&self, path: &str) -> Option<&[u8]> {
        self.entry(path)
            .map(|entry| &self.mapped[entry.at..entry.at + entry.len])
    }

    /// The file of the answers the pack carries at `path` within them, inflated.
    pub fn answer(&self, path: &str) -> Option<Vec<u8>> {
        use std::io::Read;
        let deflated = self.bytes(&format!("{ANSWERS}/{path}"))?;
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(deflated)
            .read_to_end(&mut out)
            .ok()?;
        Some(out)
    }

    /// A text the pack carries. The sources are UTF-8, as the front end reads them.
    pub fn text(&self, path: &str) -> Option<&str> {
        self.bytes(path).map(|bytes| {
            std::str::from_utf8(bytes)
                .unwrap_or_else(|e| panic!("`{path}` in the pack is not UTF-8: {e}"))
        })
    }

    pub fn digest_of(&self, path: &str) -> Option<Digest> {
        self.entry(path).map(|entry| entry.digest)
    }

    /// The whole pack: every path and what its bytes digest to.
    pub fn digest(&self) -> Digest {
        let mut hasher = blake3::Hasher::new();
        for entry in &self.entries {
            hasher.update(&(entry.path.len() as u64).to_le_bytes());
            hasher.update(entry.path.as_bytes());
            hasher.update(&entry.digest);
        }
        *hasher.finalize().as_bytes()
    }

    /// The files at or below `roots`: each one's path and what its bytes digest to, so it moves
    /// when one of them is added, dropped or edited, and with nothing outside them.
    pub fn digest_below(&self, roots: &[&str]) -> Digest {
        let mut hasher = blake3::Hasher::new();
        let below = |path: &str| {
            roots.iter().any(|root| {
                path.strip_prefix(root)
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
            })
        };
        for entry in self.entries.iter().filter(|entry| below(&entry.path)) {
            hasher.update(&(entry.path.len() as u64).to_le_bytes());
            hasher.update(entry.path.as_bytes());
            hasher.update(&entry.digest);
        }
        *hasher.finalize().as_bytes()
    }

    fn entry(&self, path: &str) -> Option<&Entry> {
        self.entries
            .binary_search_by(|entry| entry.path.as_str().cmp(path))
            .ok()
            .map(|at| &self.entries[at])
    }
}

/// Where a pack starts and where its table does, when `binary` ends in one.
fn located(binary: &[u8]) -> Result<Option<(usize, usize)>, String> {
    if binary.len() < TRAILER || &binary[binary.len() - MAGIC.len()..] != MAGIC {
        return Ok(None);
    }
    let trailer = binary.len() - TRAILER;
    let word = |at: usize| u64::from_le_bytes(binary[at..at + 8].try_into().expect("eight bytes"));
    let (pack_len, table_len) = (word(trailer) as usize, word(trailer + 8) as usize);
    if pack_len > trailer || table_len > pack_len {
        return Err("the pack's trailer names more bytes than the binary holds".to_string());
    }
    Ok(Some((trailer - pack_len, trailer - table_len)))
}

fn read_table(table: &[u8], start: usize, end: usize) -> Result<Vec<Entry>, String> {
    let torn = || "the pack's table is torn".to_string();
    let mut at = 0;
    let mut take = |n: usize| -> Result<&[u8], String> {
        let got = table.get(at..at + n).ok_or_else(torn)?;
        at += n;
        Ok(got)
    };
    let count = u32::from_le_bytes(take(4)?.try_into().expect("four bytes")) as usize;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let path_len = u32::from_le_bytes(take(4)?.try_into().expect("four bytes")) as usize;
        let path = String::from_utf8(take(path_len)?.to_vec()).map_err(|_| torn())?;
        let offset = u64::from_le_bytes(take(8)?.try_into().expect("eight bytes")) as usize;
        let len = u64::from_le_bytes(take(8)?.try_into().expect("eight bytes")) as usize;
        let digest: Digest = take(32)?.try_into().expect("thirty-two bytes");
        if start + offset + len > end {
            return Err(torn());
        }
        entries.push(Entry {
            path,
            at: start + offset,
            len,
            digest,
        });
    }
    if entries.windows(2).any(|pair| pair[0].path >= pair[1].path) {
        return Err("the pack's table is not in ascending order".to_string());
    }
    Ok(entries)
}

static INSTALLED: OnceLock<Pack> = OnceLock::new();

/// The pack this process reads, which a native program installs from its own binary before
/// anything runs.
pub fn install(pack: Pack) {
    if INSTALLED.set(pack).is_err() {
        panic!("a process installs one pack");
    }
}

pub fn installed() -> &'static Pack {
    INSTALLED
        .get()
        .expect("no pack is installed: a native program installs its own")
}
