//! The deployable artifact: the transitive closure of one entry point, in the same bytes the
//! content-addressed store already holds.

use crate::load::Loaded;
use ply_eval::{Fields, Value};
use ply_span::frames::Cursor;
use ply_span::{Diagnostic, Severity, SourceMap, Span, Symbol, codes};
use ply_store::body::StoredBody;
use ply_ty::ModuleName;
use ply_ty::{DefHash, HashOutput};
use ply_ty::{DefInfo, Front};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const EXTENSION: &str = "plyx";

/// The container is `crates/ply-compiler/ply/plyx.ply`: the magic, every header offset, the
/// section table and what a digest covers are written there and nowhere else.
const ENCODE: &str = "plyx.encode";
const DECODE: &str = "plyx.decode_dump";
const PLAN: &str = "plyx.plan_dump";
const FORMAT: &str = "plyx.format";

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

    /// All zeroes when the container could not be written, as an unwritable artifact has no
    /// digest to print or to key a cache on.
    pub fn digest(&self) -> [u8; 32] {
        self.encode()
            .ok()
            .and_then(|bytes| digest_of(&bytes))
            .unwrap_or([0; 32])
    }

    pub fn digest_short(&self) -> String {
        short(&self.digest())
    }

    /// The container `plyx.ply` places these sections in, with its digest written into the field
    /// no range of the digest covers.
    pub fn encode(&self) -> Result<Vec<u8>, Diagnostic> {
        let head = record(vec![
            ("frontend", Value::bytes(self.frontend)),
            ("runtime", Value::bytes(self.runtime)),
            ("body_encoding", Value::Int(i64::from(self.body_encoding))),
            ("stdlib", Value::bytes(self.std)),
            ("entry", Value::bytes(self.entry.0)),
        ]);
        let sections = Value::list(
            self.sections()
                .into_iter()
                .map(|(name, count, payload)| {
                    record(vec![
                        ("name", Value::bytes(name.as_bytes())),
                        ("count", Value::Int(i64::from(count))),
                        ("payload", Value::bytes(payload)),
                    ])
                })
                .collect(),
        );
        let answered = answer(ENCODE, &[head, sections])?;
        let Value::Bytes(written) = &answered else {
            return Err(container_failed(format!(
                "`{ENCODE}` answered something that is not a byte string"
            )));
        };
        seal(written.to_vec())
    }

    /// Each section's payload, in the order they are written. The records inside a payload are
    /// the writer's; `plyx.ply` places the payloads and hands them back.
    pub(crate) fn sections(&self) -> Vec<(&'static str, u32, Vec<u8>)> {
        let mut sections: Vec<(&'static str, u32, Vec<u8>)> = Vec::with_capacity(5);

        let mut bodies = Vec::new();
        for (hash, body) in &self.bodies {
            bodies.extend_from_slice(&hash.0);
            bodies.extend_from_slice(&(body.len() as u32).to_le_bytes());
            bodies.extend_from_slice(body.as_bytes());
        }
        sections.push(("bodies", self.bodies.len() as u32, bodies));

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
        sections.push(("names", self.names.len() as u32, names));
        sections.push(("strings", strings.len() as u32, strings));

        if !self.closure.is_empty() {
            let mut payload = Vec::new();
            for (path, text) in &self.closure {
                payload.extend_from_slice(&(path.len() as u32).to_le_bytes());
                payload.extend_from_slice(path.as_bytes());
                payload.extend_from_slice(&(text.len() as u32).to_le_bytes());
                payload.extend_from_slice(text.as_bytes());
            }
            sections.push(("closure", self.closure.len() as u32, payload));
        }
        if let Some(unit) = &self.unit {
            let mut payload = Vec::new();
            payload.extend_from_slice(&(unit.text.len() as u32).to_le_bytes());
            payload.extend_from_slice(&unit.text);
            sections.push(("unit", 1, payload));
        }
        sections
    }
}

/// A container `plyx.ply` laid out, with its digest written into the field no range of the digest
/// covers.
pub fn seal(mut out: Vec<u8>) -> Result<Vec<u8>, Diagnostic> {
    let Some(plan) = plan(out.len())? else {
        return Err(container_failed(format!(
            "a container of {} bytes is no container at all",
            out.len()
        )));
    };
    let digest = plan.over(&out).ok_or_else(|| {
        container_failed("the digest plan reaches past the container it is for".to_string())
    })?;
    let field = plan
        .at
        .checked_add(32)
        .and_then(|end| out.get_mut(plan.at..end))
        .ok_or_else(|| {
            container_failed("the digest plan writes past the container it is for".to_string())
        })?;
    field.copy_from_slice(&digest);
    Ok(out)
}

/// The container format this `ply` writes and reads.
pub fn format() -> Result<u32, Diagnostic> {
    match answer(FORMAT, &[])? {
        Value::Int(n) if (0..=i64::from(u32::MAX)).contains(&n) => Ok(n as u32),
        other => Err(container_failed(format!(
            "`{FORMAT}` answered a {} rather than a format number",
            other.type_name()
        ))),
    }
}

/// Read out of the bytes, so the writer and the reader digest the same thing by construction.
pub fn digest_of(bytes: &[u8]) -> Option<[u8; 32]> {
    plan(bytes.len()).ok().flatten()?.over(bytes)
}

/// What a program digest is taken over, as `plyx.ply` states it. The hash itself is the host's:
/// a digest covers the whole artifact, and Ply's own BLAKE3 runs at a few megabytes a second.
struct Plan {
    domain: Vec<u8>,
    /// Where the digest is written, which no range covers.
    at: usize,
    covers: Vec<(usize, usize)>,
}

impl Plan {
    fn over(&self, bytes: &[u8]) -> Option<[u8; 32]> {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&self.domain);
        for (at, len) in &self.covers {
            hasher.update(bytes.get(*at..at.checked_add(*len)?)?);
        }
        Some(*hasher.finalize().as_bytes())
    }
}

/// `None` when a file that long is too short to carry a digest at all.
fn plan(len: usize) -> Result<Option<Plan>, Diagnostic> {
    let answered = answer(PLAN, &[Value::Int(len as i64)])?;
    let Value::Bytes(dump) = &answered else {
        return Err(container_failed(format!(
            "`{PLAN}` answered something that is not a byte string"
        )));
    };
    let unreadable = |e: String| container_failed(format!("`{PLAN}`'s answer does not read: {e}"));
    let mut frames = Cursor::new(dump, "frame");
    let (words, payload) = frames.unit().map_err(unreadable)?;
    match words[..] {
        ["refused", _] => return Ok(None),
        ["plan", _] => {}
        _ => return Err(unreadable(format!("a `{}` frame", words.join(" ")))),
    }
    // Not defaulted: a plan missing its field would write the digest over the magic.
    let mut domain = None;
    let mut at = None;
    let mut covers = Vec::new();
    let mut fields = Cursor::new(payload, "field");
    while !fields.done() {
        let (key, body) = fields.unit().map_err(unreadable)?;
        match key[..] {
            ["domain"] => domain = Some(body.to_vec()),
            ["at"] => at = Some(number(body).map_err(unreadable)?),
            ["covers"] => {
                let text = std::str::from_utf8(body).map_err(|e| unreadable(e.to_string()))?;
                let Some((start, len)) = text.split_once(' ') else {
                    return Err(unreadable(format!("a range spelled `{text}`")));
                };
                let read = |what: &str, n: &str| {
                    n.parse::<usize>()
                        .map_err(|_| unreadable(format!("{what} `{n}`")))
                };
                covers.push((read("an offset", start)?, read("a length", len)?));
            }
            _ => return Err(unreadable(format!("a `{}` field", key.join(" ")))),
        }
    }
    match (domain, at) {
        (Some(domain), Some(at)) => Ok(Some(Plan { domain, at, covers })),
        _ => Err(unreadable(
            "a plan with no domain or no digest field".to_string(),
        )),
    }
}

/// One section as the container placed it; the records inside its payload are this file's to read.
struct Placed {
    name: String,
    count: usize,
    at: usize,
    len: usize,
}

/// A header read back, and where each of its sections lies.
struct Container {
    frontend: [u8; 32],
    runtime: [u8; 32],
    body_encoding: u32,
    std: [u8; 32],
    entry: DefHash,
    digest: [u8; 32],
    sections: Vec<Placed>,
}

impl Container {
    fn section(&self, name: &str) -> Option<&Placed> {
        self.sections.iter().find(|s| s.name == name)
    }
}

fn container(bytes: &[u8], path: &Path) -> Result<Container, Diagnostic> {
    let answered = answer(DECODE, &[Value::bytes(bytes)])?;
    let Value::Bytes(dump) = &answered else {
        return Err(container_failed(format!(
            "`{DECODE}` answered something that is not a byte string"
        )));
    };
    let unreadable =
        |e: String| container_failed(format!("`{DECODE}`'s answer does not read: {e}"));
    let mut frames = Cursor::new(dump, "frame");
    let (words, payload) = frames.unit().map_err(unreadable)?;
    match words[..] {
        ["refused", _] => return Err(refused(path, payload, false)?),
        ["stale", _] => return Err(refused(path, payload, true)?),
        ["head", _] => {}
        _ => return Err(unreadable(format!("a `{}` frame", words.join(" ")))),
    }

    let mut out = Container {
        frontend: [0; 32],
        runtime: [0; 32],
        body_encoding: 0,
        std: [0; 32],
        entry: DefHash([0; 32]),
        digest: [0; 32],
        sections: Vec::new(),
    };
    let mut fields = Cursor::new(payload, "field");
    while !fields.done() {
        let (key, body) = fields.unit().map_err(unreadable)?;
        let hash = |what: &str| {
            <[u8; 32]>::try_from(body)
                .map_err(|_| unreadable(format!("a `{what}` of {} bytes", body.len())))
        };
        match key[..] {
            ["frontend"] => out.frontend = hash("frontend")?,
            ["runtime"] => out.runtime = hash("runtime")?,
            ["stdlib"] => out.std = hash("stdlib")?,
            ["entry"] => out.entry = DefHash(hash("entry")?),
            ["digest"] => out.digest = hash("digest")?,
            ["body_encoding"] => out.body_encoding = number(body).map_err(unreadable)? as u32,
            _ => return Err(unreadable(format!("a `{}` field", key.join(" ")))),
        }
    }

    while !frames.done() {
        let (words, payload) = frames.unit().map_err(unreadable)?;
        if !matches!(words[..], ["section", _]) {
            return Err(unreadable(format!("a `{}` frame", words.join(" "))));
        }
        let mut placed = Placed {
            name: String::new(),
            count: 0,
            at: 0,
            len: 0,
        };
        let mut fields = Cursor::new(payload, "field");
        while !fields.done() {
            let (key, body) = fields.unit().map_err(unreadable)?;
            match key[..] {
                ["name"] => placed.name = String::from_utf8_lossy(body).into_owned(),
                ["count"] => placed.count = number(body).map_err(unreadable)?,
                ["at"] => placed.at = number(body).map_err(unreadable)?,
                ["len"] => placed.len = number(body).map_err(unreadable)?,
                _ => return Err(unreadable(format!("a `{}` field", key.join(" ")))),
            }
        }
        out.sections.push(placed);
    }
    Ok(out)
}

/// The container's own refusal, as the diagnostic the reader would have raised: a `stale` one is
/// a file no transfer will mend, and every other is a file that arrived damaged.
fn refused(path: &Path, payload: &[u8], stale: bool) -> Result<Diagnostic, Diagnostic> {
    let unreadable =
        |e: String| container_failed(format!("`{DECODE}`'s refusal does not read: {e}"));
    let mut fields = Cursor::new(payload, "field");
    let mut message = String::new();
    let mut notes: Vec<String> = Vec::new();
    while !fields.done() {
        let (key, body) = fields.unit().map_err(unreadable)?;
        let text = String::from_utf8_lossy(body).into_owned();
        match key[..] {
            ["message"] => message = text,
            ["note"] => notes.push(text),
            _ => return Err(unreadable(format!("a `{}` field", key.join(" ")))),
        }
    }
    let base = if stale {
        version(path, message)
    } else {
        invalid(path, message)
    };
    Ok(notes.into_iter().fold(base, |d, note| d.note(note)))
}

fn number(body: &[u8]) -> Result<usize, String> {
    std::str::from_utf8(body)
        .ok()
        .and_then(|text| text.parse().ok())
        .ok_or_else(|| format!("a number spelled `{}`", String::from_utf8_lossy(body)))
}

// `Value::Record` holds an `Arc`, and `Fields` is not `Send`; every construction site says so.
#[allow(clippy::arc_with_non_send_sync)]
fn record(fields: Vec<(&str, Value)>) -> Value {
    Value::Record(Arc::new(Fields::from_unsorted(
        fields
            .into_iter()
            .map(|(name, value)| (Symbol::new(name), value))
            .collect(),
    )))
}

fn answer(entry: &str, args: &[Value]) -> Result<Value, Diagnostic> {
    ply_codegen::c::producer::ensure_default();
    ply_codegen::c::producer::call(entry, args).map_err(|e| container_failed(format!("{e:#}")))
}

#[cold]
fn container_failed(why: String) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("the `.plyx` container could not be read or written: {why}"),
    )
    .primary(Span::DUMMY, "no artifact was written or opened")
    .note("this is Ply's fault: the compiler's own `plyx.ply` is what failed here")
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
    /// What the emitter refused, as `(definition, the construct that refused it)`, in the order
    /// the fixpoint dropped them: the first are the causes, the rest what those causes carried.
    pub refused: Vec<(String, String)>,
    /// Whether the embedded unit holds a body for the entry point. Without one nothing enters it,
    /// so a caller that can only run from the unit has to refuse the artifact rather than land it.
    pub entry_compiled: bool,
    pub warnings: Vec<Diagnostic>,
}

/// What the emitter made of an artifact's definitions.
struct Emission {
    unit: Option<EmbeddedUnit>,
    refused: Vec<(String, String)>,
    entry_compiled: bool,
    warnings: Vec<Diagnostic>,
}

/// The transitive closure of the entry point and of the run's start-up definitions.
pub fn build(
    loaded: &Loaded,
    entry: &DefInfo,
    startup: &[&DefInfo],
) -> Result<Built, Vec<Diagnostic>> {
    let front = &loaded.front;
    let hashes = &front.hashes;
    let bodies = ply_store::body::of_front(front);
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

    out.closure = closure_texts(&out, front)?;
    // Reopened as a target opens it, so an artifact that builds is one that opens.
    let opened = reopen(&out).map_err(|diags| vec![unreopened(&diags)])?;
    let names: Vec<&str> = out.names.iter().map(|(n, _)| n.as_str()).collect();
    let emission = embedded_unit(&opened, &opened.entry, &names).map_err(|d| vec![d])?;
    out.unit = emission.unit;

    Ok(Built {
        artifact: out,
        entry_name: entry.name.clone(),
        startup: startup.iter().map(|d| d.name.clone()).collect(),
        closure: restricted_closure(hashes, &reachable),
        refused: emission.refused,
        entry_compiled: emission.entry_compiled,
        warnings: emission.warnings,
    })
}

/// Whether the module that holds `name` exports it; a prelude effect has no entry and is public.
fn exports(front: &Front, name: &str) -> bool {
    let symbol = Symbol::new(name);
    if let Some(written) = front.defs_written.get(&symbol) {
        return written.vis.is_public();
    }
    if let Some(declared) = front.types.get(&symbol) {
        return declared.vis.is_public();
    }
    front
        .effects_written
        .get(&symbol)
        .is_none_or(|vis| vis.is_public())
}

fn closure_texts(
    artifact: &Artifact,
    front: &Front,
) -> Result<Vec<(String, String)>, Vec<Diagnostic>> {
    let bodies: Vec<&[u8]> = artifact.bodies.values().map(StoredBody::as_bytes).collect();
    let names: Vec<ply_codegen::c::producer::PrintedName<'_>> = artifact
        .names
        .iter()
        .map(|(name, hash)| ply_codegen::c::producer::PrintedName {
            name: name.as_str(),
            hash: *hash,
            public: exports(front, name),
        })
        .collect();
    let shipped: BTreeSet<&str> = artifact
        .names
        .iter()
        .filter_map(|(name, _)| name.rsplit_once('.').map(|(module, _)| module))
        .filter(|module| crate::shipped::is_shipped_name(module))
        .collect();
    let shipped: Vec<&str> = shipped.into_iter().collect();
    let printed = ply_codegen::c::producer::print_bodies(&bodies, &names, &[], &[], &shipped)
        .map_err(|d| vec![d])?;
    Ok(printed
        .into_iter()
        .map(|(module, text)| (format!("{}.ply", module.replace('.', "/")), text))
        .collect())
}

/// Embedded so `ply run` need not emit and compile C at every run; a definition the emitter
/// refused fails the build, and any other failed production is reported.
fn embedded_unit(opened: &Opened, entry: &Symbol, names: &[&str]) -> Result<Emission, Diagnostic> {
    ply_codegen::c::producer::ensure_default();
    // Definitions only: no emitter is offered effect or resource declarations.
    let names: Vec<&str> = names
        .iter()
        .copied()
        .filter(|name| opened.front.check.defs.contains_key(&Symbol::new(name)))
        .collect();
    let texts = crate::commands::common::module_texts(&opened.front.check, &opened.sources);
    let produced = ply_codegen::Unit::over_front(&opened.front, texts)
        .and_then(|unit| unit.produce(&names))
        .and_then(|produced| {
            let text = ply_codegen::c::bundle::pack(&produced.text)?;
            Ok((produced, text))
        });
    match produced {
        Ok((produced, text)) => Ok(Emission {
            unit: Some(EmbeddedUnit { text }),
            entry_compiled: produced.exports.names().iter().any(|n| n == entry.as_str()),
            refused: produced
                .refused
                .iter()
                .map(|r| (r.function.clone(), r.construct.clone()))
                .collect(),
            warnings: Vec::new(),
        }),
        // A refusal is a fact about the program, not about this host's toolchain: it fails the
        // build rather than landing an artifact whose entry finds no body.
        Err(e) => match ply_codegen::c::refused_in(&e) {
            Some(refusals) => Err(refusals.diagnostic().clone()),
            None => Ok(Emission {
                unit: None,
                refused: Vec::new(),
                entry_compiled: false,
                warnings: vec![
                    Diagnostic::warning(
                        codes::BACKEND_UNAVAILABLE,
                        format!("no compiled unit could be produced for the artifact: {e:#}"),
                    )
                    .note("the artifact's bodies are printed back to source and compiled at each run instead"),
                ],
            }),
        },
    }
}

/// Every refusal, in the order the fixpoint dropped them: a cascade names its cause first and
/// then each body that lost a callee, so reading from the top is reading the reason.
pub fn refusal_list(refused: &[(String, String)]) -> String {
    let mut out = String::from("the emitter refused, in the order it dropped them:");
    for (function, construct) in refused {
        out.push_str(&format!("\n  `{function}` ({construct})"));
    }
    out
}

pub(crate) fn stale_unit() -> Diagnostic {
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

    fn hash32(&self, at: usize) -> Result<[u8; 32], Diagnostic> {
        Ok(self.slice(at, 32)?.try_into().unwrap())
    }
}

pub fn decode(bytes: &[u8], path: &Path) -> Result<(Artifact, Vec<Diagnostic>), Diagnostic> {
    let r = Reader { bytes, path };
    let container = container(bytes, path)?;
    let stated = container.digest;

    check_versions(
        path,
        container.frontend,
        container.runtime,
        container.body_encoding,
    )?;
    let mut warnings = Vec::new();
    if container.std != ply_std::digest() {
        warnings.push(stdlib_changed());
    }

    let mut out = Artifact {
        frontend: container.frontend,
        runtime: container.runtime,
        body_encoding: container.body_encoding,
        std: container.std,
        entry: container.entry,
        bodies: BTreeMap::new(),
        names: Vec::new(),
        closure: Vec::new(),
        unit: None,
    };

    let placed = container
        .section("bodies")
        .ok_or_else(|| invalid(path, "the artifact carries no definitions"))?;
    let (records, offset, len) = (placed.count, placed.at, placed.len);
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

    let strings = match container.section("strings") {
        Some(placed) => r.slice(placed.at, placed.len)?,
        None => &[][..],
    };
    if let Some(placed) = container.section("names") {
        let (records, offset, len) = (placed.count, placed.at, placed.len);
        if len != records * 40 {
            return Err(invalid(path, "the namespace section is the wrong size"));
        }
        for i in 0..records {
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

    if let Some(placed) = container.section("closure") {
        let mut at = placed.at;
        let end = placed.at + placed.len;
        for _ in 0..placed.count {
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

    if let Some(placed) = container.section("unit") {
        let end = placed.at + placed.len;
        let mut at = placed.at;
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

    let computed = plan(bytes.len())?
        .and_then(|plan| plan.over(bytes))
        .ok_or_else(|| invalid(path, "the artifact is too short to carry a digest"))?;
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
    decode(&bytes_of(path)?, path)
}

/// The container as it lies on disk, before anything is believed about it.
pub fn bytes_of(path: &Path) -> Result<Vec<u8>, Diagnostic> {
    std::fs::read(path).map_err(|e| {
        invalid(path, format!("could not read `{}`: {e}", path.display()))
            .note("name the `.plyx` file `ply build` wrote")
    })
}

pub struct Opened {
    pub sources: SourceMap,
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

fn front_failed(why: String) -> Vec<Diagnostic> {
    vec![
        Diagnostic::error(
            codes::INTERNAL_ERROR,
            format!("the front end could not answer for this program: {why}"),
        )
        .note("this is Ply's fault: the compiler's own front end is what failed here"),
    ]
}

/// What the port answered, kept whole so it can be filed and rebuilt without asking again.
struct Answered {
    front: Front,
    modules: Vec<String>,
    dump: String,
}

/// Shipped modules live in this binary, pinned by the header's `ply_std::digest()`; the port
/// pulls in the ones the closure imports and places them after it.
fn ask_the_port(
    own: &[(String, String)],
    ids: &mut Vec<ply_span::SourceId>,
    sources: &mut SourceMap,
) -> Result<Answered, Vec<Diagnostic>> {
    ply_codegen::c::producer::ensure_default();
    let shelf = crate::shipped::sources();
    let pulled = ply_codegen::c::producer::front_pulling_std(own, shelf)
        .map_err(|e| front_failed(format!("{e:#}")))?;
    let front = place_and_read(&pulled.modules, &pulled.dump, ids, sources)?;
    Ok(Answered {
        front,
        modules: pulled.modules,
        dump: pulled.dump,
    })
}

/// The shipped modules the port pulled in, placed after the closure's files so the dump's source
/// indexes land where it wrote them, and the dump read back against those very ids.
fn place_and_read(
    modules: &[String],
    dump: &str,
    ids: &mut Vec<ply_span::SourceId>,
    sources: &mut SourceMap,
) -> Result<Front, Vec<Diagnostic>> {
    for module in modules {
        let name = ModuleName::from_dotted(module);
        let text = crate::shipped::source(&name).ok_or_else(|| {
            front_failed(format!("it pulled in `{module}`, which is not shipped"))
        })?;
        ids.push(sources.add(crate::shipped::pseudo_path(&name), text));
    }
    let front = ply_ty::read_front(dump, ids.as_slice())
        .map_err(|e| front_failed(format!("the front end's answer does not read: {e}")))?;
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

/// Where the front end's answer for one artifact is kept: beside the program, under a key that is
/// the artifact's own bytes plus what reads them.
/// An artifact's digest covers what it holds, not what it was built against: the shipped library
/// it closed over sits outside the hashed ranges. A reopened `Front` is an answer over that
/// library, so the key names it rather than relying on where the file happens to sit.
fn front_cache(artifact: &Artifact) -> PathBuf {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&artifact.digest());
    hasher.update(ply_store::FRONTEND_VERSION.as_bytes());
    hasher.update(&[0]);
    hasher.update(ply_codegen::c::producer::identity().as_bytes());
    hasher.update(&[0]);
    hasher.update(&ply_std::digest());
    crate::shipped::stage().join(format!("front.{}", &hasher.finalize().to_hex()[..16]))
}

/// The pulled module names, then the dump: the two halves a `Front` is rebuilt from in process.
fn file_front(at: &Path, modules: &[String], dump: &str) {
    let Some(parent) = at.parent() else { return };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let mut bytes = format!("{}\n", modules.len()).into_bytes();
    for module in modules {
        bytes.extend_from_slice(module.as_bytes());
        bytes.push(b'\n');
    }
    bytes.extend_from_slice(dump.as_bytes());
    let tmp = parent.join(format!("front.{}.tmp", std::process::id()));
    if std::fs::write(&tmp, &bytes).is_ok() && std::fs::rename(&tmp, at).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

fn split_front(text: &str) -> Option<(Vec<String>, &str)> {
    let (count, mut rest) = text.split_once('\n')?;
    let count: usize = count.parse().ok()?;
    let mut modules = Vec::with_capacity(count);
    for _ in 0..count {
        let (name, tail) = rest.split_once('\n')?;
        modules.push(name.to_string());
        rest = tail;
    }
    Some((modules, rest))
}

/// The `Front` an earlier run answered for this very artifact. A hit skips the check below that
/// the closure rebuilds the artifact's bodies: the key is the digest of those very bytes, and the
/// file is only ever written after that check passed on this machine.
fn cached_front(
    at: &Path,
    ids: &mut Vec<ply_span::SourceId>,
    sources: &mut SourceMap,
) -> Option<Front> {
    let text = std::fs::read_to_string(at).ok()?;
    let (modules, dump) = split_front(&text)?;
    let kept_ids = ids.clone();
    let kept_sources = sources.clone();
    match place_and_read(&modules, dump, ids, sources) {
        Ok(front) => Some(front),
        Err(_) => {
            *ids = kept_ids;
            *sources = kept_sources;
            None
        }
    }
}

fn reopen(artifact: &Artifact) -> Result<Opened, Vec<Diagnostic>> {
    let mut sources = SourceMap::new();
    let mut own: Vec<(String, String)> = Vec::new();
    let mut ids: Vec<ply_span::SourceId> = Vec::new();
    for (file, text) in &artifact.closure {
        let relative = PathBuf::from(file);
        let name = ModuleName::from_relative_path(&relative).map_err(|d| vec![d])?;
        if crate::shipped::is_shipped(&name) {
            return Err(vec![unfaithful(format!(
                "the closure carries `{file}`, a module this `ply` ships"
            ))]);
        }
        ids.push(sources.add(&relative, text.clone()));
        own.push((name.to_string(), text.clone()));
    }
    let filed = front_cache(artifact);
    if let Some(front) = cached_front(&filed, &mut ids, &mut sources) {
        return entered(artifact, sources, front);
    }
    let answered = match ask_the_port(&own, &mut ids, &mut sources) {
        Ok(answered) => answered,
        Err(diags) => return Err(over_printed(diags, &sources)),
    };
    let front = answered.front;

    let hashes = &front.hashes;
    let bodies = ply_store::body::of_front(&front);
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
    if let Some(extra) = hashes.defs.keys().chain(hashes.decls.keys()).find(|name| {
        !crate::shipped::is_shipped_name(name.as_str()) && !named.contains(name.as_str())
    }) {
        return Err(vec![unfaithful(format!(
            "the closure declares `{extra}`, which the artifact does not name"
        ))]);
    }

    file_front(&filed, &answered.modules, &answered.dump);
    entered(artifact, sources, front)
}

fn entered(
    artifact: &Artifact,
    sources: SourceMap,
    front: Front,
) -> Result<Opened, Vec<Diagnostic>> {
    let entry = artifact
        .entry_name()
        .map(Symbol::new)
        .ok_or_else(|| vec![unfaithful("the artifact names no entry point".to_string())])?;
    Ok(Opened {
        sources,
        front,
        entry,
    })
}

/// What a caller lends an entered program: the roots it may reach, the programs its
/// `process.spawn` labels may start, and the host operations only this entry may perform. What is
/// not lent here, the program cannot reach at all.
#[derive(Default)]
pub struct Binds {
    pub roots: Vec<ply_host::fs::RootSpec>,
    pub executables: ply_host::process::Executables,
    pub lent: Vec<crate::hosts::Lent>,
}

/// One entry into an opened artifact, with no line of its own on either stream: the program's
/// output is the whole of what a caller sees. The answer is the code `process.exit` asked for,
/// else `0` for a value returned and the diagnostic for a raise.
pub fn enter(
    artifact: &Artifact,
    opened: &Opened,
    argv: Vec<String>,
    binds: Binds,
) -> Result<i32, Diagnostic> {
    let Binds {
        roots,
        executables,
        lent,
    } = binds;
    // A unit built for another runtime is left aside and the bodies serve.
    let unit = if servable(artifact) {
        artifact.unit.as_ref()
    } else {
        None
    };
    let declared = opened
        .front
        .check
        .defs
        .get(&opened.entry)
        .map(|d| d.footprint.clone());
    let tier = tier(opened, None, unit)?;
    let process = ply_host::process::ProcessHost::new(
        argv,
        ply_host::process::Sink::Real {
            out: ply_host::process::Stream::Out,
        },
    )
    .executing(executables);
    let hosts = crate::hosts::Hosts::open_stopping(
        &opened.front.check,
        true,
        &crate::cli::TlsOptions::default(),
        &roots,
        None,
        crate::config::Configuration::default(),
        &crate::trace::TraceOptions::default(),
        declared.as_ref(),
        None,
        Some(process),
        lent,
    )
    .map_err(|diagnostics| bind_failed(&diagnostics))?;
    let span = opened
        .front
        .check
        .defs
        .get(&opened.entry)
        .map(|d| d.span)
        .unwrap_or(Span::DUMMY);
    let plan = crate::simulation::run_plan(None);
    let answer = evaluate(opened, span, &plan, &hosts, declared.as_ref(), tier);
    let _ = crate::run::teardown(&hosts, None, crate::run::TEARDOWN_FLOOR_MS);
    match hosts.requested_exit() {
        Some(code) => Ok(code),
        None => answer.map(|_| crate::EXIT_OK),
    }
}

fn bind_failed(diagnostics: &[Diagnostic]) -> Diagnostic {
    diagnostics.first().cloned().unwrap_or_else(|| {
        Diagnostic::error(
            codes::INTERNAL_ERROR,
            "the artifact's hosts could not be bound, and nothing said why",
        )
    })
}

/// Whether the artifact's embedded unit is one this runtime can enter. One built for another
/// runtime is left aside; one that is broken rather than foreign is passed on and refused loudly.
pub(crate) fn servable(artifact: &Artifact) -> bool {
    let Some(unit) = &artifact.unit else {
        return false;
    };
    let served = ply_codegen::c::bundle::unpack(&unit.text)
        .and_then(|text| ply_codegen::c::served(&text, "artifact"));
    !matches!(&served, Err(e) if e.downcast_ref::<ply_codegen::c::Unserved>().is_some())
}

/// The unit the artifact runs on: its embedded one as built, else one compiled from its bodies.
pub(crate) fn tier(
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
        let provider: &'static dyn ply_eval::Provider =
            ply_codegen::Unit::embedded(&opened.front, text).map_err(|e| unit_error(&e))?;
        return Ok(Some((provider, spec)));
    }
    let texts = crate::commands::common::module_texts(&opened.front.check, &opened.sources);
    let provider = crate::commands::common::build_backend_over(&spec, &opened.front, texts)?;
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
    let mut machine = ply_eval::Machine::new(&opened.front);
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

/// A refusal over text no caller holds. The spans point into the closure printed a moment ago, so
/// this is the only place they mean anything; rendered here, the reason survives as a note.
fn over_printed(diags: Vec<Diagnostic>, sources: &SourceMap) -> Vec<Diagnostic> {
    diags
        .into_iter()
        .map(|d| {
            let shown = ply_span::render::to_terminal(&d, sources, false);
            d.note(shown.trim_end().to_string())
        })
        .collect()
}

fn first_of(diags: &[Diagnostic]) -> String {
    diags.first().map_or_else(
        || "no reason was given".to_string(),
        |d| match d.labels.iter().find(|l| l.primary && !l.message.is_empty()) {
            Some(l) => format!("{}: {} ({})", d.code, d.message, l.message),
            None => format!("{}: {}", d.code, d.message),
        },
    )
}

fn unreopened(diags: &[Diagnostic]) -> Diagnostic {
    let named = Diagnostic::error(
        codes::INTERNAL_ERROR,
        "the closure printed back to source does not open as the program it was printed from",
    )
    .note(first_of(diags));
    diags
        .first()
        .map(|d| d.notes.as_slice())
        .unwrap_or_default()
        .iter()
        .fold(named, |out, note| out.note(note.clone()))
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
