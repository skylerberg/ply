//! The archive `ply bootstrap` writes: the front end emitted as the bundle the runtime builds it
//! from — the C gzipped as `unit.c.gz` beside the `SOURCES.digest` of the modules it came from.
//!
//! Emitting is Rust's: the emitter is not something to re-enter from inside a running program.
//! What either run *says* is the program's, in `crates/ply-cli/ply/bootstrap.ply`; this hands it
//! the emission as a value, and what an archive already on disk answered for `--verify`.

use crate::hosts::Lent;
use crate::payload::{count, diags_value, option, places_value, record};
use ply_eval::Value as PlyValue;
use ply_eval::host::{
    Determinism, HostAnswer, HostHandler, HostOp, HostRequest, HostResource, HostRuntime, Linearity,
};
use ply_span::{Diagnostic, SourceMap, Span, Symbol, codes};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The effect `crates/ply-cli/ply/bootstrap.ply` declares. It is lent to that one entry and
/// nowhere else.
const EFFECT: &str = "archive";

/// One registration for the one operation: the emission, which has already run.
const OPERATIONS: [(&str, &str); 1] = [("emitted", "ply_machine::bootstrap::emitted")];

/// What `ply bootstrap` is configured with, as plain data: the shell's parsed flags convert.
#[derive(Clone, Debug)]
pub struct BootstrapOptions {
    pub path: std::path::PathBuf,
    pub out: std::path::PathBuf,
    pub verify: bool,
    pub profile: String,
}

/// The emission runs here, before the program is entered: a handler is handed `&self`, and the
/// front end and the emitter are the compiler's own work.
pub fn lent() -> Vec<Lent> {
    let archive: Arc<dyn HostHandler> = Arc::new(Archive {
        done: std::sync::Mutex::new(None),
    });
    OPERATIONS
        .into_iter()
        .map(|(op, path)| (registration(op, path), Arc::clone(&archive)))
        .collect()
}

fn registration(op: &str, path: &'static str) -> HostOp {
    HostOp {
        effect: Symbol::new(EFFECT),
        op: Symbol::new(op),
        resource: HostResource::Any,
        // A tree and the files beside it are not functions of program state.
        determinism: Determinism::Nondeterministic,
        linearity: Linearity::AtMostOnce,
        // The work is done before the program is entered, so the handler answers, not dispatches.
        blocking: false,
        secrets: false,
        path,
    }
}

/// What the emission came to, as `bootstrap.ply` reads it. `recorded` is what the archive on
/// disk says, read for `--verify` and for nothing else.
#[derive(Clone)]
struct Emitted {
    source: String,
    artifact: String,
    definitions: usize,
    refusals: usize,
    bytes: usize,
    at: PathBuf,
    recorded: Option<(String, String)>,
}

/// Why nothing was emitted. `compilation` is a program that did not check, which is the user's to
/// fix and is tallied as such.
#[derive(Clone)]
struct Refused {
    diagnostics: Vec<Diagnostic>,
    sources: SourceMap,
    compilation: bool,
}

impl Refused {
    fn bare(diagnostic: Diagnostic) -> Refused {
        Refused {
            diagnostics: vec![diagnostic],
            sources: SourceMap::new(),
            compilation: false,
        }
    }
}

struct Archive {
    /// The emission, once the program has asked for it.
    done: std::sync::Mutex<Option<Result<Emitted, Refused>>>,
}

impl Archive {
    /// The emission runs when the program performs `emitted`, and once only.
    fn emitted(
        &self,
        options: &PlyValue,
        span: Span,
    ) -> Result<Result<Emitted, Refused>, Diagnostic> {
        let mut done = self.done.lock().unwrap_or_else(|e| e.into_inner());
        if done.is_none() {
            *done = Some(match options_of(options, span) {
                Ok(o) => emit(&o),
                Err(diagnostic) => Err(Refused::bare(diagnostic)),
            });
        }
        Ok(done.as_ref().unwrap().clone())
    }
}

impl HostHandler for Archive {
    fn call(&self, _: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        let value = match req.op.op.as_str() {
            "emitted" => {
                match self.emitted(req.args.first().unwrap_or(&PlyValue::Unit), req.span)? {
                    Ok(emitted) => PlyValue::ctor("Ok", vec![emitted_value(&emitted)]),
                    Err(why) => PlyValue::ctor("Err", vec![refusal_value(&why)]),
                }
            }
            other => return Err(unregistered(other, req.span)),
        };
        Ok(HostAnswer::Value(value))
    }
}

// --- Emitting -----------------------------------------------------------------

fn emit(args: &BootstrapOptions) -> Result<Emitted, Refused> {
    if let Err(diagnostic) = crate::support::select_profile(&args.profile) {
        return Err(Refused::bare(diagnostic));
    }
    let loaded = match crate::load::load(&args.path) {
        Ok(loaded) => loaded,
        Err(err) => {
            return Err(Refused {
                diagnostics: err.diagnostics,
                sources: err.sources,
                compilation: true,
            });
        }
    };
    // The load's own answer: asking again would run a second front end.
    let front = Box::leak(Box::new(loaded.front.clone()));
    let texts = crate::support::module_texts(&loaded.check, &loaded.sources);
    let modules: Vec<(String, String)> = texts.clone().into_iter().collect();
    let source = ply_codegen::c::producer::digest_of(&modules);
    let src: &'static ply_codegen::Source = Box::leak(Box::new(
        ply_codegen::Source::from_front(front, ply_codegen::emit_keys(front)).with_texts(texts),
    ));
    let names: Vec<String> = src.functions();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let (text, refused) = match ply_codegen::c::produce(src, &refs) {
        Ok(produced) => (produced.text, produced.refused),
        Err(e) => return Err(Refused::bare(unemitted(&format!("{e:#}")))),
    };
    let packed = match ply_codegen::c::bundle::pack(&text) {
        Ok(packed) => packed,
        Err(e) => return Err(Refused::bare(unemitted(&format!("{e:#}")))),
    };
    let emitted = Emitted {
        source,
        artifact: blake3::hash(&packed).to_hex().to_string(),
        definitions: names.len(),
        refusals: refused.len(),
        bytes: text.len(),
        at: args.out.clone(),
        recorded: None,
    };
    if args.verify {
        let recorded = ply_codegen::c::bundle::from_dir(&args.out).and_then(|bundle| {
            Some((
                bundle.sources_digest()?.to_string(),
                blake3::hash(bundle.unit_bytes()).to_hex().to_string(),
            ))
        });
        return Ok(Emitted {
            recorded,
            ..emitted
        });
    }
    if let Err(e) = ply_codegen::c::bundle::write(&args.out, &text, &emitted.source) {
        return Err(Refused::bare(unwritten(&args.out, &format!("{e:#}"))));
    }
    Ok(emitted)
}

// --- The values the program reads ------------------------------------------------

fn emitted_value(e: &Emitted) -> PlyValue {
    record(vec![
        ("source", PlyValue::str(&e.source)),
        ("artifact", PlyValue::str(&e.artifact)),
        ("definitions", count(e.definitions)),
        ("refusals", count(e.refusals)),
        ("bytes", count(e.bytes)),
        ("at", PlyValue::str(e.at.display().to_string())),
        (
            "recorded",
            option(e.recorded.as_ref().map(|(source, artifact)| {
                record(vec![
                    ("source", PlyValue::str(source)),
                    ("artifact", PlyValue::str(artifact)),
                ])
            })),
        ),
    ])
}

fn refusal_value(why: &Refused) -> PlyValue {
    record(vec![
        ("diags", diags_value(&why.diagnostics)),
        ("places", places_value(&why.sources)),
        ("compilation", PlyValue::Bool(why.compilation)),
    ])
}

#[cold]
fn unemitted(why: &str) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("the front end could not be emitted: {why}"),
    )
    .primary(Span::DUMMY, "this is Ply's fault, not the program's")
}

#[cold]
fn unwritten(path: &Path, why: &str) -> Diagnostic {
    Diagnostic::error(codes::RUNTIME_ERROR, format!("{}: {why}", path.display()))
        .primary(Span::DUMMY, "the archive could not be written")
}

#[cold]
fn unregistered(op: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("`{EFFECT}.{op}` is not an operation this command serves"),
    )
    .primary(span, "performed here")
    .note("this is a defect in Ply's host dispatch rather than in the program")
}

// The options record the program parsed: the path, the output directory, `--verify`, `--profile`.
fn options_of(v: &PlyValue, span: Span) -> Result<BootstrapOptions, Diagnostic> {
    use crate::payload::{field_of, str_list_at};
    let _ = str_list_at;
    Ok(BootstrapOptions {
        path: PathBuf::from(field_of(v, "path", span)?.as_str(span, "the program's root")?),
        out: PathBuf::from(field_of(v, "out", span)?.as_str(span, "the archive's directory")?),
        verify: field_of(v, "verify", span)?.as_bool(span, "verify")?,
        profile: field_of(v, "profile", span)?
            .as_str(span, "the toolchain profile")?
            .to_string(),
    })
}
