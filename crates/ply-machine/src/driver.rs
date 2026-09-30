//! The front end: the port parses, resolves and checks the program. The CLI runs it, seeded from
//! and filed into the front-end cache, and hands its answer here; a program loading a program runs
//! it here, from nothing.

use crate::load::{
    Discovered, Found, LoadError, Loaded, Stamp, anchor, discover, project_root, stamp_of,
    unreadable,
};
use ply_codegen::c::producer;
use ply_eval::Value as PlyValue;
use ply_eval::decode::{self, At};
use ply_prove::prove::{Claims, read_claims};
use ply_span::{Diagnostic, SourceId, SourceMap, Span, Symbol, codes};
use ply_store::{ContentHash, Store};
use ply_ty::{Front, ModuleInfo, ModuleName};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Where a front-end run's time went.
#[derive(Clone, Copy, Debug, Default)]
pub struct Phases {
    pub read: Duration,
    /// The port's whole front end over this program, or its answer read back.
    pub front: Duration,
    /// Filing the answer into the front-end cache, which the CLI does.
    pub write_back: Duration,
}

#[derive(Clone, Debug, Default)]
pub struct FrontEnd {
    /// Whether the load read and filed the front-end cache.
    pub incremental: bool,
    pub phases: Phases,
    pub warnings: Vec<Diagnostic>,
}

/// The front end a CLI ran and handed over: the compiler's frames, every source they name in the
/// order its ids run, what the CLI's own load cost, and whether it read and filed the front-end
/// cache for it. This side reads the answer rather than walking and analysing again.
#[derive(Clone, Debug)]
pub struct HandedFront {
    pub dump: String,
    pub files: Vec<FrontFile>,
    pub read: Duration,
    pub front: Duration,
    pub write_back: Duration,
    pub cached: bool,
}

pub fn handed_front_of(v: &ply_eval::Value, span: Span) -> Result<HandedFront, Diagnostic> {
    use crate::payload::field_of;
    let dump = String::from_utf8_lossy(field_of(v, "dump", span)?.as_bytes(span, "the frames")?)
        .into_owned();
    let mut files = Vec::new();
    for item in field_of(v, "files", span)?.as_list(span, "files")?.iter() {
        let text = String::from_utf8_lossy(field_of(item, "text", span)?.as_bytes(span, "a text")?)
            .into_owned();
        files.push(FrontFile {
            path: field_of(item, "path", span)?
                .as_str(span, "a path")?
                .to_string(),
            name: field_of(item, "name", span)?
                .as_str(span, "a module")?
                .to_string(),
            text,
        });
    }
    let millis = |name: &str| -> Result<Duration, Diagnostic> {
        let ms = field_of(v, name, span)?.as_int(span, name)?;
        Ok(Duration::from_millis(u64::try_from(ms).unwrap_or(0)))
    };
    Ok(HandedFront {
        dump,
        files,
        read: millis("read_ms")?,
        front: millis("front_ms")?,
        write_back: millis("file_ms")?,
        cached: field_of(v, "cached", span)?.as_bool(span, "whether the load was cached")?,
    })
}

/// One source as a caller's load found it: its path, the module the front end named it, and the text
/// it read.
#[derive(Clone, Debug)]
pub struct FrontFile {
    pub path: String,
    pub name: String,
    pub text: String,
}

/// The load over a front end a caller already ran: `ply test` walks the tree and runs the compiler
/// in order to report on both, so this side is handed the answer — the frames, and every source
/// they name in the order their ids run — rather than walking and analysing a second time.
pub fn load_over_front(path: &Path, handed: &HandedFront) -> Result<Loaded, LoadError> {
    let mut sources = SourceMap::new();
    let mut states = Vec::with_capacity(handed.files.len());
    for file in &handed.files {
        let path = PathBuf::from(&file.path);
        let module = ModuleName::from_dotted(&file.name);
        let content = ContentHash::of(file.text.as_bytes());
        let source = sources.add(&path, file.text.clone());
        let stamp = crate::load::stamp_of(&path);
        states.push(FileState {
            path,
            module: module.clone(),
            source,
            text: Arc::from(file.text.as_str()),
            content,
            stamp,
            shipped: crate::shelf::source(&module).is_some(),
        });
    }
    Driver {
        root: project_root(path),
        incremental: handed.cached,
        answer: Some(handed.dump.clone()),
        project: SourceMap::new(),
        manifest: None,
        packages: Vec::new(),
        sources,
        files: states,
        phases: Phases {
            read: handed.read,
            front: handed.front,
            write_back: handed.write_back,
        },
    }
    .finish()
}

/// The load a program runs of a program of its own: the walk and the whole front end, from nothing.
pub(crate) fn run(path: &Path) -> Result<Loaded, LoadError> {
    let (root, discovered) = discover(path).map_err(LoadError::bare)?;
    Driver::new(root, discovered)?.finish()
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
    let mut parts: Vec<Option<(Vec<u8>, Claims)>> = (0..modules.len())
        .map(|i| {
            let bytes = store.as_deref()?.claims_part(keys[i])?;
            let part = ply_eval::codec::decode(&bytes).ok()?;
            let read = claims_part(&part, modules[i].source).ok()?;
            Some((bytes, read))
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
        let answer = producer::claims(&sources, &loaded.front.packages, &mod_pkg)
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
        let unread = |e: decode::Error| format!("its answer does not read: {e}");
        let answer = At::new("`front.claims`' answer", &answer);
        for (&i, part) in asked
            .iter()
            .zip(split_claims(answer, &asked, &items, &laws).map_err(unread)?)
        {
            let read = claims_part(&part, modules[i].source).map_err(unread)?;
            let bytes = ply_eval::codec::encode(&part)
                .map_err(|e| format!("a module's claims do not encode: {e}"))?;
            parts[i] = Some((bytes, read));
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

/// Each asked module's part, as the store keeps it: its position among `asked`, which its spans
/// index, and the claims it owns.
fn split_claims(
    answer: At<'_>,
    asked: &[usize],
    items: &HashMap<&str, usize>,
    laws: &HashMap<&str, usize>,
) -> Result<Vec<PlyValue>, decode::Error> {
    let mut owned: Vec<Vec<PlyValue>> = vec![Vec::new(); asked.len()];
    for claim in answer.list()? {
        let c = claim.ctor()?;
        let owner = match c.name() {
            "ClaimLaw" => laws.get(c.arg(0)?.field("key")?.utf8()?),
            _ => items.get(c.arg(0)?.field("name")?.utf8()?),
        };
        let at = owner
            .and_then(|i| asked.iter().position(|j| j == i))
            .ok_or_else(|| claim.error("a claim of no module asked"))?;
        owned[at].push(claim.value().clone());
    }
    Ok(owned
        .into_iter()
        .enumerate()
        .map(|(at, claims)| {
            crate::payload::record(vec![
                ("at", crate::payload::count(at)),
                ("claims", PlyValue::list(claims)),
            ])
        })
        .collect())
}

/// A module's claims span only it, so every position up to its own reads as its source.
fn claims_part(part: &PlyValue, source: SourceId) -> Result<Claims, decode::Error> {
    let part = At::new("a module's claims", part);
    let at: usize = part.field("at")?.number()?;
    read_claims(part.field("claims")?, &vec![source; at + 1])
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

struct Driver {
    root: PathBuf,
    incremental: bool,
    /// The front end a caller already ran, which [`Driver::ask_the_port`] reads rather than pulls.
    /// Nothing here walks or analyses when it is set: the caller did both, and the answer is the
    /// one the report is about.
    answer: Option<String>,
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
/// One package root as a walk found it: the key it is named by, and the directory its files are in.
/// They are the same thing for a path dependency, and a fetched tree's directory for a git one.
fn read_package(root: &str, dir: &Path) -> DepPackage {
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
/// The directories `ply vendor` wrote, by the want each answers: `<want>\t<dir>` a line, read once
/// per walk. A project that was not vendored has no index and reads nothing extra.
fn vendored(root: &Path) -> Vec<(String, PathBuf)> {
    let Ok(text) = std::fs::read_to_string(root.join("vendor").join("index")) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| {
            let (want, dir) = line.split_once('\t')?;
            Some((want.to_string(), root.join(dir)))
        })
        .collect()
}

fn walk_packages(
    root: &Path,
    manifest: &Option<(PathBuf, Arc<str>)>,
) -> Result<Vec<DepPackage>, String> {
    let root_key = root.to_string_lossy().into_owned();
    let vendored = vendored(root);
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
            // A git want is fetched first, and is named by its key rather than by where the fetch
            // put it: the front end judges roots, and a tree's address is the walker's business.
            // A vendored copy answers the want whether it is a path or a git key, and asks for
            // nothing: that is what makes a vendored checkout build with no cache and no git.
            let tree = match vendored.iter().find(|(want, _)| *want == w) {
                Some((_, dir)) => dir.clone(),
                None if w.starts_with("git+") => match crate::vcs::fetch(root, &w) {
                    Ok(dir) => dir,
                    Err(diagnostic) => return Err(diagnostic.message.clone()),
                },
                None => PathBuf::from(&w),
            };
            let package = read_package(&w, &tree);
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

impl Driver {
    fn new(root: PathBuf, discovered: Vec<Discovered>) -> Result<Driver, LoadError> {
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
            incremental: false,
            answer: None,
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

        let warnings = front
            .diagnostics
            .iter()
            .filter(|d| !self.in_shipped(d))
            .cloned()
            .collect();
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
                incremental: self.incremental,
                phases: self.phases,
                warnings,
            },
            promised,
        })
    }

    fn ask_the_port(&mut self) -> Result<Front, LoadError> {
        if let Some(dump) = self.answer.take() {
            let ids: Vec<SourceId> = self.files.iter().map(|f| f.source).collect();
            let started = Instant::now();
            let answer = ply_ty::read_front(&dump, &ids).map_err(|e| {
                self.seam_failed(&format!("the front end's answer does not read: {e}"))
            });
            self.phases.front += started.elapsed();
            return answer;
        }
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
        let pulled = ply_codegen::c::producer::front_pulling_std_with(&own, shelf, &packages)
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
