//! The deployable artifact: the transitive closure of one entry point, in the same bytes the
//! content-addressed store already holds.

use crate::load::Loaded;
use ply_core::{CheckOutput, DefInfo};
use ply_hash::body::{BodySet, Namespace, StoredBody, reconstruct_named};
use ply_hash::{DefHash, HashOutput, hash_program_with_bodies};
use ply_span::{Diagnostic, SourceMap, Span, Symbol, codes};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::resolve::Resolved;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The generation of the container below.
pub const ARTIFACT_FORMAT: u32 = 1;

/// The extension `ply run` recognises, and the reason it does not have to guess.
pub const EXTENSION: &str = "plyx";

const MAGIC: &[u8; 8] = b"PLYPROG1";

/// Domain tag, so a program digest can never be confused with a definition hash or with `ply hosts
/// --digest`.
const DIGEST_DOMAIN: &[u8] = b"ply.program.1";

/// Bit 0 of `flags`: the `SOURCES` section is present.
const FLAG_SOURCES: u32 = 1;

const HEADER_LEN: usize = 188;
const DESCRIPTOR_LEN: usize = 24;
const OFF_FORMAT: usize = 8;
const OFF_FLAGS: usize = 12;
const OFF_FRONTEND: usize = 16;
const OFF_RUNTIME: usize = 48;
const OFF_BODY_ENC: usize = 80;
const OFF_STD: usize = 84;
const OFF_ENTRY: usize = 116;
const OFF_DIGEST: usize = 148;
/// Where the digest's coverage begins: every byte of the file from here on.
const OFF_SECTIONS: usize = 180;
const OFF_RESERVED: usize = 184;

const KIND_BODIES: u32 = 1;
const KIND_NAMES: u32 = 2;
const KIND_STRINGS: u32 = 3;
const KIND_SOURCES: u32 = 4;

/// A checked program, identified by a digest.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Artifact {
    pub frontend: [u8; 32],
    pub runtime: [u8; 32],
    pub body_encoding: u32,
    pub std: [u8; 32],
    pub entry: DefHash,
    /// Sorted by hash, which is what makes two builds byte-identical.
    pub bodies: BTreeMap<DefHash, StoredBody>,
    /// The namespace: program-wide name to hash, sorted.
    pub names: Vec<(String, DefHash)>,
    /// Source text, keyed by the path that names the module, relative to the project root so that
    /// two builds from two roots agree.
    pub sources: Vec<(String, String)>,
}

impl Artifact {
    pub fn has_sources(&self) -> bool {
        !self.sources.is_empty()
    }

    /// The program-wide name the entry point was built under.
    pub fn entry_name(&self) -> Option<&str> {
        self.names
            .iter()
            .find(|(_, hash)| *hash == self.entry)
            .map(|(name, _)| name.as_str())
    }

    /// BLAKE3 over every byte of the encoded file from `sections` onward: the section table and
    /// every payload, domain-tagged.
    pub fn digest(&self) -> [u8; 32] {
        let bytes = self.encode();
        digest_of(&bytes).unwrap_or([0; 32])
    }

    pub fn digest_short(&self) -> String {
        short(&self.digest())
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut sections: Vec<(u32, u32, Vec<u8>)> = Vec::with_capacity(4);

        let mut bodies = Vec::new();
        for (hash, body) in &self.bodies {
            bodies.extend_from_slice(&hash.0);
            bodies.extend_from_slice(&(body.len() as u32).to_le_bytes());
            bodies.extend_from_slice(body.as_bytes());
        }
        sections.push((KIND_BODIES, self.bodies.len() as u32, bodies));

        // One string per distinct name, appended in the order the records are written, so the blob
        // is a function of the record list and of nothing else.
        let mut strings: Vec<u8> = Vec::new();
        let mut offsets: BTreeMap<&str, u32> = BTreeMap::new();
        let mut names = Vec::new();
        for (name, hash) in &self.names {
            let offset = *offsets.entry(name.as_str()).or_insert_with(|| {
                let at = strings.len() as u32;
                strings.extend_from_slice(name.as_bytes());
                at
            });
            names.extend_from_slice(&offset.to_le_bytes());
            names.extend_from_slice(&(name.len() as u32).to_le_bytes());
            names.extend_from_slice(&hash.0);
        }
        sections.push((KIND_NAMES, self.names.len() as u32, names));
        sections.push((KIND_STRINGS, strings.len() as u32, strings));

        if self.has_sources() {
            let mut payload = Vec::new();
            for (path, text) in &self.sources {
                payload.extend_from_slice(&(path.len() as u32).to_le_bytes());
                payload.extend_from_slice(path.as_bytes());
                payload.extend_from_slice(&(text.len() as u32).to_le_bytes());
                payload.extend_from_slice(text.as_bytes());
            }
            sections.push((KIND_SOURCES, self.sources.len() as u32, payload));
        }

        let table = HEADER_LEN + DESCRIPTOR_LEN * sections.len();
        let mut out = vec![0u8; table];
        out[..8].copy_from_slice(MAGIC);
        out[OFF_FORMAT..OFF_FORMAT + 4].copy_from_slice(&ARTIFACT_FORMAT.to_le_bytes());
        let flags = if self.has_sources() { FLAG_SOURCES } else { 0 };
        out[OFF_FLAGS..OFF_FLAGS + 4].copy_from_slice(&flags.to_le_bytes());
        out[OFF_FRONTEND..OFF_FRONTEND + 32].copy_from_slice(&self.frontend);
        out[OFF_RUNTIME..OFF_RUNTIME + 32].copy_from_slice(&self.runtime);
        out[OFF_BODY_ENC..OFF_BODY_ENC + 4].copy_from_slice(&self.body_encoding.to_le_bytes());
        out[OFF_STD..OFF_STD + 32].copy_from_slice(&self.std);
        out[OFF_ENTRY..OFF_ENTRY + 32].copy_from_slice(&self.entry.0);
        out[OFF_SECTIONS..OFF_SECTIONS + 4].copy_from_slice(&(sections.len() as u32).to_le_bytes());
        out[OFF_RESERVED..OFF_RESERVED + 4].copy_from_slice(&0u32.to_le_bytes());

        let mut at = table as u64;
        for (i, (kind, count, payload)) in sections.iter().enumerate() {
            let d = HEADER_LEN + DESCRIPTOR_LEN * i;
            out[d..d + 4].copy_from_slice(&kind.to_le_bytes());
            out[d + 4..d + 8].copy_from_slice(&count.to_le_bytes());
            out[d + 8..d + 16].copy_from_slice(&at.to_le_bytes());
            out[d + 16..d + 24].copy_from_slice(&(payload.len() as u64).to_le_bytes());
            at += payload.len() as u64;
        }
        for (_, _, payload) in &sections {
            out.extend_from_slice(payload);
        }

        let digest = digest_of(&out).unwrap_or([0; 32]);
        out[OFF_DIGEST..OFF_DIGEST + 32].copy_from_slice(&digest);
        out
    }
}

/// The digest of an encoded artifact, read out of the bytes rather than out of the structure — so
/// the writer and the reader compute it over the same thing by construction rather than by two
/// functions agreeing.
fn digest_of(bytes: &[u8]) -> Option<[u8; 32]> {
    if bytes.len() < OFF_SECTIONS {
        return None;
    }
    let mut hasher = blake3::Hasher::new();
    hasher.update(DIGEST_DOMAIN);
    hasher.update(&bytes[OFF_SECTIONS..]);
    Some(*hasher.finalize().as_bytes())
}

/// `b3:` plus twelve hex characters, the shape `ply hosts --digest` and `ply std --digest` already
/// print.
pub fn short(digest: &[u8; 32]) -> String {
    let mut out = String::from("b3:");
    for byte in &digest[..6] {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

// --- building ---------------------------------------------------------------

/// What a build produced, and the facts a report needs that the artifact itself does not carry.
pub struct Built {
    pub artifact: Artifact,
    pub entry_name: Symbol,
    /// The start-up definitions shipped beside the entry point, in the order they were named.
    pub startup: Vec<Symbol>,
    /// Definition name to everything it reaches, restricted to the artifact.
    pub closure: BTreeMap<String, BTreeSet<String>>,
}

/// The transitive closure of the entry point **and of the run's start-up definitions**, and nothing
/// else.
pub fn build(
    loaded: &Loaded,
    entry: &DefInfo,
    startup: &[&DefInfo],
    sources: bool,
) -> Result<Built, Vec<Diagnostic>> {
    let (hashes, bodies) = hash_program_with_bodies(&loaded.program, &loaded.resolved)?;
    let Some(entry_hash) = hashes.defs.get(&entry.name).copied() else {
        return Err(vec![missing_entry(&entry.name)]);
    };
    let mut reachable = hashes.closure.get(&entry.name).cloned().unwrap_or_default();
    for root in startup {
        match hashes.closure.get(&root.name) {
            Some(closure) => reachable.extend(closure.iter().cloned()),
            None => return Err(vec![missing_entry(&root.name)]),
        }
    }

    let mut out = Artifact {
        frontend: *blake3::hash(ply_store::FRONTEND_VERSION.as_bytes()).as_bytes(),
        runtime: *blake3::hash(ply_store::RUNTIME_VERSION.as_bytes()).as_bytes(),
        body_encoding: ply_store::BODY_ENCODING,
        std: ply_std::digest(),
        entry: entry_hash,
        bodies: BTreeMap::new(),
        names: Vec::new(),
        sources: Vec::new(),
    };

    let mut absent: Vec<Symbol> = Vec::new();
    for name in &reachable {
        for hash in [hashes.defs.get(name), hashes.decls.get(name)]
            .into_iter()
            .flatten()
        {
            match bodies.get(*hash) {
                Some(body) => {
                    out.bodies.insert(*hash, body.clone());
                    out.names.push((name.to_string(), *hash));
                }
                None => absent.push(name.clone()),
            }
        }
    }
    if !absent.is_empty() {
        return Err(vec![no_body(&absent)]);
    }
    out.names.sort();
    out.names.dedup();

    if sources {
        out.sources = embedded_sources(loaded);
    }

    Ok(Built {
        artifact: out,
        entry_name: entry.name.clone(),
        startup: startup.iter().map(|d| d.name.clone()).collect(),
        closure: restricted_closure(&hashes, &reachable),
    })
}

/// Every project file, keyed by its path relative to the project root.
fn embedded_sources(loaded: &Loaded) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for file in loaded.sources.files() {
        if ply_std::is_pseudo_path(&file.path) {
            continue;
        }
        let relative = file.path.strip_prefix(&loaded.root).unwrap_or(&file.path);
        let key = relative
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        out.push((key, file.text.to_string()));
    }
    out.sort();
    out.dedup();
    out
}

fn restricted_closure(
    hashes: &HashOutput,
    reachable: &BTreeSet<Symbol>,
) -> BTreeMap<String, BTreeSet<String>> {
    reachable
        .iter()
        .map(|name| {
            let inner = hashes
                .closure
                .get(name)
                .map(|set| {
                    set.iter()
                        .filter(|n| reachable.contains(*n))
                        .map(|n| n.to_string())
                        .collect()
                })
                .unwrap_or_default();
            (name.to_string(), inner)
        })
        .collect()
}

// --- reading ----------------------------------------------------------------

struct Reader<'a> {
    bytes: &'a [u8],
    path: &'a Path,
}

impl<'a> Reader<'a> {
    fn slice(&self, at: usize, len: usize) -> Result<&'a [u8], Diagnostic> {
        at.checked_add(len)
            .and_then(|end| self.bytes.get(at..end))
            .ok_or_else(|| truncated(self.path, at, len, self.bytes.len()))
    }

    fn u32(&self, at: usize) -> Result<u32, Diagnostic> {
        Ok(u32::from_le_bytes(self.slice(at, 4)?.try_into().unwrap()))
    }

    fn u64(&self, at: usize) -> Result<u64, Diagnostic> {
        Ok(u64::from_le_bytes(self.slice(at, 8)?.try_into().unwrap()))
    }

    fn hash32(&self, at: usize) -> Result<[u8; 32], Diagnostic> {
        Ok(self.slice(at, 32)?.try_into().unwrap())
    }
}

/// What a target checks, and each check answers a different question.
pub fn decode(bytes: &[u8], path: &Path) -> Result<(Artifact, Vec<Diagnostic>), Diagnostic> {
    let r = Reader { bytes, path };
    if bytes.len() < HEADER_LEN {
        return Err(truncated(path, 0, HEADER_LEN, bytes.len()));
    }
    if &bytes[..8] != MAGIC {
        return Err(invalid(path, "this file is not a Ply program artifact")
            .note("`ply build` writes one; the first eight bytes are `PLYPROG1`"));
    }
    let format = r.u32(OFF_FORMAT)?;
    if format != ARTIFACT_FORMAT {
        return Err(version(
            path,
            format!(
                "the artifact is format {format} and this `ply` writes and reads format \
                 {ARTIFACT_FORMAT}"
            ),
        ));
    }

    let frontend = r.hash32(OFF_FRONTEND)?;
    let runtime = r.hash32(OFF_RUNTIME)?;
    let body_encoding = r.u32(OFF_BODY_ENC)?;
    let std = r.hash32(OFF_STD)?;
    let entry = DefHash(r.hash32(OFF_ENTRY)?);
    let stated = r.hash32(OFF_DIGEST)?;
    let count = r.u32(OFF_SECTIONS)? as usize;

    // Bound before it is multiplied: a section count read out of a corrupt header is otherwise one
    // allocation away from taking the process down.
    let table = HEADER_LEN.saturating_add(DESCRIPTOR_LEN.saturating_mul(count));
    if table > bytes.len() {
        return Err(truncated(path, HEADER_LEN, table - HEADER_LEN, bytes.len()));
    }

    let mut payloads: BTreeMap<u32, (u32, usize, usize)> = BTreeMap::new();
    for i in 0..count {
        let d = HEADER_LEN + DESCRIPTOR_LEN * i;
        let kind = r.u32(d)?;
        let records = r.u32(d + 4)?;
        let offset = r.u64(d + 8)? as usize;
        let len = r.u64(d + 16)? as usize;
        if offset < table || offset.checked_add(len).is_none_or(|end| end > bytes.len()) {
            return Err(invalid(
                path,
                format!(
                    "section {kind} claims bytes {offset}..{} of a {}-byte file",
                    offset.saturating_add(len),
                    bytes.len()
                ),
            ));
        }
        if payloads.insert(kind, (records, offset, len)).is_some() {
            return Err(invalid(path, format!("section {kind} appears twice")));
        }
    }

    check_versions(path, frontend, runtime, body_encoding)?;
    let mut warnings = Vec::new();
    if std != ply_std::digest() {
        warnings.push(stdlib_changed());
    }

    let mut out = Artifact {
        frontend,
        runtime,
        body_encoding,
        std,
        entry,
        bodies: BTreeMap::new(),
        names: Vec::new(),
        sources: Vec::new(),
    };

    let (records, offset, len) = *payloads
        .get(&KIND_BODIES)
        .ok_or_else(|| invalid(path, "the artifact carries no definitions"))?;
    let mut at = offset;
    let end = offset + len;
    for _ in 0..records {
        let hash = DefHash(r.hash32(at)?);
        let body_len = r.u32(at + 32)? as usize;
        let payload = r.slice(at + 36, body_len)?;
        let body =
            StoredBody::from_bytes(payload.to_vec()).ok_or_else(|| corrupt_body(path, hash, at))?;
        if !body.verify(hash) {
            return Err(corrupt_body(path, hash, at));
        }
        if out.bodies.insert(hash, body).is_some() {
            return Err(invalid(
                path,
                format!("the artifact carries `{}` twice", hash.short()),
            ));
        }
        at += 36 + body_len;
    }
    if at != end {
        return Err(invalid(
            path,
            format!(
                "the definition section has {} bytes nothing claims",
                end - at
            ),
        ));
    }

    let strings = match payloads.get(&KIND_STRINGS) {
        Some(&(_, offset, len)) => r.slice(offset, len)?,
        None => &[][..],
    };
    if let Some(&(records, offset, len)) = payloads.get(&KIND_NAMES) {
        if len != records as usize * 40 {
            return Err(invalid(path, "the namespace section is the wrong size"));
        }
        for i in 0..records as usize {
            let at = offset + i * 40;
            let name_off = r.u32(at)? as usize;
            let name_len = r.u32(at + 4)? as usize;
            let hash = DefHash(r.hash32(at + 8)?);
            let raw = strings
                .get(name_off..name_off + name_len)
                .ok_or_else(|| invalid(path, "a name lies outside the string section"))?;
            let name =
                std::str::from_utf8(raw).map_err(|_| invalid(path, "a name is not valid UTF-8"))?;
            out.names.push((name.to_string(), hash));
        }
    }

    if let Some(&(records, offset, len)) = payloads.get(&KIND_SOURCES) {
        let mut at = offset;
        let end = offset + len;
        for _ in 0..records {
            let path_len = r.u32(at)? as usize;
            let raw_path = r.slice(at + 4, path_len)?;
            let text_len = r.u32(at + 4 + path_len)? as usize;
            let raw_text = r.slice(at + 8 + path_len, text_len)?;
            let file = std::str::from_utf8(raw_path)
                .map_err(|_| invalid(path, "an embedded source path is not valid UTF-8"))?;
            let text = std::str::from_utf8(raw_text)
                .map_err(|_| invalid(path, "an embedded source file is not valid UTF-8"))?;
            out.sources.push((file.to_string(), text.to_string()));
            at += 8 + path_len + text_len;
        }
        if at != end {
            return Err(invalid(
                path,
                format!("the source section has {} bytes nothing claims", end - at),
            ));
        }
    }

    let computed = digest_of(bytes).ok_or_else(|| truncated(path, 0, OFF_SECTIONS, bytes.len()))?;
    if computed != stated {
        return Err(invalid(
            path,
            format!(
                "the artifact's digest is {} and its contents hash to {}",
                short(&stated),
                short(&computed)
            ),
        )
        .note("the file was altered or truncated after it was built; transfer it again"));
    }
    if !out.bodies.contains_key(&out.entry) {
        return Err(invalid(
            path,
            format!(
                "the entry point `{}` is not among the artifact's definitions",
                out.entry.short()
            ),
        ));
    }
    if out.entry_name().is_none() {
        return Err(invalid(
            path,
            format!(
                "the artifact's namespace names no definition `{}`",
                out.entry.short()
            ),
        ));
    }

    Ok((out, warnings))
}

fn check_versions(
    path: &Path,
    frontend: [u8; 32],
    runtime: [u8; 32],
    body_encoding: u32,
) -> Result<(), Diagnostic> {
    let mine_frontend = *blake3::hash(ply_store::FRONTEND_VERSION.as_bytes()).as_bytes();
    let mine_runtime = *blake3::hash(ply_store::RUNTIME_VERSION.as_bytes()).as_bytes();
    if body_encoding != ply_store::BODY_ENCODING {
        return Err(version(
            path,
            format!(
                "the artifact's definition bodies are encoding {body_encoding} and this `ply` \
                 reads encoding {}",
                ply_store::BODY_ENCODING
            ),
        ));
    }
    if frontend != mine_frontend {
        return Err(version(
            path,
            format!(
                "the artifact was built by a different front end; this `ply` is FRONTEND_VERSION \
                 {}",
                ply_store::FRONTEND_VERSION
            ),
        ));
    }
    if runtime != mine_runtime {
        return Err(version(
            path,
            format!(
                "the artifact was built for a different runtime; this `ply` is RUNTIME_VERSION {}",
                ply_store::RUNTIME_VERSION
            ),
        ));
    }
    Ok(())
}

pub fn read(path: &Path) -> Result<(Artifact, Vec<Diagnostic>), Diagnostic> {
    let bytes = std::fs::read(path).map_err(|e| {
        invalid(path, format!("could not read `{}`: {e}", path.display()))
            .note("name the `.plyx` file `ply build` wrote")
    })?;
    decode(&bytes, path)
}

// --- loading a program out of one -------------------------------------------

/// A program an artifact decoded into, and everything an evaluator needs.
pub struct Opened {
    pub sources: SourceMap,
    pub program: Program,
    pub resolved: Resolved,
    pub check: CheckOutput,
    /// The name the entry point answers to *in this program*.
    pub entry: Symbol,
    /// Whether spans point into real text.
    pub located: bool,
}

/// Turns an artifact into something runnable.
pub fn open(artifact: &Artifact, path: &Path) -> Result<Opened, Vec<Diagnostic>> {
    if artifact.has_sources() {
        return open_sources(artifact, path);
    }
    let mut set = BodySet::default();
    for (hash, body) in &artifact.bodies {
        set.insert(*hash, body.clone());
    }
    let mut namespace = Namespace::new();
    for (name, hash) in &artifact.names {
        namespace.entry(*hash).or_insert_with(|| Symbol::new(name));
    }
    // `reconstruct_named` is where the reference closure is checked: a body naming a hash the
    // artifact does not hold has no name the program could give it.
    let mut rebuilt = reconstruct_named(&set, &namespace).map_err(|diags| {
        vec![
            invalid(
                path,
                "the artifact's definitions do not form a whole program",
            )
            .note(first_note(&diags)),
        ]
    })?;
    // Mutable because `resolve` also fills defaults.
    let resolved = ply_syntax::resolve(&mut rebuilt.program).map_err(|diags| {
        vec![invalid(path, "the artifact's definitions do not resolve").note(first_note(&diags))]
    })?;
    let check = ply_core::check_program(&rebuilt.program, &resolved).map_err(|diags| {
        vec![invalid(path, "the artifact's definitions do not typecheck").note(first_note(&diags))]
    })?;
    let entry = rebuilt
        .name_of(artifact.entry)
        .cloned()
        .ok_or_else(|| vec![invalid(path, "the artifact's entry point was not rebuilt")])?;
    Ok(Opened {
        sources: SourceMap::new(),
        program: rebuilt.program,
        resolved,
        check,
        entry,
        located: false,
    })
}

fn open_sources(artifact: &Artifact, path: &Path) -> Result<Opened, Vec<Diagnostic>> {
    let mut sources = SourceMap::new();
    let mut inputs: Vec<(ply_span::SourceId, ModuleName, String)> = Vec::new();
    for (file, text) in &artifact.sources {
        let relative = PathBuf::from(file);
        let name = ModuleName::from_relative_path(&relative).map_err(|d| vec![d])?;
        let id = sources.add(&relative, text.clone());
        inputs.push((id, name, text.clone()));
    }

    // The shipped modules are not in the artifact — they are in this binary, and the header's
    // `ply_std::digest()` is what pins them — so they are pulled in here by the same rule the
    // driver uses, to a fixed point because a shipped module may import another.
    let mut program = parse(&inputs)?;
    loop {
        let mut added = false;
        let present: BTreeSet<Symbol> = program
            .modules
            .iter()
            .map(|m| m.name.as_symbol().clone())
            .collect();
        let mut wanted: BTreeSet<ModuleName> = BTreeSet::new();
        for module in &program.modules {
            for import in &module.imports {
                let name = import.module_name();
                if ply_std::is_std(&name) && !present.contains(name.as_symbol()) {
                    wanted.insert(name.clone());
                }
            }
        }
        for name in wanted {
            let Some(text) = ply_std::source(&name) else {
                continue;
            };
            let id = sources.add(ply_std::pseudo_path(&name), text);
            inputs.push((id, name, text.to_string()));
            added = true;
        }
        if !added {
            break;
        }
        program = parse(&inputs)?;
    }

    let diags = ply_derive::expand_program(&mut program);
    if !diags.is_empty() {
        return Err(diags);
    }
    let resolved = ply_syntax::resolve(&mut program)?;
    let check = ply_core::check_program(&program, &resolved)?;

    // The sources are believed only if they build the artifact they arrived in.
    let (hashes, bodies) = hash_program_with_bodies(&program, &resolved)?;
    let mut rebuilt: BTreeMap<DefHash, StoredBody> = BTreeMap::new();
    for (name, hash) in &artifact.names {
        let symbol = Symbol::new(name);
        let known =
            hashes.defs.get(&symbol) == Some(hash) || hashes.decls.get(&symbol) == Some(hash);
        match bodies.get(*hash) {
            Some(body) if known => {
                rebuilt.insert(*hash, body.clone());
            }
            _ => {
                return Err(vec![
                    invalid(
                        path,
                        format!("the embedded source does not define `{name}`"),
                    )
                    .note("the artifact's sources are not the sources it was built from"),
                ]);
            }
        }
    }
    if rebuilt != artifact.bodies {
        return Err(vec![
            invalid(
                path,
                "the embedded source does not rebuild the artifact's definitions",
            )
            .note("the artifact's sources are not the sources it was built from"),
        ]);
    }

    let entry = artifact
        .entry_name()
        .map(Symbol::new)
        .ok_or_else(|| vec![invalid(path, "the artifact names no entry point")])?;
    Ok(Opened {
        sources,
        program,
        resolved,
        check,
        entry,
        located: true,
    })
}

fn parse(inputs: &[(ply_span::SourceId, ModuleName, String)]) -> Result<Program, Vec<Diagnostic>> {
    ply_syntax::parse_program(
        inputs
            .iter()
            .map(|(id, name, text)| (*id, name.clone(), text.as_str())),
    )
}

fn first_note(diags: &[Diagnostic]) -> String {
    diags
        .first()
        .map(|d| d.message.clone())
        .unwrap_or_else(|| "no reason was given".to_string())
}

// --- the difference between two artifacts ------------------------------------

/// What is actually going out, in the language's own terms.
#[derive(Default, Debug)]
pub struct Diff {
    pub added: Vec<String>,
    pub changed: Vec<String>,
    pub dropped: Vec<String>,
    pub unchanged: usize,
    /// Definitions in the new artifact that reach a changed or added one.
    pub reached: Vec<String>,
}

pub fn diff(old: &Artifact, built: &Built) -> Diff {
    let new = &built.artifact;
    let before: BTreeMap<&str, BTreeSet<DefHash>> = group(&old.names);
    let after: BTreeMap<&str, BTreeSet<DefHash>> = group(&new.names);

    let mut out = Diff::default();
    for (name, hashes) in &after {
        match before.get(name) {
            None => out.added.push(name.to_string()),
            Some(was) if was != hashes => out.changed.push(name.to_string()),
            Some(_) => out.unchanged += 1,
        }
    }
    for name in before.keys() {
        if !after.contains_key(name) {
            out.dropped.push(name.to_string());
        }
    }

    let moved: BTreeSet<&String> = out.added.iter().chain(out.changed.iter()).collect();
    out.reached = built
        .closure
        .iter()
        .filter(|(_, reaches)| reaches.iter().any(|n| moved.contains(n)))
        .map(|(name, _)| name.clone())
        .collect();
    out
}

fn group(names: &[(String, DefHash)]) -> BTreeMap<&str, BTreeSet<DefHash>> {
    let mut out: BTreeMap<&str, BTreeSet<DefHash>> = BTreeMap::new();
    for (name, hash) in names {
        out.entry(name.as_str()).or_default().insert(*hash);
    }
    out
}

// --- running one -------------------------------------------------------------

/// `ply run FILE.plyx` — the target's side of a deploy.
pub fn run(args: &crate::cli::RunArgs, style: crate::style::Style) -> i32 {
    use crate::commands::common::{
        IND, diagnostic_json, diagnostics_json, emit_json, print_diagnostics, print_warnings,
        report_bind_error,
    };
    use crate::{EXIT_COMPILE_ERROR, EXIT_FAILED, EXIT_OK};

    let empty = SourceMap::new();
    let refuse = |diagnostics: &[Diagnostic]| -> i32 {
        if args.json {
            emit_json(&serde_json::json!({
                "command": "run",
                "ok": false,
                "exit_code": EXIT_COMPILE_ERROR,
                "artifact": args.path.display().to_string(),
                "diagnostics": diagnostics_json(diagnostics, &empty),
            }));
        } else {
            print_diagnostics(diagnostics, &empty, style);
        }
        EXIT_COMPILE_ERROR
    };

    let (artifact, warnings) = match read(&args.path) {
        Ok(pair) => pair,
        Err(diagnostic) => return refuse(std::slice::from_ref(&diagnostic)),
    };
    let opened = match open(&artifact, &args.path) {
        Ok(opened) => opened,
        Err(diagnostics) => return refuse(&diagnostics),
    };

    let db = match args.db.resolve(args.host) {
        Ok(db) => db,
        Err(diagnostics) => {
            return report_bind_error("run", &diagnostics, &opened.sources, args.json, style);
        }
    };
    let declared = opened
        .check
        .defs
        .get(&opened.entry)
        .map(|d| d.footprint.clone());
    // An artifact is configured exactly as a source tree is, which is the point: the thing that
    // differs between two deployments is the command line, not the program.
    let (configuration, config_warnings) = match crate::config::Configuration::open(
        &opened.program,
        &opened.resolved,
        &opened.check,
        args.host,
        &args.config,
    ) {
        Ok(resolved) => resolved,
        Err(diagnostics) => {
            return report_bind_error("run", &diagnostics, &opened.sources, args.json, style);
        }
    };
    if !args.json {
        print_diagnostics(&config_warnings, &opened.sources, style);
    }
    // A deployed artifact drains exactly as a source tree does.
    let shutdown = args
        .host
        .then(|| ply_host::signal::Shutdown::new(args.shutdown.bounds()));
    if let Some(shutdown) = &shutdown
        && let Err(diagnostic) = ply_host::signal::listen(shutdown)
    {
        return report_bind_error(
            "run",
            std::slice::from_ref(&diagnostic),
            &opened.sources,
            args.json,
            style,
        );
    }
    let hosts = match crate::hosts::Hosts::open_stopping(
        &opened.check,
        args.host,
        &args.tls.tls,
        &args.fs.fs,
        db,
        configuration,
        &args.trace,
        declared.as_ref(),
        shutdown.clone(),
    ) {
        Ok(hosts) => hosts,
        Err(diagnostics) => {
            return report_bind_error("run", &diagnostics, &opened.sources, args.json, style);
        }
    };
    if !args.json {
        print_warnings(&warnings, style);
        crate::commands::run::print_binding(&hosts, style);
        println!(
            "{IND}{}",
            style.dim(&format!(
                "program {} · {} definitions · {}",
                artifact.digest_short(),
                artifact.bodies.len(),
                if opened.located {
                    "sources embedded"
                } else {
                    "no sources: failures carry no line number"
                }
            ))
        );
        if let Some(shutdown) = &shutdown {
            eprintln!(
                "{IND}{}",
                style.dim(&format!(
                    "shutdown    signals {} · lead {}ms · drain {}ms · second signal exits 130/143",
                    shutdown
                        .signals()
                        .iter()
                        .map(|s| s.name())
                        .collect::<Vec<_>>()
                        .join(" "),
                    args.shutdown.drain_lead_ms,
                    args.shutdown.drain_ms,
                ))
            );
        }
    }

    let span = opened
        .check
        .defs
        .get(&opened.entry)
        .map(|d| d.span)
        .unwrap_or(Span::DUMMY);
    let plan = crate::simulation::run_plan(args.seed.as_ref());
    let answer = evaluate(&opened, span, &plan, &hosts, declared.as_ref());

    // ADR 0015 §4.4's pinned order, on the machine's own thread and never from a signal handler:
    // roll every open transaction back, close every open span, flush the sink, close the pool.
    let teardown =
        crate::commands::run::teardown(&hosts, shutdown.as_ref(), args.shutdown.drain_ms);
    let teardown_json =
        crate::commands::run::teardown_json(shutdown.as_ref(), teardown.as_ref(), &args.shutdown);
    if !args.json {
        for line in
            crate::commands::run::stop_lines(&hosts, shutdown.as_ref(), teardown.as_ref(), &answer)
        {
            eprintln!("{IND}{}", style.dim(&line));
        }
    }

    match answer {
        Ok(value) => {
            let rendered = value.to_string();
            if args.json {
                emit_json(&serde_json::json!({
                    "command": "run",
                    "ok": true,
                    "exit_code": EXIT_OK,
                    "artifact": args.path.display().to_string(),
                    "digest": artifact.digest_short(),
                    "entry": artifact.entry_name(),
                    "definitions": artifact.bodies.len(),
                    "located": opened.located,
                    "binding": hosts.label(),
                    "hosts": hosts.summary_json(),
                    "value": rendered,
                    "configuration": hosts.configuration().to_json(),
                    "shutdown": teardown_json,
                    "diagnostics": diagnostics_json(&warnings, &empty),
                }));
            } else {
                println!("{IND}{rendered}");
            }
            EXIT_OK
        }
        Err(diagnostic) => {
            // A drain that expired is the run's configuration at fault rather than the artifact's,
            // and the exit code is what carries it: a deployment that sees `3` knows it lost
            // requests.
            let code = if ply_eval::is_drain_incomplete(&diagnostic) {
                crate::EXIT_DRAIN_INCOMPLETE
            } else {
                EXIT_FAILED
            };
            if args.json {
                emit_json(&serde_json::json!({
                    "command": "run",
                    "ok": false,
                    "exit_code": code,
                    "artifact": args.path.display().to_string(),
                    "digest": artifact.digest_short(),
                    "entry": artifact.entry_name(),
                    "located": opened.located,
                    "binding": hosts.label(),
                    "configuration": hosts.configuration().to_json(),
                    "value": serde_json::Value::Null,
                    "shutdown": teardown_json,
                    "diagnostics": [diagnostic_json(&diagnostic, &opened.sources)],
                }));
            } else {
                print_diagnostics(std::slice::from_ref(&diagnostic), &opened.sources, style);
            }
            code
        }
    }
}

/// The same evaluation `ply run` gives source, over a decoded artifact.
fn evaluate(
    opened: &Opened,
    span: Span,
    plan: &ply_eval::Plan,
    hosts: &crate::hosts::Hosts,
    declared: Option<&ply_core::ty::Footprint>,
) -> Result<ply_eval::Value, Diagnostic> {
    use ply_eval::Machine;

    let name = opened.entry.as_str();
    let mut machine = Machine::new(&opened.program, &opened.resolved, &opened.check);
    machine.set_host_binding(hosts.binding());
    if let Some(runtime) = hosts.runtime() {
        machine.set_host_runtime(runtime);
    }
    if let Some(declared) = declared {
        machine.set_declared_footprint(declared.clone());
    }
    ply_test::sim::seed_run(&mut machine, &plan.seeds()[0], plan.steps);
    machine.call(name, Vec::new(), span)
}

// --- diagnostics -------------------------------------------------------------

/// An artifact has no source text, so there is no span to point at and inventing one would send a
/// reader to a file that has nothing to do with the failure.
fn invalid(path: &Path, message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(codes::ARTIFACT_INVALID, message.into())
        .primary(Span::DUMMY, format!("in `{}`", path.display()))
        .note("rebuild it with `ply build`, or transfer the file again")
}

fn version(path: &Path, message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(codes::ARTIFACT_VERSION, message.into())
        .primary(Span::DUMMY, format!("in `{}`", path.display()))
        .note("rebuild the artifact with this `ply`; re-transferring it will not help")
}

fn truncated(path: &Path, at: usize, wanted: usize, len: usize) -> Diagnostic {
    invalid(
        path,
        format!("the artifact is {len} bytes and wants {wanted} more at offset {at}"),
    )
}

fn corrupt_body(path: &Path, hash: DefHash, offset: usize) -> Diagnostic {
    invalid(
        path,
        format!(
            "the definition filed under `{}` at offset {offset} is not the definition that hash \
             names",
            hash.short()
        ),
    )
    .note("a body is a function of its hash, so this is corruption rather than a difference of opinion")
}

fn stdlib_changed() -> Diagnostic {
    Diagnostic::warning(
        codes::STDLIB_CHANGED,
        "the artifact was built against a different standard library",
    )
    .note(format!("this `ply` ships {}", ply_std::digest_short()))
    .note("a shipped definition is content-addressed like any other, so the digest differs over modules the program may never import")
}

fn missing_entry(name: &Symbol) -> Diagnostic {
    Diagnostic::error(
        codes::ARTIFACT_INVALID,
        format!("`{name}` has no definition hash, so nothing could be built from it"),
    )
    .primary(Span::DUMMY, "this entry point was not hashed")
}

fn no_body(absent: &[Symbol]) -> Diagnostic {
    let named: Vec<String> = absent.iter().take(8).map(|s| s.to_string()).collect();
    Diagnostic::error(
        codes::ARTIFACT_INVALID,
        format!(
            "{} of the entry point's definitions have no stored body",
            absent.len()
        ),
    )
    .primary(Span::DUMMY, "the closure is incomplete")
    .note(format!("missing: {}", named.join(", ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bodies that are not decodable definitions, deliberately: everything below is about the
    /// *container*, and `decode` checks a body against its key and nothing more.
    fn sample() -> Artifact {
        let mut bodies = BTreeMap::new();
        let mut names = Vec::new();
        let mut entry = DefHash([0; 32]);
        for (i, label) in ["m.main", "m.helper", "m.Colour"].iter().enumerate() {
            let body = StoredBody::from_bytes(vec![0u8, i as u8, 2, 3]).expect("a solo envelope");
            let hash = body.key().unwrap();
            if i == 0 {
                entry = hash;
            }
            bodies.insert(hash, body);
            names.push((label.to_string(), hash));
        }
        names.sort();
        Artifact {
            frontend: *blake3::hash(ply_store::FRONTEND_VERSION.as_bytes()).as_bytes(),
            runtime: *blake3::hash(ply_store::RUNTIME_VERSION.as_bytes()).as_bytes(),
            body_encoding: ply_store::BODY_ENCODING,
            std: ply_std::digest(),
            entry,
            bodies,
            names,
            sources: Vec::new(),
        }
    }

    #[test]
    fn a_round_trip_preserves_every_field() {
        let artifact = sample();
        let (back, warnings) = decode(&artifact.encode(), Path::new("t.plyx")).unwrap();
        assert_eq!(back, artifact);
        assert!(warnings.is_empty());
        assert_eq!(back.entry_name(), Some("m.main"));
    }

    #[test]
    fn sources_round_trip_and_are_believed_only_as_bytes() {
        let mut artifact = sample();
        artifact.sources = vec![
            ("a.ply".to_string(), "fn a() -> Int = 1\n".to_string()),
            ("sub/b.ply".to_string(), "fn b() -> Int = 2\n".to_string()),
        ];
        let (back, _) = decode(&artifact.encode(), Path::new("t.plyx")).unwrap();
        assert_eq!(back, artifact);
    }

    #[test]
    fn a_flipped_bit_in_a_body_names_that_definition_and_its_offset() {
        let artifact = sample();
        let mut bytes = artifact.encode();
        // The first body's payload, past the 32-byte key and the length.
        let at = HEADER_LEN + DESCRIPTOR_LEN * 3 + 36;
        bytes[at] ^= 0xff;
        let err = decode(&bytes, Path::new("t.plyx")).unwrap_err();
        assert_eq!(err.code, codes::ARTIFACT_INVALID);
        assert!(err.message.contains("offset"), "{}", err.message);
        let first = artifact.bodies.keys().next().unwrap();
        assert!(err.message.contains(&first.short()), "{}", err.message);
    }

    #[test]
    fn a_foreign_body_encoding_is_a_version_refusal_and_not_a_corruption_one() {
        let mut artifact = sample();
        artifact.body_encoding += 1;
        let err = decode(&artifact.encode(), Path::new("t.plyx")).unwrap_err();
        assert_eq!(err.code, codes::ARTIFACT_VERSION);
    }

    #[test]
    fn a_foreign_stdlib_is_a_warning_and_the_artifact_still_loads() {
        let mut artifact = sample();
        artifact.std = [9; 32];
        let (back, warnings) = decode(&artifact.encode(), Path::new("t.plyx")).unwrap();
        assert_eq!(back.std, [9; 32]);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, codes::STDLIB_CHANGED);
    }

    #[test]
    fn the_digest_covers_every_byte_after_it() {
        let artifact = sample();
        let mut bytes = artifact.encode();
        let before = digest_of(&bytes).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        assert_ne!(digest_of(&bytes).unwrap(), before);
    }

    #[test]
    fn embedding_sources_moves_the_digest() {
        let bare = sample();
        let mut with = sample();
        with.sources = vec![("m.ply".to_string(), "fn main() -> Int = 1\n".to_string())];
        assert_ne!(bare.digest(), with.digest());
        assert!(!bare.has_sources() && with.has_sources());
    }

    #[test]
    fn a_header_shorter_than_a_header_is_a_diagnostic_rather_than_a_panic() {
        let err = decode(&[0u8; 4], Path::new("t.plyx")).unwrap_err();
        assert_eq!(err.code, codes::ARTIFACT_INVALID);
    }

    #[test]
    fn a_wild_section_count_allocates_nothing() {
        let artifact = sample();
        let mut bytes = artifact.encode();
        bytes[OFF_SECTIONS..OFF_SECTIONS + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        let err = decode(&bytes, Path::new("t.plyx")).unwrap_err();
        assert_eq!(err.code, codes::ARTIFACT_INVALID);
    }

    #[test]
    fn every_prefix_of_an_artifact_is_refused_rather_than_believed() {
        let artifact = sample();
        let bytes = artifact.encode();
        for cut in 0..bytes.len() {
            let err = decode(&bytes[..cut], Path::new("t.plyx"))
                .expect_err("a truncated artifact must not decode");
            assert!(
                err.code == codes::ARTIFACT_INVALID || err.code == codes::ARTIFACT_VERSION,
                "cut {cut} produced {}",
                err.code
            );
        }
    }

    #[test]
    fn short_is_the_shape_a_ci_check_pins() {
        assert_eq!(short(&[0xab; 32]), "b3:abababababab");
        assert_eq!(short(&[0xab; 32]).len(), 15);
    }
}
