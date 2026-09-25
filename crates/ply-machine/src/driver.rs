//! The front end: the port parses, resolves and checks the program. Every run hands it what the
//! last one published, definition by definition, so it walks what moved and what depends on it.

use crate::load::{
    Discovered, Found, LoadError, Loaded, Stamp, anchor, discover, stamp_of, unreadable,
};
use ply_codegen::c::producer::{self, KnownDef, KnownTest};
use ply_prove::prove::{Claims, read_claims};
use ply_span::frames::Cursor;
use ply_span::{Diagnostic, SourceId, SourceMap, Span, Symbol, codes};
use ply_store::body::StoredBody;
use ply_store::{
    CachedCtor, CachedDecl, CachedDef, CachedOp, CachedTest, ContentHash, DeclBody, DefBody,
    DefEntry, DefKind, FileSpan, Member, NameRef, SourceFingerprint, Store,
};
use ply_ty::ModuleName;
use ply_ty::{DefHash, HashOutput};
use ply_ty::{Front, ModuleInfo};
use std::collections::{BTreeMap, BTreeSet, HashMap};
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
    /// The port's whole front end over this program, or its answer read back.
    pub front: Duration,
    pub write_back: Duration,
}

impl Phases {
    pub fn total(&self) -> Duration {
        self.read + self.front + self.write_back
    }

    pub fn labelled(&self) -> [(&'static str, Duration); 3] {
        [
            ("read", self.read),
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

/// The port's lowered claims, kept per module: a module's part is reused while its text and the
/// texts of all it imports are unchanged, and the rest are asked with all they import, so the port
/// sees a closed program.
pub fn claims(loaded: &Loaded, store: Option<&mut Store>) -> Result<Claims, String> {
    let modules: Vec<&ModuleInfo> = loaded.check.modules.values().collect();
    let texts = modules
        .iter()
        .map(|m| {
            loaded
                .sources
                .get(m.source)
                .map(|f| f.text.clone())
                .ok_or_else(|| format!("module `{}` has no source text", m.name))
        })
        .collect::<Result<Vec<Arc<str>>, String>>()?;
    let by_module: BTreeMap<Symbol, usize> = modules
        .iter()
        .enumerate()
        .map(|(i, m)| (m.name.as_symbol().clone(), i))
        .collect();
    let reach = |from: &[usize]| reaching(from, |i| modules[i].imports.as_slice(), &by_module);

    let store = store.filter(|_| loaded.frontend.incremental);
    let keys: Vec<ContentHash> = if store.is_some() {
        let emitter = ply_codegen::c::producer::emitter();
        let contents: Vec<ContentHash> = texts
            .iter()
            .map(|t| ContentHash::of(t.as_bytes()))
            .collect();
        (0..modules.len())
            .map(|i| {
                let reached = reach(&[i]).into_iter();
                let reached = reached.map(|j| (modules[j].name.as_str(), &contents[j]));
                module_key("claims", &emitter, &modules[i].name, reached)
            })
            .collect()
    } else {
        Vec::new()
    };
    let mut parts: Vec<Option<(String, Claims)>> = (0..modules.len())
        .map(|i| {
            let text = store.as_deref()?.claims_part(keys[i])?;
            let read = claims_part(&text, modules[i].source).ok()?;
            Some((text, read))
        })
        .collect();

    let missed: Vec<usize> = (0..parts.len()).filter(|&i| parts[i].is_none()).collect();
    if !missed.is_empty() {
        let asked = reach(&missed);
        let sources: Vec<(String, String)> = asked
            .iter()
            .map(|&i| (modules[i].name.to_string(), texts[i].to_string()))
            .collect();
        let mod_pkg: Vec<usize> = asked
            .iter()
            .map(|&i| loaded.front.mod_pkg.get(i).copied().unwrap_or(0))
            .collect();
        let dump =
            ply_codegen::c::producer::claims_dump(&sources, &loaded.front.packages, &mod_pkg)
                .map_err(|e| format!("{e:#}"))?;
        let items: HashMap<&str, usize> = modules
            .iter()
            .enumerate()
            .flat_map(|(i, m)| m.items.iter().map(move |name| (name.as_str(), i)))
            .collect();
        let laws: HashMap<&str, usize> = loaded
            .check
            .laws
            .iter()
            .filter_map(|law| Some((law.key.as_str(), *by_module.get(law.module.as_symbol())?)))
            .collect();
        let unread = |e: String| format!("its answer does not read: {e}");
        for (&i, text) in asked
            .iter()
            .zip(split_claims(&dump, &asked, &items, &laws).map_err(unread)?)
        {
            let read = claims_part(&text, modules[i].source).map_err(unread)?;
            parts[i] = Some((text, read));
        }
        if let Some(store) = store {
            let filed = keys
                .iter()
                .zip(&parts)
                .filter_map(|(key, part)| Some((*key, part.as_ref()?.0.clone())))
                .collect();
            store.put_claims_parts(filed);
        }
    }

    let mut claims = Claims::default();
    for (_, read) in parts.into_iter().flatten() {
        claims.defs.extend(read.defs);
        claims.laws.extend(read.laws);
        claims.sums.extend(read.sums);
    }
    Ok(claims)
}

/// Each asked module's part: its position among `asked`, then the frames it owns.
fn split_claims(
    dump: &str,
    asked: &[usize],
    items: &HashMap<&str, usize>,
    laws: &HashMap<&str, usize>,
) -> Result<Vec<String>, String> {
    let mut parts: Vec<String> = (0..asked.len()).map(|at| format!("{at}\n")).collect();
    let mut frames = Cursor::new(dump.as_bytes(), "frame");
    while !frames.done() {
        let start = frames.at();
        let (words, payload) = frames.unit()?;
        let owner = match words[..] {
            ["law", _] => law_key(payload)?.and_then(|key| laws.get(key)),
            [_, name] => items.get(name),
            _ => None,
        };
        let at = owner
            .and_then(|i| asked.iter().position(|j| j == i))
            .ok_or_else(|| format!("`{}` belongs to no module asked", words.join(" ")))?;
        parts[at].push_str(&dump[start..frames.at()]);
    }
    Ok(parts)
}

fn law_key(payload: &[u8]) -> Result<Option<&str>, String> {
    let mut fields = Cursor::new(payload, "field");
    while !fields.done() {
        let (words, text) = fields.unit()?;
        if words == ["key"] {
            return Ok(std::str::from_utf8(text).ok());
        }
    }
    Ok(None)
}

/// A module's frames span only it, so every position up to its own reads as its source.
fn claims_part(text: &str, source: SourceId) -> Result<Claims, String> {
    let (at, frames) = text
        .split_once('\n')
        .ok_or("a module's claims have no position")?;
    let at: usize = at
        .parse()
        .map_err(|_| format!("a module's claims are at `{at}`"))?;
    read_claims(frames, &vec![source; at + 1])
}

struct FileState {
    path: PathBuf,
    module: ModuleName,
    source: SourceId,
    text: Arc<str>,
    content: ContentHash,
    /// What the file stamped before this load read it, which a watcher compares against.
    stamp: Stamp,
    /// Embedded in the binary rather than discovered on disk.
    shipped: bool,
}

struct Driver<'s> {
    root: PathBuf,
    mode: Mode,
    store: Option<&'s mut Store>,
    whole_project: bool,
    /// The project's own files, which every placement of the shipped modules follows.
    project: SourceMap,
    /// The root's `ply.pkg`, when there is one: the front end checks it and places it last.
    manifest: Option<(PathBuf, Arc<str>)>,
    /// The packages the walk reached, in walk order: each root's manifest text and its
    /// modules as `(file, name relative to the package root, text)`.
    packages: Vec<DepPackage>,
    sources: SourceMap,
    files: Vec<FileState>,
    phases: Phases,
}

/// One dependency package of the walk: its manifest text, and its modules by file.
struct DepPackage {
    root: String,
    manifest: Option<String>,
    files: Vec<(PathBuf, String, Arc<str>)>,
}

/// The modules a package root holds, named relative to it; a root with no readable `ply.pkg`
/// answers nothing, and the front end's `E0135` says why.
fn read_package(root: &str) -> DepPackage {
    let dir = Path::new(root);
    let manifest = std::fs::read_to_string(dir.join("ply.pkg")).ok();
    let files = if manifest.is_some() {
        let mut out = Vec::new();
        if let Ok(paths) = crate::load::ply_files(dir) {
            for path in paths {
                let Ok(relative) = path.strip_prefix(dir) else {
                    continue;
                };
                let Ok(module) = ModuleName::from_relative_path(relative) else {
                    continue;
                };
                if let Ok(text) = std::fs::read_to_string(&path) {
                    out.push((path, module.to_string(), Arc::from(text.as_str())));
                }
            }
        }
        out
    } else {
        Vec::new()
    };
    DepPackage {
        root: root.to_string(),
        manifest,
        files,
    }
}

/// What the manifests on hand ask for, round by round, until nothing is new: the closure of
/// the root package's path dependencies.
fn walk_packages(
    root: &Path,
    manifest: &Option<(PathBuf, Arc<str>)>,
) -> Result<Vec<DepPackage>, String> {
    let root_key = root.to_string_lossy().into_owned();
    let mut known = vec![root_key.clone()];
    let mut manifests = vec![producer::SuppliedPackage {
        root: root_key,
        manifest: manifest.as_ref().map(|(_, text)| text.to_string()),
        modules: Vec::new(),
    }];
    let mut packages = Vec::new();
    loop {
        let wanted = producer::pkg_wants(&known, &manifests).map_err(|e| format!("{e:#}"))?;
        if wanted.is_empty() {
            return Ok(packages);
        }
        for w in wanted {
            known.push(w.clone());
            let package = read_package(&w);
            manifests.push(producer::SuppliedPackage {
                root: w.clone(),
                manifest: package.manifest.clone(),
                modules: Vec::new(),
            });
            packages.push(package);
        }
    }
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
                // Before the read: a save that lands between the two then moves the stamp away
                // from what this load recorded, so the next one looks rather than trusts it.
                let stamp = stamp_of(&file.path);
                match std::fs::read_to_string(&file.path) {
                    Ok(text) => {
                        let content = ContentHash::of(text.as_bytes());
                        let id = sources.add(&file.path, text);
                        read.push((id, content, stamp));
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
        for (file, &(source, content, stamp)) in discovered.iter().zip(&read) {
            match ModuleName::from_relative_path(&file.relative) {
                Ok(module) => files.push(FileState {
                    path: file.path.clone(),
                    module,
                    source,
                    text: sources
                        .get(source)
                        .map(|f| f.text.clone())
                        .unwrap_or_else(|| "".into()),
                    content,
                    stamp,
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

        let manifest_path = root.join("ply.pkg");
        let manifest = match std::fs::read_to_string(&manifest_path) {
            Ok(text) => Some((manifest_path, Arc::from(text.as_str()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                return Err(LoadError {
                    sources,
                    diagnostics: vec![unreadable(&manifest_path, &e)],
                });
            }
        };

        let packages = walk_packages(&root, &manifest).map_err(|e| LoadError {
            sources: sources.clone(),
            diagnostics: vec![Diagnostic::error(
                codes::INTERNAL_ERROR,
                format!("the package walk could not answer for this project: {e}"),
            )],
        })?;
        let mut driver = Driver {
            packages,
            root,
            mode,
            store,
            whole_project,
            manifest,
            project: sources.clone(),
            sources,
            files,
            phases,
        };
        driver.place(&[]);
        Ok(driver)
    }

    fn finish(mut self) -> Result<Loaded, LoadError> {
        let front = self.ask_the_port()?;
        if front.has_error() {
            return Err(LoadError {
                sources: self.sources.clone(),
                diagnostics: front.diagnostics,
            });
        }

        // The front end named the dependency modules by prefix; take the names it gave them,
        // matched by source, so cache fingerprints key the same modules it answered for.
        for i in self.own()..self.files.len() {
            if self.files[i].shipped {
                continue;
            }
            if let Some(info) = front
                .check
                .modules
                .values()
                .find(|m| m.source == self.files[i].source)
            {
                self.files[i].module = info.name.clone();
            }
        }

        let stdlib = self.stdlib_notice(&front.hashes);
        let writing = Instant::now();
        let cache = self.write_back(&front);
        self.phases.write_back += writing.elapsed();

        let mut warnings = stdlib;
        warnings.extend(
            front
                .diagnostics
                .iter()
                .filter(|d| !self.in_shipped(d))
                .cloned(),
        );
        warnings.extend(cache);

        let files = self
            .files
            .iter()
            .map(|f| Found {
                path: f.path.clone(),
                stamp: f.stamp,
                content: f.content,
            })
            .collect();
        // Whether the whole-program promise check has anything to check.
        let promised = front.defs_written.values().any(|w| w.reuse);
        Ok(Loaded {
            root: self.root,
            files,
            sources: self.sources,
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

    fn ask_the_port(&mut self) -> Result<Front, LoadError> {
        ply_codegen::c::producer::ensure_default();
        let started = Instant::now();
        let answer = self.whole();
        self.phases.front += started.elapsed();
        answer
    }

    fn own(&self) -> usize {
        self.project.files().len()
    }

    /// The port pulls in the shipped modules the program imports, so its answer also places them.
    fn whole(&mut self) -> Result<Front, LoadError> {
        let own: Vec<(String, String)> = self.files[..self.own()]
            .iter()
            .map(|f| (f.module.to_string(), f.text.to_string()))
            .collect();
        let shelf = crate::shelf::sources();
        let (defs, tests) = self.known();
        let packages = producer::Packages {
            root: self.root.to_string_lossy().into_owned(),
            manifest: self.manifest.as_ref().map(|(_, text)| text.to_string()),
            supplied: self
                .packages
                .iter()
                .map(|p| producer::SuppliedPackage {
                    root: p.root.clone(),
                    manifest: p.manifest.clone(),
                    modules: p
                        .files
                        .iter()
                        .map(|(_, name, text)| (name.clone(), text.to_string()))
                        .collect(),
                })
                .collect(),
        };
        let pulled =
            ply_codegen::c::producer::front_pulling_std_with(&own, shelf, &defs, &tests, &packages)
                .map_err(|e| self.seam_failed(&format!("{e:#}")))?;
        self.place(&pulled.modules);
        // The front end parses the root's modules, then the dependency modules in walk order,
        // then the pulled shelf; the manifest slots follow them all.
        let pulled_files = self.files.split_off(self.own());
        for package in &self.packages {
            for (path, name, text) in &package.files {
                let source = self.sources.add(path, text.to_string());
                self.files.push(FileState {
                    stamp: stamp_of(path),
                    path: path.clone(),
                    module: ModuleName::from_dotted(name),
                    source,
                    text: text.clone(),
                    content: ContentHash::of(text.as_bytes()),
                    shipped: false,
                });
            }
        }
        self.files.extend(pulled_files);
        // Manifest slots: the root's first, then each supplied package's in walk order, so a
        // manifest diagnostic's module index lands on its own file.
        if let Some((path, text)) = &self.manifest {
            let source = self.sources.add(path, text.to_string());
            self.files.push(FileState {
                stamp: stamp_of(path),
                path: path.clone(),
                module: ModuleName::from_dotted("pkg"),
                source,
                text: text.clone(),
                content: ContentHash::of(text.as_bytes()),
                shipped: false,
            });
        }
        for package in &self.packages {
            let Some(text) = &package.manifest else {
                continue;
            };
            let path = PathBuf::from(&package.root).join("ply.pkg");
            let source = self.sources.add(&path, text.to_string());
            self.files.push(FileState {
                stamp: stamp_of(&path),
                path,
                module: ModuleName::from_dotted("pkg"),
                source,
                text: Arc::from(text.as_str()),
                content: ContentHash::of(text.as_bytes()),
                shipped: false,
            });
        }
        let ids: Vec<SourceId> = self.files.iter().map(|f| f.source).collect();
        ply_ty::read_front(&pulled.dump, &ids)
            .map_err(|e| self.seam_failed(&format!("the front end's answer does not read: {e}")))
    }

    /// What the last answer published, definition by definition and test by test. The front end
    /// takes a row wherever this program hashes that item the same, so a run walks what moved and
    /// what depends on it, and nothing else. A row filed under a hash nothing has now is ignored.
    fn known(&self) -> (Vec<KnownDef>, Vec<KnownTest>) {
        let (mut defs, mut tests) = (Vec::new(), Vec::new());
        if self.mode != Mode::Incremental {
            return (defs, tests);
        }
        let Some(store) = self.store.as_deref() else {
            return (defs, tests);
        };
        let filed: Vec<(ModuleName, Arc<SourceFingerprint>)> = self
            .fingerprinted()
            .into_iter()
            .filter_map(|(path, module)| Some((module, store.fingerprint(&path)?)))
            .collect();
        // What each effect hashed to when these rows were filed, which is what witnesses them.
        let recorded: BTreeMap<Symbol, DefHash> = filed
            .iter()
            .flat_map(|(_, f)| f.defs.iter())
            .filter(|e| e.kind == DefKind::Effect)
            .map(|e| (e.name.clone(), e.hash))
            .collect();

        for (module, fingerprint) in &filed {
            for entry in &fingerprint.defs {
                if entry.kind != DefKind::Fn {
                    continue;
                }
                let Some(cached) = store.def_of(entry.hash, &entry.name) else {
                    continue;
                };
                defs.push(KnownDef {
                    name: entry.name.to_string(),
                    hash: entry.hash,
                    witness: witness_for(&recorded, &[&cached.footprint, &cached.performed]),
                    footprint: ply_ty::print_footprint(&cached.footprint),
                    performed: ply_ty::print_footprint(&cached.performed),
                });
            }
            for test in &fingerprint.tests {
                tests.push(KnownTest {
                    key: format!("{module}.{}", test.name),
                    hash: test.hash,
                    witness: witness_for(&recorded, &[&test.footprint]),
                    footprint: ply_ty::print_footprint(&test.footprint),
                });
            }
        }
        (defs, tests)
    }

    /// The files a fingerprint may be on record for, before the port says which are in play: the
    /// project's own, then every module this binary ships, under the path each is keyed by.
    fn fingerprinted(&self) -> Vec<(PathBuf, ModuleName)> {
        let mut out: Vec<(PathBuf, ModuleName)> = self.files[..self.own()]
            .iter()
            .map(|f| (f.path.clone(), f.module.clone()))
            .collect();
        out.extend(crate::shelf::sources().iter().map(|(name, _)| {
            let module = ModuleName::from_dotted(name);
            (crate::shelf::pseudo_path(&module), module)
        }));
        out
    }

    /// Shipped modules follow the project's files, placed as the port pulls them in.
    fn place(&mut self, shipped: &[String]) {
        let own = self.own();
        self.files.truncate(own);
        self.sources = self.project.clone();
        for module in shipped.iter().map(ModuleName::from_dotted) {
            let Some(text) = crate::shelf::source(&module) else {
                continue;
            };
            let path = crate::shelf::pseudo_path(&module);
            let content = ContentHash::of(text.as_bytes());
            let source = self.sources.add(&path, text);
            let text = self
                .sources
                .get(source)
                .map(|f| f.text.clone())
                .unwrap_or_else(|| "".into());
            self.files.push(FileState {
                stamp: stamp_of(&path),
                path,
                module,
                source,
                text,
                content,
                shipped: true,
            });
        }
    }

    /// A warning inside a module the compiler ships is its maintainers', not this program's.
    fn in_shipped(&self, d: &Diagnostic) -> bool {
        d.primary_span().is_some_and(|span| {
            self.files
                .iter()
                .any(|f| f.shipped && f.source == span.source)
        })
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
        // Keyed, not placed: a shipped module's key is not relative to this run's root.
        for path in store.source_keys().into_iter().map(PathBuf::from) {
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

    fn write_back(&mut self, front: &Front) -> Vec<Diagnostic> {
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

/// The declaration each effect these rows name had, by name and hash. A row is about the program
/// that filed it only while the effects it names are still those declarations; a prelude effect is
/// declared by no source, so it has no entry and no edit can rename it.
fn witness_for(
    recorded: &BTreeMap<Symbol, DefHash>,
    rows: &[&ply_ty::Footprint],
) -> Vec<(String, DefHash)> {
    let named: BTreeSet<&Symbol> = rows
        .iter()
        .flat_map(|row| row.atoms())
        .map(|a| &a.effect)
        .collect();
    named
        .into_iter()
        .filter_map(|effect| Some((effect.to_string(), *recorded.get(effect)?)))
        .collect()
}

/// `from` and every module it imports, transitively, in order.
fn reaching<'a>(
    from: &[usize],
    imports: impl Fn(usize) -> &'a [ModuleName],
    by_module: &BTreeMap<Symbol, usize>,
) -> Vec<usize> {
    let mut seen: BTreeSet<usize> = from.iter().copied().collect();
    let mut stack = from.to_vec();
    while let Some(i) = stack.pop() {
        for imported in imports(i) {
            if let Some(&j) = by_module.get(imported.as_symbol())
                && seen.insert(j)
            {
                stack.push(j);
            }
        }
    }
    seen.into_iter().collect()
}

/// A module's `kind` of answer is fixed by the emitter and the texts of all the module reaches.
fn module_key<'a>(
    kind: &str,
    emitter: &str,
    module: &ModuleName,
    reached: impl Iterator<Item = (&'a str, &'a ContentHash)>,
) -> ContentHash {
    let mut reached: Vec<(&str, &ContentHash)> = reached.collect();
    reached.sort_unstable();
    let mut key = format!("{kind}\0{emitter}\0{module}").into_bytes();
    for (name, content) in reached {
        key.push(0);
        key.extend_from_slice(name.as_bytes());
        key.push(0);
        key.extend_from_slice(&content.0);
    }
    ContentHash::of(&key)
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
                CachedDef::new(d.scheme.clone(), d.footprint.clone(), d.performed.clone())
                    .witnessed_by(names.clone()),
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
