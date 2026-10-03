//! What `ply build` builds, hashes and writes, as the program in `crates/ply-cli/ply` performs it,
//! over the front end that program ran and handed over.
//!
//! The emitter stays here: a compiled unit is not a value a program can hold. Which entry is
//! built, what the container carries, where it lands and what the report says are the program's,
//! in `crates/ply-cli/ply/build.ply`.

use crate::artifact::{self, Built};

use crate::driver::{
    LoadedAnalysis, load_over_analysis, load_over_analysis_in, loaded_analysis_of,
};
use crate::hosts::Lent;
use crate::load::{LoadError, Loaded};
use crate::payload::{count, diags_value, option, places_value, record};
use ply_eval::host::{HostAnswer, HostHandler, HostRequest, HostRuntime, Linearity};
use ply_eval::{DefHash, DefInfo, Diagnostic, Severity, Span, Symbol, Value as PlyValue, codes};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// The effect `crates/ply-cli/ply/build.ply` declares. It is lent to that one entry and nowhere
/// else: `ply run` and the shipped program open artifacts too, and neither is a program.
const EFFECT: &str = "builder";

const HERMETIC: &str = "hermetic_builder";

const OPERATIONS: [(&str, &str); 5] = [
    ("loaded", "ply_machine::builder::loaded"),
    ("made", "ply_machine::builder::made"),
    ("previous", "ply_machine::builder::previous"),
    ("unit", "ply_machine::builder::unit"),
    ("git", "ply_machine::builder::git"),
];

const HERMETIC_OPERATIONS: [(&str, &str); 5] = [
    ("loaded", "ply_machine::builder::hermetic::loaded"),
    ("made", "ply_machine::builder::hermetic::made"),
    ("previous", "ply_machine::builder::hermetic::previous"),
    ("unit", "ply_machine::builder::hermetic::unit"),
    ("git", "ply_machine::builder::hermetic::git"),
];

/// An entry into a compiled unit does not nest on a thread, and a build enters the emitter's while
/// the program's own entry is live; the stack is a front end's, not a report's.
const BUILD_STACK: usize = 256 << 20;

/// The ops and the one handler serving them, and the hermetic half's.
pub fn lent() -> Vec<Lent> {
    let mut ops = lent_by(false);
    ops.extend(lent_by(true));
    ops
}

fn lent_by(hermetic: bool) -> Vec<Lent> {
    let site: Arc<dyn HostHandler> = Arc::new(Site {
        hermetic,
        program: Mutex::new(None),
    });
    let (effect, operations) = if hermetic {
        (HERMETIC, HERMETIC_OPERATIONS)
    } else {
        (EFFECT, OPERATIONS)
    };
    // A load is a function of the root and front it is handed, a build of the names it is given.
    operations
        .into_iter()
        .map(|(op, path)| {
            let op = if hermetic {
                crate::hosts::hermetic_op(effect, op, Linearity::Repeatable, path)
            } else {
                crate::hosts::privileged_op(effect, op, Linearity::Repeatable, path)
            };
            (op, Arc::clone(&site))
        })
        .collect()
}

struct Site {
    /// Answers from what it is handed alone: it measures no binary and fetches nothing.
    hermetic: bool,
    /// The last load, which the ops after it build from.
    program: Mutex<Option<Result<Loaded, LoadError>>>,
}

impl HostHandler for Site {
    fn call(&self, _: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        let span = req.span;
        let value = match (req.op.op.as_str(), req.args) {
            ("loaded", [path, front]) => {
                let path = PathBuf::from(path.as_str(span, "the program's root")?);
                let front = loaded_analysis_of(front, span)?;
                let loaded = self.load(&path, &front);
                self.loaded(loaded)
            }
            ("made", [entry, startup, reaches]) => self.made(
                entry.as_str(span, "an entry point's name")?,
                &texts(startup, span)?,
                reaches.as_bool(span, "whether the closure is wanted")?,
            ),
            ("previous", [path, bytes]) => {
                let path = PathBuf::from(path.as_str(span, "the deployed artifact's path")?);
                let bytes =
                    crate::payload::option_of(bytes, "the deployed artifact's bytes", span)?
                        .map(|b| b.as_bytes(span, "the deployed artifact's bytes"))
                        .transpose()?;
                answered(
                    read_deployed(&path, bytes.map(|b| &b[..])).map(|old| deployed_value(&old)),
                )
            }
            ("unit", [names]) => self.unit(&texts(names, span)?),
            ("git", [_, _]) if self.hermetic => answered(Err(hermetic_fetch())),
            ("git", [root, key]) => {
                let root = PathBuf::from(root.as_str(span, "the project's root")?);
                let key = key.as_str(span, "a git dependency's key")?.to_string();
                // A fetch is I/O and a subprocess, not an entry into a compiled body: it runs on
                // this thread, and its failure is the dependency's trouble rather than a refusal
                // of the whole program.
                answered(
                    crate::vcs::fetch(&root, &key)
                        .map(|dir| PlyValue::str(dir.display().to_string())),
                )
            }
            (other, _) => return Err(crate::hosts::unserved(EFFECT, other, span)),
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
    /// A load is of the root and front end it is handed, whatever was loaded before.
    fn load(
        &self,
        path: &Path,
        front: &LoadedAnalysis,
    ) -> std::sync::MutexGuard<'_, Option<Result<Loaded, LoadError>>> {
        let mut program = self.program.lock().unwrap_or_else(|e| e.into_inner());
        *program = Some(if self.hermetic {
            load_over_analysis_in(crate::load::tidy(path), front)
        } else {
            load_over_analysis(path, front)
        });
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
        PlyValue::ctor(
            "Ok",
            vec![record(vec![
                ("root", PlyValue::str(loaded.root.display().to_string())),
                ("mains", crate::drive::mains_value(loaded)),
                ("modules", crate::drive::modules_value(loaded)),
                ("places", places_value(&loaded.sources)),
                ("pins", pins_value(&loaded.front.pins)),
                // The running binary's size is no part of what a hermetic build answers.
                (
                    "binary_bytes",
                    option(binary_bytes().filter(|_| !self.hermetic).map(size)),
                ),
                ("version", PlyValue::str(env!("CARGO_PKG_VERSION"))),
            ])],
        )
    }

    /// The compiled unit for a set of definitions, with no entry: what a library's `.plyz`
    /// carries. Nothing but the unit comes back — a library has no artifact to open.
    fn unit(&self, names: &[String]) -> PlyValue {
        let program = self.program.lock().unwrap_or_else(|e| e.into_inner());
        let Some(Ok(loaded)) = &*program else {
            return answered(Err(unloaded()));
        };
        // The refusals carry their own diagnostics; a build reports the first, as `made` does.
        answered(
            aside(|| {
                crate::artifact::library_unit(loaded, names)
                    .map_err(|ds| ds.into_iter().next().unwrap_or_else(unloaded))
            })
            .map(|unit| {
                record(vec![
                    (
                        "head",
                        record(vec![
                            ("frontend", PlyValue::bytes(unit.frontend)),
                            ("runtime", PlyValue::bytes(unit.runtime)),
                            ("stdlib", PlyValue::bytes(unit.stdlib)),
                            ("entry", PlyValue::bytes([])),
                        ]),
                    ),
                    ("count", count(1)),
                    ("payload", PlyValue::bytes(&unit.payload)),
                ])
            }),
        )
    }

    fn made(&self, entry: &str, startup: &[String], reaches: bool) -> PlyValue {
        let program = self.program.lock().unwrap_or_else(|e| e.into_inner());
        let Some(Ok(loaded)) = &*program else {
            return answered(Err(unloaded()));
        };
        let built = aside(|| build(loaded, entry, startup));
        answered(built.map(|built| {
            let reached = reaches.then(|| loaded.hashes.reaches_among(&built.reachable));
            made_value(&built, reached.as_ref())
        }))
    }
}

// --- The build ----------------------------------------------------------------

/// Each dependency as the front end pinned it: its name, its version, and the digest of the
/// modules it contributed. `E0131`'s judgments decide what a package is, so the pin is the front
/// end's answer rather than anything this side derives from a path.
fn pins_value(pins: &[ply_eval::Pinned]) -> PlyValue {
    PlyValue::list(
        pins.iter()
            .map(|pin| {
                record(vec![
                    ("name", PlyValue::str(&pin.name)),
                    ("version", PlyValue::str(&pin.version)),
                    ("digest", PlyValue::str(&pin.digest)),
                ])
            })
            .collect(),
    )
}

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

/// `reached` is what each reachable definition reaches, when the caller asked for it.
fn made_value(built: &Built, reached: Option<&BTreeMap<Symbol, BTreeSet<Symbol>>>) -> PlyValue {
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
    let reached: Vec<PlyValue> = reached
        .into_iter()
        .flatten()
        .map(|(name, to)| {
            record(vec![
                ("name", PlyValue::str(name.as_str())),
                (
                    "reaches",
                    PlyValue::list(to.iter().map(|n| PlyValue::str(n.as_str())).collect()),
                ),
            ])
        })
        .collect();
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

/// The deployed artifact `--diff` names, from the bytes its caller read there.
fn read_deployed(path: &Path, bytes: Option<&[u8]>) -> Result<Deployed, Diagnostic> {
    let bytes = bytes.ok_or_else(|| artifact::unreadable(path))?;
    let (old, warnings) = artifact::decode(bytes, path)?;
    let digest = artifact::digest_of(bytes).unwrap_or([0; 32]);
    Ok(Deployed {
        digest: ply_std::short_digest(&digest),
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
fn hermetic_fetch() -> Diagnostic {
    Diagnostic::error(
        codes::CAPABILITY_UNDECLARED,
        "`hermetic_builder` fetches nothing, and the project names a git dependency",
    )
    .note("a program whose build fetches reads the network, and performs `builder`")
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
