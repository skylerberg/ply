//! The front-end cache: `path -> SourceFingerprint` and `DefHash -> interface`.
//!
//! The result cache answers "has this test already passed". This one answers
//! "has this definition already been compiled".
//!
//! The two maps have opposite invalidation characters, and every method here
//! depends on the difference:
//!
//! - `defs` / `decls` are keyed by content, so an entry is never wrong for its
//!   key and merging two processes' entries is always safe.
//! - `sources` is keyed by a path, which says nothing about what is at that path
//!   now. An entry is therefore never trusted until its `content_hash` has been
//!   checked against the bytes actually on disk.

use crate::canonical::{canonicalize_decl_body, canonicalize_scheme};
use crate::{ContentHash, disk};
use ply_core::{Footprint, Scheme, Type};
use ply_hash::DefHash;
use ply_span::{Diagnostic, SourceId, Span, Symbol};
use ply_syntax::ast::Mode;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

pub(crate) const FRONTEND_FILE: &str = "frontend.json";
pub(crate) const FRONTEND_STEM: &str = "frontend";

/// Independent of [`FORMAT`] in `disk`: the two cache files version separately
/// because they are invalidated by different kinds of change.
const FORMAT: u32 = 2;

/// A byte range within one source file, which is what a span degrades to once it
/// leaves the process. [`Span`] carries a [`SourceId`], and a `SourceId` is an
/// index into the run's `SourceMap` — adding or removing a file shifts every
/// later id, so persisting one would silently point diagnostics at the wrong
/// file.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct FileSpan {
    pub start: u32,
    pub end: u32,
}

impl FileSpan {
    /// A dummy span has no file to be relative to, so it degrades to the empty
    /// range and rebases onto a real offset 0 rather than back to
    /// [`Span::DUMMY`]. Nothing worth persisting carries one — every definition
    /// and test in a fingerprint has a real extent.
    pub fn of(span: Span) -> FileSpan {
        if span.is_dummy() {
            FileSpan { start: 0, end: 0 }
        } else {
            FileSpan {
                start: span.start,
                end: span.end,
            }
        }
    }

    pub fn rebase(self, source: SourceId) -> Span {
        Span::new(source, self.start, self.end)
    }
}

/// A name paired with what it denoted; see [`witness_holds`].
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct NameRef {
    pub name: Symbol,
    pub hash: DefHash,
}

impl NameRef {
    pub fn new(name: impl Into<Symbol>, hash: DefHash) -> NameRef {
        NameRef {
            name: name.into(),
            hash,
        }
    }
}

/// A cached interface is written in terms of *names* — `Scheme` holds
/// `Type::Con(Symbol, ..)` and a `Footprint` holds effect labels — while a
/// `DefHash` erases them, which is exactly what makes renaming free. So a hash
/// does not by itself determine the interface: `type A = | X(Int)` and
/// `type B = | Y(Int)` hash alike, and `fn f(a: A) -> Int` and
/// `fn g(b: B) -> Int` hash alike while having different schemes.
///
/// Every entry therefore records the names its interface mentions and the hash
/// each denoted when it was written, and is usable only while all of them still
/// hold. Renaming a type costs a recheck of the definitions that mention it —
/// it still changes no `DefHash`, so it still selects no test.
pub fn witness_holds(
    names: &[NameRef],
    mut resolve: impl FnMut(&Symbol) -> Option<DefHash>,
) -> bool {
    names.iter().all(|n| resolve(&n.name) == Some(n.hash))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefKind {
    Fn,
    Type,
    Effect,
}

/// A variant of a `type`, or an operation of an `effect`. Aligned with the
/// `ctors` / `ops` of the [`CachedDecl`] under the same hash — by name for an
/// operation, whose declaration order a hash erases, and by position for a
/// variant, whose order a hash keeps.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Member {
    pub name: Symbol,
    pub span: FileSpan,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct DefEntry {
    pub name: Symbol,
    pub hash: DefHash,
    pub span: FileSpan,
    pub kind: DefKind,
    /// Empty for a `fn`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<Member>,
    /// The names this definition mentions directly, in normalization order.
    /// A skipped file still has to contribute its slice of the reference graph:
    /// `HashOutput`'s `deps` and `closure` are what a failing test's suspect set
    /// is derived from, and a hole in them is a suspect nobody names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deps: Vec<Symbol>,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct CachedTest {
    pub name: String,
    pub hash: DefHash,
    pub nondet: bool,
    pub footprint: Footprint,
    pub span: FileSpan,
    pub name_span: FileSpan,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deps: Vec<Symbol>,
}

/// A module this file imports, with a digest of the exports it was compiled
/// against. Gate 1's cheap module-granular check; see [`exports_digest`].
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ImportEdge {
    pub module: Symbol,
    pub exports: ContentHash,
}

/// `content_hash` is over the file's **raw bytes**, not over anything derived
/// from parsing it: gate 1 has to decide whether to parse before it has anything
/// a parse would produce.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SourceFingerprint {
    pub content_hash: ContentHash,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub imports: Vec<ImportEdge>,
    /// Every top-level name this file mentions but does not declare, and what it
    /// resolved to. Gate 1's exact check: a file whose bytes are unchanged and
    /// whose every external name still denotes the same definition cannot
    /// compile to anything different.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deps: Vec<NameRef>,
    pub defs: Vec<DefEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tests: Vec<CachedTest>,
}

impl SourceFingerprint {
    pub fn new(content_hash: ContentHash) -> SourceFingerprint {
        SourceFingerprint {
            content_hash,
            imports: Vec::new(),
            deps: Vec::new(),
            defs: Vec::new(),
            tests: Vec::new(),
        }
    }

    /// Gate 1's first condition. A fingerprint says what the file at this path
    /// compiled to *then*, and nothing about what is there now, so it must not
    /// be believed until this returns `true`.
    pub fn matches_bytes(&self, bytes: &[u8]) -> bool {
        self.content_hash == ContentHash::of(bytes)
    }

    /// The `(name, hash)` pairs this file publishes, sorted — the input
    /// [`exports_digest`] is defined over.
    pub fn exports(&self) -> Vec<NameRef> {
        let mut out: Vec<NameRef> = self
            .defs
            .iter()
            .map(|d| NameRef {
                name: d.name.clone(),
                hash: d.hash,
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name).then(a.hash.cmp(&b.hash)));
        out
    }

    /// Every hash this fingerprint refers to, so a garbage collector can tell a
    /// live interface from an abandoned one.
    pub fn referenced_hashes(&self) -> impl Iterator<Item = DefHash> + '_ {
        self.defs.iter().map(|d| d.hash)
    }
}

/// A stable digest over a module's exported `(name, hash)` pairs. Sorted and
/// length-prefixed so that reordering a file's items, or reordering the files
/// within a module, cannot change it.
pub fn exports_digest(exports: &[NameRef]) -> ContentHash {
    let mut sorted: Vec<&NameRef> = exports.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name).then(a.hash.cmp(&b.hash)));
    sorted.dedup_by(|a, b| a.name == b.name && a.hash == b.hash);

    let mut bytes = Vec::new();
    bytes.extend_from_slice(&(sorted.len() as u32).to_le_bytes());
    for entry in sorted {
        let name = entry.name.as_str().as_bytes();
        bytes.extend_from_slice(&(name.len() as u32).to_le_bytes());
        bytes.extend_from_slice(name);
        bytes.extend_from_slice(&entry.hash.0);
    }
    ContentHash::of(&bytes)
}

/// The published interface of one `fn`, keyed by its [`DefHash`].
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct CachedDef {
    pub scheme: Scheme,
    pub footprint: Footprint,
    /// See [`witness_holds`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub names: Vec<NameRef>,
}

impl CachedDef {
    pub fn new(scheme: Scheme, footprint: Footprint) -> CachedDef {
        CachedDef {
            scheme,
            footprint,
            names: Vec::new(),
        }
    }

    pub fn witnessed_by(mut self, names: Vec<NameRef>) -> CachedDef {
        self.names = names;
        self
    }

    pub fn witness_holds(&self, resolve: impl FnMut(&Symbol) -> Option<DefHash>) -> bool {
        witness_holds(&self.names, resolve)
    }

    /// What [`crate::Store::put_def`] stores. Applied there rather than left to
    /// callers, so that no run can ever persist a scheme numbered by whatever
    /// its global counter reached.
    pub fn canonicalized(self) -> CachedDef {
        CachedDef {
            scheme: canonicalize_scheme(&self.scheme),
            footprint: self.footprint,
            names: canonical_names(self.names),
        }
    }
}

/// A witness is a set, so two callers recording the same one in different orders
/// must not produce different bytes on disk.
fn canonical_names(mut names: Vec<NameRef>) -> Vec<NameRef> {
    names.sort_by(|a, b| a.name.cmp(&b.name).then(a.hash.cmp(&b.hash)));
    names.dedup();
    names
}

/// The published interface of one `type` or `effect`, keyed by its [`DefHash`].
///
/// Separate from [`CachedDef`] because a declaration has no scheme or footprint
/// of its own, and because a file that declares one cannot be skipped by gate 1
/// unless its signatures can be restored without parsing — nearly every real
/// file declares something, so this is not an optional extra.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct CachedDecl {
    pub body: DeclBody,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub names: Vec<NameRef>,
}

impl CachedDecl {
    pub fn new(body: DeclBody) -> CachedDecl {
        CachedDecl {
            body,
            names: Vec::new(),
        }
    }

    pub fn witnessed_by(mut self, names: Vec<NameRef>) -> CachedDecl {
        self.names = names;
        self
    }

    pub fn witness_holds(&self, resolve: impl FnMut(&Symbol) -> Option<DefHash>) -> bool {
        witness_holds(&self.names, resolve)
    }

    /// What [`crate::Store::put_decl`] stores.
    pub fn canonicalized(self) -> CachedDecl {
        CachedDecl {
            body: canonicalize_decl_body(&self.body),
            names: canonical_names(self.names),
        }
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(tag = "decl", rename_all = "snake_case")]
pub enum DeclBody {
    Type {
        /// Type parameters, by count: their names are binders and never escape.
        arity: usize,
        ctors: Vec<CachedCtor>,
    },
    Effect {
        nondet: bool,
        ops: Vec<CachedOp>,
    },
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct CachedCtor {
    pub fields: Vec<Type>,
    pub scheme: Scheme,
}

/// The operation's name is stored beside its signature because normalization
/// sorts an effect's operations away — reordering them in source moves no
/// `DefHash`, so a restore that paired them by position would hand every
/// operation its neighbour's mode and signature.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct CachedOp {
    pub name: Symbol,
    pub mode: Mode,
    pub resource_param: bool,
    pub params: Vec<Type>,
    pub ret: Type,
}

pub(crate) type Sources = BTreeMap<String, SourceFingerprint>;

/// A `Vec` per hash rather than one entry, because two structurally identical
/// definitions in different modules *share* a `DefHash` — that is the design,
/// not a collision — while their interfaces still differ, a `Scheme` being
/// written in names a hash erases. With one slot per hash the second one
/// written evicts the first, and everything on the losing side of a shared
/// hash then fails its own witness check and is rechecked forever.
pub(crate) type Defs = BTreeMap<DefHash, Vec<CachedDef>>;
pub(crate) type Decls = BTreeMap<DefHash, Vec<CachedDecl>>;

/// The name an interface was written for: the one entry of its witness that
/// names something at the interface's own hash. `None` for an entry with no
/// witness, which is then the only entry its hash can hold.
pub fn self_name(names: &[NameRef], hash: DefHash) -> Option<&Symbol> {
    names.iter().find(|n| n.hash == hash).map(|n| &n.name)
}

pub fn declares(names: &[NameRef], name: &Symbol, hash: DefHash) -> bool {
    match self_name(names, hash) {
        Some(found) => found == name,
        None => names.is_empty(),
    }
}

/// Replaces the slot written for the same name, or appends. Keyed on the name
/// rather than on the whole witness so that re-storing a definition whose
/// witness grew does not leave the old one behind forever.
///
/// Returns whether anything actually changed, so that a run which re-derives
/// exactly what is already cached does not dirty it.
pub(crate) fn upsert<T: PartialEq>(
    slots: &mut Vec<T>,
    entry: T,
    hash: DefHash,
    names: impl Fn(&T) -> &[NameRef],
) -> bool {
    let key = self_name(names(&entry), hash).cloned();
    match slots.iter().position(|e| self_name(names(e), hash).cloned() == key) {
        Some(i) if slots[i] == entry => false,
        Some(i) => {
            slots[i] = entry;
            true
        }
        None => {
            slots.push(entry);
            true
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub(crate) struct Frontend {
    #[serde(default)]
    pub(crate) sources: Sources,
    #[serde(default)]
    pub(crate) defs: Defs,
    #[serde(default)]
    pub(crate) decls: Decls,
}

impl Frontend {
    pub(crate) fn is_empty(&self) -> bool {
        self.sources.is_empty() && self.defs.is_empty() && self.decls.is_empty()
    }
}

#[derive(Deserialize)]
struct FrontendFile {
    format: u32,
    frontend_version: String,
    #[serde(flatten)]
    frontend: Frontend,
}

#[derive(Serialize)]
struct FrontendFileRef<'a> {
    format: u32,
    frontend_version: &'a str,
    #[serde(flatten)]
    frontend: &'a Frontend,
}

pub(crate) fn load(path: &Path) -> Result<Frontend, LoadError> {
    Ok(load_with_digest(path)?.0)
}

pub(crate) fn load_with_digest(path: &Path) -> Result<(Frontend, ContentHash), LoadError> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == ErrorKind::NotFound => return Err(LoadError::Missing),
        Err(e) => return Err(LoadError::Io(e)),
    };
    let digest = ContentHash::of(text.as_bytes());
    let file: FrontendFile = serde_json::from_str(&text).map_err(LoadError::Parse)?;
    if file.format != FORMAT {
        return Err(LoadError::Format(file.format));
    }
    if file.frontend_version != crate::FRONTEND_VERSION {
        return Err(LoadError::Version(file.frontend_version));
    }
    Ok((file.frontend, digest))
}

/// The bytes on disk now, without parsing them. What tells a flush whether some
/// other process wrote since this one opened — parsing a multi-megabyte cache
/// only to discover it is unchanged is the largest cost in an edit-then-test
/// cycle at scale.
pub(crate) fn digest_of(path: &Path) -> Option<ContentHash> {
    fs::read(path).ok().map(|bytes| ContentHash::of(&bytes))
}

/// Returns the digest of what was written, so the caller need not read the file
/// back to learn it.
///
/// Compact rather than pretty: at ten thousand definitions the indentation is
/// more than half the file, and this file is rewritten whole every time one
/// definition changes.
pub(crate) fn save(dir: &Path, path: &Path, frontend: &Frontend) -> anyhow::Result<ContentHash> {
    let file = FrontendFileRef {
        format: FORMAT,
        frontend_version: crate::FRONTEND_VERSION,
        frontend,
    };
    let mut bytes = serde_json::to_vec(&file)
        .map_err(|e| anyhow::Error::new(e).context("could not serialize the front-end cache"))?;
    bytes.push(b'\n');
    disk::write_atomic(dir, path, FRONTEND_STEM, &bytes, "front-end cache")?;
    Ok(ContentHash::of(&bytes))
}

pub(crate) enum LoadError {
    Missing,
    Io(std::io::Error),
    Parse(serde_json::Error),
    Format(u32),
    Version(String),
}

impl LoadError {
    pub(crate) fn into_diagnostic(self, path: &Path) -> Diagnostic {
        let path = path.display();
        let d = match self {
            LoadError::Missing => Diagnostic::warning(
                crate::codes::CACHE_UNREADABLE,
                format!("no front-end cache at `{path}`"),
            ),
            LoadError::Io(e) => Diagnostic::warning(
                crate::codes::CACHE_UNREADABLE,
                format!("could not read the front-end cache `{path}`: {e}"),
            ),
            LoadError::Parse(e) => Diagnostic::warning(
                crate::codes::CACHE_CORRUPT,
                format!("the front-end cache `{path}` is corrupt: {e}"),
            ),
            LoadError::Format(found) => Diagnostic::warning(
                crate::codes::CACHE_CORRUPT,
                format!(
                    "the front-end cache `{path}` is format {found}, this build reads {FORMAT}"
                ),
            ),
            LoadError::Version(found) => Diagnostic::warning(
                crate::codes::CACHE_VERSION_CHANGED,
                format!(
                    "the front-end cache `{path}` was written by front end `{found}`, \
                     this build is `{}`",
                    crate::FRONTEND_VERSION
                ),
            ),
        };
        d.note(
            "continuing without it; every file is parsed and every definition rechecked, \
             and the cache is rewritten",
        )
    }
}

/// The cache key for a source file: its path relative to the store root, with
/// `/` separators.
///
/// Relative so that a cache survives the checkout moving, and so that
/// `ply test .` and `ply test /abs/path` agree. A path that cannot be expressed
/// that way — not UTF-8, or escaping the root — yields `None`, and a file
/// keyed `None` simply never takes the fast path.
pub(crate) fn source_key(root: &Path, path: &Path) -> Option<String> {
    use std::path::Component;
    let rel = path.strip_prefix(root).unwrap_or(path);
    let mut parts: Vec<&str> = Vec::new();
    for component in rel.components() {
        match component {
            Component::Normal(s) => parts.push(s.to_str()?),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}
