//! The incremental front end: two gates decide how much of a run has to be
//! redone.
//!
//! **Gate 1** is a file-level gate keyed on the file's raw bytes. A file whose
//! bytes are unchanged and whose every free name still denotes the same
//! `DefHash` is never parsed at all; its definitions' types, footprints and
//! hashes are read back out of the store.
//!
//! **Gate 2** is a definition-level gate keyed on `DefHash`. Because a reference
//! normalizes to the referent's hash rather than its name, a definition whose
//! dependency changed already has a different hash, so one condition covers both
//! "its own form changed" and "something it calls changed".
//!
//! The two keys differ because of when each gate runs: a `DefHash` cannot be
//! computed without parsing, and gate 1 has to decide whether to parse. Gate 1
//! is therefore conservative about formatting and gate 2 is exact.
//!
//! Everything here exists to make one thing impossible: a definition being
//! handed a type that is no longer what a from-scratch check would produce. A
//! stale *result* costs a test that did not need to run; a stale *type* corrupts
//! the hashes every other cache is keyed on. Where the two gates cannot decide
//! safely they refuse, and refusing only ever costs time.

use crate::load::{Discovered, LoadError, Loaded, anchor, discover, unreadable};
use indexmap::IndexMap;
use ply_core::{
    CheckOutput, CtorInfo, DefInfo, EffectInfo, Footprint, Known, KnownDef, KnownTest, ModuleInfo,
    OpInfo, TestInfo, check_program_with,
};
use ply_hash::graph::NodeId;
use ply_hash::{DefHash, HashOutput, hash_program_ast};
use ply_span::{Diagnostic, SourceId, SourceMap, Span, Symbol, codes};
use ply_syntax::ast::{Item, Module, ModuleName, Program, TypeDefBody, Visibility};
use ply_syntax::resolve::resolve;
use ply_store::{
    CachedCtor, CachedDecl, CachedDef, CachedOp, CachedTest, ContentHash, DeclBody, DefEntry,
    DefKind, FileSpan, ImportEdge, Member, NameRef, SourceFingerprint, Store,
    canonicalize_decl_body, canonicalize_scheme, exports_digest, witness_holds,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Whether a run may consult the front-end cache. `Full` is what
/// `--no-incremental` selects: every file is parsed, every definition is
/// rechecked, and nothing is read from or written to the front-end cache.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Full,
    Incremental,
}

/// Why a file could not take the fast path. Reported verbatim by `--explain`,
/// because "it was slow again and I do not know why" is the failure mode an
/// incremental front end is most likely to produce.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Refusal {
    None,
    NotIncremental,
    NoFingerprint,
    ContentChanged,
    Dependency(Symbol),
    Import(Symbol),
    /// The fingerprint survived but the interface it points at did not.
    InterfaceMissing,
    /// A name this file reaches lost its `pub`. Normalization erases visibility
    /// — deliberately, so that adding or removing `pub` moves no hash — so no
    /// other condition in either gate can see the change, and only resolving
    /// the reference again reports it.
    Private(Symbol),
    /// Something that had to be parsed imports this file, so its interface has
    /// to be derived rather than restored.
    ImportedByParsed(Symbol),
    /// A test that has to run lives here, or is reachable from one. Evaluation
    /// needs a body, and a body only exists in an AST.
    NeededToEvaluate,
}

impl Refusal {
    pub fn describe(&self) -> String {
        match self {
            Refusal::None => "unchanged".to_string(),
            Refusal::NotIncremental => "--no-incremental".to_string(),
            Refusal::NoFingerprint => "no fingerprint".to_string(),
            Refusal::ContentChanged => "content changed".to_string(),
            Refusal::Dependency(name) => format!("dependency `{name}` changed"),
            Refusal::Import(module) => format!("import `{module}` changed"),
            Refusal::InterfaceMissing => "cached interface missing".to_string(),
            Refusal::Private(name) => format!("`{name}` is no longer public"),
            Refusal::ImportedByParsed(m) => format!("imported by `{m}`, which was parsed"),
            Refusal::NeededToEvaluate => "a selected test needs its body".to_string(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct FileReport {
    pub path: PathBuf,
    pub module: ModuleName,
    pub parsed: bool,
    pub rechecked: bool,
    pub refusal: Refusal,
}

#[derive(Clone, Debug)]
pub struct DefReport {
    pub name: Symbol,
    pub cached: bool,
}

/// Where a front-end run's time went. Reported rather than inferred, because
/// "the gates fired" and "the run got faster" are different claims and only the
/// second one is the point.
#[derive(Clone, Copy, Debug, Default)]
pub struct Phases {
    pub read: Duration,
    pub parse: Duration,
    pub resolve: Duration,
    pub hash: Duration,
    pub check: Duration,
    /// Rebuilding types and spans from the store for everything not rechecked.
    pub restore: Duration,
    pub write_back: Duration,
}

impl Phases {
    pub fn total(&self) -> Duration {
        self.read + self.parse + self.resolve + self.hash + self.check + self.restore
            + self.write_back
    }

    pub fn labelled(&self) -> [(&'static str, Duration); 7] {
        [
            ("read", self.read),
            ("parse", self.parse),
            ("resolve", self.resolve),
            ("hash", self.hash),
            ("check", self.check),
            ("restore", self.restore),
            ("write back", self.write_back),
        ]
    }
}

/// What the two gates decided, for `--explain` and for the tests that assert the
/// gates actually fired.
#[derive(Clone, Debug, Default)]
pub struct FrontEnd {
    pub incremental: bool,
    pub files: Vec<FileReport>,
    pub defs: Vec<DefReport>,
    pub phases: Phases,
    /// Trouble the cache took on the way out. Empty in the normal case; a caller
    /// that never reports these turns an unwritable cache into a program that is
    /// mysteriously slow forever.
    pub warnings: Vec<Diagnostic>,
}

impl FrontEnd {
    pub fn parsed(&self) -> usize {
        self.files.iter().filter(|f| f.parsed).count()
    }

    pub fn skipped(&self) -> usize {
        self.files.iter().filter(|f| !f.parsed).count()
    }

    pub fn rechecked(&self) -> usize {
        self.defs.iter().filter(|d| !d.cached).count()
    }

    pub fn cached(&self) -> usize {
        self.defs.iter().filter(|d| d.cached).count()
    }
}

pub fn load_full(path: &Path) -> Result<Loaded, LoadError> {
    run(path, Mode::Full, None, &[])
}

/// The incremental path. `store` is both the source of cached interfaces and
/// where this run's are written; passing `None` is exactly `--no-incremental`.
pub fn load_incremental(path: &Path, store: &mut Store) -> Result<Loaded, LoadError> {
    run(path, Mode::Incremental, Some(store), &[])
}

/// The incremental path, with `needed` and everything they import parsed
/// whatever the gates would have said.
///
/// Evaluating a test needs its body, and gate 1 skips a file without producing
/// one. Naming the modules a run must actually execute keeps the rest of the
/// project on the fast path — the alternative, reparsing and rechecking
/// everything the moment one test is selected, costs the whole cache exactly
/// when it is most valuable.
pub fn load_to_evaluate(
    path: &Path,
    store: &mut Store,
    needed: &[ModuleName],
) -> Result<Loaded, LoadError> {
    run(path, Mode::Incremental, Some(store), needed)
}

pub fn run(
    path: &Path,
    mode: Mode,
    store: Option<&mut Store>,
    needed: &[ModuleName],
) -> Result<Loaded, LoadError> {
    let (root, discovered) = discover(path).map_err(LoadError::bare)?;
    // Pruning deletes every fingerprint the run did not see, which is only
    // correct when the run saw everything. `ply check one.ply` did not.
    let whole_project = std::fs::metadata(path).map(|m| m.is_dir()).unwrap_or(false);
    let needed = needed.iter().map(|m| m.as_symbol().clone()).collect();
    Driver::new(root, discovered, mode, store, whole_project, needed)?.finish()
}

struct FileState {
    path: PathBuf,
    module: ModuleName,
    source: SourceId,
    text: Arc<str>,
    content: ContentHash,
    fingerprint: Option<SourceFingerprint>,
    ast: Option<Module>,
    parse: bool,
    recheck: bool,
    refusal: Refusal,
}

impl FileState {
    /// The definitions this file publishes, as gate 1 knows them without a
    /// parse. Only meaningful while the file is a skip candidate.
    fn cached_defs(&self) -> &[DefEntry] {
        self.fingerprint.as_ref().map(|f| f.defs.as_slice()).unwrap_or(&[])
    }
}

struct Driver<'s> {
    root: PathBuf,
    mode: Mode,
    store: Option<&'s mut Store>,
    whole_project: bool,
    needed: BTreeSet<Symbol>,
    sources: SourceMap,
    files: Vec<FileState>,
    by_module: IndexMap<Symbol, usize>,
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
        needed: BTreeSet<Symbol>,
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
            return Err(LoadError { sources, diagnostics });
        }

        // Naming is checked with the text already on hand so an unusable path is
        // reported against the file itself rather than against nowhere.
        let mut files = Vec::with_capacity(discovered.len());
        for (file, &(source, content)) in discovered.iter().zip(&read) {
            match ModuleName::from_relative_path(&file.relative) {
                Ok(module) => files.push(FileState {
                    path: file.path.clone(),
                    module,
                    source,
                    text: sources.get(source).map(|f| f.text.clone()).unwrap_or_else(|| "".into()),
                    content,
                    fingerprint: None,
                    ast: None,
                    parse: false,
                    recheck: false,
                    refusal: Refusal::None,
                }),
                Err(diagnostic) => diagnostics.push(anchor(diagnostic, &sources, source)),
            }
        }
        if !diagnostics.is_empty() {
            return Err(LoadError { sources, diagnostics });
        }

        let mut by_module = IndexMap::new();
        for (i, file) in files.iter().enumerate() {
            by_module.insert(file.module.as_symbol().clone(), i);
        }

        let mut driver =
            Driver { root, mode, store, whole_project, needed, sources, files, by_module, phases };
        driver.load_fingerprints();
        Ok(driver)
    }

    fn load_fingerprints(&mut self) {
        let Some(store) = self.store.as_deref() else { return };
        if self.mode == Mode::Full {
            return;
        }
        for file in &mut self.files {
            file.fingerprint = store.source(&file.path).cloned();
        }
    }

    /// Gate 1's first condition, plus the two reasons a run can have that have
    /// nothing to do with whether the file changed.
    fn forced(&mut self) {
        for file in &mut self.files {
            file.refusal = match &file.fingerprint {
                _ if self.mode == Mode::Full => Refusal::NotIncremental,
                _ if self.needed.contains(file.module.as_symbol()) => Refusal::NeededToEvaluate,
                None => Refusal::NoFingerprint,
                Some(f) if f.content_hash != file.content => Refusal::ContentChanged,
                Some(_) => Refusal::None,
            };
            file.parse = file.refusal != Refusal::None;
        }
    }

    fn finish(mut self) -> Result<Loaded, LoadError> {
        self.forced();

        // Gate 1 runs to a fixed point: parsing a file can change what its
        // importers see, and refusing a file can pull in the files it imports.
        // Each round can only add to the parse set, so it terminates.
        let (program, resolved, hashes) = loop {
            self.close_over_imports()?;
            let (program, resolved, hashes) = self.parse_and_hash()?;
            let table = self.hash_table(&hashes);
            if !self.refuse_candidates(&table) {
                break (program, resolved, hashes);
            }
        };

        let gate_two = self.decide_rechecks(&hashes);
        let started = Instant::now();
        let check = check_program_with(&program, &resolved, &gate_two.known)
            .map_err(|diagnostics| LoadError { sources: self.sources.clone(), diagnostics })?;
        self.phases.check += started.elapsed();
        self.merge(program, resolved, hashes, check, gate_two.cached)
    }

    /// A parsed module's imports must be parsed too: `resolve` needs every
    /// module a reference can name, and inference needs the imported
    /// definitions' types.
    fn close_over_imports(&mut self) -> Result<(), LoadError> {
        loop {
            self.parse_pending()?;
            let mut added = false;
            for i in 0..self.files.len() {
                if !self.files[i].parse {
                    continue;
                }
                let Some(ast) = &self.files[i].ast else { continue };
                let imports: Vec<Symbol> = ast
                    .imports
                    .iter()
                    .map(|d| d.module_name().as_symbol().clone())
                    .collect();
                let importer = self.files[i].module.as_symbol().clone();
                for name in imports {
                    let Some(&j) = self.by_module.get(&name) else { continue };
                    if !self.files[j].parse {
                        self.files[j].parse = true;
                        self.files[j].refusal = Refusal::ImportedByParsed(importer.clone());
                        added = true;
                    }
                }
            }
            if !added {
                return Ok(());
            }
        }
    }

    fn parse_pending(&mut self) -> Result<(), LoadError> {
        let mut diagnostics = Vec::new();
        let files = &mut self.files;
        timed(&mut self.phases.parse, || {
            for file in files {
                if !file.parse || file.ast.is_some() {
                    continue;
                }
                match ply_syntax::parse_module(file.source, file.module.clone(), &file.text) {
                    Ok(module) => file.ast = Some(module),
                    Err(mut d) => diagnostics.append(&mut d),
                }
            }
        });
        if diagnostics.is_empty() {
            Ok(())
        } else {
            Err(LoadError { sources: self.sources.clone(), diagnostics })
        }
    }

    fn parse_and_hash(&mut self) -> Result<(Program, ply_syntax::resolve::Resolved, HashOutput), LoadError> {
        let modules: Vec<Module> =
            self.files.iter().filter_map(|f| f.ast.clone()).collect();
        let program = Program { modules };
        let resolved = timed(&mut self.phases.resolve, || resolve(&program))
            .map_err(|diagnostics| LoadError { sources: self.sources.clone(), diagnostics })?;
        let hashes = timed(&mut self.phases.hash, || hash_program_ast(&program, &resolved))
            .map_err(|diagnostics| LoadError { sources: self.sources.clone(), diagnostics })?;
        Ok((program, resolved, hashes))
    }

    /// Program-wide name -> current hash, over parsed and skipped files alike.
    /// A skipped file's entries come from its fingerprint, which is only ever
    /// trusted after its content hash matched the bytes on disk.
    fn hash_table(&self, hashes: &HashOutput) -> BTreeMap<Symbol, DefHash> {
        let mut table: BTreeMap<Symbol, DefHash> = BTreeMap::new();
        for (name, hash) in hashes.defs.iter().chain(hashes.decls.iter()) {
            table.insert(name.clone(), *hash);
        }
        for file in &self.files {
            if file.parse {
                continue;
            }
            for entry in file.cached_defs() {
                table.insert(entry.name.clone(), entry.hash);
            }
        }
        table
    }

    /// Gate 1's second condition, evaluated for every file still hoping to skip.
    /// Returns whether any file was demoted, which means the round has to run
    /// again with a larger parse set.
    fn refuse_candidates(&mut self, table: &BTreeMap<Symbol, DefHash>) -> bool {
        let mut demoted = false;
        let private = self.private_names();
        for i in 0..self.files.len() {
            if self.files[i].parse {
                continue;
            }
            if let Some(refusal) = self.gate_one_refusal(i, table, &private) {
                self.files[i].parse = true;
                self.files[i].refusal = refusal;
                demoted = true;
            }
        }
        demoted
    }

    /// Qualified names a *parsed* module declares without `pub`. A skipped
    /// module's bytes are unchanged, so nothing it declares can have changed
    /// visibility, which is why only parsed files are consulted.
    fn private_names(&self) -> BTreeSet<Symbol> {
        let mut out = BTreeSet::new();
        for file in &self.files {
            let Some(ast) = &file.ast else { continue };
            for item in &ast.items {
                let Some(ident) = item.name() else { continue };
                if item.visibility() == Visibility::Private {
                    out.insert(file.module.qualify(&ident.name));
                }
            }
        }
        out
    }

    fn gate_one_refusal(
        &self,
        i: usize,
        table: &BTreeMap<Symbol, DefHash>,
        private: &BTreeSet<Symbol>,
    ) -> Option<Refusal> {
        let file = &self.files[i];
        // Written to fail closed. `None` here means "skipping is fine", and
        // every condition below has to reach a decision on evidence rather than
        // on the absence of it.
        let Some(fingerprint) = file.fingerprint.as_ref() else {
            return Some(Refusal::NoFingerprint);
        };
        let Some(store) = self.store.as_deref() else {
            return Some(Refusal::NotIncremental);
        };

        // A module this file imports may have been deleted outright, in which
        // case nothing about this file's own bytes would reveal it.
        for edge in &fingerprint.imports {
            if !self.by_module.contains_key(&edge.module) {
                return Some(Refusal::Import(edge.module.clone()));
            }
        }

        // The exact condition: every free name still denotes what it denoted.
        // A definition deleted, moved to another module, or renamed all land
        // here, and so does a dependency whose body changed.
        for dep in &fingerprint.deps {
            if table.get(&dep.name) != Some(&dep.hash) {
                return Some(Refusal::Dependency(dep.name.clone()));
            }
            // Every entry here crossed a module boundary to get in, so every one
            // of them had to be `pub` for this file to have compiled.
            if private.contains(&dep.name) {
                return Some(Refusal::Private(dep.name.clone()));
            }
        }

        let resolve = |name: &Symbol| table.get(name).copied();
        for entry in &fingerprint.defs {
            // Asked for by name, not by hash alone: several definitions can share
            // a hash, and a `Scheme` written in another one's names is not this
            // one's interface.
            let names = match entry.kind {
                DefKind::Fn => store.cached_def_of(entry.hash, &entry.name).map(|d| d.names.as_slice()),
                _ => store.cached_decl_of(entry.hash, &entry.name).map(|d| d.names.as_slice()),
            };
            let Some(names) = names else {
                return Some(Refusal::InterfaceMissing);
            };
            if !witness_holds(names, resolve) {
                return Some(Refusal::InterfaceMissing);
            }
        }
        None
    }

    /// Gate 2, decided one definition at a time. A parsed definition whose
    /// `DefHash` is already in the store, under a witness this run would write
    /// again, is handed to inference as a finished interface rather than
    /// re-inferred. That is the compile-once mechanism: a module that imports a
    /// definition being rechecked is *not* itself rechecked, because it takes
    /// that definition's type from the store instead of from a fresh walk.
    ///
    /// `type` and `effect` declarations are always re-derived. A declaration's
    /// signature comes from its own text and reaches no body, so deriving it
    /// costs less than looking it up and checking a witness — but the report
    /// still says `cached` when nothing about it moved, which is the question
    /// `--explain` is asking.
    fn decide_rechecks(&mut self, hashes: &HashOutput) -> GateTwo {
        let witnesses = self.witnesses(hashes);
        let private = self.private_names();
        let table = self.hash_table(hashes);
        let mut gate = GateTwo::default();
        let mut rechecked = vec![false; self.files.len()];

        for (i, flag) in rechecked.iter_mut().enumerate() {
            if !self.files[i].parse {
                continue;
            }
            *flag = self.gather(i, hashes, &witnesses, &table, &private, &mut gate);
        }
        for (i, flag) in rechecked.into_iter().enumerate() {
            self.files[i].recheck = flag;
        }
        gate
    }

    /// Fills in what gate 2 accepted for one parsed file. Returns whether
    /// anything in it has to be inferred.
    fn gather(
        &self,
        i: usize,
        hashes: &HashOutput,
        witnesses: &BTreeMap<Symbol, Vec<NameRef>>,
        table: &BTreeMap<Symbol, DefHash>,
        private: &BTreeSet<Symbol>,
        gate: &mut GateTwo,
    ) -> bool {
        let file = &self.files[i];
        let (Some(store), Some(ast)) = (self.store.as_deref(), file.ast.as_ref()) else {
            return true;
        };
        // A referent that stopped being `pub` moves no hash and fails no
        // witness, so nothing below would notice; the body has to be walked
        // again for the error to be reported against the reference.
        if self.free_names(i, hashes).iter().any(|n| private.contains(n)) {
            return true;
        }

        let mut rechecked = false;
        for item in &ast.items {
            let Some(ident) = item.name() else { continue };
            let name = file.module.qualify(&ident.name);
            let Some((&hash, witness)) = hashes
                .defs
                .get(&name)
                .or_else(|| hashes.decls.get(&name))
                .zip(witnesses.get(&name))
            else {
                rechecked = true;
                continue;
            };
            let held = match item {
                Item::Fn(_) => store.cached_def_of(hash, &name).and_then(|cached| {
                    same_witness(&cached.names, witness).then(|| KnownDef {
                        scheme: cached.scheme.clone(),
                        footprint: cached.footprint.clone(),
                    })
                }),
                _ => {
                    let unmoved = store
                        .cached_decl_of(hash, &name)
                        .is_some_and(|cached| same_witness(&cached.names, witness));
                    if unmoved {
                        gate.cached.insert(name.clone());
                    } else {
                        rechecked = true;
                    }
                    continue;
                }
            };
            match held {
                Some(entry) => {
                    gate.cached.insert(name.clone());
                    gate.known.defs.insert(name, entry);
                }
                None => rechecked = true,
            }
        }

        rechecked | self.gather_tests(i, hashes, table, private, gate)
    }

    /// A test's footprint is written in effect *names*, which a hash erases, and
    /// `CachedTest` carries no witness of its own. Reusing one is therefore only
    /// safe for a file whose every free name still denotes what it denoted — a
    /// file that would have skipped gate 1 outright had it not been dragged into
    /// the parse set for some other reason.
    ///
    /// The pairing is by hash rather than by position. A test's label is not
    /// part of its hash, so relabelling one, reordering two, or deleting a third
    /// all leave the surviving hashes intact and the positions wrong.
    ///
    /// A hash two cached tests disagree about is dropped rather than guessed at.
    /// Two tests can share a hash and still have different footprints — one
    /// performs `a.op`, the other the identically declared `b.op` — because a
    /// footprint is written in effect names and a hash erases them.
    fn gather_tests(
        &self,
        i: usize,
        hashes: &HashOutput,
        table: &BTreeMap<Symbol, DefHash>,
        private: &BTreeSet<Symbol>,
        gate: &mut GateTwo,
    ) -> bool {
        let file = &self.files[i];
        let Some(ast) = &file.ast else { return true };
        let count = ast.items.iter().filter(|i| matches!(i, Item::Test(_))).count();
        if count == 0 {
            return false;
        }
        if self.gate_one_refusal(i, table, private).is_some() {
            return true;
        }
        let Some(fingerprint) = &file.fingerprint else { return true };

        let mut by_hash: BTreeMap<DefHash, Option<&Footprint>> = BTreeMap::new();
        for test in &fingerprint.tests {
            match by_hash.entry(test.hash) {
                std::collections::btree_map::Entry::Vacant(slot) => {
                    slot.insert(Some(&test.footprint));
                }
                std::collections::btree_map::Entry::Occupied(mut slot) => {
                    if slot.get() != &Some(&test.footprint) {
                        slot.insert(None);
                    }
                }
            }
        }

        let mut slots = Vec::with_capacity(count);
        let mut rechecked = false;
        for hash in self.test_hashes_of(i, hashes) {
            match by_hash.get(&hash).copied().flatten() {
                Some(footprint) => {
                    slots.push(Some(KnownTest { footprint: footprint.clone() }))
                }
                None => {
                    slots.push(None);
                    rechecked = true;
                }
            }
        }
        if slots.len() == count {
            gate.known.tests.insert(file.module.as_symbol().clone(), slots);
        } else {
            rechecked = true;
        }
        rechecked
    }

    /// Every top-level name this file mentions but does not declare. A
    /// constructor reference reaches the type that owns it, so a variant's own
    /// name never appears.
    fn free_names(&self, i: usize, hashes: &HashOutput) -> BTreeSet<Symbol> {
        let file = &self.files[i];
        let Some(ast) = &file.ast else { return BTreeSet::new() };
        let mut declared = BTreeSet::new();
        let mut keys: Vec<Symbol> = Vec::new();
        for item in &ast.items {
            match item {
                Item::Test(def) => keys.push(file.module.qualify(&Symbol::new(&def.name))),
                _ => {
                    let Some(ident) = item.name() else { continue };
                    let name = file.module.qualify(&ident.name);
                    declared.insert(name.clone());
                    keys.push(name);
                }
            }
        }
        let mut out = BTreeSet::new();
        for key in &keys {
            for dep in hashes.deps.get(key).into_iter().flatten() {
                if !declared.contains(dep) {
                    out.insert(dep.clone());
                }
            }
        }
        out
    }

    /// `HashOutput::tests` is parallel to the program's tests walked module by
    /// module in load order, and the parsed program holds the parsed files in
    /// that same order, so the offsets line up by counting.
    fn test_hashes_of(&self, i: usize, hashes: &HashOutput) -> Vec<DefHash> {
        let mut offset = 0;
        for (j, file) in self.files.iter().enumerate() {
            let Some(ast) = &file.ast else { continue };
            let count = ast.items.iter().filter(|i| matches!(i, Item::Test(_))).count();
            if j == i {
                return hashes.tests.iter().skip(offset).take(count).copied().collect();
            }
            offset += count;
        }
        Vec::new()
    }

    /// The witness this run would write for every parsed definition and test.
    ///
    /// A `Scheme` and a `Footprint` are written in names — `Type::Con(Symbol)`
    /// and effect labels — while a `DefHash` erases them. So the hash alone does
    /// not determine the interface, and every cached interface records which
    /// `type` and `effect` declarations it reached and what each hashed to.
    ///
    /// Three parts, and each earns its place. The definition's own name comes
    /// first, so an entry written by a structurally identical definition
    /// elsewhere is never mistaken for this one's. Then its direct declaration
    /// references in normalization order, which is what tells two definitions
    /// apart when they reach the same declarations in a different arrangement.
    /// Then every declaration in its transitive closure, sorted, which is what
    /// catches a `type` renamed three calls away — a rename changes no hash, so
    /// nothing else would.
    fn witnesses(&self, hashes: &HashOutput) -> BTreeMap<Symbol, Vec<NameRef>> {
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

    fn merge(
        mut self,
        program: Program,
        resolved: ply_syntax::resolve::Resolved,
        hashes: HashOutput,
        checked: CheckOutput,
        cached: BTreeSet<Symbol>,
    ) -> Result<Loaded, LoadError> {
        let hashes = &hashes;
        let out = CheckOutput {
            defs: canonical_defs(&checked.defs),
            tests: Vec::new(),
            effects: canonical_effects(&checked.effects),
            ctors: canonical_ctors(&checked.ctors),
            modules: IndexMap::new(),
        };
        let merged = HashOutput {
            defs: hashes.defs.clone(),
            decls: hashes.decls.clone(),
            tests: Vec::new(),
            deps: hashes.deps.clone(),
            closure: hashes.closure.clone(),
        };
        let report = FrontEnd { incremental: self.mode == Mode::Incremental, ..Default::default() };
        let restoring = Instant::now();

        let mut into = Merged { out, merged, report };
        for i in 0..self.files.len() {
            if self.files[i].parse {
                self.restate_checked(i, &checked, hashes, &cached, &mut into);
            } else {
                self.restore_skipped(i, &mut into)?;
            }
            into.report.files.push(FileReport {
                path: self.files[i].path.clone(),
                module: self.files[i].module.clone(),
                parsed: self.files[i].parse,
                rechecked: self.files[i].recheck,
                refusal: self.files[i].refusal.clone(),
            });
        }
        let Merged { out, mut merged, mut report } = into;

        merged.closure = closure_of(&merged.deps);
        self.phases.restore += restoring.elapsed();

        let writing = Instant::now();
        report.warnings = self.write_back(hashes, &out, &merged);
        self.phases.write_back += writing.elapsed();
        report.phases = self.phases;

        let files = self.files.iter().map(|f| f.path.clone()).collect();
        let complete = self.files.iter().all(|f| f.parse);
        Ok(Loaded {
            root: self.root,
            files,
            sources: self.sources,
            program,
            resolved,
            check: out,
            hashes: merged,
            complete,
            frontend: report,
        })
    }

    /// A parsed module. Every type, footprint, name and span comes from the
    /// check, whether it inferred the definition or published an interface gate
    /// 2 handed it; only the test *order* has to be rebuilt, so that it follows
    /// the run's files rather than the checked program's.
    fn restate_checked(
        &self,
        i: usize,
        checked: &CheckOutput,
        hashes: &HashOutput,
        cached: &BTreeSet<Symbol>,
        into: &mut Merged,
    ) {
        let Merged { out, merged, report } = into;
        let file = &self.files[i];
        let Some(ast) = &file.ast else { return };
        out.modules.insert(file.module.as_symbol().clone(), module_info(ast, file.source));
        for item in &ast.items {
            if let Some(ident) = item.name() {
                let name = file.module.qualify(&ident.name);
                report.defs.push(DefReport { cached: cached.contains(&name), name });
            }
        }
        for test in checked.tests.iter().filter(|t| t.module == file.module) {
            let index = out.tests.len();
            out.tests.push(TestInfo { index, ..test.clone() });
        }
        merged.tests.extend(self.test_hashes_of(i, hashes));
    }

    /// A module gate 1 skipped. There is no AST at all: every name, span, type
    /// and hash comes out of the fingerprint and the interface store.
    fn restore_skipped(&self, i: usize, into: &mut Merged) -> Result<(), LoadError> {
        let Merged { out, merged, report } = into;
        let file = &self.files[i];
        let Some(store) = self.store.as_deref() else {
            return Err(self.corrupt(file.module.as_symbol()));
        };
        let Some(fingerprint) = &file.fingerprint else {
            return Err(self.corrupt(file.module.as_symbol()));
        };
        let source = file.source;

        let mut items = Vec::new();
        for entry in &fingerprint.defs {
            items.push(entry.name.clone());
            if entry.kind == DefKind::Type {
                items.extend(entry.members.iter().map(|m| file.module.qualify(&m.name)));
            }
        }
        out.modules.insert(
            file.module.as_symbol().clone(),
            ModuleInfo {
                name: file.module.clone(),
                source,
                items,
                imports: fingerprint
                    .imports
                    .iter()
                    .map(|e| ModuleName::from_dotted(e.module.as_str()))
                    .collect(),
            },
        );

        for entry in &fingerprint.defs {
            report.defs.push(DefReport { name: entry.name.clone(), cached: true });
            record_deps(merged, &entry.name, &entry.deps);
            let simple = simple_name(&file.module, &entry.name);
            match entry.kind {
                DefKind::Fn => {
                    let Some(cached) = store.cached_def_of(entry.hash, &entry.name) else {
                        return Err(self.corrupt(&entry.name));
                    };
                    merged.defs.insert(entry.name.clone(), entry.hash);
                    out.defs.insert(
                        entry.name.clone(),
                        DefInfo {
                            name: entry.name.clone(),
                            module: file.module.clone(),
                            simple_name: simple,
                            scheme: cached.scheme.clone(),
                            footprint: cached.footprint.clone(),
                            span: entry.span.rebase(source),
                        },
                    );
                }
                DefKind::Type => {
                    let Some(cached) = store.cached_decl_of(entry.hash, &entry.name) else {
                        return Err(self.corrupt(&entry.name));
                    };
                    let DeclBody::Type { ctors, .. } = &cached.body else {
                        return Err(self.corrupt(&entry.name));
                    };
                    merged.decls.insert(entry.name.clone(), entry.hash);
                    if entry.members.len() != ctors.len() {
                        return Err(self.corrupt(&entry.name));
                    }
                    for (index, (member, cached)) in entry.members.iter().zip(ctors).enumerate() {
                        let ctor = file.module.qualify(&member.name);
                        out.ctors.insert(
                            ctor.clone(),
                            CtorInfo {
                                name: ctor,
                                module: file.module.clone(),
                                simple_name: member.name.clone(),
                                type_name: entry.name.clone(),
                                index,
                                arity: cached.fields.len(),
                                fields: cached.fields.clone(),
                                scheme: cached.scheme.clone(),
                                span: member.span.rebase(source),
                            },
                        );
                    }
                }
                DefKind::Effect => {
                    let Some(cached) = store.cached_decl_of(entry.hash, &entry.name) else {
                        return Err(self.corrupt(&entry.name));
                    };
                    let DeclBody::Effect { nondet, ops } = &cached.body else {
                        return Err(self.corrupt(&entry.name));
                    };
                    merged.decls.insert(entry.name.clone(), entry.hash);
                    if entry.members.len() != ops.len() {
                        return Err(self.corrupt(&entry.name));
                    }
                    // By name, never by position: normalization sorts an
                    // effect's operations away, so their source order is not
                    // part of the hash the signatures were stored under.
                    let by_name: BTreeMap<&Symbol, &CachedOp> =
                        ops.iter().map(|op| (&op.name, op)).collect();
                    let mut infos = IndexMap::new();
                    for member in &entry.members {
                        let Some(cached) = by_name.get(&member.name) else {
                            return Err(self.corrupt(&entry.name));
                        };
                        infos.insert(
                            member.name.clone(),
                            OpInfo {
                                name: member.name.clone(),
                                mode: cached.mode,
                                resource_param: cached.resource_param,
                                params: cached.params.clone(),
                                ret: cached.ret.clone(),
                                span: member.span.rebase(source),
                            },
                        );
                    }
                    out.effects.insert(
                        entry.name.clone(),
                        EffectInfo {
                            name: entry.name.clone(),
                            module: file.module.clone(),
                            simple_name: simple,
                            nondet: *nondet,
                            ops: infos,
                            span: entry.span.rebase(source),
                        },
                    );
                }
            }
        }

        for test in &fingerprint.tests {
            let index = out.tests.len();
            let key = file.module.qualify(&Symbol::new(&test.name));
            record_deps(merged, &key, &test.deps);
            out.tests.push(TestInfo {
                name: test.name.clone(),
                module: file.module.clone(),
                key,
                index,
                nondet: test.nondet,
                footprint: test.footprint.clone(),
                span: test.span.rebase(source),
            });
            merged.tests.push(test.hash);
        }
        Ok(())
    }

    /// The cache promised an interface it does not hold. Nothing the user wrote
    /// caused this, so it names the cache and says how to clear it.
    fn corrupt(&self, name: &Symbol) -> LoadError {
        LoadError {
            sources: self.sources.clone(),
            diagnostics: vec![
                Diagnostic::error(
                    codes::CACHE_CORRUPT,
                    format!("the front-end cache has no interface for `{name}`"),
                )
                .primary(Span::DUMMY, "this definition was supposed to be cached")
                .note("run `ply cache clear`, or pass `--no-incremental` to bypass the cache"),
            ],
        }
    }

    fn write_back(
        &mut self,
        hashes: &HashOutput,
        check: &CheckOutput,
        merged: &HashOutput,
    ) -> Vec<Diagnostic> {
        if self.mode != Mode::Incremental {
            return Vec::new();
        }
        let witnesses = self.witnesses(hashes);
        let exports = self.export_table(merged);
        let table = self.hash_table(hashes);
        let paths: Vec<PathBuf> = self.files.iter().map(|f| f.path.clone()).collect();
        let whole_project = self.whole_project;

        let footprints: BTreeMap<Symbol, Footprint> = check
            .tests
            .iter()
            .map(|t| (t.key.clone(), t.footprint.clone()))
            .collect();
        let fingerprints: Vec<(usize, SourceFingerprint)> = (0..self.files.len())
            .filter(|&i| self.files[i].parse)
            .filter_map(|i| {
                self.fingerprint_of(i, hashes, &table, &exports, &footprints).map(|f| (i, f))
            })
            .collect();
        let interfaces = self.interfaces(hashes, check, &witnesses);

        let Some(store) = self.store.as_deref_mut() else { return Vec::new() };
        for (hash, entry) in interfaces {
            match entry {
                Interface::Def(def) => store.put_def(hash, def),
                Interface::Decl(decl) => store.put_decl(hash, decl),
            }
        }
        for (i, fingerprint) in fingerprints {
            store.put_source(&paths[i], fingerprint);
        }
        if whole_project {
            store.prune(&paths);
        }
        match store.flush() {
            Ok(()) => Vec::new(),
            // A cache that could not be written costs the next run its work and
            // costs this one nothing, so it is a warning rather than a failure.
            Err(e) => vec![
                Diagnostic::warning(
                    codes::CACHE_UNREADABLE,
                    format!("could not write the front-end cache: {e:#}"),
                )
                .note("this run is unaffected; the next one will parse and check everything again"),
            ],
        }
    }

    /// Every module's `(name, hash)` pairs, which is what an importer's
    /// `ImportEdge` digest is taken over.
    fn export_table(&self, merged: &HashOutput) -> BTreeMap<Symbol, Vec<NameRef>> {
        let mut out: BTreeMap<Symbol, Vec<NameRef>> = BTreeMap::new();
        for file in &self.files {
            let mut names: Vec<NameRef> = Vec::new();
            match &file.ast {
                Some(ast) => {
                    for item in &ast.items {
                        let Some(ident) = item.name() else { continue };
                        let name = file.module.qualify(&ident.name);
                        if let Some(hash) =
                            merged.defs.get(&name).or_else(|| merged.decls.get(&name))
                        {
                            names.push(NameRef::new(name, *hash));
                        }
                    }
                }
                None => {
                    names.extend(
                        file.cached_defs()
                            .iter()
                            .map(|d| NameRef::new(d.name.clone(), d.hash)),
                    );
                }
            }
            out.insert(file.module.as_symbol().clone(), names);
        }
        out
    }

    fn fingerprint_of(
        &self,
        i: usize,
        hashes: &HashOutput,
        table: &BTreeMap<Symbol, DefHash>,
        exports: &BTreeMap<Symbol, Vec<NameRef>>,
        footprints: &BTreeMap<Symbol, Footprint>,
    ) -> Option<SourceFingerprint> {
        let file = &self.files[i];
        let ast = file.ast.as_ref()?;
        let mut fingerprint = SourceFingerprint::new(file.content);

        let mut seen = BTreeSet::new();
        for import in &ast.imports {
            let module = import.module_name().as_symbol().clone();
            if !seen.insert(module.clone()) {
                continue;
            }
            let digest = exports_digest(exports.get(&module).map(Vec::as_slice).unwrap_or(&[]));
            fingerprint.imports.push(ImportEdge { module, exports: digest });
        }

        // Free names, and what each resolved to: gate 1's exact condition.
        fingerprint.deps = self
            .free_names(i, hashes)
            .into_iter()
            .filter_map(|name| table.get(&name).map(|hash| NameRef::new(name, *hash)))
            .collect();

        for item in &ast.items {
            let Some(ident) = item.name() else { continue };
            let name = file.module.qualify(&ident.name);
            let hash = *hashes.defs.get(&name).or_else(|| hashes.decls.get(&name))?;
            let (kind, members) = match item {
                Item::Fn(_) => (DefKind::Fn, Vec::new()),
                Item::Type(def) => (
                    DefKind::Type,
                    match &def.body {
                        TypeDefBody::Sum(variants) => variants
                            .iter()
                            .map(|v| Member {
                                name: v.name.name.clone(),
                                span: FileSpan::of(v.span),
                            })
                            .collect(),
                        TypeDefBody::Alias(_) => Vec::new(),
                    },
                ),
                Item::Effect(def) => (
                    DefKind::Effect,
                    def.ops
                        .iter()
                        .map(|op| Member {
                            name: op.name.name.clone(),
                            span: FileSpan::of(op.span),
                        })
                        .collect(),
                ),
                Item::Test(_) => continue,
            };
            let deps = hashes.deps.get(&name).cloned().unwrap_or_default();
            fingerprint.defs.push(DefEntry {
                name,
                hash,
                span: FileSpan::of(item.span()),
                kind,
                members,
                deps,
            });
        }

        let test_hashes = self.test_hashes_of(i, hashes);
        let tests: Vec<&ply_syntax::ast::TestDef> = ast
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Test(def) => Some(def.as_ref()),
                _ => None,
            })
            .collect();
        if tests.len() != test_hashes.len() {
            return None;
        }
        for (def, hash) in tests.iter().zip(&test_hashes) {
            let key = file.module.qualify(&Symbol::new(&def.name));
            fingerprint.tests.push(CachedTest {
                name: def.name.clone(),
                hash: *hash,
                nondet: def.nondet,
                footprint: footprints.get(&key)?.clone(),
                span: FileSpan::of(def.span),
                name_span: FileSpan::of(def.name_span),
                deps: hashes.deps.get(&key).cloned().unwrap_or_default(),
            });
        }
        Some(fingerprint)
    }

    /// Only parsed files contribute: a skipped file's entries are already in the
    /// store and were the very thing that let it skip.
    fn interfaces(
        &self,
        hashes: &HashOutput,
        check: &CheckOutput,
        witnesses: &BTreeMap<Symbol, Vec<NameRef>>,
    ) -> Vec<(DefHash, Interface)> {
        let mut out = Vec::new();
        for file in self.files.iter().filter(|f| f.parse) {
            let Some(ast) = &file.ast else { continue };
            for item in &ast.items {
                let Some(ident) = item.name() else { continue };
                let name = file.module.qualify(&ident.name);
                let Some(hash) = hashes.defs.get(&name).or_else(|| hashes.decls.get(&name)) else {
                    continue;
                };
                let Some(names) = witnesses.get(&name).cloned() else { continue };
                let entry = match item {
                    Item::Fn(_) => check.defs.get(&name).map(|d| {
                        Interface::Def(
                            CachedDef::new(d.scheme.clone(), d.footprint.clone())
                                .witnessed_by(names),
                        )
                    }),
                    Item::Type(def) => {
                        let variants = match &def.body {
                            TypeDefBody::Sum(variants) => variants.as_slice(),
                            TypeDefBody::Alias(_) => &[],
                        };
                        let ctors: Option<Vec<CachedCtor>> = variants
                            .iter()
                            .map(|v| {
                                check.ctors.get(&file.module.qualify(&v.name.name)).map(|c| {
                                    CachedCtor {
                                        fields: c.fields.clone(),
                                        scheme: c.scheme.clone(),
                                    }
                                })
                            })
                            .collect();
                        ctors.map(|ctors| {
                            Interface::Decl(
                                CachedDecl::new(DeclBody::Type {
                                    arity: def.params.len(),
                                    ctors,
                                })
                                .witnessed_by(names),
                            )
                        })
                    }
                    Item::Effect(def) => check.effects.get(&name).and_then(|info| {
                        let ops: Option<Vec<CachedOp>> = def
                            .ops
                            .iter()
                            .map(|op| {
                                info.ops.get(&op.name.name).map(|o| CachedOp {
                                    name: op.name.name.clone(),
                                    mode: o.mode,
                                    resource_param: o.resource_param,
                                    params: o.params.clone(),
                                    ret: o.ret.clone(),
                                })
                            })
                            .collect();
                        ops.map(|ops| {
                            Interface::Decl(
                                CachedDecl::new(DeclBody::Effect { nondet: info.nondet, ops })
                                    .witnessed_by(names),
                            )
                        })
                    }),
                    Item::Test(_) => None,
                };
                if let Some(entry) = entry {
                    out.push((*hash, entry));
                }
            }
        }
        out
    }
}

enum Interface {
    Def(CachedDef),
    Decl(CachedDecl),
}

/// The three maps every file contributes to, whichever way it got here.
struct Merged {
    out: CheckOutput,
    merged: HashOutput,
    report: FrontEnd,
}

/// What gate 2 decided: the interfaces inference may publish without walking a
/// body, and every name it accepted, which is what `--explain` reports.
#[derive(Default)]
struct GateTwo {
    known: Known,
    cached: BTreeSet<Symbol>,
}

/// Merges rather than overwrites, because two entries can share a name — a
/// `type` and a `fn` may, as may two tests. Mirrors what a from-scratch run
/// does, so that the two land on the same map.
fn record_deps(merged: &mut HashOutput, name: &Symbol, deps: &[Symbol]) {
    match merged.deps.get_mut(name) {
        Some(existing) => {
            for d in deps {
                if !existing.contains(d) {
                    existing.push(d.clone());
                }
            }
        }
        None => {
            merged.deps.insert(name.clone(), deps.to_vec());
        }
    }
}

/// The transitive closure of the reference graph, each name included in its
/// own. Recomputed from the merged edges on every path rather than taken from
/// the parsed program's, because a skipped file contributes edges to that map
/// and nothing else would fold them in.
fn closure_of(deps: &IndexMap<Symbol, Vec<Symbol>>) -> IndexMap<Symbol, BTreeSet<Symbol>> {
    let names: Vec<&Symbol> = deps.keys().collect();
    let index: BTreeMap<&Symbol, usize> =
        names.iter().enumerate().map(|(i, name)| (*name, i)).collect();
    let edges: Vec<Vec<NodeId>> = deps
        .values()
        .map(|ds| ds.iter().filter_map(|d| index.get(d).map(|&i| NodeId(i))).collect())
        .collect();

    let components = ply_hash::graph::tarjan(names.len(), &edges);
    let mut component_of = vec![usize::MAX; names.len()];
    for (ci, component) in components.iter().enumerate() {
        for &v in component {
            component_of[v] = ci;
        }
    }

    // Dependency-first, so every closure a component splices in is already built.
    let mut closures: Vec<BTreeSet<Symbol>> = Vec::with_capacity(components.len());
    for (ci, component) in components.iter().enumerate() {
        let mut closure: BTreeSet<Symbol> =
            component.iter().map(|&v| names[v].clone()).collect();
        for &v in component {
            for r in &edges[v] {
                if component_of[r.0] != ci
                    && let Some(inner) = closures.get(component_of[r.0])
                {
                    closure.extend(inner.iter().cloned());
                }
            }
        }
        closures.push(closure);
    }

    names
        .iter()
        .enumerate()
        .map(|(v, name)| ((*name).clone(), closures[component_of[v]].clone()))
        .collect()
}

fn module_info(module: &Module, source: SourceId) -> ModuleInfo {
    let mut items = Vec::new();
    for item in &module.items {
        let Some(ident) = item.name() else { continue };
        items.push(module.name.qualify(&ident.name));
        if let Item::Type(def) = item
            && let TypeDefBody::Sum(variants) = &def.body
        {
            items.extend(variants.iter().map(|v| module.name.qualify(&v.name.name)));
        }
    }
    let mut imports: Vec<ModuleName> = Vec::new();
    for import in &module.imports {
        let name = import.module_name();
        if !imports.contains(&name) {
            imports.push(name);
        }
    }
    ModuleInfo { name: module.name.clone(), source, items, imports }
}

fn simple_name(module: &ModuleName, qualified: &Symbol) -> Symbol {
    let prefix = format!("{}.", module.as_str());
    match qualified.as_str().strip_prefix(&prefix) {
        Some(rest) => Symbol::new(rest),
        None => qualified.clone(),
    }
}

/// Two witnesses agree when they name the same declarations with the same
/// hashes. Compared as sets: the store canonicalizes what it holds, and this
/// side must not depend on which order it chose.
fn same_witness(stored: &[NameRef], fresh: &[NameRef]) -> bool {
    let mut a = stored.to_vec();
    let mut b = fresh.to_vec();
    let key = |n: &NameRef| (n.name.clone(), n.hash);
    a.sort_by_key(key);
    a.dedup();
    b.sort_by_key(key);
    b.dedup();
    a == b
}

fn canonical_defs(defs: &IndexMap<Symbol, DefInfo>) -> IndexMap<Symbol, DefInfo> {
    let mut out = IndexMap::with_capacity(defs.len());
    for (name, def) in defs {
        out.insert(
            name.clone(),
            DefInfo { scheme: canonicalize_scheme(&def.scheme), ..def.clone() },
        );
    }
    out
}

/// Constructors are renumbered per *type*, not per constructor: a type's
/// parameters are shared by every variant, and numbering each alone would make
/// `P(a)` and `Q(b)` of `type Pair<a, b>` both mention `t0`. Going through
/// [`canonicalize_decl_body`] is what keeps a freshly checked declaration
/// byte-identical to the same declaration restored from the store.
fn canonical_ctors(ctors: &IndexMap<Symbol, CtorInfo>) -> IndexMap<Symbol, CtorInfo> {
    let mut owners: IndexMap<Symbol, Vec<Symbol>> = IndexMap::new();
    for (name, ctor) in ctors {
        owners.entry(ctor.type_name.clone()).or_default().push(name.clone());
    }
    let mut out = ctors.clone();
    for (_, mut names) in owners {
        names.sort_by_key(|n| ctors[n].index);
        let body = DeclBody::Type {
            arity: 0,
            ctors: names
                .iter()
                .map(|n| CachedCtor {
                    fields: ctors[n].fields.clone(),
                    scheme: ctors[n].scheme.clone(),
                })
                .collect(),
        };
        let DeclBody::Type { ctors: canonical, .. } = canonicalize_decl_body(&body) else {
            continue;
        };
        for (name, cached) in names.iter().zip(canonical) {
            let entry = &mut out[name];
            entry.fields = cached.fields;
            entry.scheme = cached.scheme;
        }
    }
    out
}

fn canonical_effects(effects: &IndexMap<Symbol, EffectInfo>) -> IndexMap<Symbol, EffectInfo> {
    let mut out = IndexMap::with_capacity(effects.len());
    for (name, effect) in effects {
        let body = DeclBody::Effect {
            nondet: effect.nondet,
            ops: effect
                .ops
                .values()
                .map(|op| CachedOp {
                    name: op.name.clone(),
                    mode: op.mode,
                    resource_param: op.resource_param,
                    params: op.params.clone(),
                    ret: op.ret.clone(),
                })
                .collect(),
        };
        let mut ops = effect.ops.clone();
        if let DeclBody::Effect { ops: canonical, .. } = canonicalize_decl_body(&body) {
            for (info, cached) in ops.values_mut().zip(canonical) {
                info.params = cached.params;
                info.ret = cached.ret;
            }
        }
        out.insert(name.clone(), EffectInfo { ops, ..effect.clone() });
    }
    out
}
