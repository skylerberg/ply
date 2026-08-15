//! Where a run's configuration comes from, and what it refuses at start-up.
//!
//! [`DECLARATION`] is the Ply source this registers against — the module
//! `std.config`, which ships with the compiler — and `HostRegistry::bind` is
//! what checks the two still agree: an operation renamed on either side is
//! `E0421` before anything runs.
//!
//! Three properties decide everything below, and the first is the whole of this
//! module's soundness.
//!
//! - **The sources are read exactly once, at bind time, into an immutable map,
//!   and the process environment is never consulted again.** That is one line of
//!   implementation carrying three properties. It is what makes `config.read[k]`
//!   honestly a *read*, so two definitions that read configuration do not
//!   conflict and the scheduler is right to run them side by side. It is what
//!   stops one test's `setenv` from being seen by another — the pooled-connection
//!   defect of ADR 0014 in a new costume, and the one W5 hazard that a
//!   `BTreeMap` closes outright. And it is what makes a run reproducible: the
//!   snapshot is a fact of the run, it is printed, and its non-secret half is in
//!   `ply hosts --json`.
//! - **A missing, malformed or wrongly-shaped value is a start-up refusal.**
//!   [`Snapshot::resolve`] runs before anything is bound. A service that starts,
//!   serves two hundred requests and then answers wrongly because a key was
//!   unset is the failure this module exists to prevent, and a service that
//!   refuses to start is strictly better than one that dies on its first
//!   request.
//! - **A `SSecret` key never leaves as a `String`.** [`Snapshot::get`] answers
//!   `None` for a key the schema declares `SSecret`, and [`Snapshot::secret`]
//!   answers `None` for a key it declares anything else. Without that the
//!   type-level containment of ADR 0015 §2 would be one call site away from a
//!   plaintext, and the `Secret` a program holds would be a suggestion. Without
//!   a schema there is no `SSecret` key, so a run with no `--config-schema` can
//!   read a password as a `String`; that is stated here because it is the exact
//!   boundary of the claim.
//!
//! The format is `KEY=VALUE`, one per line. TOML, YAML and JSON are refused for
//! one reason: the effect's return type is `Option<String>`, so a nested typed
//! format would carry structure this program cannot receive, and a format richer
//! than the type it feeds is a format whose extra structure is silently dropped.

use ply_eval::Value;
use ply_eval::host::{
    Determinism, HostAnswer, HostHandler, HostOp, HostRegistry, HostRequest, HostResource,
    HostRuntime, Linearity,
};
use ply_span::{Diagnostic, Span, Symbol, codes};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The Ply declaration the registrations below are checked against.
pub const DECLARATION: &str = ply_std::CONFIG;

/// The module the declaration ships as, which is what qualifies [`EFFECT`].
pub const MODULE: &str = "std.config";

/// The program-wide effect name. Effect names are qualified, so the `config`
/// declared by `std.config` is `std.config.config`, and a program that declares
/// its own `config` instead is `E0421` rather than silently acquiring the
/// environment.
pub const EFFECT: &str = "std.config.config";

/// What every rendering of a secret configuration value is. The same four
/// characters `ply_cli::db::Secret` prints, because they make the same promise
/// at two layers.
pub const REDACTED: &str = "****";

/// The two operations a program can perform.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Op {
    Get,
    Secret,
}

impl Op {
    pub const ALL: [Op; 2] = [Op::Get, Op::Secret];

    pub fn name(self) -> &'static str {
        match self {
            Op::Get => "get",
            Op::Secret => "secret",
        }
    }

    /// How a diagnostic names it.
    pub fn what(self) -> &'static str {
        match self {
            Op::Get => "`config.get`",
            Op::Secret => "`config.secret`",
        }
    }

    /// The Rust path `ply hosts` prints — the reviewable identity of a member of
    /// the trusted computing base.
    pub fn path(self) -> &'static str {
        match self {
            Op::Get => "ply_host::config::get",
            Op::Secret => "ply_host::config::secret",
        }
    }

    pub fn declaration(self) -> HostOp {
        HostOp {
            effect: Symbol::new(EFFECT),
            op: Symbol::new(self.name()),
            // Whichever namespaces the program writes. `bind` expands this
            // against the program's own atoms and `ply hosts` prints one row per
            // expansion, so a run that reads credentials has a
            // `config.read[credentials]` line of its own rather than one row
            // claiming everything.
            resource: HostResource::Any,
            // The environment is not a function of the program's state, so a
            // `det` test that reaches this is E0412 at compile time and has to
            // supply the values itself.
            determinism: Determinism::Nondeterministic,
            // Reading a frozen map twice is the definition of harmless, which is
            // the only reason a `Repeatable` claim is safe here: it is true
            // because of `Snapshot`'s immutability and of nothing else.
            linearity: Linearity::Repeatable,
            // No file is opened and no syscall is made: every source was read
            // before this handler existed.
            blocking: false,
            // `config.secret` **answers** a `Secret`; neither operation is ever
            // handed one, because both take a key. The column is about what
            // crosses the boundary downward, which is where the type system
            // stops being able to say anything.
            secrets: false,
            path: self.path(),
        }
    }
}

// --- what a value's shape is ------------------------------------------------

/// A declared key's type, as the run checks a resolved value against it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Shape {
    Text,
    Int,
    Bool,
    Secret,
}

impl Shape {
    /// The constructor `std.config` declares, by simple name. This is the one
    /// mapping between the Ply schema and the Rust one, so a rename on either
    /// side is a resolution failure rather than a shape silently read as `SText`.
    pub fn from_ctor(name: &str) -> Option<Shape> {
        Some(match name {
            "SText" => Shape::Text,
            "SInt" => Shape::Int,
            "SBool" => Shape::Bool,
            "SSecret" => Shape::Secret,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Shape::Text => "SText",
            Shape::Int => "SInt",
            Shape::Bool => "SBool",
            Shape::Secret => "SSecret",
        }
    }

    /// Whether a value of this shape may be printed. The answer is the whole
    /// difference between an operator who can debug a wrong value and a
    /// credential in a log line.
    pub fn is_secret(self) -> bool {
        self == Shape::Secret
    }

    /// Why a value is not of this shape, or `None` when it is.
    ///
    /// A secret's rule is that it is not empty. That is not arbitrary: an empty
    /// credential is the shape an unset environment variable takes after a shell
    /// expands it, so accepting one is accepting the exact mistake `--config-schema`
    /// exists to catch, one layer further in and with the failure moved to
    /// whichever request first authenticates.
    fn refuse(self, value: &str) -> Option<String> {
        match self {
            Shape::Text => None,
            Shape::Int => value
                .parse::<i64>()
                .is_err()
                .then(|| "is not an `Int`".to_string()),
            Shape::Bool => (value != "true" && value != "false")
                .then(|| "is neither `true` nor `false`".to_string()),
            Shape::Secret => value
                .is_empty()
                .then(|| "is empty, and an empty credential is an unset one".to_string()),
        }
    }
}

impl fmt::Display for Shape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One declared key.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Key {
    pub name: String,
    pub shape: Shape,
    pub required: bool,
    pub default: Option<String>,
}

/// The schema a `--config-schema` function returns, materialised.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Spec {
    pub keys: Vec<Key>,
}

impl Spec {
    /// Every key, checked to be declared once.
    ///
    /// Two declarations of one name is refused rather than resolved by taking
    /// the first: the two may disagree about the shape, and then whether a value
    /// is valid would depend on the order the schema function happened to build
    /// its list in.
    pub fn new(keys: Vec<Key>) -> Result<Spec, Diagnostic> {
        let mut seen = BTreeSet::new();
        for key in &keys {
            if !seen.insert(key.name.as_str()) {
                return Err(Diagnostic::error(
                    codes::CONFIG_UNAVAILABLE,
                    format!("the configuration schema declares `{}` twice", key.name),
                )
                .primary(Span::DUMMY, "a key is declared once or the run cannot resolve it")
                .note("two declarations of one key may disagree about its shape, and which one applied would depend on the order the schema function built its list in")
                .note("remove the duplicate from the `ConfigSpec`"));
            }
        }
        Ok(Spec { keys })
    }

    fn get(&self, name: &str) -> Option<&Key> {
        self.keys.iter().find(|k| k.name == name)
    }
}

// --- where a value came from ------------------------------------------------

/// Which of the four sources supplied a value, highest precedence first.
///
/// Printed beside every key, because "it connected to the wrong thing" is
/// answered by which source won and by nothing else.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Source {
    /// `--set KEY=VALUE`.
    Set,
    /// A `--config` file, by the path as the command line wrote it.
    File(String),
    /// The process environment, read once at bind time.
    Environment,
    /// The schema's own `default`.
    Default,
}

impl Source {
    /// The short word `ply hosts` prints in the `keys` line.
    pub fn as_str(&self) -> &str {
        match self {
            Source::Set => "--set",
            Source::File(path) => path,
            Source::Environment => "env",
            Source::Default => "default",
        }
    }

    /// Whether a person typed this key on purpose. Only the explicit sources
    /// raise `W0607` for an undeclared key: an environment is full of names that
    /// have nothing to do with this program, while a `--set` is something
    /// somebody meant and a typo in one is the classic silent deploy failure.
    pub fn is_explicit(&self) -> bool {
        matches!(self, Source::Set | Source::File(_))
    }
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::Set => f.write_str("--set"),
            Source::File(path) => write!(f, "--config {path}"),
            Source::Environment => f.write_str("the process environment"),
            Source::Default => f.write_str("the schema's default"),
        }
    }
}

// --- the sources, read exactly once -----------------------------------------

/// Every key any source supplies, with the source that won.
///
/// Built once, at bind time. Nothing mutates one and nothing re-reads the
/// process environment after it exists.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Sources {
    resolved: BTreeMap<String, (String, Source)>,
    /// How many `--set` arguments and `--config` files the command line carried,
    /// and how many variables the environment held. Counts rather than contents,
    /// because the report prints "environment 217" and must never print 217
    /// variable names that have nothing to do with this program.
    pub sets: usize,
    pub files: Vec<PathBuf>,
    pub environment: usize,
}

impl Sources {
    /// The empty sources: what a run with no `--host` has, and what it keeps
    /// whatever the environment holds.
    ///
    /// ADR 0011's rule is untouched here — a reviewer reads `--host` in the
    /// command or the run reached nothing. Configuration may supply a value and
    /// may never cause a binding.
    pub fn unopened() -> Sources {
        Sources::default()
    }

    /// Read `--set`, then `--config`, then the process environment, once.
    pub fn read(set: &[String], files: &[PathBuf]) -> Result<Sources, Vec<Diagnostic>> {
        Sources::read_with(set, files, &std::env::vars().collect::<Vec<_>>(), &|path| {
            std::fs::read_to_string(path)
        })
    }

    /// [`read`], against an explicit environment and an explicit reader.
    ///
    /// Tests take this rather than `std::env`, which is process-global and
    /// therefore a race between two of them under one test binary — which is the
    /// same hazard this module exists to close for a *run*, so closing it for
    /// the suite too costs one parameter.
    ///
    /// [`read`]: Sources::read
    pub fn read_with(
        set: &[String],
        files: &[PathBuf],
        env: &[(String, String)],
        read: &dyn Fn(&Path) -> std::io::Result<String>,
    ) -> Result<Sources, Vec<Diagnostic>> {
        let mut diagnostics = Vec::new();
        let mut resolved: BTreeMap<String, (String, Source)> = BTreeMap::new();

        // Lowest precedence first, each overwriting what is under it, so the
        // last write for a key is the winner and the source recorded with it is
        // the one that won.
        for (key, value) in env {
            if key_shape(key).is_ok() {
                resolved.insert(key.clone(), (value.clone(), Source::Environment));
            }
        }

        for path in files {
            let shown = path.display().to_string();
            let text = match read(path) {
                Ok(text) => text,
                Err(e) => {
                    diagnostics.push(err_unreadable(&shown, &e));
                    continue;
                }
            };
            for (number, line) in text.lines().enumerate() {
                match parse_line(line) {
                    Ok(None) => {}
                    Ok(Some((key, value))) => {
                        resolved.insert(key, (value, Source::File(shown.clone())));
                    }
                    Err(why) => diagnostics.push(err_bad_line(&shown, number + 1, line, &why)),
                }
            }
        }

        for argument in set {
            match parse_line(argument) {
                Ok(Some((key, value))) => {
                    resolved.insert(key, (value, Source::Set));
                }
                // A `--set` is never blank and never a comment: somebody typed
                // it, so a line the file format would ignore is a mistake here.
                Ok(None) => diagnostics.push(err_bad_set(argument, "it supplies no key")),
                Err(why) => diagnostics.push(err_bad_set(argument, &why)),
            }
        }

        if !diagnostics.is_empty() {
            return Err(diagnostics);
        }
        Ok(Sources {
            resolved,
            sets: set.len(),
            files: files.to_vec(),
            environment: env.len(),
        })
    }

    fn get(&self, key: &str) -> Option<&(String, Source)> {
        self.resolved.get(key)
    }

    /// Every key an explicit source supplied, ascending. What `W0607` is
    /// computed from.
    fn explicit(&self) -> impl Iterator<Item = &str> {
        self.resolved
            .iter()
            .filter(|(_, (_, source))| source.is_explicit())
            .map(|(key, _)| key.as_str())
    }
}

/// A key must be an identifier extended with `.`, in every source alike.
///
/// The environment is filtered by this rather than refused by it: a process
/// environment holds names no program chose, and `BASH_FUNC_x%%` is not a
/// mistake somebody made in this run's configuration. A `--set` or a file line
/// is refused, because there it is.
fn key_shape(key: &str) -> Result<(), String> {
    if key.is_empty() {
        return Err("it has an empty key".to_string());
    }
    let mut chars = key.chars();
    let first = chars.next().expect("checked non-empty");
    if !(first.is_ascii_alphabetic() || first == '_') {
        return Err(format!(
            "`{key}` starts with `{first}`, and a key starts with a letter or `_`"
        ));
    }
    if let Some(bad) = chars.find(|c| !(c.is_ascii_alphanumeric() || *c == '_' || *c == '.')) {
        return Err(format!(
            "`{key}` contains `{bad}`, and a key is letters, digits, `_` and `.`"
        ));
    }
    Ok(())
}

/// One `KEY=VALUE` line, or `None` for a blank line or a comment.
///
/// A `#` begins a comment only where a *key* would begin. With no quoting there
/// would otherwise be no way to write a `#` in a value, and a password silently
/// truncated at one is exactly the class of failure this module exists to
/// prevent — so the value is the rest of the line, horizontal whitespace
/// trimmed off each end, and nothing in it is interpreted.
fn parse_line(line: &str) -> Result<Option<(String, String)>, String> {
    let trimmed = line.trim_matches(|c: char| c == ' ' || c == '\t' || c == '\r');
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return Ok(None);
    }
    let Some((key, value)) = trimmed.split_once('=') else {
        return Err("it has no `=`".to_string());
    };
    let key = key.trim_matches(|c: char| c == ' ' || c == '\t');
    key_shape(key)?;
    let value = value.trim_matches(|c: char| c == ' ' || c == '\t');
    Ok(Some((key.to_string(), value.to_string())))
}

// --- the snapshot -----------------------------------------------------------

/// One resolved key.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Resolved {
    pub value: String,
    pub source: Source,
    /// The shape the schema declared, or `None` for a key no schema mentions.
    pub shape: Option<Shape>,
}

impl Resolved {
    /// What a report may print. A secret's value never leaves this type.
    pub fn shown(&self) -> &str {
        match self.shape {
            Some(Shape::Secret) => REDACTED,
            _ => &self.value,
        }
    }
}

/// The frozen configuration of one run.
///
/// Immutable by construction: there is no method that changes one, which is
/// what makes `config.read[k]` a read and two readers non-conflicting. There is
/// deliberately no live reload — a configuration that can change mid-run is a
/// nondeterminism that would have to be in every reader's row, and two requests
/// in one run that saw different values is a class of bug with no repro.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Snapshot {
    values: BTreeMap<String, Resolved>,
    /// Only the keys a schema declared, so the `SSecret` gate can tell "declared
    /// as something else" from "not declared at all".
    declared: BTreeMap<String, Shape>,
    /// Whether this run named a `--config-schema`. Without one there are no
    /// `SSecret` keys, and containment for configured values is only as strong
    /// as the schema.
    has_spec: bool,
    /// Counts, for the start-up banner and `ply hosts`.
    pub sets: usize,
    pub files: Vec<PathBuf>,
    pub environment: usize,
}

impl Snapshot {
    /// What a run with no `--host` holds: nothing, whatever the environment has.
    pub fn unopened() -> Snapshot {
        Snapshot::default()
    }

    /// Resolve the sources against the schema, before anything is bound.
    ///
    /// Returns the snapshot and the warnings it produced. Every *error* here is
    /// the run's configuration at fault rather than the program's, and every one
    /// of them is raised before a socket is opened.
    pub fn resolve(sources: &Sources, spec: Option<&Spec>) -> Result<Report, Vec<Diagnostic>> {
        let mut diagnostics = Vec::new();
        let mut warnings = Vec::new();
        let mut values: BTreeMap<String, Resolved> = BTreeMap::new();
        let mut declared: BTreeMap<String, Shape> = BTreeMap::new();

        for (key, (value, source)) in &sources.resolved {
            values.insert(
                key.clone(),
                Resolved {
                    value: value.clone(),
                    source: source.clone(),
                    shape: None,
                },
            );
        }

        if let Some(spec) = spec {
            for key in &spec.keys {
                declared.insert(key.name.clone(), key.shape);
                let resolved = match (sources.get(&key.name), &key.default) {
                    (Some((value, source)), _) => Resolved {
                        value: value.clone(),
                        source: source.clone(),
                        shape: Some(key.shape),
                    },
                    (None, Some(default)) => Resolved {
                        value: default.clone(),
                        source: Source::Default,
                        shape: Some(key.shape),
                    },
                    (None, None) => {
                        if key.required {
                            diagnostics.push(err_missing(key, sources));
                        }
                        continue;
                    }
                };
                if let Some(why) = key.shape.refuse(&resolved.value) {
                    diagnostics.push(err_invalid(key, &resolved, &why));
                    continue;
                }
                values.insert(key.name.clone(), resolved);
            }

            for key in sources.explicit() {
                if spec.get(key).is_none() {
                    let source = sources
                        .get(key)
                        .map(|(_, s)| s.clone())
                        .unwrap_or(Source::Set);
                    warnings.push(err_undeclared(key, &source, spec));
                }
            }
        }

        if !diagnostics.is_empty() {
            return Err(diagnostics);
        }
        Ok(Report {
            snapshot: Snapshot {
                values,
                declared,
                has_spec: spec.is_some(),
                sets: sources.sets,
                files: sources.files.clone(),
                environment: sources.environment,
            },
            warnings,
        })
    }

    /// What `config.get` answers.
    ///
    /// `None` for a key the schema declares `SSecret`, whatever any source
    /// holds. This is the line that closes the window in which a credential
    /// would exist as an ordinary `String`: without it the containment claim
    /// would hold for every route except the one the credential actually
    /// arrives on.
    pub fn get(&self, key: &str) -> Option<&str> {
        let resolved = self.values.get(key)?;
        if resolved.shape == Some(Shape::Secret) {
            return None;
        }
        Some(&resolved.value)
    }

    /// The plaintext behind `config.secret`, for the one caller that turns it
    /// into a `Secret`.
    ///
    /// `None` for a key the schema declares as anything other than `SSecret`,
    /// so a schema is the thing that decides which keys are credentials and a
    /// program cannot launder a plain key into one. With no schema every key a
    /// source supplies is available here, which is the same latitude
    /// [`Snapshot::get`] has and is why a run that wants containment names a
    /// `--config-schema`.
    fn plaintext(&self, key: &str) -> Option<&str> {
        let resolved = self.values.get(key)?;
        if self.has_spec && resolved.shape != Some(Shape::Secret) {
            return None;
        }
        Some(&resolved.value)
    }

    /// Every key the schema declared and the run resolved, ascending. What the
    /// `keys` line of `ply hosts` prints, and the only accessor a report has:
    /// there is none that walks the environment, because printing 217 variables
    /// that have nothing to do with this program is how a listing becomes
    /// something nobody reads.
    pub fn declared(&self) -> impl Iterator<Item = (&str, &Resolved)> {
        self.values
            .iter()
            .filter(|(_, r)| r.shape.is_some())
            .map(|(k, r)| (k.as_str(), r))
    }

    /// How many declared keys each source won, for the banner's config line.
    pub fn counts(&self) -> Counts {
        let mut counts = Counts::default();
        for (_, resolved) in self.declared() {
            counts.keys += 1;
            match resolved.source {
                Source::Set => counts.set += 1,
                Source::File(_) => counts.file += 1,
                Source::Environment => counts.environment += 1,
                Source::Default => counts.default += 1,
            }
            if resolved.shape == Some(Shape::Secret) {
                counts.secret += 1;
            }
        }
        counts
    }

    pub fn has_spec(&self) -> bool {
        self.has_spec
    }
}

/// The declared keys, counted by the source that won.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Counts {
    pub keys: usize,
    pub set: usize,
    pub file: usize,
    pub environment: usize,
    pub default: usize,
    pub secret: usize,
}

/// A resolution's outcome: the snapshot, and what it wants to say about the
/// command line that produced it.
///
/// The warnings are carried rather than printed here, because `ply-host` has no
/// output and a `W0607` belongs in whichever report the command is writing.
#[derive(Debug)]
pub struct Report {
    pub snapshot: Snapshot,
    pub warnings: Vec<Diagnostic>,
}

// --- the handlers -----------------------------------------------------------

/// Register both operations against a snapshot.
///
/// The snapshot is shared by `Arc` across every worker in the run and is never
/// mutated, which is the whole reason two host-backed tests that read
/// configuration are not coupled: there is no state to couple them through.
pub fn register(registry: &mut HostRegistry, snapshot: Arc<Snapshot>) {
    for op in Op::ALL {
        registry.register(
            op.declaration(),
            Arc::new(Operation {
                op,
                snapshot: Arc::clone(&snapshot),
            }),
        );
    }
}

/// A registry serving `config` and nothing else.
pub fn registry(snapshot: Arc<Snapshot>) -> HostRegistry {
    let mut registry = HostRegistry::new();
    register(&mut registry, snapshot);
    registry
}

struct Operation {
    op: Op,
    snapshot: Arc<Snapshot>,
}

impl HostHandler for Operation {
    fn call(&self, _: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        let span = req.span;
        let [key] = req.args else {
            return Err(arity(self.op, req.args.len(), span));
        };
        let key = key.as_str(span, "a configuration key")?;
        Ok(HostAnswer::Value(match self.op {
            Op::Get => option(self.snapshot.get(key).map(|v| Value::Str(v.into()))),
            Op::Secret => option(self.snapshot.plaintext(key).map(secret)),
        }))
    }
}

/// The one place in this crate a Ply-level `Secret` is built.
///
/// A credential resolved from a source becomes a `Secret` here and is a
/// `Secret` everywhere above, so there is no window in which it is an ordinary
/// `String` a trace field, a `++` or a derived codec could reach. The plaintext
/// exists in this function's argument and in the snapshot, and nowhere in the
/// program.
fn secret(plain: &str) -> Value {
    Value::secret(Value::Str(plain.into()))
}

fn option(value: Option<Value>) -> Value {
    match value {
        Some(value) => Value::Ctor {
            name: Symbol::new("Some"),
            args: Arc::new(vec![value]),
        },
        None => Value::Ctor {
            name: Symbol::new("None"),
            args: Arc::new(Vec::new()),
        },
    }
}

// --- diagnostics ------------------------------------------------------------

#[cold]
fn arity(op: Op, got: usize, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("{} was performed with {got} arguments and takes 1", op.what()),
    )
    .primary(span, "this perform reached the configuration snapshot")
    .note("inference checks a perform's arity, so reaching this means the evaluator was handed a module that was never checked")
}

#[cold]
fn err_unreadable(path: &str, e: &std::io::Error) -> Diagnostic {
    Diagnostic::error(
        codes::CONFIG_UNAVAILABLE,
        format!("`--config {path}` could not be read: {e}"),
    )
    .primary(Span::DUMMY, "this file is the run's configuration")
    .note("the file is `KEY=VALUE`, one per line, with `#` comments and no quoting")
    .note("correct the path, or drop the `--config` and supply the keys with `--set KEY=VALUE`")
}

#[cold]
fn err_bad_line(path: &str, number: usize, line: &str, why: &str) -> Diagnostic {
    Diagnostic::error(
        codes::CONFIG_UNAVAILABLE,
        format!("`{path}` line {number} is not `KEY=VALUE`: {why}"),
    )
    .primary(Span::DUMMY, "this line is the run's configuration")
    .note(format!("the line reads `{}`", line.trim()))
    .note("a key is letters, digits, `_` and `.`, starting with a letter or `_`; the value is the rest of the line")
    .note("a whole-line `#` is a comment; a `#` after the `=` is part of the value, because there is no quoting to escape it with")
}

#[cold]
fn err_bad_set(argument: &str, why: &str) -> Diagnostic {
    Diagnostic::error(
        codes::CONFIG_UNAVAILABLE,
        format!("`--set {argument}` is not `KEY=VALUE`: {why}"),
    )
    .primary(Span::DUMMY, "this argument is the run's configuration")
    .note("write `--set KEY=VALUE`, once per key; the last `--set` of a key wins")
}

#[cold]
fn err_missing(key: &Key, sources: &Sources) -> Diagnostic {
    let files = if sources.files.is_empty() {
        "no `--config` file".to_string()
    } else {
        format!(
            "the `--config` file{} {}",
            if sources.files.len() == 1 { "" } else { "s" },
            sources
                .files
                .iter()
                .map(|p| format!("`{}`", p.display()))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    Diagnostic::error(
        codes::CONFIG_MISSING,
        format!(
            "the configuration schema requires `{}` (`{}`) and nothing supplies it",
            key.name, key.shape
        ),
    )
    .primary(Span::DUMMY, "this key is the run's configuration")
    .note(format!(
        "the run looked in: {} `--set` argument{}, {}, {} environment variable{}, and the schema's own default",
        sources.sets,
        if sources.sets == 1 { "" } else { "s" },
        files,
        sources.environment,
        if sources.environment == 1 { "" } else { "s" },
    ))
    .note(format!(
        "supply it with `--set {}=...`, put it in a `--config` file, or export it",
        key.name
    ))
    .note("this is refused before anything is bound: a service that discovers a key is missing on its first request has already told a client it was listening")
}

#[cold]
fn err_invalid(key: &Key, resolved: &Resolved, why: &str) -> Diagnostic {
    // The value is printed for every shape but a secret. An operator debugging
    // "it used the wrong region" needs to see `eu-wesst`; one debugging a
    // credential needs to know only which source won, and printing the value
    // would put it in this diagnostic's message — which reaches stderr, `--json`
    // and, for a failure inside a run, the result cache.
    let mut diagnostic = Diagnostic::error(
        codes::CONFIG_INVALID,
        format!(
            "`{}` is declared `{}` and {} supplied a value that {why}",
            key.name, key.shape, resolved.source
        ),
    )
    .primary(Span::DUMMY, "this value is the run's configuration");
    if !key.shape.is_secret() {
        diagnostic = diagnostic.note(format!("the value is `{}`", resolved.value));
    } else {
        diagnostic = diagnostic
            .note("the value is not printed, because a diagnostic's message reaches stderr, `--json` and a cached failure report");
    }
    diagnostic.note(format!(
        "change what {} supplies, or declare `{}` with a shape it satisfies",
        resolved.source, key.name
    ))
}

#[cold]
fn err_undeclared(key: &str, source: &Source, spec: &Spec) -> Diagnostic {
    let mut declared: Vec<&str> = spec.keys.iter().map(|k| k.name.as_str()).collect();
    declared.sort_unstable();
    Diagnostic::warning(
        codes::CONFIG_UNDECLARED,
        format!("{source} supplies `{key}`, which the configuration schema does not declare"),
    )
    .primary(Span::DUMMY, "nothing in this program reads this key")
    .note(if declared.is_empty() {
        "the schema declares no keys at all".to_string()
    } else {
        format!("the schema declares: {}", declared.join(", "))
    })
    .note("only `--set` and `--config` are checked: an environment is full of names that have nothing to do with this program")
}

#[cfg(test)]
mod tests;
