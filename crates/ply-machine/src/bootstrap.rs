//! The archive `ply bootstrap` writes: the front end emitted as the C that builds it, and the two
//! files it lands.
//!
//! Emitting is Rust's: the emitter is not something to re-enter from inside a running program.
//! What either run *says*, and the manifest itself, are the program's, in
//! `crates/ply-cli/ply/bootstrap.ply`; this hands it the emission as a value and lands the
//! document it answers with.

use crate::hosts::Lent;
use crate::payload::{count, diags_value, option, places_value, record, strings};
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

/// One registration per operation: the emission, which has already run, and the landing.
const OPERATIONS: [(&str, &str); 2] = [
    ("emitted", "ply_cli::bootstrap::emitted"),
    ("land", "ply_cli::bootstrap::land"),
];

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

fn manifest_path(args: &BootstrapOptions) -> PathBuf {
    args.out.join("manifest.json")
}

fn c_path(args: &BootstrapOptions) -> PathBuf {
    args.out.join("frontend.c")
}

/// What the emission came to, as `bootstrap.ply` reads it.
#[derive(Clone)]
struct Emitted {
    source: String,
    artifact: String,
    definitions: usize,
    refusals: usize,
    bytes: usize,
    at: String,
    recorded: Option<String>,
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

    fn land(&self, options: &PlyValue, document: &str, span: Span) -> Result<PlyValue, Diagnostic> {
        let o = options_of(options, span)?;
        let manifest = manifest_path(&o);
        let c = c_path(&o);
        Ok(match std::fs::write(&manifest, document) {
            Ok(()) => PlyValue::ctor(
                "Ok",
                vec![strings(
                    [c.display().to_string(), manifest.display().to_string()]
                        .iter()
                        .map(String::as_str),
                )],
            ),
            Err(e) => PlyValue::ctor(
                "Err",
                vec![PlyValue::str(format!("{}: {e}", manifest.display()))],
            ),
        })
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
            "land" => {
                let options = req
                    .args
                    .first()
                    .ok_or_else(|| unregistered("land", req.span))?;
                let document = req
                    .args
                    .get(1)
                    .ok_or_else(|| unregistered("land", req.span))?
                    .as_str(req.span, "the manifest to write")?;
                self.land(options, document, req.span)?
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
    let source = source_digest(front);
    // Without the module texts the port answers no bodies, and the archive would be empty.
    let src: &'static ply_codegen::Source = Box::leak(Box::new(
        ply_codegen::Source::from_front(front, ply_codegen::emit_keys(front))
            .with_texts(crate::support::module_texts(&loaded.check, &loaded.sources)),
    ));
    let names: Vec<String> = src.functions();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let (text, refused) = match ply_codegen::c::produce(src, &refs) {
        Ok(produced) => (produced.text, produced.refused),
        Err(e) => return Err(Refused::bare(unemitted(&format!("{e:#}")))),
    };
    let emitted = Emitted {
        source,
        artifact: blake3::hash(text.as_bytes()).to_hex().to_string(),
        definitions: names.len(),
        refusals: refused.len(),
        bytes: text.len(),
        at: manifest_path(args).display().to_string(),
        recorded: None,
    };
    if args.verify {
        return Ok(Emitted {
            recorded: std::fs::read_to_string(manifest_path(args)).ok(),
            ..emitted
        });
    }
    if let Err(e) = std::fs::create_dir_all(&args.out) {
        return Err(Refused::bare(unwritten(&args.out, &e.to_string())));
    }
    // The C first, so a manifest never describes an artifact that is not on disk.
    let c = c_path(args);
    if let Err(e) = std::fs::write(&c, &text) {
        return Err(Refused::bare(unwritten(&c, &e.to_string())));
    }
    Ok(emitted)
}

/// Every definition's name and hash, sorted by name, so moving a definition between files does
/// not rename the compiler.
fn source_digest(front: &ply_ty::Front) -> String {
    let mut pairs: Vec<(String, String)> = front
        .hashes
        .defs
        .iter()
        .map(|(name, hash)| (name.to_string(), hash.to_hex()))
        .collect();
    pairs.sort();
    let mut hasher = blake3::Hasher::new();
    for (name, hash) in &pairs {
        hasher.update(name.as_bytes());
        hasher.update(&[0]);
        hasher.update(hash.as_bytes());
        hasher.update(&[0]);
    }
    hasher.finalize().to_hex().to_string()
}

// --- The values the program reads ------------------------------------------------

fn emitted_value(e: &Emitted) -> PlyValue {
    record(vec![
        ("source", PlyValue::str(&e.source)),
        ("artifact", PlyValue::str(&e.artifact)),
        ("definitions", count(e.definitions)),
        ("refusals", count(e.refusals)),
        ("bytes", count(e.bytes)),
        ("at", PlyValue::str(&e.at)),
        ("recorded", option(e.recorded.as_deref().map(PlyValue::str))),
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
