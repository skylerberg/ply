//! The deployable artifact: the transitive closure of one entry point, in the same bytes the
//! content-addressed store already holds.

use crate::load::Loaded;
use ply_hash::body::{BodySet, StoredBody};
use ply_hash::{DefHash, HashOutput};
use ply_span::{Diagnostic, Severity, SourceMap, Span, Symbol, codes};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::resolve::Resolved;
use ply_ty::{DefInfo, Front};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub const ARTIFACT_FORMAT: u32 = 4;

pub const EXTENSION: &str = "plyx";

const MAGIC: &[u8; 8] = b"PLYPROG1";

/// So a program digest is never confused with a definition hash or `ply hosts --digest`.
const DIGEST_DOMAIN: &[u8] = b"ply.program.2";

const FLAG_CLOSURE: u32 = 1;
const FLAG_UNIT: u32 = 2;

pub const HEADER_LEN: usize = 188;
pub const DESCRIPTOR_LEN: usize = 24;
const OFF_FORMAT: usize = 8;
const OFF_FLAGS: usize = 12;
const OFF_FRONTEND: usize = 16;
const OFF_RUNTIME: usize = 48;
const OFF_BODY_ENC: usize = 80;
const OFF_STD: usize = 84;
const OFF_ENTRY: usize = 116;
const OFF_DIGEST: usize = 148;
/// The digest covers every byte of the file from here on, plus the entry point.
pub const OFF_SECTIONS: usize = 180;
const OFF_RESERVED: usize = 184;

const KIND_BODIES: u32 = 1;
const KIND_NAMES: u32 = 2;
const KIND_STRINGS: u32 = 3;
const KIND_CLOSURE: u32 = 4;
const KIND_UNIT: u32 = 5;

/// The emitted C, compressed; left aside when its helper table is not a prefix of this runtime's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbeddedUnit {
    pub text: Vec<u8>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Artifact {
    pub frontend: [u8; 32],
    pub runtime: [u8; 32],
    pub body_encoding: u32,
    pub std: [u8; 32],
    pub entry: DefHash,
    /// Sorted by hash, which makes two builds byte-identical.
    pub bodies: BTreeMap<DefHash, StoredBody>,
    pub names: Vec<(String, DefHash)>,
    /// The closure printed back to source, per module path; shipped modules are the target's own.
    pub closure: Vec<(String, String)>,
    pub unit: Option<EmbeddedUnit>,
}

impl Artifact {
    pub fn has_unit(&self) -> bool {
        self.unit.is_some()
    }

    /// The program-wide name the entry point was built under.
    pub fn entry_name(&self) -> Option<&str> {
        self.names
            .iter()
            .find(|(_, hash)| *hash == self.entry)
            .map(|(name, _)| name.as_str())
    }

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

        // In record order, so the blob is a function of the record list alone.
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

        if !self.closure.is_empty() {
            let mut payload = Vec::new();
            for (path, text) in &self.closure {
                payload.extend_from_slice(&(path.len() as u32).to_le_bytes());
                payload.extend_from_slice(path.as_bytes());
                payload.extend_from_slice(&(text.len() as u32).to_le_bytes());
                payload.extend_from_slice(text.as_bytes());
            }
            sections.push((KIND_CLOSURE, self.closure.len() as u32, payload));
        }
        if let Some(unit) = &self.unit {
            fn put(payload: &mut Vec<u8>, bytes: &[u8]) {
                payload.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
                payload.extend_from_slice(bytes);
            }
            let mut payload = Vec::new();
            put(&mut payload, &unit.text);
            sections.push((KIND_UNIT, 1, payload));
        }

        let table = HEADER_LEN + DESCRIPTOR_LEN * sections.len();
        let mut out = vec![0u8; table];
        out[..8].copy_from_slice(MAGIC);
        out[OFF_FORMAT..OFF_FORMAT + 4].copy_from_slice(&ARTIFACT_FORMAT.to_le_bytes());
        let mut flags = if self.closure.is_empty() {
            0
        } else {
            FLAG_CLOSURE
        };
        if self.has_unit() {
            flags |= FLAG_UNIT;
        }
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

/// Read out of the bytes, so the writer and the reader digest the same thing by construction.
pub fn digest_of(bytes: &[u8]) -> Option<[u8; 32]> {
    if bytes.len() < OFF_SECTIONS {
        return None;
    }
    let mut hasher = blake3::Hasher::new();
    hasher.update(DIGEST_DOMAIN);
    hasher.update(&bytes[OFF_ENTRY..OFF_ENTRY + 32]);
    hasher.update(&bytes[OFF_SECTIONS..]);
    Some(*hasher.finalize().as_bytes())
}

/// `b3:` plus twelve hex characters, as `ply hosts --digest` and `ply std --digest` print.
pub fn short(digest: &[u8; 32]) -> String {
    let mut out = String::from("b3:");
    for byte in &digest[..6] {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

pub struct Built {
    pub artifact: Artifact,
    pub entry_name: Symbol,
    /// In the order they were named.
    pub startup: Vec<Symbol>,
    pub closure: BTreeMap<String, BTreeSet<String>>,
    pub warnings: Vec<Diagnostic>,
}

/// The transitive closure of the entry point and of the run's start-up definitions.
pub fn build(
    loaded: &Loaded,
    entry: &DefInfo,
    startup: &[&DefInfo],
) -> Result<Built, Vec<Diagnostic>> {
    let front = &loaded.front;
    let hashes = &front.hashes;
    let bodies = ply_hash::body::of_front(front);
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
        closure: Vec::new(),
        unit: None,
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

    out.closure = closure_texts(&out)?;
    // Reopened as a target opens it, so an artifact that builds is one that opens.
    let opened = reopen(&out).map_err(|diags| vec![unreopened(&diags)])?;
    let names: Vec<&str> = out.names.iter().map(|(n, _)| n.as_str()).collect();
    let (unit, warnings) = embedded_unit(&opened, &names);
    out.unit = unit;

    Ok(Built {
        artifact: out,
        entry_name: entry.name.clone(),
        startup: startup.iter().map(|d| d.name.clone()).collect(),
        closure: restricted_closure(hashes, &reachable),
        warnings,
    })
}

fn closure_texts(artifact: &Artifact) -> Result<Vec<(String, String)>, Vec<Diagnostic>> {
    let mut bodies = BodySet::default();
    for (hash, body) in &artifact.bodies {
        bodies.insert(*hash, body.clone());
    }
    let names: Vec<(Symbol, DefHash)> = artifact
        .names
        .iter()
        .map(|(name, hash)| (Symbol::new(name), *hash))
        .collect();
    let program = ply_hash::body::reconstruct_exact(&bodies, &names, ply_std::is_std)?;
    Ok(program
        .modules
        .iter()
        .map(|module| {
            let path = module.name.segments().collect::<Vec<_>>().join("/");
            (format!("{path}.ply"), ply_syntax::print::module(module))
        })
        .collect())
}

/// Embedded so `ply run` need not emit and compile C at every run; a failed production is reported.
fn embedded_unit(opened: &Opened, names: &[&str]) -> (Option<EmbeddedUnit>, Vec<Diagnostic>) {
    ply_codegen::c::producer::ensure_default();
    // Definitions only: no emitter is offered effect or resource declarations.
    let names: Vec<&str> = names
        .iter()
        .copied()
        .filter(|name| opened.front.check.defs.contains_key(&Symbol::new(name)))
        .collect();
    let texts = crate::commands::common::module_texts(&opened.front.check, &opened.sources);
    let produced = ply_codegen::Unit::over_front(&opened.program, &opened.front, texts)
        .and_then(|unit| unit.produce(&names))
        .and_then(|produced| {
            let text = ply_codegen::c::bundle::pack(&produced.text)?;
            Ok((produced, text))
        });
    match produced {
        Ok((produced, text)) => {
            let mut warnings = Vec::new();
            if !produced.refused.is_empty() {
                let listed: Vec<String> = produced
                    .refused
                    .iter()
                    .map(|r| format!("`{}` ({})", r.function, r.construct))
                    .collect();
                warnings.push(
                    Diagnostic::warning(
                        codes::BACKEND_UNAVAILABLE,
                        format!(
                            "the emitter refused {} of the artifact's definitions, which will not run from it: {}",
                            listed.len(),
                            listed.join(", ")
                        ),
                    )
                    .note("a refused definition is entered from nothing at run time; make it one the emitter compiles"),
                );
            }
            (Some(EmbeddedUnit { text }), warnings)
        }
        Err(e) => (
            None,
            vec![
                Diagnostic::warning(
                    codes::BACKEND_UNAVAILABLE,
                    format!("no compiled unit could be produced for the artifact: {e:#}"),
                )
                .note("the artifact's bodies are printed back to source and compiled at each run instead"),
            ],
        ),
    }
}

fn stale_unit() -> Diagnostic {
    Diagnostic::warning(
        codes::ARTIFACT_VERSION,
        "the artifact's compiled unit was built for another runtime and is left aside",
    )
    .note("the artifact's bodies are printed back to source and compiled at this run instead")
    .note("rebuild the artifact with this `ply` to carry a unit it can enter")
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

    // Bound before multiplying: a corrupt header's section count could otherwise abort the process.
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
        closure: Vec::new(),
        unit: None,
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

    if let Some(&(records, offset, len)) = payloads.get(&KIND_CLOSURE) {
        let mut at = offset;
        let end = offset + len;
        for _ in 0..records {
            let path_len = r.u32(at)? as usize;
            let raw_path = r.slice(at + 4, path_len)?;
            let text_len = r.u32(at + 4 + path_len)? as usize;
            let raw_text = r.slice(at + 8 + path_len, text_len)?;
            let file = std::str::from_utf8(raw_path)
                .map_err(|_| invalid(path, "a module path in the closure is not valid UTF-8"))?;
            let text = std::str::from_utf8(raw_text)
                .map_err(|_| invalid(path, "a module in the closure is not valid UTF-8"))?;
            out.closure.push((file.to_string(), text.to_string()));
            at += 8 + path_len + text_len;
        }
        if at != end {
            return Err(invalid(
                path,
                format!("the closure section has {} bytes nothing claims", end - at),
            ));
        }
    }

    if let Some(&(_, offset, len)) = payloads.get(&KIND_UNIT) {
        let end = offset + len;
        let mut at = offset;
        let text_len = r.u32(at)? as usize;
        let text = r.slice(at + 4, text_len)?.to_vec();
        at += 4 + text_len;
        if at != end {
            return Err(invalid(
                path,
                format!("the unit section has {} bytes nothing claims", end - at),
            ));
        }
        out.unit = Some(EmbeddedUnit { text });
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

pub struct Opened {
    pub sources: SourceMap,
    pub program: Program,
    pub resolved: Resolved,
    pub front: Front,
    /// The name the entry point answers to in this program.
    pub entry: Symbol,
}

/// Believed only if the closure is exactly the definitions the artifact names.
pub fn open(artifact: &Artifact, path: &Path) -> Result<Opened, Vec<Diagnostic>> {
    reopen(artifact).map_err(|diags| {
        if diags
            .first()
            .is_some_and(|d| d.code == codes::INTERNAL_ERROR)
        {
            return diags;
        }
        vec![
            invalid(
                path,
                "the artifact's closure does not open as the program it names",
            )
            .note(first_of(&diags)),
        ]
    })
}

fn ask_the_port(
    inputs: &[(ply_span::SourceId, ModuleName, String)],
) -> Result<Front, Vec<Diagnostic>> {
    ply_codegen::c::producer::ensure_default();
    let sources: Vec<(String, String)> = inputs
        .iter()
        .map(|(_, name, text)| (name.to_string(), text.clone()))
        .collect();
    let ids: Vec<ply_span::SourceId> = inputs.iter().map(|(id, _, _)| *id).collect();
    let front = ply_codegen::c::producer::front(&sources, &ids).map_err(|e| {
        vec![
            Diagnostic::error(
                codes::INTERNAL_ERROR,
                format!("the front end could not answer for this program: {e:#}"),
            )
            .note("this is Ply's fault: the compiler's own front end is what failed here"),
        ]
    })?;
    let errors: Vec<Diagnostic> = front
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .cloned()
        .collect();
    if errors.is_empty() {
        Ok(front)
    } else {
        Err(errors)
    }
}

fn reopen(artifact: &Artifact) -> Result<Opened, Vec<Diagnostic>> {
    let mut sources = SourceMap::new();
    let mut inputs: Vec<(ply_span::SourceId, ModuleName, String)> = Vec::new();
    for (file, text) in &artifact.closure {
        let relative = PathBuf::from(file);
        let name = ModuleName::from_relative_path(&relative).map_err(|d| vec![d])?;
        if ply_std::is_std(&name) {
            return Err(vec![unfaithful(format!(
                "the closure carries `{file}`, a module this `ply` ships"
            ))]);
        }
        let id = sources.add(&relative, text.clone());
        inputs.push((id, name, text.clone()));
    }

    // Shipped modules live in this binary, pinned by the header's `ply_std::digest()`.
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

    let front = ask_the_port(&inputs)?;

    let hashes = &front.hashes;
    let bodies = ply_hash::body::of_front(&front);
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
                return Err(vec![unfaithful(format!(
                    "the closure does not define `{name}` as the artifact does"
                ))]);
            }
        }
    }
    if rebuilt != artifact.bodies {
        return Err(vec![unfaithful(
            "the closure does not rebuild the artifact's definitions".to_string(),
        )]);
    }
    let named: BTreeSet<&str> = artifact
        .names
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    if let Some(extra) = hashes
        .defs
        .keys()
        .chain(hashes.decls.keys())
        .find(|name| !ply_std::is_reserved(name.as_str()) && !named.contains(name.as_str()))
    {
        return Err(vec![unfaithful(format!(
            "the closure declares `{extra}`, which the artifact does not name"
        ))]);
    }

    let entry = artifact
        .entry_name()
        .map(Symbol::new)
        .ok_or_else(|| vec![unfaithful("the artifact names no entry point".to_string())])?;
    Ok(Opened {
        sources,
        program,
        resolved,
        front,
        entry,
    })
}

fn parse(inputs: &[(ply_span::SourceId, ModuleName, String)]) -> Result<Program, Vec<Diagnostic>> {
    ply_syntax::parse_program(
        inputs
            .iter()
            .map(|(id, name, text)| (*id, name.clone(), text.as_str())),
    )
}

#[derive(Default, Debug)]
pub struct Diff {
    pub added: Vec<String>,
    pub changed: Vec<String>,
    pub dropped: Vec<String>,
    pub unchanged: usize,
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

pub fn run(args: &crate::cli::RunArgs, style: crate::style::Style) -> i32 {
    use crate::commands::common::{
        IND, diagnostic_json, diagnostics_json, emit_json, print_diagnostics, print_warnings,
        report_bind_error,
    };
    use crate::{EXIT_COMPILE_ERROR, EXIT_FAILED, EXIT_OK};

    // A closure's positions are in text printed at build, which no reader wrote.
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

    let (artifact, mut warnings) = match read(&args.path) {
        Ok(pair) => pair,
        Err(diagnostic) => return refuse(std::slice::from_ref(&diagnostic)),
    };
    let opened = match open(&artifact, &args.path) {
        Ok(opened) => opened,
        Err(diagnostics) => return refuse(&diagnostics),
    };
    // The object is cached, so `evaluate` loading it again costs nothing. A broken, rather than
    // foreign, unit is passed on and refused loudly there.
    let unit = match &artifact.unit {
        Some(unit) => {
            let served = ply_codegen::c::bundle::unpack(&unit.text)
                .and_then(|text| ply_codegen::c::served(&text, "artifact"));
            match served {
                Err(e) if e.downcast_ref::<ply_codegen::c::Unserved>().is_some() => {
                    warnings.push(stale_unit());
                    None
                }
                _ => Some(unit),
            }
        }
        None => None,
    };

    let db = match args.db.resolve(args.host) {
        Ok(db) => db,
        Err(diagnostics) => {
            return report_bind_error("run", &diagnostics, &empty, args.json, style);
        }
    };
    let declared = opened
        .front
        .check
        .defs
        .get(&opened.entry)
        .map(|d| d.footprint.clone());
    // Before the configuration: its schema is entered on this unit.
    let tier = tier(&opened, args.backend.as_ref(), unit);
    let constant = |name: &str| match &tier {
        Ok(tier) => crate::commands::common::enter_constant(
            tier.as_ref().map(|(provider, _)| *provider),
            name,
        ),
        Err(diagnostic) => Err(diagnostic.clone()),
    };
    let (configuration, config_warnings) = match crate::config::Configuration::open(
        &opened.front.check,
        args.host,
        &args.config,
        &constant,
    ) {
        Ok(resolved) => resolved,
        Err(diagnostics) => {
            return report_bind_error("run", &diagnostics, &empty, args.json, style);
        }
    };
    if !args.json {
        print_diagnostics(&config_warnings, &empty, style);
    }
    let shutdown = args
        .host
        .then(|| ply_host::signal::Shutdown::new(args.shutdown.bounds()));
    if let Some(shutdown) = &shutdown
        && let Err(diagnostic) = ply_host::signal::listen(shutdown)
    {
        return report_bind_error(
            "run",
            std::slice::from_ref(&diagnostic),
            &empty,
            args.json,
            style,
        );
    }
    let hosts = match crate::hosts::Hosts::open_stopping(
        &opened.front.check,
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
            return report_bind_error("run", &diagnostics, &empty, args.json, style);
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
                if unit.is_some() {
                    "compiled unit embedded"
                } else {
                    "no compiled unit: compiled from its bodies at each run"
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
        .front
        .check
        .defs
        .get(&opened.entry)
        .map(|d| d.span)
        .unwrap_or(Span::DUMMY);
    let plan = crate::simulation::run_plan(args.seed.as_ref());
    let answer =
        tier.and_then(|tier| evaluate(&opened, span, &plan, &hosts, declared.as_ref(), tier));

    // On the machine's own thread, never from a signal handler.
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
            // An expired drain is the configuration's fault; exit `3` says requests were lost.
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
                    "binding": hosts.label(),
                    "configuration": hosts.configuration().to_json(),
                    "value": serde_json::Value::Null,
                    "shutdown": teardown_json,
                    "diagnostics": [diagnostic_json(&diagnostic, &empty)],
                }));
            } else {
                print_diagnostics(std::slice::from_ref(&diagnostic), &empty, style);
            }
            code
        }
    }
}

/// The unit the artifact runs on: its embedded one as built, else one compiled from its bodies.
fn tier(
    opened: &Opened,
    backend: Option<&String>,
    unit: Option<&EmbeddedUnit>,
) -> Result<Option<(&'static dyn ply_eval::Provider, ply_eval::BackendSpec)>, Diagnostic> {
    let Some(spec) = crate::commands::common::backend_spec(backend)? else {
        return Ok(None);
    };
    // Entered as built: no producer is asked.
    if let Some(unit) = unit {
        let unit_error = |e: &dyn std::fmt::Display| {
            Diagnostic::error(
                codes::ARTIFACT_INVALID,
                format!("the artifact's compiled unit could not be entered: {e:#}"),
            )
        };
        let text = ply_codegen::c::bundle::unpack(&unit.text).map_err(|e| unit_error(&e))?;
        let provider: &'static dyn ply_eval::Provider = ply_codegen::Unit::embedded(
            &opened.program,
            &opened.resolved,
            &opened.front.check,
            text,
        )
        .map_err(|e| unit_error(&e))?;
        return Ok(Some((provider, spec)));
    }
    let texts = crate::commands::common::module_texts(&opened.front.check, &opened.sources);
    let provider =
        crate::commands::common::build_backend_over(&spec, &opened.program, &opened.front, texts)?;
    Ok(Some((provider, spec)))
}

fn evaluate(
    opened: &Opened,
    span: Span,
    plan: &ply_eval::Plan,
    hosts: &crate::hosts::Hosts,
    declared: Option<&ply_ty::ty::Footprint>,
    tier: Option<(&'static dyn ply_eval::Provider, ply_eval::BackendSpec)>,
) -> Result<ply_eval::Value, Diagnostic> {
    let mut machine =
        ply_eval::Machine::new(&opened.program, &opened.resolved, &opened.front.check);
    if let Some((provider, spec)) = tier {
        machine.set_compiled(provider.attach(&spec));
    }
    machine.set_host_binding(hosts.binding());
    if let Some(runtime) = hosts.runtime() {
        machine.set_host_runtime(runtime);
    }
    if let Some(declared) = declared {
        machine.set_declared_footprint(declared.clone());
    }
    ply_test::sim::seed_run(&mut machine, &plan.seeds()[0], plan.steps);
    machine.call(opened.entry.as_str(), Vec::new(), span)
}

/// No span: a container failure is about the file, not a point in the program.
fn invalid(path: &Path, message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(codes::ARTIFACT_INVALID, message.into())
        .primary(Span::DUMMY, format!("in `{}`", path.display()))
        .note("rebuild it with `ply build`, or transfer the file again")
}

fn unfaithful(message: String) -> Diagnostic {
    Diagnostic::error(codes::ARTIFACT_INVALID, message)
}

fn first_of(diags: &[Diagnostic]) -> String {
    diags.first().map_or_else(
        || "no reason was given".to_string(),
        |d| format!("{}: {}", d.code, d.message),
    )
}

fn unreopened(diags: &[Diagnostic]) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        "the closure printed back to source does not open as the program it was printed from",
    )
    .note(first_of(diags))
    .note("this is Ply's fault, not the program's, and nothing was built")
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
