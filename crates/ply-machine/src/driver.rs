//! The front end's answer a program hands over: the compiler runs in the program that drives a
//! machine, and this side reads its answer over the files it names rather than running one.

use crate::load::{Found, LoadError, Loaded, project_root};
use ply_eval::{Analysis, Diagnostic, ModuleName, SourceId, SourceMap, Span, codes};
use std::path::{Path, PathBuf};
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
    let mut files = Vec::with_capacity(handed.files.len());
    let mut shipped = Vec::new();
    for file in handed.files {
        let path = PathBuf::from(&file.path);
        let source = sources.add(&path, file.text);
        if crate::shipped_modules::ships(&ModuleName::from_dotted(&file.name)) {
            shipped.push(source);
        }
        files.push(Found { path });
    }
    let front = handed.answer;
    if front.has_error() {
        return Err(LoadError {
            sources,
            diagnostics: front.diagnostics,
        });
    }
    // A warning inside a module the compiler ships is its maintainers', not this program's.
    let warnings = front
        .diagnostics
        .iter()
        .filter(|d| {
            !d.primary_span()
                .is_some_and(|span| shipped.contains(&span.source))
        })
        .cloned()
        .collect();
    Ok(Loaded {
        root,
        files,
        sources,
        check: published_order(&front),
        hashes: front.hashes.clone(),
        front: std::sync::Arc::new(front),
        frontend: FrontEnd {
            incremental: handed.cached,
            phases: Phases {
                read: handed.read,
                front: handed.front,
                write_back: handed.write_back,
            },
            warnings,
        },
    })
}

fn port_failed(why: &str) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("the front end could not answer for this program: {why}"),
    )
    .primary(Span::DUMMY, "nothing was checked, so nothing is claimed")
    .note("this is Ply's fault: the compiler's own front end is what failed here")
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
