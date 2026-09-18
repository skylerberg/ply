//! The front end: the Rust chain that still has to run, and the port that answers for the rest.
//! Asking the port costs a whole front end, so it is asked once per load and the answer travels.

use crate::load::{Discovered, LoadError, Loaded, anchor, discover, unreadable};
use ply_hash::body::StoredBody;
use ply_hash::{DefHash, HashOutput};
use ply_span::{Diagnostic, SourceId, SourceMap, Span, Symbol, codes};
use ply_store::{
    CachedCtor, CachedDecl, CachedDef, CachedOp, CachedTest, ContentHash, DeclBody, DefBody,
    DefEntry, DefKind, FileSpan, Member, NameRef, SourceFingerprint, Store,
};
use ply_syntax::ast::{Module, ModuleName, Program};
use ply_syntax::resolve::resolve;
use ply_ty::Front;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Whether a run may consult the front-end cache.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Full,
    Incremental,
}

/// Where a front-end run's time went.
#[derive(Clone, Copy, Debug, Default)]
pub struct Phases {
    pub read: Duration,
    pub parse: Duration,
    pub resolve: Duration,
    /// The port's whole front end over this program, or its answer read back.
    pub front: Duration,
    pub write_back: Duration,
}

impl Phases {
    pub fn total(&self) -> Duration {
        self.read + self.parse + self.resolve + self.front + self.write_back
    }

    pub fn labelled(&self) -> [(&'static str, Duration); 5] {
        [
            ("read", self.read),
            ("parse", self.parse),
            ("resolve", self.resolve),
            ("front", self.front),
            ("write back", self.write_back),
        ]
    }
}

#[derive(Clone, Debug, Default)]
pub struct FrontEnd {
    pub incremental: bool,
    pub phases: Phases,
    pub warnings: Vec<Diagnostic>,
}

pub fn load_full(path: &Path) -> Result<Loaded, LoadError> {
    run(path, Mode::Full, None)
}

pub fn load_incremental(path: &Path, store: &mut Store) -> Result<Loaded, LoadError> {
    run(path, Mode::Incremental, Some(store))
}

pub fn run(path: &Path, mode: Mode, store: Option<&mut Store>) -> Result<Loaded, LoadError> {
    let (root, discovered) = discover(path).map_err(LoadError::bare)?;
    // Pruning deletes every fingerprint the run did not see, so it needs the whole project.
    let whole_project = std::fs::metadata(path).map(|m| m.is_dir()).unwrap_or(false);
    Driver::new(root, discovered, mode, store, whole_project)?.finish()
}

struct FileState {
    path: PathBuf,
    module: ModuleName,
    source: SourceId,
    text: Arc<str>,
    content: ContentHash,
    /// Taken when the program is assembled; nothing after that reads a tree through this.
    ast: Option<Module>,
    /// Embedded in the binary rather than discovered on disk.
    shipped: bool,
}

impl FileState {
    fn imports(&self) -> Vec<ModuleName> {
        match &self.ast {
            Some(ast) => ast.imports.iter().map(|i| i.module_name()).collect(),
            None => Vec::new(),
        }
    }
}

/// The port's answer as it gave it, under its key, when this run asked and may keep it.
type Fresh = Option<(ContentHash, String)>;

struct Driver<'s> {
    root: PathBuf,
    mode: Mode,
    store: Option<&'s mut Store>,
    whole_project: bool,
    sources: SourceMap,
    files: Vec<FileState>,
    by_module: BTreeMap<Symbol, usize>,
    phases: Phases,
}

fn timed<T>(slot: &mut Duration, f: impl FnOnce() -> T) -> T {
    let started = Instant::now();
    let value = f();
    *slot += started.elapsed();
    value
}

impl<'s> Driver<'s> {
    fn new(
        root: PathBuf,
        discovered: Vec<Discovered>,
        mode: Mode,
        store: Option<&'s mut Store>,
        whole_project: bool,
    ) -> Result<Driver<'s>, LoadError> {
        let mut phases = Phases::default();
        let mut sources = SourceMap::new();
        let mut diagnostics = Vec::new();
        let mut read = Vec::with_capacity(discovered.len());

        timed(&mut phases.read, || {
            for file in &discovered {
                match std::fs::read_to_string(&file.path) {
                    Ok(text) => {
                        let content = ContentHash::of(text.as_bytes());
                        let id = sources.add(&file.path, text);
                        read.push((id, content));
                    }
                    Err(e) => diagnostics.push(unreadable(&file.path, &e)),
                }
            }
        });
        if !diagnostics.is_empty() {
            return Err(LoadError {
                sources,
                diagnostics,
            });
        }

        // Checked with the text on hand, so an unusable path is reported against the file.
        let mut files = Vec::with_capacity(discovered.len());
        for (file, &(source, content)) in discovered.iter().zip(&read) {
            match ModuleName::from_relative_path(&file.relative) {
                Ok(module) if ply_std::is_reserved(module.as_str()) => {
                    let diagnostic = ply_std::reserved_diagnostic(&file.path, module.as_str());
                    diagnostics.push(anchor(diagnostic, &sources, source));
                }
                Ok(module) => files.push(FileState {
                    path: file.path.clone(),
                    module,
                    source,
                    text: sources
                        .get(source)
                        .map(|f| f.text.clone())
                        .unwrap_or_else(|| "".into()),
                    content,
                    ast: None,
                    shipped: false,
                }),
                Err(diagnostic) => diagnostics.push(anchor(diagnostic, &sources, source)),
            }
        }
        if !diagnostics.is_empty() {
            return Err(LoadError {
                sources,
                diagnostics,
            });
        }

        let mut by_module = BTreeMap::new();
        for (i, file) in files.iter().enumerate() {
            by_module.insert(file.module.as_symbol().clone(), i);
        }

        Ok(Driver {
            root,
            mode,
            store,
            whole_project,
            sources,
            files,
            by_module,
            phases,
        })
    }

    fn finish(mut self) -> Result<Loaded, LoadError> {
        self.parse_all()?;
        let (program, resolved) = self.assemble()?;
        let (front, fresh) = self.ask_the_port()?;
        if front.has_error() {
            return Err(LoadError {
                sources: self.sources.clone(),
                diagnostics: front.diagnostics,
            });
        }

        let stdlib = self.stdlib_notice(&front.hashes);
        let writing = Instant::now();
        let cache = self.write_back(&front, fresh);
        self.phases.write_back += writing.elapsed();

        let mut warnings = stdlib;
        warnings.extend(front.diagnostics.iter().cloned());
        warnings.extend(cache);

        let files = self.files.iter().map(|f| f.path.clone()).collect();
        // Whether the whole-program promise check has anything to check.
        let promised = front.defs_written.values().any(|w| w.reuse);
        Ok(Loaded {
            root: self.root,
            files,
            sources: self.sources,
            program,
            resolved,
            check: published_order(&front),
            hashes: front.hashes.clone(),
            front,
            frontend: FrontEnd {
                incremental: self.mode == Mode::Incremental,
                phases: self.phases,
                warnings,
            },
            promised,
        })
    }

    /// The port's answer, run or read back from the store, plus the answer to keep when fresh.
    /// Texts go in `self.files` order, so a span's module index reads back as its `SourceId`.
    fn ask_the_port(&mut self) -> Result<(Front, Fresh), LoadError> {
        ply_codegen::c::producer::ensure_default();
        let ids: Vec<SourceId> = self.files.iter().map(|f| f.source).collect();
        let started = Instant::now();
        let key = self.answer_key();
        let kept = key.and_then(|key| self.store.as_deref()?.front_answer(key));
        if let Some(front) = kept.and_then(|dump| ply_ty::read_front(&dump, &ids).ok()) {
            self.phases.front += started.elapsed();
            return Ok((front, None));
        }
        let sources: Vec<(String, String)> = self
            .files
            .iter()
            .map(|f| (f.module.to_string(), f.text.to_string()))
            .collect();
        let dump = ply_codegen::c::producer::front_dump(&sources);
        self.phases.front += started.elapsed();
        let dump = dump.map_err(|e| self.seam_failed(&format!("{e:#}")))?;
        let front = ply_ty::read_front(&dump, &ids)
            .map_err(|e| self.seam_failed(&format!("the front end's answer does not read: {e}")))?;
        let fresh = key.filter(|_| !front.has_error()).map(|key| (key, dump));
        Ok((front, fresh))
    }

    /// The emitter plus each module's name and text, in handover order; `None` without a cache.
    fn answer_key(&self) -> Option<ContentHash> {
        if self.mode != Mode::Incremental || self.store.is_none() {
            return None;
        }
        let mut key = ply_codegen::c::producer::emitter().into_bytes();
        for file in &self.files {
            key.push(0);
            key.extend_from_slice(file.module.as_str().as_bytes());
            key.push(0);
            key.extend_from_slice(&file.content.0);
        }
        Some(ContentHash::of(&key))
    }

    /// This compiler failing, rather than the program.
    fn seam_failed(&self, why: &str) -> LoadError {
        LoadError {
            sources: self.sources.clone(),
            diagnostics: vec![
                Diagnostic::error(
                    codes::INTERNAL_ERROR,
                    format!("the front end could not answer for this program: {why}"),
                )
                .primary(Span::DUMMY, "nothing was checked, so nothing is claimed")
                .note("this is Ply's fault: the compiler's own front end is what failed here")
                .note("the emitter comes from `crates/ply-compiler/bootstrap`; sources with no bundle are emitted by the one this binary carries"),
            ],
        }
    }

    /// The trees are moved out of the files: the port reads text, and nothing reads a tree again.
    fn assemble(&mut self) -> Result<(Program, ply_syntax::resolve::Resolved), LoadError> {
        let modules: Vec<Module> = self.files.iter_mut().filter_map(|f| f.ast.take()).collect();
        // Mutable because `resolve` fills defaults and places named arguments.
        let mut program = Program { modules };
        let resolved =
            timed(&mut self.phases.resolve, || resolve(&mut program)).map_err(|diagnostics| {
                LoadError {
                    sources: self.sources.clone(),
                    diagnostics,
                }
            })?;
        Ok((program, resolved))
    }

    /// To a fixed point, because a shipped module may import another.
    fn parse_all(&mut self) -> Result<(), LoadError> {
        loop {
            self.parse_pending()?;
            if !self.pull_stdlib()? {
                return Ok(());
            }
        }
    }

    /// Shipped modules are pulled in only when something imports them, transitively.
    fn pull_stdlib(&mut self) -> Result<bool, LoadError> {
        let mut diagnostics = Vec::new();
        let mut wanted: BTreeSet<Symbol> = BTreeSet::new();

        for file in &self.files {
            for imported in file.imports() {
                // A shipped module may import only `std.*`.
                if file.shipped && !ply_std::is_std(&imported) {
                    diagnostics.push(self.foreign_import(file, &imported));
                    continue;
                }
                if !ply_std::is_std(&imported) || self.by_module.contains_key(imported.as_symbol())
                {
                    continue;
                }
                match (ply_std::source(&imported).is_some(), file.shipped) {
                    (true, _) => {
                        wanted.insert(imported.as_symbol().clone());
                    }
                    (false, true) => diagnostics.push(self.foreign_import(file, &imported)),
                    (false, false) => diagnostics.push(self.unknown_std(file, &imported)),
                }
            }
        }
        if !diagnostics.is_empty() {
            return Err(LoadError {
                sources: self.sources.clone(),
                diagnostics,
            });
        }

        let added = !wanted.is_empty();
        for name in wanted {
            self.add_shipped(ModuleName::from_dotted(name.as_str()));
        }
        Ok(added)
    }

    /// Filed under its pseudo-path, so the store keys its fingerprint like any file's.
    fn add_shipped(&mut self, module: ModuleName) {
        let Some(source) = ply_std::source(&module) else {
            return;
        };
        let path = ply_std::pseudo_path(&module);
        let id = self.sources.add(&path, source.to_string());
        let text = self
            .sources
            .get(id)
            .map(|f| f.text.clone())
            .unwrap_or_else(|| "".into());
        self.by_module
            .insert(module.as_symbol().clone(), self.files.len());
        self.files.push(FileState {
            ast: None,
            path,
            module,
            source: id,
            text,
            content: ContentHash::of(source.as_bytes()),
            shipped: true,
        });
    }

    fn import_span(&self, file: &FileState, imported: &ModuleName) -> Span {
        file.ast
            .as_ref()
            .and_then(|ast| {
                ast.imports
                    .iter()
                    .find(|i| &i.module_name() == imported)
                    .map(|i| i.path_span())
            })
            .unwrap_or(Span::DUMMY)
    }

    fn unknown_std(&self, file: &FileState, imported: &ModuleName) -> Diagnostic {
        let span = self.import_span(file, imported);
        anchor(
            ply_std::unknown_module(imported, span),
            &self.sources,
            file.source,
        )
    }

    fn foreign_import(&self, file: &FileState, imported: &ModuleName) -> Diagnostic {
        let span = self.import_span(file, imported);
        anchor(
            ply_std::foreign_import(&file.module, imported.as_symbol(), span),
            &self.sources,
            file.source,
        )
    }

    fn parse_pending(&mut self) -> Result<(), LoadError> {
        let mut diagnostics = Vec::new();
        let files = &mut self.files;
        timed(&mut self.phases.parse, || {
            for file in files {
                if file.ast.is_some() {
                    continue;
                }
                match ply_syntax::parse_module(file.source, file.module.clone(), &file.text) {
                    // Expansion reads only this file's own type declarations.
                    Ok(mut module) => {
                        diagnostics.append(&mut ply_derive::expand_module(&mut module));
                        file.ast = Some(module);
                    }
                    Err(mut d) => diagnostics.append(&mut d),
                }
            }
        });
        if diagnostics.is_empty() {
            Ok(())
        } else {
            Err(LoadError {
                sources: self.sources.clone(),
                diagnostics,
            })
        }
    }

    /// What a compiler upgrade did to this project, said once.
    fn stdlib_notice(&self, hashes: &HashOutput) -> Vec<Diagnostic> {
        if self.mode != Mode::Incremental {
            return Vec::new();
        }
        let Some(store) = self.store.as_deref() else {
            return Vec::new();
        };
        let current = ply_std::digest_short();
        let Some(previous) = store.stdlib_digest() else {
            return Vec::new();
        };
        if previous == current {
            return Vec::new();
        }

        let mut moved: BTreeSet<Symbol> = BTreeSet::new();
        for path in store.source_paths() {
            if !ply_std::is_pseudo_path(&path) {
                continue;
            }
            let Some(fingerprint) = store.fingerprint(&path) else {
                continue;
            };
            for entry in &fingerprint.defs {
                let now = hashes
                    .defs
                    .get(&entry.name)
                    .or_else(|| hashes.decls.get(&entry.name));
                if now != Some(&entry.hash) {
                    moved.insert(entry.name.clone());
                }
            }
        }

        let reached = hashes
            .defs
            .keys()
            .chain(hashes.decls.keys())
            .filter(|name| {
                hashes
                    .closure
                    .get(*name)
                    .is_some_and(|closure| closure.iter().any(|n| moved.contains(n)))
            })
            .count();

        let what = match reached {
            0 => "no definition this program reaches changed".to_string(),
            1 => "1 definition this program reaches changed".to_string(),
            n => format!("{n} definitions this program reaches changed"),
        };
        vec![
            Diagnostic::warning(
                codes::STDLIB_CHANGED,
                format!("the modules that ship with `ply` moved: {previous} -> {current}"),
            )
            .note(what)
            .note("a `std` definition hashes like any other, so only what a change reached re-runs")
            .note("`ply std` lists the shipped modules and this digest"),
        ]
    }

    fn write_back(&mut self, front: &Front, fresh: Fresh) -> Vec<Diagnostic> {
        if self.mode != Mode::Incremental {
            return Vec::new();
        }
        let witnesses = witnesses(&front.hashes);
        let paths: Vec<PathBuf> = self.files.iter().map(|f| f.path.clone()).collect();
        let whole_project = self.whole_project;

        let fingerprints: Vec<(usize, SourceFingerprint)> = (0..self.files.len())
            .filter_map(|i| self.fingerprint_of(i, front).map(|f| (i, f)))
            .collect();
        let interfaces = interfaces(front, &witnesses);
        let bodies = stored_bodies(front);

        let Some(store) = self.store.as_deref_mut() else {
            return Vec::new();
        };
        if let Some((key, dump)) = fresh {
            store.put_front_answer(key, dump);
        }
        for (hash, entry) in interfaces {
            match entry {
                Interface::Def(def) => store.put_def(hash, def),
                Interface::Decl(decl) => store.put_decl(hash, decl),
            }
        }
        for (hash, body) in bodies {
            store.put_body(hash, body);
        }
        for (i, fingerprint) in fingerprints {
            store.put_source(&paths[i], fingerprint);
        }
        // A `std` module no longer imported is pruned like any file that left the program.
        if whole_project {
            store.prune(&paths);
        }
        store.set_stdlib_digest(ply_std::digest_short());
        match store.flush() {
            Ok(()) => Vec::new(),
            // A flush writes both caches, so naming the one that failed would be a guess.
            Err(e) => vec![
                Diagnostic::warning(
                    codes::CACHE_UNREADABLE,
                    format!("could not update the cache: {e:#}"),
                )
                .note("this run is unaffected; the next one will do this work again"),
            ],
        }
    }

    fn fingerprint_of(&self, i: usize, front: &Front) -> Option<SourceFingerprint> {
        let file = &self.files[i];
        let info = front.check.modules.get(file.module.as_symbol())?;
        let hashes = &front.hashes;
        let mut fingerprint = SourceFingerprint::new(file.content);

        // A name in two namespaces is in `items` twice and gets one entry per namespace.
        let mut seen: BTreeSet<&Symbol> = BTreeSet::new();
        for name in &info.items {
            if seen.insert(name) {
                fingerprint.defs.extend(def_entries(front, name));
            }
        }

        for (index, test) in front
            .check
            .tests
            .iter()
            .enumerate()
            .filter(|(_, t)| t.module == file.module)
        {
            fingerprint.tests.push(CachedTest {
                name: test.name.clone(),
                hash: *hashes.tests.get(index)?,
                nondet: test.nondet,
                footprint: test.footprint.clone(),
                span: FileSpan::of(test.span),
            });
        }
        Some(fingerprint)
    }
}

enum Interface {
    Def(CachedDef),
    Decl(CachedDecl),
}

/// Files in load order, then items as written; the port answers dependency-first.
fn published_order(front: &Front) -> ply_ty::CheckOutput {
    let mut check = front.check.clone();
    let mut defs = indexmap::IndexMap::with_capacity(check.defs.len());
    for (_, items) in &front.ordinals {
        for item in items {
            if let ply_ty::Ordinal::Fn(name, _) = item
                && let Some(info) = check.defs.shift_remove(name)
            {
                defs.insert(name.clone(), info);
            }
        }
    }
    defs.extend(check.defs.drain(..));
    check.defs = defs;
    check
}

/// Two entries when the source spells one name in two namespaces.
fn def_entries(front: &Front, name: &Symbol) -> Vec<DefEntry> {
    let hashes = &front.hashes;
    let mut out = Vec::new();
    let mut entry = |kind: DefKind, hash: DefHash, span: Span, members: Vec<Member>| {
        out.push(DefEntry {
            name: name.clone(),
            hash,
            span: FileSpan::of(span),
            kind,
            members,
        });
    };

    if let (Some(def), Some(&hash)) = (front.check.defs.get(name), hashes.defs.get(name)) {
        entry(DefKind::Fn, hash, def.span, Vec::new());
    }
    if let (Some(ty), Some(&hash)) = (front.types.get(name), hashes.decls.get(name)) {
        entry(DefKind::Type, hash, ty.span, ctor_members(front, name));
    }
    if let (Some(effect), Some(&hash)) = (front.check.effects.get(name), hashes.decls.get(name)) {
        let ops = effect
            .ops
            .values()
            .map(|op| Member {
                name: op.name.clone(),
                span: FileSpan::of(op.span),
            })
            .collect();
        entry(DefKind::Effect, hash, effect.span, ops);
    }
    out
}

/// A sum type's constructors in declaration order; an alias has none.
fn ctors_of<'a>(front: &'a Front, type_name: &Symbol) -> Vec<&'a ply_ty::CtorInfo> {
    let mut out: Vec<&ply_ty::CtorInfo> = front
        .check
        .ctors
        .values()
        .filter(|c| &c.type_name == type_name)
        .collect();
    out.sort_by_key(|c| c.index);
    out
}

fn ctor_members(front: &Front, type_name: &Symbol) -> Vec<Member> {
    ctors_of(front, type_name)
        .into_iter()
        .map(|c| Member {
            name: c.simple_name.clone(),
            span: FileSpan::of(c.span),
        })
        .collect()
}

/// Slots are keyed by the witnessed name, so definitions sharing a hash keep their own entries.
fn witnesses(hashes: &HashOutput) -> BTreeMap<Symbol, Vec<NameRef>> {
    let mut out = BTreeMap::new();
    let named = |name: &Symbol| -> Option<NameRef> {
        hashes
            .defs
            .get(name)
            .or_else(|| hashes.decls.get(name))
            .map(|hash| NameRef::new(name.clone(), *hash))
    };
    let is_decl = |name: &Symbol| hashes.decls.contains_key(name);

    for (name, hash) in hashes.defs.iter().chain(hashes.decls.iter()) {
        let mut witness = vec![NameRef::new(name.clone(), *hash)];
        if let Some(deps) = hashes.deps.get(name) {
            witness.extend(deps.iter().filter(|d| is_decl(d)).filter_map(&named));
        }
        if let Some(closure) = hashes.closure.get(name) {
            witness.extend(closure.iter().filter(|d| is_decl(d)).filter_map(&named));
        }
        out.insert(name.clone(), witness);
    }
    out
}

fn interfaces(
    front: &Front,
    witnesses: &BTreeMap<Symbol, Vec<NameRef>>,
) -> Vec<(DefHash, Interface)> {
    let hashes = &front.hashes;
    let mut out = Vec::new();

    for (name, d) in &front.check.defs {
        let (Some(&hash), Some(names)) = (hashes.defs.get(name), witnesses.get(name)) else {
            continue;
        };
        out.push((
            hash,
            Interface::Def(
                CachedDef::new(d.scheme.clone(), d.footprint.clone()).witnessed_by(names.clone()),
            ),
        ));
    }

    for (name, t) in &front.types {
        let (Some(&hash), Some(names)) = (hashes.decls.get(name), witnesses.get(name)) else {
            continue;
        };
        let ctors = ctors_of(front, name)
            .into_iter()
            .map(|c| CachedCtor {
                fields: c.fields.clone(),
                scheme: c.scheme.clone(),
            })
            .collect();
        out.push((
            hash,
            Interface::Decl(
                CachedDecl::new(DeclBody::Type {
                    arity: t.arity,
                    ctors,
                })
                .witnessed_by(names.clone()),
            ),
        ));
    }

    // A prelude effect is declared by no source, so it has no `decls` entry.
    for (name, e) in &front.check.effects {
        let (Some(&hash), Some(names)) = (hashes.decls.get(name), witnesses.get(name)) else {
            continue;
        };
        let ops = e
            .ops
            .values()
            .map(|o| CachedOp {
                name: o.name.clone(),
                mode: o.mode,
                resource_param: o.resource_param,
                params: o.params.clone(),
                ret: o.ret.clone(),
            })
            .collect();
        out.push((
            hash,
            Interface::Decl(
                CachedDecl::new(DeclBody::Effect {
                    nondet: e.nondet,
                    ops,
                })
                .witnessed_by(names.clone()),
            ),
        ));
    }
    out
}

/// A name in two namespaces has two bodies, so each body is matched to its hash, not assumed.
fn stored_bodies(front: &Front) -> Vec<(DefHash, DefBody)> {
    let hashes = &front.hashes;
    let mut by_name: BTreeMap<&Symbol, Vec<StoredBody>> = BTreeMap::new();
    for (name, bytes) in &front.bodies {
        if let Some(body) = StoredBody::from_bytes(bytes.clone()) {
            by_name.entry(name).or_default().push(body);
        }
    }
    let mut out = Vec::new();
    for (name, stored) in by_name {
        for hash in [hashes.defs.get(name), hashes.decls.get(name)]
            .into_iter()
            .flatten()
        {
            let found = match stored.as_slice() {
                [only] => Some(only),
                many => many.iter().find(|b| b.verify(*hash)),
            };
            if let Some(body) = found {
                out.push((*hash, DefBody::of(body.clone())));
            }
        }
    }
    out
}
