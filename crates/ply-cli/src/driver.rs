//! The front end: the port parses, resolves and checks the program; its answer is kept per module.

use crate::load::{Discovered, LoadError, Loaded, anchor, discover, unreadable};
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

/// The port's lowered claims, kept per module as its front answer is: a module's part is reused
/// while its text and the texts of all it imports are unchanged, and the rest are asked with all
/// they import, so the port sees a closed program.
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
        let dump = ply_codegen::c::producer::claims_dump(&sources).map_err(|e| format!("{e:#}"))?;
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
    /// As the port answered them, or, until it has, as the last filed answer recorded them.
    imports: Vec<ModuleName>,
    /// Embedded in the binary rather than discovered on disk.
    shipped: bool,
}

type Fresh = Option<BTreeMap<ContentHash, String>>;

/// A module's part and the text it is filed as.
type Part = Option<(String, Front)>;

struct Keys {
    /// The emitter and the import graph, which fix the order the checker publishes in.
    program: ContentHash,
    modules: Vec<ContentHash>,
}

struct Driver<'s> {
    root: PathBuf,
    mode: Mode,
    store: Option<&'s mut Store>,
    whole_project: bool,
    /// The project's own files, which every placement of the shipped modules follows.
    project: SourceMap,
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
                    imports: Vec::new(),
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

        let mut driver = Driver {
            root,
            mode,
            store,
            whole_project,
            project: sources.clone(),
            sources,
            files,
            by_module: BTreeMap::new(),
            phases,
        };
        driver.place(Vec::new());
        Ok(driver)
    }

    fn finish(mut self) -> Result<Loaded, LoadError> {
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
        warnings.extend(
            front
                .diagnostics
                .iter()
                .filter(|d| !self.in_shipped(d))
                .cloned(),
        );
        warnings.extend(cache);

        let files = self.files.iter().map(|f| f.path.clone()).collect();
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

    fn ask_the_port(&mut self) -> Result<(Front, Fresh), LoadError> {
        ply_codegen::c::producer::ensure_default();
        let started = Instant::now();
        let answer = if self.keyed() {
            self.in_parts()
        } else {
            self.whole()
        };
        self.phases.front += started.elapsed();
        answer
    }

    fn keyed(&self) -> bool {
        self.mode == Mode::Incremental && self.store.is_some()
    }

    fn own(&self) -> usize {
        self.project.files().len()
    }

    /// The port pulls in the shipped modules the program imports, so its answer also places them.
    fn whole(&mut self) -> Result<(Front, Fresh), LoadError> {
        let own: Vec<(String, String)> = self.files[..self.own()]
            .iter()
            .map(|f| (f.module.to_string(), f.text.to_string()))
            .collect();
        let shelf: Vec<(String, String)> = ply_std::sources()
            .map(|(module, text)| (module.to_string(), text.to_string()))
            .collect();
        let pulled = ply_codegen::c::producer::front_pulling_std(&own, &shelf)
            .map_err(|e| self.seam_failed(&format!("{e:#}")))?;
        self.place(
            pulled
                .modules
                .iter()
                .map(|m| (ModuleName::from_dotted(m), Vec::new()))
                .collect(),
        );
        let ids: Vec<SourceId> = self.files.iter().map(|f| f.source).collect();
        let front = ply_ty::read_front(&pulled.dump, &ids)
            .map_err(|e| self.seam_failed(&format!("the front end's answer does not read: {e}")))?;
        Ok(self.filed(front))
    }

    /// An answer over every module: what each imports is now known, and it splits into parts.
    fn filed(&mut self, front: Front) -> (Front, Fresh) {
        if front.has_error() {
            return (front, None);
        }
        for file in &mut self.files {
            if let Some(info) = front.check.modules.get(file.module.as_symbol()) {
                file.imports = info.imports.clone();
            }
        }
        let fresh = self.keys().and_then(|keys| self.parts(&front, &keys));
        (front, fresh)
    }

    fn parts(&self, front: &Front, keys: &Keys) -> Fresh {
        let (program, parts) = front.split().ok()?;
        if parts.len() != self.files.len() {
            return None;
        }
        let mut filed = BTreeMap::from([
            (keys.program, ply_ty::write_front(&program, &[]).ok()?),
            self.imports_record(),
        ]);
        for ((key, part), file) in keys.modules.iter().zip(&parts).zip(&self.files) {
            if !part.check.modules.contains_key(file.module.as_symbol()) {
                return None;
            }
            filed.insert(*key, ply_ty::write_front(part, &[file.source]).ok()?);
        }
        Some(filed)
    }

    /// A module's part is reused while its text and the texts of all it imports are unchanged; the
    /// rest are asked with all they import, so the port sees a closed program.
    fn in_parts(&mut self) -> Result<(Front, Fresh), LoadError> {
        let recorded = self
            .store
            .as_deref()
            .and_then(|store| store.front_part(imports_key()));
        let Some(recorded) = recorded else {
            return self.whole();
        };
        if !self.hint(&recorded) {
            return self.whole();
        }
        let Some(keys) = self.keys() else {
            return self.whole();
        };
        let Some(((program_text, program), mut kept)) = self.filed_parts(&keys) else {
            return self.whole();
        };
        // A part's text fixes its imports, so a hint that keyed a part it does not match is stale.
        if !kept.iter().zip(&self.files).all(|(part, file)| {
            part.as_ref()
                .is_none_or(|(_, part)| keyed_alike(part, file))
        }) {
            return self.whole();
        }
        let missed: Vec<usize> = (0..kept.len()).filter(|&i| kept[i].is_none()).collect();

        if !missed.is_empty() {
            let asked = self.reaching(&missed);
            let front = self.ask(&asked)?;
            // An answer with errors does not split, so its diagnostics come from the whole program.
            if front.has_error() {
                return self.whole();
            }
            if asked.len() == self.files.len() {
                let front = self.filed(front);
                return if self.placed_alike() {
                    Ok(front)
                } else {
                    self.whole()
                };
            }
            let taken = front
                .split()
                .is_ok_and(|(_, fresh)| self.take(&asked, fresh, &mut kept));
            if !taken {
                return self.whole();
            }
        }

        let mut filed = BTreeMap::from([(keys.program, program_text), self.imports_record()]);
        let mut parts = Vec::with_capacity(kept.len());
        for (key, entry) in keys.modules.iter().zip(kept) {
            let Some((text, part)) = entry else {
                return self.whole();
            };
            filed.insert(*key, text);
            parts.push(part);
        }
        match Front::join(program, parts) {
            Ok(front) => Ok((front, (!missed.is_empty()).then_some(filed))),
            Err(_) => self.whole(),
        }
    }

    /// Imports as the last filed answer recorded them: exact for a module unchanged since, and for
    /// an edited one a guess its answer checks. `false` for a module with none on record.
    fn hint(&mut self, recorded: &str) -> bool {
        let recorded: BTreeMap<Symbol, Vec<ModuleName>> = recorded
            .lines()
            .filter_map(|line| {
                let mut words = line.split(' ');
                let module = words.next().filter(|m| !m.is_empty())?;
                Some((
                    Symbol::new(module),
                    words.map(ModuleName::from_dotted).collect(),
                ))
            })
            .collect();
        let own = self.own();
        for file in &mut self.files[..own] {
            let Some(imports) = recorded.get(file.module.as_symbol()) else {
                return false;
            };
            file.imports = imports.clone();
        }
        match shipped_by(&self.files[..own], |m| recorded.get(m.as_symbol()).cloned()) {
            Some(shipped) => {
                self.place(shipped);
                true
            }
            None => false,
        }
    }

    /// Whether the shipped modules placed from the hints are the ones the answered imports pull in.
    fn placed_alike(&self) -> bool {
        let own = self.own();
        let answered = |m: &ModuleName| {
            self.files[own..]
                .iter()
                .find(|f| &f.module == m)
                .map(|f| f.imports.clone())
        };
        shipped_by(&self.files[..own], answered).is_some_and(|pulled| {
            pulled
                .iter()
                .map(|(m, _)| m)
                .eq(self.files[own..].iter().map(|f| &f.module))
        })
    }

    /// Shipped modules follow the project's files, placed as the port pulls them in.
    fn place(&mut self, shipped: Vec<(ModuleName, Vec<ModuleName>)>) {
        let own = self.own();
        self.files.truncate(own);
        self.sources = self.project.clone();
        for (module, imports) in shipped {
            let Some(text) = ply_std::source(&module) else {
                continue;
            };
            let path = ply_std::pseudo_path(&module);
            let content = ContentHash::of(text.as_bytes());
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
                content,
                imports,
                shipped: true,
            });
        }
        self.by_module = self
            .files
            .iter()
            .enumerate()
            .map(|(i, f)| (f.module.as_symbol().clone(), i))
            .collect();
    }

    fn filed_parts(&self, keys: &Keys) -> Option<((String, Front), Vec<Part>)> {
        let store = self.store.as_deref()?;
        let text = store.front_part(keys.program)?;
        let program = ply_ty::read_front(&text, &[]).ok()?;
        let kept = keys
            .modules
            .iter()
            .zip(&self.files)
            .map(|(key, file)| {
                let text = store.front_part(*key)?;
                let part = ply_ty::read_front(&text, &[file.source]).ok()?;
                Some((text, part))
            })
            .collect();
        Some(((text, program), kept))
    }

    /// Files each asked module's part; `false` when one moved though nothing it reaches did, or
    /// imports other than its hint named.
    fn take(&self, asked: &[usize], fresh: Vec<Front>, kept: &mut [Part]) -> bool {
        if fresh.len() != asked.len() {
            return false;
        }
        for (&i, part) in asked.iter().zip(fresh) {
            let file = &self.files[i];
            let Ok(text) = ply_ty::write_front(&part, &[file.source]) else {
                return false;
            };
            let moved = kept[i].as_ref().is_some_and(|(filed, _)| *filed != text);
            if moved || !keyed_alike(&part, file) {
                return false;
            }
            kept[i] = Some((text, part));
        }
        true
    }

    /// Every module's imports, one line each, so the next load can key the parts before it asks.
    fn imports_record(&self) -> (ContentHash, String) {
        let mut text = String::new();
        for file in &self.files {
            text.push_str(file.module.as_str());
            for import in &file.imports {
                text.push(' ');
                text.push_str(import.as_str());
            }
            text.push('\n');
        }
        (imports_key(), text)
    }

    fn ask(&self, which: &[usize]) -> Result<Front, LoadError> {
        let sources: Vec<(String, String)> = which
            .iter()
            .map(|&i| {
                (
                    self.files[i].module.to_string(),
                    self.files[i].text.to_string(),
                )
            })
            .collect();
        let ids: Vec<SourceId> = which.iter().map(|&i| self.files[i].source).collect();
        let dump = ply_codegen::c::producer::front_dump(&sources)
            .map_err(|e| self.seam_failed(&format!("{e:#}")))?;
        ply_ty::read_front(&dump, &ids)
            .map_err(|e| self.seam_failed(&format!("the front end's answer does not read: {e}")))
    }

    fn reaching(&self, from: &[usize]) -> Vec<usize> {
        reaching(from, |i| self.files[i].imports.as_slice(), &self.by_module)
    }

    fn keys(&self) -> Option<Keys> {
        if !self.keyed() {
            return None;
        }
        let emitter = ply_codegen::c::producer::emitter();
        let mut graph = format!("order\0{emitter}").into_bytes();
        for file in &self.files {
            graph.push(0);
            graph.extend_from_slice(file.module.as_str().as_bytes());
            for imported in &file.imports {
                graph.push(1);
                graph.extend_from_slice(imported.as_str().as_bytes());
            }
        }
        let modules = (0..self.files.len())
            .map(|i| {
                let reached = self.reaching(&[i]).into_iter().map(|j| &self.files[j]);
                module_key(
                    "module",
                    &emitter,
                    &self.files[i].module,
                    reached.map(|f| (f.module.as_str(), &f.content)),
                )
            })
            .collect();
        Some(Keys {
            program: ContentHash::of(&graph),
            modules,
        })
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
        if let Some(parts) = fresh {
            store.put_front_parts(parts);
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

/// Where the imports every filed module answered with are kept.
fn imports_key() -> ContentHash {
    ContentHash::of(format!("imports\0{}", ply_codegen::c::producer::emitter()).as_bytes())
}

/// Whether a module's part names the imports its file was keyed by.
fn keyed_alike(part: &Front, file: &FileState) -> bool {
    part.check
        .modules
        .get(file.module.as_symbol())
        .is_some_and(|info| info.imports == file.imports)
}

/// The shipped modules `files` import, transitively, as the port pulls them in: a round of newly
/// imported ones at a time, each round in byte order. `None` for a module with no imports to go by.
fn shipped_by(
    files: &[FileState],
    imports_of: impl Fn(&ModuleName) -> Option<Vec<ModuleName>>,
) -> Option<Vec<(ModuleName, Vec<ModuleName>)>> {
    let mut present: BTreeSet<Symbol> =
        files.iter().map(|f| f.module.as_symbol().clone()).collect();
    let mut round: Vec<ModuleName> = files
        .iter()
        .flat_map(|f| f.imports.iter().cloned())
        .collect();
    let mut pulled = Vec::new();
    loop {
        let wanted: BTreeSet<Symbol> = round
            .iter()
            .filter(|m| ply_std::is_std(m) && !present.contains(m.as_symbol()))
            .map(|m| m.as_symbol().clone())
            .collect();
        if wanted.is_empty() {
            return Some(pulled);
        }
        round.clear();
        for name in wanted {
            let module = ModuleName::from_dotted(name.as_str());
            ply_std::source(&module)?;
            let imports = imports_of(&module)?;
            round.extend(imports.iter().cloned());
            present.insert(name);
            pulled.push((module, imports));
        }
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
