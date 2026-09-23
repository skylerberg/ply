//! What `ply build` loads, builds, hashes and writes, as the program in `crates/ply-cli/ply`
//! performs it.
//!
//! The front end and the emitter stay here: a front end or a compiled unit is not a value a
//! program can hold. Which entry is built, what the
//! container carries, where it lands and what the report says are the program's, in
//! `crates/ply-cli/ply/build.ply`.

use crate::artifact::{self, Built};

use crate::hosts::Lent;
use crate::load::{LoadError, Loaded, load};
use crate::payload::{count, diags_value, option, places_value, record};
use ply_eval::Value as PlyValue;
use ply_eval::host::{
    Determinism, HostAnswer, HostHandler, HostOp, HostRequest, HostResource, HostRuntime, Linearity,
};
use ply_span::{Diagnostic, Severity, Span, Symbol, codes};
use ply_ty::{DefHash, DefInfo};
use std::path::Path;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// The effect `crates/ply-cli/ply/build.ply` declares. It is lent to that one entry and nowhere
/// else: `ply run` and the shipped program open artifacts too, and neither is a program.
const EFFECT: &str = "builder";

const OPERATIONS: [(&str, &str); 4] = [
    ("loaded", "ply_cli::build::loaded"),
    ("made", "ply_cli::build::made"),
    ("previous", "ply_cli::build::previous"),
    ("stored", "ply_cli::build::stored"),
];

/// An entry into a compiled unit does not nest on a thread, and a build enters the emitter's while
/// the program's own entry is live; the stack is a front end's, not a report's.
const BUILD_STACK: usize = 256 << 20;

/// What `ply build` is configured with, as plain data: the shell's parsed flags convert into
/// this.
#[derive(Clone, Debug, Default)]
pub struct BuildOptions {
    pub path: std::path::PathBuf,
    pub diff: Option<std::path::PathBuf>,
}

/// The ops and the one handler serving them. Nothing is read before the program asks: `loaded`
/// loads the path it is handed, `previous` the artifact it names.
pub fn lent() -> Vec<Lent> {
    let site: Arc<dyn HostHandler> = Arc::new(Site {
        program: Mutex::new(None),
        deployed: Mutex::new(None),
    });
    OPERATIONS
        .into_iter()
        .map(|(op, path)| (registration(op, path), Arc::clone(&site)))
        .collect()
}

fn registration(op: &str, path: &'static str) -> HostOp {
    HostOp {
        effect: Symbol::new(EFFECT),
        op: Symbol::new(op),
        resource: HostResource::Any,
        // A tree, a toolchain and a binary's own size are not functions of program state.
        determinism: Determinism::Nondeterministic,
        // One load serves the whole command, and a build is a function of the names it is given.
        linearity: Linearity::Repeatable,
        blocking: false,
        secrets: false,
        path,
    }
}

#[derive(Default)]
struct Site {
    program: Mutex<Option<Result<Loaded, LoadError>>>,
    /// `None` while `--diff`'s artifact has not been asked for.
    deployed: Mutex<Option<Result<Deployed, Diagnostic>>>,
}

impl HostHandler for Site {
    fn call(&self, _: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        let span = req.span;
        let value = match (req.op.op.as_str(), req.args) {
            ("loaded", [path]) => {
                let path = PathBuf::from(path.as_str(span, "the program's root")?);
                let loaded = self.load_once(path);
                self.loaded(loaded)
            }
            ("made", [entry, startup, reaches]) => self.made(
                entry.as_str(span, "an entry point's name")?,
                &texts(startup, span)?,
                reaches.as_bool(span, "whether the closure is wanted")?,
            ),
            ("previous", [diff]) => {
                let deployed = self.deployed_once(diff, span)?;
                answered(match deployed.as_ref().expect("the diff was read") {
                    Ok(old) => Ok(deployed_value(old)),
                    Err(diagnostic) => Err(diagnostic.clone()),
                })
            }
            ("stored", [path, body]) => answered(
                stored(
                    Path::new(path.as_str(span, "a file to write")?),
                    body.as_bytes(span, "an artifact")?,
                )
                .map(|()| PlyValue::Unit),
            ),
            (other, _) => return Err(unasked(other, span)),
        };
        Ok(HostAnswer::Value(value))
    }
}

fn texts(value: &PlyValue, span: Span) -> Result<Vec<String>, Diagnostic> {
    value
        .as_list(span, "the start-up roots")?
        .iter()
        .map(|item| {
            item.as_str(span, "a start-up root's name")
                .map(str::to_string)
        })
        .collect()
}

/// Work that enters the emitter's compiled unit, on a thread of its own: the program performing
/// this is itself inside an entry, and two entries do not nest on one thread. The budgets a run
/// gives a program are not the compiler's, here as on the thread the program runs on.
fn aside<T: Send>(work: impl FnOnce() -> Result<T, Diagnostic> + Send) -> Result<T, Diagnostic> {
    std::thread::scope(|scope| {
        let thread = std::thread::Builder::new()
            .stack_size(BUILD_STACK)
            .spawn_scoped(scope, || ply_codegen::rt::unbounded(work))
            .map_err(|e| unspawned(&e))?;
        match thread.join() {
            Ok(answer) => answer,
            Err(panic) => std::panic::resume_unwind(panic),
        }
    })
}

/// `Ok(v)` or `Err(Refusal)`, as the program reads an operation's answer.
fn answered(answer: Result<PlyValue, Diagnostic>) -> PlyValue {
    match answer {
        Ok(value) => PlyValue::ctor("Ok", vec![value]),
        Err(diagnostic) => PlyValue::ctor(
            "Err",
            vec![record(vec![
                ("diags", diags_value(std::slice::from_ref(&diagnostic))),
                ("places", PlyValue::list(Vec::new())),
            ])],
        ),
    }
}

// --- The program as it loaded -------------------------------------------------

impl Site {
    /// The deployed artifact `--diff` names is read at most once, on the op that asks for it.
    fn deployed_once(
        &self,
        diff: &PlyValue,
        span: Span,
    ) -> Result<std::sync::MutexGuard<'_, Option<Result<Deployed, Diagnostic>>>, Diagnostic> {
        let mut deployed = self.deployed.lock().unwrap_or_else(|e| e.into_inner());
        if deployed.is_none() {
            let read = match diff {
                PlyValue::Ctor { name, args } if name.as_str() == "Some" => args
                    .first()
                    .map(|v| {
                        v.as_str(span, "the deployed artifact's path")
                            .map(str::to_string)
                    })
                    .transpose()?,
                PlyValue::Ctor { name, .. } if name.as_str() == "None" => None,
                _ => None,
            };
            *deployed = Some(match read {
                Some(path) => read_deployed(Path::new(&path)),
                None => Err(undeployed()),
            });
        }
        Ok(deployed)
    }

    /// The load runs at most once, on the op that asks for it.
    fn load_once(
        &self,
        path: PathBuf,
    ) -> std::sync::MutexGuard<'_, Option<Result<Loaded, LoadError>>> {
        let mut program = self.program.lock().unwrap_or_else(|e| e.into_inner());
        if program.is_none() {
            *program = Some(load(&path));
        }
        program
    }

    fn loaded(
        &self,
        program: std::sync::MutexGuard<'_, Option<Result<Loaded, LoadError>>>,
    ) -> PlyValue {
        let program = program;
        let loaded = match program.as_ref().expect("the load ran") {
            Ok(loaded) => loaded,
            Err(err) => {
                return PlyValue::ctor(
                    "Err",
                    vec![record(vec![
                        ("diags", diags_value(&err.diagnostics)),
                        ("places", places_value(&err.sources)),
                    ])],
                );
            }
        };
        let defs: Vec<PlyValue> = loaded
            .check
            .defs
            .values()
            .filter(|d| !crate::shelf::is_shipped(&d.module))
            .map(def_value)
            .collect();
        PlyValue::ctor(
            "Ok",
            vec![record(vec![
                ("root", PlyValue::str(loaded.root.display().to_string())),
                ("defs", PlyValue::list(defs)),
                ("mains", crate::drive::mains_value(loaded)),
                ("modules", crate::drive::modules_value(loaded)),
                ("places", places_value(&loaded.sources)),
                ("binary_bytes", option(binary_bytes().map(size))),
                ("version", PlyValue::str(env!("CARGO_PKG_VERSION"))),
            ])],
        )
    }

    fn made(&self, entry: &str, startup: &[String], reaches: bool) -> PlyValue {
        let program = self.program.lock().unwrap_or_else(|e| e.into_inner());
        let Some(Ok(loaded)) = &*program else {
            return answered(Err(unloaded()));
        };
        let built = aside(|| build(loaded, entry, startup));
        answered(built.map(|built| made_value(&built, reaches)))
    }
}

fn def_value(def: &DefInfo) -> PlyValue {
    record(vec![
        ("name", PlyValue::str(def.name.as_str())),
        ("simple", PlyValue::str(def.simple_name.as_str())),
        ("module", PlyValue::str(def.module.as_str())),
        (
            "at",
            record(vec![
                ("module", PlyValue::Int(i64::from(def.span.source.0))),
                ("start", PlyValue::Int(i64::from(def.span.start))),
                ("end", PlyValue::Int(i64::from(def.span.end))),
            ]),
        ),
        ("arity", count(arity(def))),
    ])
}

fn arity(def: &DefInfo) -> usize {
    match &def.scheme.ty {
        ply_ty::ty::Type::Fn { params, .. } => params.len(),
        _ => 0,
    }
}

// --- The build ----------------------------------------------------------------

fn build(loaded: &Loaded, entry: &str, startup: &[String]) -> Result<Built, Diagnostic> {
    let named = |name: &str| {
        loaded
            .check
            .defs
            .get(&Symbol::new(name))
            .ok_or_else(|| unnamed(name))
    };
    let entry = named(entry)?;
    let mut roots: Vec<&DefInfo> = Vec::with_capacity(startup.len());
    for root in startup {
        roots.push(named(root)?);
    }
    artifact::build(loaded, entry, &roots).map_err(first_of)
}

fn made_value(built: &Built, reaches: bool) -> PlyValue {
    let artifact = &built.artifact;
    let sections: Vec<PlyValue> = artifact
        .sections()
        .into_iter()
        .map(|(name, records, payload)| {
            record(vec![
                ("name", PlyValue::bytes(name.as_bytes())),
                ("count", PlyValue::Int(i64::from(records))),
                ("payload", PlyValue::bytes(payload)),
            ])
        })
        .collect();
    let reached: Vec<PlyValue> = if reaches {
        built
            .closure
            .iter()
            .map(|(name, to)| {
                record(vec![
                    ("name", PlyValue::str(name)),
                    (
                        "reaches",
                        PlyValue::list(to.iter().map(PlyValue::str).collect()),
                    ),
                ])
            })
            .collect()
    } else {
        Vec::new()
    };
    record(vec![
        ("entry", PlyValue::str(built.entry_name.as_str())),
        ("entry_hash", PlyValue::str(artifact.entry.to_hex())),
        (
            "startup",
            PlyValue::list(
                built
                    .startup
                    .iter()
                    .map(|name| PlyValue::str(name.as_str()))
                    .collect(),
            ),
        ),
        (
            "head",
            record(vec![
                ("frontend", PlyValue::bytes(artifact.frontend)),
                ("runtime", PlyValue::bytes(artifact.runtime)),
                (
                    "body_encoding",
                    PlyValue::Int(i64::from(artifact.body_encoding)),
                ),
                ("stdlib", PlyValue::bytes(artifact.std)),
                ("entry", PlyValue::bytes(artifact.entry.0)),
            ]),
        ),
        ("sections", PlyValue::list(sections)),
        ("definitions", count(artifact.bodies.len())),
        ("names", named_value(&artifact.names)),
        ("unit", PlyValue::Bool(artifact.has_unit())),
        ("reaches", PlyValue::list(reached)),
        ("warnings", diags_value(&built.warnings)),
    ])
}

/// Sorted and carrying no pair twice, which is what the program's diff reads them as.
fn named_value(names: &[(String, DefHash)]) -> PlyValue {
    let mut pairs: Vec<(&str, String)> = names
        .iter()
        .map(|(name, hash)| (name.as_str(), hash.to_hex()))
        .collect();
    pairs.sort();
    pairs.dedup();
    PlyValue::list(
        pairs
            .into_iter()
            .map(|(name, hash)| {
                record(vec![
                    ("name", PlyValue::str(name)),
                    ("hash", PlyValue::str(hash)),
                ])
            })
            .collect(),
    )
}

// --- The artifact `--diff` measures against -----------------------------------

struct Deployed {
    digest: String,
    names: Vec<(String, DefHash)>,
    warnings: Vec<Diagnostic>,
}

fn read_deployed(path: &Path) -> Result<Deployed, Diagnostic> {
    let bytes = artifact::bytes_of(path)?;
    let (old, warnings) = artifact::decode(&bytes, path)?;
    let digest = artifact::digest_of(&bytes).unwrap_or([0; 32]);
    Ok(Deployed {
        digest: artifact::short(&digest),
        names: old.names,
        warnings,
    })
}

fn deployed_value(old: &Deployed) -> PlyValue {
    record(vec![
        ("digest", PlyValue::str(&old.digest)),
        ("names", named_value(&old.names)),
        ("warnings", diags_value(&old.warnings)),
    ])
}

// --- Where the artifact lands -------------------------------------------------

fn stored(path: &Path, bytes: &[u8]) -> Result<(), Diagnostic> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty())
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        return Err(unwritable(path, &e));
    }
    std::fs::write(path, bytes).map_err(|e| unwritable(path, &e))
}

fn unwritable(path: &Path, e: &std::io::Error) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("could not write `{}`: {e}", path.display()),
    )
    .primary(Span::DUMMY, "the artifact was built but not stored")
}

// --- Small things -------------------------------------------------------------

/// `None` when the running binary cannot be measured, which never fails a build.
fn binary_bytes() -> Option<u64> {
    std::env::current_exe()
        .and_then(std::fs::metadata)
        .map(|m| m.len())
        .ok()
}

fn size(n: u64) -> PlyValue {
    PlyValue::Int(n as i64)
}

fn first_of(diagnostics: Vec<Diagnostic>) -> Diagnostic {
    diagnostics
        .into_iter()
        .find(|d| d.severity == Severity::Error)
        .unwrap_or_else(|| {
            Diagnostic::error(
                codes::INTERNAL_ERROR,
                "the artifact could not be built, and nothing said why",
            )
        })
}

#[cold]
fn unspawned(e: &std::io::Error) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("the build could not be started on a thread of its own: {e}"),
    )
    .primary(Span::DUMMY, "nothing was built")
}

#[cold]
fn unloaded() -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        "a build was asked for over a program that did not load",
    )
    .note("the load's own refusal is what `ply build` answers with; this is Ply's fault")
}

#[cold]
fn undeployed() -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        "a deployed artifact was asked for and `--diff` named none",
    )
    .note("the flag and the report that reads it are written together; this is Ply's fault")
}

#[cold]
fn unnamed(name: &str) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("`{name}` was chosen to build and this program declares no such definition"),
    )
    .primary(Span::DUMMY, "the choice and the program disagree")
    .note("the names offered and the name picked are written together; this is Ply's fault")
}

#[cold]
fn unasked(op: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("`{EFFECT}.{op}` reached the binding, and `ply build` serves no such operation"),
    )
    .primary(span, "this perform reached `ply build`")
    .note("the effect and its handler are written together; this is Ply's fault")
}
