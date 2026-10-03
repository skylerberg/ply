//! The front end: the port parses, resolves and checks the program. The CLI runs it, seeded from
//! and filed into the front-end cache, and hands its answer here; a program loading a program runs
//! it here, from nothing.

use crate::load::{
    Discovered, Found, LoadError, Loaded, anchor, discover, project_root, tidy, unreadable,
};
use crate::payload::record;
use ply_codegen::c::producer::{self, KnownRows};
use ply_eval::decode::At;
use ply_eval::{Analysis, Diagnostic, ModuleName, SourceId, SourceMap, Span, Value, codes};
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

/// The front end a CLI ran and handed over: the compiler's answer, every source it names in the
/// order its ids run, what the CLI's own load cost, and whether it read and filed the front-end
/// cache for it. This side reads the answer rather than walking and analysing again.
#[derive(Clone, Debug)]
pub struct LoadedAnalysis {
    pub answer: Analysis,
    pub files: Vec<LoadedFile>,
    pub read: Duration,
    /// The CLI's front end, and reading its answer here.
    pub front: Duration,
    pub write_back: Duration,
    pub cached: bool,
}

/// The answer is read as it is handed over: a `Value` may not cross to another thread, and the load
/// it is for runs on a machine's own. [`load_over_analysis`] adds the files to a fresh map in order, so
/// file `i` is `SourceId(i)`.
pub fn loaded_analysis_of(v: &ply_eval::Value, span: Span) -> Result<LoadedAnalysis, Diagnostic> {
    use crate::payload::field_of;
    let mut files = Vec::new();
    for item in field_of(v, "files", span)?.as_list(span, "files")?.iter() {
        let text = String::from_utf8_lossy(field_of(item, "text", span)?.as_bytes(span, "a text")?)
            .into_owned();
        files.push(LoadedFile {
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
    let ids: Vec<SourceId> = (0..files.len()).map(|i| SourceId(i as u32)).collect();
    let started = Instant::now();
    let answer = ply_codegen::c::dump::read(field_of(v, "dump", span)?, &ids)
        .map_err(|e| port_failed(&format!("the front end's answer does not read: {e}")))?;
    Ok(LoadedAnalysis {
        answer,
        files,
        read: millis("read_ms")?,
        front: millis("front_ms")? + started.elapsed(),
        write_back: millis("file_ms")?,
        cached: field_of(v, "cached", span)?.as_bool(span, "whether the load was cached")?,
    })
}

/// One source as a caller's load found it: its path, the module the front end named it, and the text
/// it read.
#[derive(Clone, Debug)]
pub struct LoadedFile {
    pub path: String,
    pub name: String,
    pub text: String,
}

/// The load over a front end a caller already ran: `ply test` walks the tree and runs the compiler
/// in order to report on both, so this side is handed the answer — the tables, and every source
/// they name in the order their ids run — rather than walking and analysing a second time.
pub fn load_over_analysis(path: &Path, handed: &LoadedAnalysis) -> Result<Loaded, LoadError> {
    load_over_analysis_in(project_root(path), handed)
}

/// [`load_over_analysis`] with the root decided, so nothing on disk is read.
pub fn load_over_analysis_in(root: PathBuf, handed: &LoadedAnalysis) -> Result<Loaded, LoadError> {
    load_over_analysis_taken(root, handed.clone())
}

/// [`load_over_analysis_in`] taking the front it is handed, so nothing in it is copied.
pub fn load_over_analysis_taken(
    root: PathBuf,
    handed: LoadedAnalysis,
) -> Result<Loaded, LoadError> {
    let mut sources = SourceMap::new();
    let mut states = Vec::with_capacity(handed.files.len());
    for file in handed.files {
        let path = PathBuf::from(&file.path);
        let module = ModuleName::from_dotted(&file.name);
        let source = sources.add(&path, file.text);
        let text = sources
            .get(source)
            .map(|f| f.text.clone())
            .unwrap_or_else(|| "".into());
        states.push(FileState {
            path,
            shipped: crate::shelf::source(&module).is_some(),
            module,
            source,
            text,
        });
    }
    Driver {
        root,
        incremental: handed.cached,
        answer: Some(handed.answer),
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
        known: KnownRows::default(),
        keep: false,
        kept: None,
        seeded: 0,
    }
    .finish()
    .map(|seeded| seeded.loaded)
}

/// The load a program runs of a program of its own: the walk and the whole front end.
pub(crate) fn run(path: &Path) -> Result<Loaded, LoadError> {
    let (root, discovered) = discover(path).map_err(LoadError::bare)?;
    Ok(Driver::new(root, discovered)?.finish()?.loaded)
}

/// What a seeded load leaves for the next process: the rows that seed the next load of the same
/// program, and its front, which [`kept_front`] reads back in place of the front end.
pub struct Seeded {
    pub loaded: Loaded,
    pub rows: KnownRows,
    /// `None` when the answer would not encode.
    pub front: Option<Vec<u8>>,
    /// How many definitions the front end took from the rows it was seeded with.
    pub seeded: usize,
}

/// [`run`] seeded with `known`.
pub(crate) fn run_seeded(path: &Path, known: KnownRows) -> Result<Seeded, LoadError> {
    let (root, discovered) = discover(path).map_err(LoadError::bare)?;
    let mut driver = Driver::new(root, discovered)?;
    driver.known = known;
    driver.keep = true;
    driver.finish()
}

/// What [`Seeded::front`] is, as its `format` field says.
const KEPT: &str = "ply kept front 1";

/// A front [`Seeded::front`] kept, read back: every file it was over, and the front end's answer.
/// `None` when the bytes are not one.
pub fn kept_front(bytes: &[u8]) -> Option<LoadedAnalysis> {
    let started = Instant::now();
    let kept = ply_eval::codec::decode(bytes).ok()?;
    let at = At::new("a kept front", &kept);
    if at.field("format").ok()?.str().ok()? != KEPT {
        return None;
    }
    let files = at
        .field("files")
        .ok()?
        .items(|file| {
            Ok(LoadedFile {
                path: file.field("path")?.str()?.to_string(),
                name: file.field("name")?.str()?.to_string(),
                text: file.field("text")?.str()?.to_string(),
            })
        })
        .ok()?;
    let ids: Vec<SourceId> = (0..files.len()).map(|i| SourceId(i as u32)).collect();
    let answer = ply_codegen::c::dump::read(at.field("dump").ok()?.value(), &ids).ok()?;
    Some(LoadedAnalysis {
        answer,
        files,
        read: Duration::ZERO,
        front: started.elapsed(),
        write_back: Duration::ZERO,
        cached: false,
    })
}

struct FileState {
    path: PathBuf,
    module: ModuleName,
    source: SourceId,
    text: Arc<str>,
    /// Embedded in the binary rather than discovered on disk.
    shipped: bool,
}

struct Driver {
    root: PathBuf,
    incremental: bool,
    /// The front end a caller already ran, which [`Driver::ask_the_port`] takes rather than pulls.
    /// Nothing here walks or analyses when it is set: the caller did both, and the answer is the
    /// one the report is about.
    answer: Option<Analysis>,
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
    /// Whether this is a seeded load: seeded with `known`, which it replaces with the rows it
    /// published, and keeping its answer encoded as `kept` for [`Seeded::front`].
    keep: bool,
    known: KnownRows,
    kept: Option<Vec<u8>>,
    seeded: usize,
}

/// One dependency package of the walk: its manifest text, and its modules by file.
struct DepPackage {
    root: String,
    manifest: Option<String>,
    files: Vec<(PathBuf, String, Arc<str>)>,
}

/// The modules a package root holds, named relative to it; a root with no readable `ply.pkg`
/// answers nothing, and the front end's `E0135` says why. `root` is the key the walk names the
/// package by and `dir` the directory its files are in: the same for a path dependency, and a
/// fetched tree's directory for a git one.
fn read_package(root: &str, dir: &Path) -> DepPackage {
    // The walk records its paths tidied, so `./vendor/x` is stripped from them as `vendor/x`.
    let dir = crate::load::tidy(dir);
    let manifest = std::fs::read_to_string(dir.join("ply.pkg")).ok();
    let files = if manifest.is_some() {
        let mut out = Vec::new();
        if let Ok(paths) = crate::load::ply_files(&dir) {
            for path in paths {
                let Ok(relative) = path.strip_prefix(&dir) else {
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

/// What the manifests on hand ask for, round by round, until nothing is new: the closure of the
/// root package's dependencies, each read from its vendored copy, its path or its fetched tree.
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

/// What `modules` of the package at `root` embed, each read relative to its module's own file. Only
/// a module whose text names `embed` is parsed for it.
fn read_embeds(
    root: &str,
    modules: &[(&Path, String, &str)],
) -> Result<Vec<producer::ReadEmbed>, String> {
    let named: Vec<(String, String)> = modules
        .iter()
        .filter(|(_, _, text)| text.contains("embed"))
        .map(|(_, name, text)| (name.clone(), text.to_string()))
        .collect();
    if named.is_empty() {
        return Ok(Vec::new());
    }
    Ok(producer::embeds_wanted(root, &named)
        .map_err(|e| format!("{e:#}"))?
        .into_iter()
        .map(|(module, path, dir)| {
            let read = match modules.iter().find(|(_, name, _)| *name == module) {
                Some((file, _, _)) => read_embed(
                    &tidy(&file.parent().unwrap_or(Path::new("")).join(&path)),
                    dir,
                ),
                None => Err("its module's file was not read".to_string()),
            };
            producer::ReadEmbed {
                root: root.to_string(),
                module,
                path,
                dir,
                read,
            }
        })
        .collect())
}

type EmbedRead = Result<Vec<(String, Vec<u8>)>, String>;

fn unread(path: &Path, what: &str) -> EmbedRead {
    Err(format!("`{}` {what}", path.display()))
}

fn read_embed(target: &Path, dir: bool) -> EmbedRead {
    match std::fs::symlink_metadata(target) {
        Err(_) => unread(target, "does not exist"),
        Ok(meta) if meta.is_dir() && !dir => {
            unread(target, "is a directory, and `embed` takes a file")
        }
        Ok(meta) if meta.is_file() && dir => {
            unread(target, "is a file, and `embed_dir` takes a directory")
        }
        Ok(_) if dir => read_dir(target),
        Ok(_) => match std::fs::read(target) {
            Ok(bytes) => Ok(vec![(String::new(), bytes)]),
            Err(_) => unread(target, "could not be read"),
        },
    }
}

/// Every file under `dir`, by its path below it and in that order; nothing under a name starting
/// with `.` is read.
fn read_dir(dir: &Path) -> EmbedRead {
    let mut found = std::collections::BTreeMap::new();
    listed(dir, "", &mut found)?;
    let mut files = Vec::with_capacity(found.len());
    for (name, path) in found {
        match std::fs::read(&path) {
            Ok(bytes) => files.push((name, bytes)),
            Err(_) => return unread(&path, "could not be read"),
        }
    }
    Ok(files)
}

fn listed(
    dir: &Path,
    below: &str,
    found: &mut std::collections::BTreeMap<String, PathBuf>,
) -> Result<(), String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Err(format!("`{}` could not be listed", dir.display()));
    };
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        let path = dir.join(&name);
        let name = if below.is_empty() {
            name
        } else {
            format!("{below}/{name}")
        };
        if std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
            listed(&path, &name, found)?;
        } else {
            found.insert(name, path);
        }
    }
    Ok(())
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
                match std::fs::read_to_string(&file.path) {
                    Ok(text) => read.push(sources.add(&file.path, text)),
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
        for (file, &source) in discovered.iter().zip(&read) {
            match ModuleName::from_relative_path(&file.relative) {
                Ok(module) => files.push(FileState {
                    path: file.path.clone(),
                    module,
                    source,
                    text: sources
                        .get(source)
                        .map(|f| f.text.clone())
                        .unwrap_or_else(|| "".into()),
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
            known: KnownRows::default(),
            keep: false,
            kept: None,
            seeded: 0,
        };
        driver.place(&[]);
        Ok(driver)
    }

    fn finish(mut self) -> Result<Seeded, LoadError> {
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
            })
            .collect();
        let loaded = Loaded {
            root: self.root,
            files,
            sources: self.sources,
            check: published_order(&front),
            hashes: front.hashes.clone(),
            front: std::sync::Arc::new(front),
            frontend: FrontEnd {
                incremental: self.incremental,
                phases: self.phases,
                warnings,
            },
        };
        Ok(Seeded {
            loaded,
            rows: self.known,
            front: self.kept,
            seeded: self.seeded,
        })
    }

    fn ask_the_port(&mut self) -> Result<Analysis, LoadError> {
        if let Some(front) = self.answer.take() {
            return Ok(front);
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
    fn whole(&mut self) -> Result<Analysis, LoadError> {
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
        let embeds = self
            .embeds(&packages.root)
            .map_err(|e| self.seam_failed(&e))?;
        let pulled = if self.keep {
            let answered =
                producer::front_rows_pulling_std_with(&own, shelf, &packages, &embeds, &self.known)
                    .map_err(|e| self.seam_failed(&format!("{e:#}")))?;
            self.known = answered.rows;
            self.seeded = answered.seeded;
            answered.pulled
        } else {
            producer::front_pulling_std_with(&own, shelf, &packages, &embeds)
                .map_err(|e| self.seam_failed(&format!("{e:#}")))?
        };
        self.place(&pulled.modules);
        // The front end parses the root's modules, then the dependency modules in walk order,
        // then the pulled shelf; the manifest slots follow them all.
        let pulled_files = self.files.split_off(self.own());
        for package in &self.packages {
            for (path, name, text) in &package.files {
                let source = self.sources.add(path, text.to_string());
                self.files.push(FileState {
                    path: path.clone(),
                    module: ModuleName::from_dotted(name),
                    source,
                    text: text.clone(),
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
                path: path.clone(),
                module: ModuleName::from_dotted("pkg"),
                source,
                text: text.clone(),
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
                path,
                module: ModuleName::from_dotted("pkg"),
                source,
                text: Arc::from(text.as_str()),
                shipped: false,
            });
        }
        let ids: Vec<SourceId> = self.files.iter().map(|f| f.source).collect();
        if self.keep {
            self.kept = self.encoded(&pulled.dump);
        }
        ply_codegen::c::dump::read(&pulled.dump, &ids)
            .map_err(|e| self.seam_failed(&format!("the front end's answer does not read: {e}")))
    }

    /// The answer as [`kept_front`] reads it back: every file's place, module and text in the
    /// order the dump's module indices run, and the dump.
    fn encoded(&self, dump: &Value) -> Option<Vec<u8>> {
        let files = self
            .files
            .iter()
            .map(|f| {
                record(vec![
                    ("path", Value::str(f.path.to_string_lossy())),
                    ("name", Value::str(f.module.to_string())),
                    ("text", Value::str(&*f.text)),
                ])
            })
            .collect();
        ply_eval::codec::encode(&record(vec![
            ("format", Value::str(KEPT)),
            ("files", Value::list(files)),
            ("dump", dump.clone()),
        ]))
        .ok()
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
            let source = self.sources.add(&path, text);
            let text = self
                .sources
                .get(source)
                .map(|f| f.text.clone())
                .unwrap_or_else(|| "".into());
            self.files.push(FileState {
                path,
                module,
                source,
                text,
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

    /// What the root's modules and each dependency's embed, read as `ply`'s own load reads them.
    fn embeds(&self, root: &str) -> Result<Vec<producer::ReadEmbed>, String> {
        let own: Vec<(&Path, String, &str)> = self.files[..self.own()]
            .iter()
            .map(|f| (f.path.as_path(), f.module.to_string(), &*f.text))
            .collect();
        let mut out = read_embeds(root, &own)?;
        for package in &self.packages {
            let files: Vec<(&Path, String, &str)> = package
                .files
                .iter()
                .map(|(path, name, text)| (path.as_path(), name.clone(), &**text))
                .collect();
            out.extend(read_embeds(&package.root, &files)?);
        }
        Ok(out)
    }

    /// This compiler failing, rather than the program.
    fn seam_failed(&self, why: &str) -> LoadError {
        LoadError {
            sources: self.sources.clone(),
            diagnostics: vec![port_failed(why)],
        }
    }
}

fn port_failed(why: &str) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("the front end could not answer for this program: {why}"),
    )
    .primary(Span::DUMMY, "nothing was checked, so nothing is claimed")
    .note("this is Ply's fault: the compiler's own front end is what failed here")
    .note("the emitter comes from `crates/ply-compiler/bootstrap`; sources with no bundle are emitted by the one this binary carries")
}

/// Files in load order, then items as written; the port answers dependency-first.
fn published_order(front: &Analysis) -> ply_eval::CheckOutput {
    let mut check = front.check.clone();
    let mut defs = indexmap::IndexMap::with_capacity(check.defs.len());
    for (_, items) in &front.ordinals {
        for item in items {
            if let ply_eval::Ordinal::Fn(name, _) = item
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
