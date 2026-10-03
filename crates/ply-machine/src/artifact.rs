//! The deployable artifact: the transitive closure of one entry point, in the same bytes the
//! content-addressed store already holds.

use crate::body::StoredBody;
use crate::load::Loaded;
use crate::payload::record;
use ply_eval::decode::{self, At};
use ply_eval::{
    DefHash, DefInfo, Diagnostic, Ended, Front, ModuleName, Severity, SourceMap, Span, Symbol,
    Value, codes,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub const EXTENSION: &str = "plyx";

/// A library's container: a package to depend on, not a program, and never entered.
pub const LIBRARY_EXTENSION: &str = "plyz";

/// The container is `crates/ply-compiler/ply/plyx.ply`: the magic, every header offset, the
/// section table and what a digest covers are written there and nowhere else.
const ENCODE: &str = "plyx.encode";
const DECODE: &str = "plyx.decode";
const PLAN: &str = "plyx.plan";
const FORMAT: &str = "plyx.format";

/// The emitted C, compressed; left aside when its helper table is not a prefix of this runtime's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbeddedUnit {
    pub text: Vec<u8>,
}

/// The compiler that built an artifact, as the digest of its sources: one built by another reads its
/// closure and bodies another way.
pub fn compiler() -> [u8; 32] {
    *blake3::hash(ply_codegen::c::producer::identity().as_bytes()).as_bytes()
}

/// The runtime an artifact's unit was compiled against, as the digest of the helper table it calls.
pub fn runtime() -> [u8; 32] {
    *blake3::hash(ply_codegen::c::exports::helpers_digest().as_bytes()).as_bytes()
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Artifact {
    /// [`compiler`] where it was built.
    pub frontend: [u8; 32],
    /// [`runtime`] where it was built.
    pub runtime: [u8; 32],
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
        ply_std::short_digest(&self.digest())
    }

    /// The container `plyx.ply` places these sections in, with its digest written into the field
    /// no range of the digest covers.
    pub fn encode(&self) -> Result<Vec<u8>, Diagnostic> {
        let head = record(vec![
            ("frontend", Value::bytes(self.frontend)),
            ("runtime", Value::bytes(self.runtime)),
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
    pub fn sections(&self) -> Vec<(&'static str, u32, Vec<u8>)> {
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

/// What a program digest is taken over, as `plyx.ply` states it. The hash itself is the host's,
/// as the decode that verifies it is.
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
    let what = format!("`{PLAN}`'s answer");
    let read = || -> Result<Option<Plan>, decode::Error> {
        let Some(plan) = At::new(&what, &answered).option()? else {
            return Ok(None);
        };
        Ok(Some(Plan {
            domain: plan.field("domain")?.bytes()?.to_vec(),
            at: plan.field("at")?.number()?,
            covers: plan
                .field("covers")?
                .items(|range| Ok((range.field("at")?.number()?, range.field("len")?.number()?)))?,
        }))
    };
    read().map_err(|e| container_failed(format!("the answer does not read: {e}")))
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
    let what = format!("`{DECODE}`'s answer");
    let read = || -> Result<Result<Container, Diagnostic>, decode::Error> {
        let opened = match At::new(&what, &answered).result()? {
            Ok(opened) => opened,
            Err(refusal) => return Ok(Err(refused(path, refusal)?)),
        };
        let hash = |name: &str| opened.field(name)?.byte_array::<32>();
        Ok(Ok(Container {
            frontend: hash("frontend")?,
            runtime: hash("runtime")?,
            std: hash("stdlib")?,
            entry: DefHash(hash("entry")?),
            digest: hash("digest")?,
            sections: opened.field("sections")?.items(|section| {
                Ok(Placed {
                    name: section.field("name")?.utf8()?.to_string(),
                    count: section.field("count")?.number()?,
                    at: section.field("at")?.number()?,
                    len: section.field("len")?.number()?,
                })
            })?,
        }))
    };
    read().map_err(|e| container_failed(format!("the answer does not read: {e}")))?
}

/// The container's own refusal, as the diagnostic the reader would have raised: a stale one is a
/// file no transfer will mend, and every other is a file that arrived damaged.
fn refused(path: &Path, refusal: At<'_>) -> Result<Diagnostic, decode::Error> {
    let lossy = |text: &[u8]| String::from_utf8_lossy(text).into_owned();
    let message = lossy(refusal.field("message")?.bytes()?);
    let base = if refusal.field("stale")?.bool()? {
        version(path, message)
    } else {
        invalid(path, message)
    };
    let note = refusal.field("note")?.bytes()?;
    Ok(if note.is_empty() {
        base
    } else {
        base.note(lossy(note))
    })
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

pub struct Built {
    pub artifact: Artifact,
    pub entry_name: Symbol,
    /// In the order they were named.
    pub startup: Vec<Symbol>,
    /// Every definition the entry and the startup roots reach, them included.
    pub reachable: BTreeSet<Symbol>,
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

/// What a build of a program is a function of, as one digest: its sources (already digested),
/// the shelf, the compiler and the runtime a decode refuses a mismatch of. The launcher gates the
/// committed CLI artifact on this: behind the sources, a binary runs the sources instead.
pub fn toolchain_stamp(program_digest: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    for part in [
        program_digest.as_bytes(),
        ply_codegen::c::producer::digest_of(crate::shelf::sources()).as_bytes(),
        &compiler(),
        &runtime(),
    ] {
        hasher.update(part);
        hasher.update(&[0]);
    }
    hasher.finalize().to_hex()[..16].to_string()
}

pub fn build(
    loaded: &Loaded,
    entry: &DefInfo,
    startup: &[&DefInfo],
) -> Result<Built, Vec<Diagnostic>> {
    let front = &loaded.front;
    let hashes = &front.hashes;
    let bodies = crate::body::of_front(front);
    let Some(entry_hash) = hashes.defs.get(&entry.name).copied() else {
        return Err(vec![missing_entry(&entry.name)]);
    };
    if let Some(root) = startup.iter().find(|r| !hashes.deps.contains_key(&r.name)) {
        return Err(vec![missing_entry(&root.name)]);
    }
    let reachable =
        hashes.reach(std::iter::once(&entry.name).chain(startup.iter().map(|r| &r.name)));

    let mut out = Artifact {
        frontend: compiler(),
        runtime: runtime(),
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
        reachable,
        refused: emission.refused,
        entry_compiled: emission.entry_compiled,
        warnings: emission.warnings,
    })
}

/// What a library's `.plyz` carries: the compiled unit for a set of definitions, and the head a
/// consumer's gate reads. A library has no artifact — no entry, no closure — so the head fields
/// are the ones a program's artifact computes.
pub struct LibraryUnit {
    pub frontend: [u8; 32],
    pub runtime: [u8; 32],
    pub stdlib: [u8; 32],
    pub payload: Vec<u8>,
}

/// The compiled unit for a set of definitions, with no entry: what a library's `.plyz` carries.
/// Every definition named is compiled, since a library has no closure to prune against — a consumer
/// imports whichever modules it likes — and the budget a `ply build` gives an entry is not spent.
pub fn library_unit(loaded: &Loaded, names: &[String]) -> Result<LibraryUnit, Vec<Diagnostic>> {
    ply_codegen::c::producer::ensure_default();
    let texts = crate::support::module_texts(&loaded.check, &loaded.sources);
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let produced =
        ply_codegen::Unit::over_front(&loaded.front, texts).and_then(|unit| unit.produce(&names));
    match produced {
        Ok(produced) => {
            let payload = ply_codegen::c::bundle::pack(&produced.text).map_err(|e| {
                vec![Diagnostic::error(
                    codes::INTERNAL_ERROR,
                    format!("the library's unit would not pack: {e}"),
                )]
            })?;
            Ok(LibraryUnit {
                frontend: compiler(),
                runtime: runtime(),
                stdlib: ply_std::digest(),
                payload,
            })
        }
        Err(e) => {
            match ply_codegen::c::refused_in(&e) {
                Some(refusals) => Err(vec![refusals.diagnostic().clone()]),
                None => Err(
                    vec![Diagnostic::error(
                codes::INTERNAL_ERROR,
                format!("the library's unit would not compile: {e}"),
            )
            .note("this is Ply's fault: the front end accepted the program the emitter refused")],
                ),
            }
        }
    }
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
        .filter(|module| crate::shelf::is_shipped_name(module))
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
    let texts = crate::support::module_texts(&opened.front.check, &opened.sources);
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

    check_versions(path, container.frontend, container.runtime)?;
    let mut warnings = Vec::new();
    if container.std != ply_std::digest() {
        warnings.push(stdlib_changed());
    }

    let mut out = Artifact {
        frontend: container.frontend,
        runtime: container.runtime,
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
                ply_std::short_digest(&stated),
                ply_std::short_digest(&computed)
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

fn check_versions(path: &Path, frontend: [u8; 32], runtime: [u8; 32]) -> Result<(), Diagnostic> {
    if frontend != compiler() {
        return Err(version(
            path,
            "the artifact was built by another compiler than this `ply` ships",
        ));
    }
    if runtime != self::runtime() {
        return Err(version(
            path,
            "the artifact was compiled for another runtime than this `ply` runs",
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

pub fn unreadable(path: &Path) -> Diagnostic {
    invalid(path, format!("could not read `{}`", path.display()))
        .note("name the `.plyx` file `ply build` wrote")
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
    dump: ply_eval::Value,
}

/// Shipped modules live in this binary, pinned by the header's `ply_std::digest()`; the port
/// pulls in the ones the closure imports and places them after it.
fn ask_the_port(
    own: &[(String, String)],
    ids: &mut Vec<ply_eval::SourceId>,
    sources: &mut SourceMap,
) -> Result<Answered, Vec<Diagnostic>> {
    ply_codegen::c::producer::ensure_default();
    let shelf = crate::shelf::sources();
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
    dump: &ply_eval::Value,
    ids: &mut Vec<ply_eval::SourceId>,
    sources: &mut SourceMap,
) -> Result<Front, Vec<Diagnostic>> {
    for module in modules {
        let name = ModuleName::from_dotted(module);
        let text = crate::shelf::source(&name).ok_or_else(|| {
            front_failed(format!("it pulled in `{module}`, which is not shipped"))
        })?;
        ids.push(sources.add(crate::shelf::pseudo_path(&name), text));
    }
    let front = ply_codegen::c::dump::read(dump, ids.as_slice())
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

/// Where the front end's answer for one artifact is kept: in the stage's fronts directory, under a
/// key that is the artifact's own bytes but for its unit, plus what reads them.
///
/// An artifact's digest covers what it holds, not what it was built against: the shipped library
/// it closed over sits outside the hashed ranges. A reopened `Front` is an answer over that
/// library, so the key names it rather than relying on where the file happens to sit.
///
/// The unit is emitted from the front after `build` reopens the closure, so leaving it out is what
/// lets a built artifact's first run find the front its build answered.
pub fn front_cache(artifact: &Artifact) -> PathBuf {
    let closure = Artifact {
        unit: None,
        ..artifact.clone()
    };
    let mut hasher = blake3::Hasher::new();
    // What the entry is written as is part of its key.
    hasher.update(b"front.FrontAnswer as ply_eval::codec\0");
    hasher.update(&closure.digest());
    hasher.update(ply_codegen::c::producer::identity().as_bytes());
    hasher.update(&[0]);
    hasher.update(&ply_std::digest());
    ply_codegen::c::bundle::stage_dir(ply_codegen::c::sweep::FRONTS)
        .join(format!("front.{}", &hasher.finalize().to_hex()[..16]))
}

/// The pulled module names and the dump, as `front.answer_pulling_std_with` answers them: the two
/// halves a `Front` is rebuilt from in process.
fn file_front(at: &Path, modules: &[String], dump: &ply_eval::Value) {
    let Some(parent) = at.parent() else { return };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let answer = record(vec![
        (
            "pulled",
            ply_eval::Value::list(
                modules
                    .iter()
                    .map(|m| ply_eval::Value::bytes(m.as_bytes()))
                    .collect(),
            ),
        ),
        ("dump", dump.clone()),
    ]);
    let Ok(bytes) = ply_eval::codec::encode(&answer) else {
        return;
    };
    let _ = ply_eval::files::write_atomically(at, &bytes);
}

/// The `Front` an earlier run answered for this very artifact. A hit skips the check below that
/// the closure rebuilds the artifact's bodies: the key is the digest of those very bytes, and the
/// file is only ever written after that check passed on this machine.
fn cached_front(
    at: &Path,
    ids: &mut Vec<ply_eval::SourceId>,
    sources: &mut SourceMap,
) -> Option<Front> {
    let answer = ply_eval::codec::decode(&std::fs::read(at).ok()?).ok()?;
    let filed = ply_eval::decode::At::new("a filed front end", &answer);
    let modules = filed
        .field("pulled")
        .and_then(|m| m.items(|name| Ok(name.utf8()?.to_string())))
        .ok()?;
    let dump = filed.field("dump").ok()?.value();
    let kept_ids = ids.clone();
    let kept_sources = sources.clone();
    match place_and_read(&modules, dump, ids, sources) {
        Ok(front) => {
            ply_codegen::c::sweep::used(at);
            Some(front)
        }
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
    let mut ids: Vec<ply_eval::SourceId> = Vec::new();
    for (file, text) in &artifact.closure {
        let relative = PathBuf::from(file);
        let name = ModuleName::from_relative_path(&relative).map_err(|d| vec![d])?;
        if crate::shelf::is_shipped(&name) {
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
    let bodies = crate::body::of_front(&front);
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
        !crate::shelf::is_shipped_name(name.as_str()) && !named.contains(name.as_str())
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
    /// Certificates its `net.connect_tls` accepts beside the built-in roots, as `--trust` names them.
    pub trust: Vec<PathBuf>,
}

/// The code an entry answers with when it asked for none.
pub const EXIT_OK: i32 = 0;

/// One entry into an opened artifact, with no line of its own on either stream: the program's
/// output is the whole of what a caller sees. The answer is the code `process.exit` asked for,
/// else `0` for a value returned and the diagnostic for a raise; what the entry ended with is the
/// caller's to report.
pub fn enter(artifact: &Artifact, opened: &Opened, argv: Vec<String>, binds: Binds) -> Ended<i32> {
    match artifact.unit.as_ref().map(served_text).transpose() {
        Ok(text) => entered_with(text.flatten(), opened, argv, binds),
        Err(refused) => Ended::refused(refused),
    }
}

/// [`enter`] for a program no artifact carries: its own load, compiled here from the body cache.
pub fn enter_loaded(opened: &Opened, argv: Vec<String>, binds: Binds) -> Ended<i32> {
    entered_with(None, opened, argv, binds)
}

fn entered_with(
    unit: Option<String>,
    opened: &Opened,
    argv: Vec<String>,
    binds: Binds,
) -> Ended<i32> {
    let Binds {
        roots,
        executables,
        lent,
        trust,
    } = binds;
    let declared = opened
        .front
        .check
        .defs
        .get(&opened.entry)
        .map(|d| d.footprint.clone());
    let tier = match tier(opened, unit) {
        Ok(tier) => tier,
        Err(refused) => return Ended::refused(refused),
    };
    let process = ply_host::process::ProcessHost::new(
        argv,
        ply_host::process::Sink::Real {
            out: ply_host::process::Stream::Out,
        },
    )
    .executing(executables);
    let hosts = match crate::hosts::Hosts::open_stopping(
        &opened.front.check,
        true,
        &crate::options::TlsOptions {
            tls: Vec::new(),
            trust,
        },
        &roots,
        crate::config::Configuration::default(),
        &crate::trace::TraceOptions::default(),
        None,
        Some(process),
        lent,
    ) {
        Ok(hosts) => hosts,
        Err(diagnostics) => return Ended::refused(bind_failed(&diagnostics)),
    };
    let span = opened
        .front
        .check
        .defs
        .get(&opened.entry)
        .map(|d| d.span)
        .unwrap_or(Span::DUMMY);
    let ended = evaluate(opened, span, &hosts, declared.as_ref(), tier);
    let _ = crate::drive::teardown(&hosts);
    let requested = hosts.requested_exit();
    ended.map(|answer| match requested {
        Some(code) => Ok(code),
        None => answer.map(|_| EXIT_OK),
    })
}

fn bind_failed(diagnostics: &[Diagnostic]) -> Diagnostic {
    diagnostics.first().cloned().unwrap_or_else(|| {
        Diagnostic::error(
            codes::INTERNAL_ERROR,
            "the artifact's hosts could not be bound, and nothing said why",
        )
    })
}

fn unit_error(e: &dyn std::fmt::Display) -> Diagnostic {
    Diagnostic::error(
        codes::ARTIFACT_INVALID,
        format!("the artifact's compiled unit could not be entered: {e:#}"),
    )
}

/// The embedded unit's C when it serves this runtime, `None` when it was emitted for another (the
/// bodies serve instead), and a refusal when it is broken rather than foreign. Read from the text,
/// so nothing is compiled to find out.
pub(crate) fn served_text(unit: &EmbeddedUnit) -> Result<Option<String>, Diagnostic> {
    let text = ply_codegen::c::bundle::unpack(&unit.text).map_err(|e| unit_error(&e))?;
    let exports = ply_codegen::c::Exports::from_text(&text)
        .ok_or_else(|| unit_error(&"its table of what it holds does not read"))?;
    Ok(exports.unserved().is_none().then_some(text))
}

pub(crate) fn stale_unit() -> Diagnostic {
    Diagnostic::warning(
        codes::ARTIFACT_VERSION,
        "the artifact's compiled unit was built for another runtime and is left aside",
    )
    .note("the artifact's bodies are printed back to source and compiled at this run instead")
    .note("rebuild the artifact with this `ply` to carry a unit it can enter")
}

/// The unit the artifact runs on: its embedded one as built, else one compiled from its bodies.
/// `unit` is the embedded unit's C, as [`served_text`] answers it.
pub(crate) fn tier(
    opened: &Opened,
    unit: Option<String>,
) -> Result<&'static dyn ply_eval::Provider, Diagnostic> {
    // Entered as built: no producer is asked.
    if let Some(text) = unit {
        let provider: &'static dyn ply_eval::Provider =
            ply_codegen::Unit::handed(&opened.front, text).map_err(|e| unit_error(&e))?;
        return Ok(provider);
    }
    ply_codegen::c::producer::ensure_default();
    let texts = crate::support::module_texts(&opened.front.check, &opened.sources);
    ply_codegen::Unit::over_front(&opened.front, texts)
        .map(|unit| unit as &'static dyn ply_eval::Provider)
        .map_err(|error| match ply_codegen::c::refused_in(&error) {
            Some(refusals) => refusals.diagnostic().clone(),
            None => Diagnostic::error(
                codes::BACKEND_UNAVAILABLE,
                format!("the C backend could not be built: {error:#}"),
            ),
        })
}

fn evaluate(
    opened: &Opened,
    span: Span,
    hosts: &crate::hosts::Hosts,
    declared: Option<&ply_eval::Footprint>,
    tier: &'static dyn ply_eval::Provider,
) -> Ended<ply_eval::Value> {
    let mut machine = match ply_eval::Machine::new(&opened.front, tier.attach()) {
        Ok(machine) => machine,
        Err(refused) => return Ended::refused(refused),
    };
    machine.set_host_binding(hosts.binding());
    if let Some(runtime) = hosts.runtime_factory() {
        machine.set_host_runtime(runtime);
    }
    if let Some(declared) = declared {
        machine.set_declared_footprint(declared.clone());
    }
    machine.set_seed(ply_eval::Seed::default(), ply_eval::sim::DEFAULT_STEPS);
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
/// this is the only place they mean anything; each label that places survives as a note.
fn over_printed(diags: Vec<Diagnostic>, sources: &SourceMap) -> Vec<Diagnostic> {
    diags.into_iter().map(|d| d.placed(sources)).collect()
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
