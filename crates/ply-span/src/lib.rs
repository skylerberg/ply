//! Every diagnostic renders two ways from one value: `ariadne` for a terminal
//! and JSON for an agent. The JSON form is not a lossy summary of the pretty
//! form — both are projections of the same [`Diagnostic`].

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// An interned, cheaply-cloned name. Thread-safe without a global interner,
/// which matters because inference and hashing run under `rayon`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Symbol(Arc<str>);

impl Symbol {
    pub fn new(s: impl AsRef<str>) -> Self {
        Symbol(Arc::from(s.as_ref()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl fmt::Debug for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", &*self.0)
    }
}
impl From<&str> for Symbol {
    fn from(s: &str) -> Self {
        Symbol::new(s)
    }
}
impl From<String> for Symbol {
    fn from(s: String) -> Self {
        Symbol::new(s)
    }
}
impl std::ops::Deref for Symbol {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}
impl Serialize for Symbol {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}
impl<'de> Deserialize<'de> for Symbol {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        Ok(Symbol::new(String::deserialize(d)?))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct SourceId(pub u32);

/// A half-open byte range within a [`SourceId`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct Span {
    pub source: SourceId,
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub fn new(source: SourceId, start: u32, end: u32) -> Self {
        Span { source, start, end }
    }

    /// A span usable where no real source location exists (builtins, synthesized
    /// nodes). Never rendered with a snippet.
    pub const DUMMY: Span = Span {
        source: SourceId(u32::MAX),
        start: 0,
        end: 0,
    };

    pub fn is_dummy(&self) -> bool {
        self.source.0 == u32::MAX
    }

    /// Smallest span covering both. Mismatched sources yield `self` — spans
    /// never straddle files.
    pub fn to(self, other: Span) -> Span {
        if self.is_dummy() {
            return other;
        }
        if other.is_dummy() || self.source != other.source {
            return self;
        }
        Span {
            source: self.source,
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }

    pub fn range(&self) -> std::ops::Range<usize> {
        self.start as usize..self.end as usize
    }
}

#[derive(Clone, Debug)]
pub struct SourceFile {
    pub id: SourceId,
    pub path: PathBuf,
    pub text: Arc<str>,
    line_starts: Vec<u32>,
}

impl SourceFile {
    /// 1-based line and column (column counted in `char`s, not bytes).
    pub fn line_col(&self, offset: u32) -> (u32, u32) {
        let line = self
            .line_starts
            .partition_point(|&s| s <= offset)
            .saturating_sub(1);
        let line_start = self.line_starts[line] as usize;
        let col = self.text[line_start..(offset as usize).min(self.text.len())]
            .chars()
            .count();
        (line as u32 + 1, col as u32 + 1)
    }

    pub fn line_text(&self, line: u32) -> &str {
        let idx = (line.saturating_sub(1)) as usize;
        let start = self.line_starts.get(idx).copied().unwrap_or(0) as usize;
        let end = self
            .line_starts
            .get(idx + 1)
            .map(|&e| e as usize)
            .unwrap_or(self.text.len());
        self.text[start..end].trim_end_matches(['\n', '\r'])
    }
}

#[derive(Clone, Debug, Default)]
pub struct SourceMap {
    files: Vec<SourceFile>,
}

impl SourceMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, path: impl AsRef<Path>, text: impl Into<String>) -> SourceId {
        let text: Arc<str> = Arc::from(text.into());
        let mut line_starts = vec![0u32];
        line_starts.extend(
            text.char_indices()
                .filter(|(_, c)| *c == '\n')
                .map(|(i, _)| (i + 1) as u32),
        );
        let id = SourceId(self.files.len() as u32);
        self.files.push(SourceFile {
            id,
            path: path.as_ref().to_path_buf(),
            text,
            line_starts,
        });
        id
    }

    pub fn get(&self, id: SourceId) -> Option<&SourceFile> {
        self.files.get(id.0 as usize)
    }

    pub fn files(&self) -> &[SourceFile] {
        &self.files
    }

    pub fn snippet(&self, span: Span) -> &str {
        self.get(span.source)
            .map(|f| &f.text[span.range()])
            .unwrap_or("")
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Note,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Label {
    pub span: Span,
    pub message: String,
    /// The primary label points at the cause; secondaries add context.
    pub primary: bool,
}

/// Stable diagnostic codes. Numbers are permanent once released so that tooling
/// and agents can match on them.
///
/// Two things about this module are checked rather than remembered, both by
/// `crates/ply-span/tests/armed.rs`:
///
/// * every constant here has a row in the registry table below
///   (`the_code_registry_table_is_total_over_the_codes_module`). Before that
///   test existed a constant could be added here and registered nowhere, and
///   one was: `REFERENCE_CYCLE` sits out of numeric order between `W0608` and
///   `W0609`, and had no row — 83 constants against 82 rows.
/// * every constant here is passed to `Diagnostic::error` or
///   `Diagnostic::warning` by some production source
///   (`every_registered_code_is_constructed_in_production`), or is listed in
///   that file's `UNARMED_CODES` with the reason it is reserved and unraised.
///   `E0435` and `E0438` are listed there.
pub mod codes {
    pub const UNEXPECTED_TOKEN: &str = "E0001";
    pub const UNTERMINATED_STRING: &str = "E0002";
    pub const UNKNOWN_NAME: &str = "E0101";
    pub const UNKNOWN_TYPE: &str = "E0102";
    pub const UNKNOWN_EFFECT: &str = "E0103";
    pub const UNKNOWN_OPERATION: &str = "E0104";
    pub const DUPLICATE_DEFINITION: &str = "E0105";
    pub const UNKNOWN_MODULE: &str = "E0106";
    pub const PRIVATE_NAME: &str = "E0107";
    pub const AMBIGUOUS_IMPORT: &str = "E0108";
    pub const MODULE_CYCLE: &str = "E0109";
    pub const DUPLICATE_IMPORT: &str = "E0110";
    pub const INVALID_MODULE_PATH: &str = "E0111";
    pub const AMBIGUOUS_ENTRY_POINT: &str = "E0112";
    /// A project file whose path would name a module under a root the language
    /// reserves — `std` today. The stdlib ships with the compiler and is found
    /// by name, so a project module that could claim one of its names would
    /// decide what `import std.json` means by where a file happens to sit.
    pub const RESERVED_MODULE_NAME: &str = "E0113";
    /// A row, or an `effect set` body, naming a set the module does not
    /// declare. Also the qualified form `other::Web` and `pub effect set`,
    /// because a set is private to the module that declares it and the three
    /// have one fix.
    ///
    /// The restriction is what makes expansion a function of the file. Gate 1
    /// skips a file whose raw bytes are unchanged, so a set expanding across a
    /// module boundary would let an edit in the declaring module leave a stale
    /// published row behind — a footprint that under-reports, which corrupts
    /// scheduling and isolation silently rather than loudly.
    pub const UNKNOWN_EFFECT_SET: &str = "E0114";
    /// An `effect set` that contains itself, directly or through another.
    /// Expansion is a fixed point and a cycle has none.
    pub const EFFECT_SET_CYCLE: &str = "E0115";
    /// The base of a record update `{..b, f: e}` has no record shape this
    /// module can name.
    ///
    /// Expansion runs inside the parser and reads **this module's own `type`
    /// items and the annotations written in this file**, for the reason
    /// [`UNKNOWN_EFFECT_SET`] gives: gate 1 skips a file whose raw bytes are
    /// unchanged, so a shape read across a module boundary would let an edit in
    /// the declaring module leave a stale expansion behind in a file that never
    /// moved — and a stale expansion is a wrong record, not a stale name.
    pub const RECORD_UPDATE_SHAPE: &str = "E0116";
    /// A record update names a field the base does not have. Update replaces;
    /// it does not widen, because the result's type is the base's type.
    pub const RECORD_UPDATE_FIELD: &str = "E0117";
    /// A `?` whose enclosing function has no return type this file can read as
    /// `Result` or `Option`, so the expansion has no constructors to name.
    ///
    /// `?` is expanded in the parser (`docs/adr/0027`), which runs before
    /// inference — the driver hashes before it infers (ADR 0002) — so the mode
    /// is read off the **written** `->` of the enclosing `fn`, following this
    /// file's own `type` aliases and nothing across a module boundary, for the
    /// reason [`RECORD_UPDATE_SHAPE`] gives. A lambda, a `handle` clause, a
    /// region, a `test`, a `law` and a spec expression all have no such type
    /// and are refused with a note saying which.
    pub const TRY_SCOPE: &str = "E0118";
    /// A `?` written where its early exit would change what runs, or would
    /// discard something written.
    ///
    /// Expansion lifts the `?`'s operand to the head of its region, so anything
    /// evaluated before it in that region has to be pure (the predicate is
    /// `ply_syntax::is_pure`, the same one normalization uses to license
    /// reordering a run of `let`s) and nothing conditional may sit between the
    /// region root and the `?`. `let x: T = e?;` is refused here too: the
    /// expansion has no `let` left to carry `T` on, and a written annotation
    /// must not evaporate.
    pub const TRY_POSITION: &str = "E0119";
    /// A parameter default written where no call could ever fill it in: on a
    /// lambda, on an effect operation, or on a handler clause.
    ///
    /// A default is spliced into a call site by matching that call against a
    /// *signature*, which needs a name to match against. A lambda is reached
    /// through a value; an operation's arguments come from a `perform` and its
    /// clause must bind exactly what the operation declares.
    pub const DEFAULT_NOT_ALLOWED: &str = "E0120";
    /// A parameter default that is not a pure, closed expression.
    ///
    /// The expression is copied into every call site that omits the argument
    /// (`docs/adr/0029`), so it must mean the same thing at each. A call or a
    /// `perform` would run there rather than here; a mention of another
    /// parameter or of a local would resolve against the caller's binders. The
    /// predicate is [`ply_syntax::is_default_expr`], which is
    /// [`ply_syntax::is_pure`] widened to admit constructor applications —
    /// `Some(0)` is a default a program will actually want.
    ///
    /// [`ply_syntax::is_pure`]: https://docs.rs/ply-syntax
    /// [`ply_syntax::is_default_expr`]: https://docs.rs/ply-syntax
    pub const DEFAULT_NOT_PURE: &str = "E0121";
    /// A default on a `pub fn` that mentions a name the callee's module does
    /// not export.
    ///
    /// Expansion qualifies the default's free names against the module that
    /// wrote them and splices the result into the caller, so a private name
    /// would become a reference no other module is allowed to make. Checked at
    /// the definition rather than at each call site: the answer does not vary
    /// by caller, and a diagnostic on the signature is one a reader can act on.
    pub const DEFAULT_PRIVATE_NAME: &str = "E0122";
    /// A named argument that names no parameter of the callee, or names one
    /// twice, or names one a positional argument already filled.
    pub const UNKNOWN_ARGUMENT_NAME: &str = "E0123";
    /// A positional argument after a named one. Positional arguments fill
    /// parameters left to right, which a name in front of them makes ambiguous.
    pub const ARGUMENT_ORDER: &str = "E0124";
    /// A parameter filled neither positionally nor by name, carrying no
    /// default.
    ///
    /// Distinct from [`ARITY_MISMATCH`], which is what a call through a *value*
    /// gets: this one can name the parameter, because the callee was reached by
    /// a name and its signature is in hand.
    pub const MISSING_ARGUMENT: &str = "E0125";
    pub const TYPE_MISMATCH: &str = "E0201";
    pub const ARITY_MISMATCH: &str = "E0202";
    pub const OCCURS_CHECK: &str = "E0203";
    pub const NOT_A_FUNCTION: &str = "E0204";
    pub const NON_EXHAUSTIVE_MATCH: &str = "E0205";
    /// A type has no derivation for the deriver asked for, and the diagnostic
    /// names the field that blocks it rather than the type as a whole.
    ///
    /// Three shapes, one claim — `derivable(D, t)` does not hold: a `derive`
    /// whose target contains a function, a cell, a task or a continuation; a
    /// call site instantiating a `where derivable(D, a)` parameter with such a
    /// type; and a `Map<k, v>` whose key type is not `derivable(ord, k)`.
    pub const NOT_DERIVABLE: &str = "E0206";
    /// A `derive` or a `where` clause naming something that is not one of the
    /// derivers the language defines. Derivers are fixed; there are no
    /// user-defined ones.
    pub const UNKNOWN_DERIVER: &str = "E0207";
    /// A `derive` in a module other than the one declaring its target type.
    /// This is the orphan rule, and it is what makes "one canonical codec per
    /// type" checkable from what a module can see rather than globally.
    pub const ORPHAN_DERIVE: &str = "E0208";
    /// `/` applied to `Decimal`. The exact quotient of two decimals is not in
    /// general a decimal, so the operator would have to round — and a rounding
    /// nobody wrote down is the defect the type exists to prevent. The
    /// diagnostic names `decimal_div`, which takes the scale and the rounding
    /// mode as arguments.
    pub const DECIMAL_DIVISION: &str = "E0209";
    pub const UNBOUND_ROW_VAR: &str = "E0301";
    pub const EFFECT_NOT_PERMITTED: &str = "E0302";
    pub const UNHANDLED_EFFECT: &str = "E0303";
    pub const RESOURCE_REQUIRED: &str = "E0304";
    pub const NONDET_IN_DET_TEST: &str = "E0412";
    /// A `Task` in a `simulate` region's result type, or a `join` of a task
    /// whose region has already ended. The scheduler dies with its region, so a
    /// task handle that outlives it names nothing.
    pub const TASK_ESCAPES_SCOPE: &str = "E0413";
    /// A simulated region made no more progress: nothing is enabled and no timer
    /// can fire, or the per-interleaving step budget was spent. One code for
    /// both, because from the program's side they are one problem with one fix
    /// and the message is where the two differ.
    pub const DEADLOCK: &str = "E0414";
    /// Replaying a seed did not reproduce the recorded schedule. Ply's fault
    /// rather than the program's — the same class as [`ENGINE_DIVERGENCE`], and
    /// handled the same way, because a run that is not a function of its seed
    /// invalidates every artifact the simulation produced.
    pub const SIMULATION_DIVERGENCE: &str = "E0415";
    /// A `simulate` region inside a `simulate` region, lexically or through a
    /// call. Two schedulers means two notions of "runnable".
    pub const NESTED_SIMULATION: &str = "E0416";
    /// A `requires`, `ensures` or `where` guard whose row is not empty, or a law
    /// body whose row is not a subset of `{sim.read}`. A claim that can perform
    /// effects can change what it observes.
    pub const EFFECT_IN_SPEC: &str = "E0417";
    /// A `forall` binder whose type cannot be quantified over: no generator
    /// (`Cell`, `Task`), a function type with a non-empty row, or an effect-row
    /// variable. A law nobody can check is a claim nobody will read.
    pub const UNQUANTIFIABLE_TYPE: &str = "E0418";
    /// An obligation was refuted by a counterexample. The program's fault, and
    /// attributed like any other failure.
    pub const OBLIGATION_REFUTED: &str = "E0419";
    /// The guard admitted no values, so the obligation is trivially valid and
    /// says nothing. Always a defect in the spec: reporting it `proved` would
    /// turn a typo in a guard into a proof of everything.
    pub const VACUOUS_OBLIGATION: &str = "E0420";
    /// A host registration names an effect, operation or resource the program
    /// does not declare. Raised before anything runs: a handler bound to a
    /// triple nothing declares is a footprint claim about nothing.
    pub const HOST_OPERATION_UNKNOWN: &str = "E0421";
    /// Two host registrations claim one atom. Ambiguity here is a coin flip over
    /// which real resource gets touched.
    pub const HOST_HANDLER_CONFLICT: &str = "E0422";
    /// A host handler declares itself nondeterministic for an effect the program
    /// did not declare `nondet`. The declaration is the authority — a binding may
    /// not change what inference computed, or `ply check` would answer
    /// differently under `--host` and every cache would split on a flag.
    pub const HOST_DETERMINISM_MISMATCH: &str = "E0423";
    /// An operation reached the host boundary with nothing bound. Deliberately
    /// not [`UNHANDLED_EFFECT`]: that one means inference should have prevented
    /// this and did not, while this one means inference was right and the run was
    /// configured hermetically. The two call for opposite responses.
    pub const HERMETIC_BOUNDARY: &str = "E0424";
    /// A host operation reached from a test the search re-runs — from inside a
    /// `simulate` region, or from the prefix or suffix around one.
    ///
    /// The search re-runs a test **whole** per interleaving, so the hazard is not
    /// confined to the region: an operation written beside it is performed once
    /// per schedule explored and the total is then reported as a proof over all
    /// of them. Both shapes are this code, because a consumer's response to them
    /// is the same one.
    pub const HOST_IN_SIMULATION: &str = "E0425";
    /// A continuation was resumed a second time across an at-most-once host
    /// operation. The second resumption would perform the operation again:
    /// charge the card twice, send the packet twice, insert the row twice.
    pub const HOST_CONTINUATION_RESUMED: &str = "E0426";
    /// A host handler answered an atom outside the declared footprint of the
    /// entry point that reached it. Ply's fault rather than the program's — the
    /// same class as [`ENGINE_DIVERGENCE`], because the run knows two of its own
    /// answers disagree and nothing in the definition graph decides which was
    /// meant.
    ///
    /// What it does and does not catch is worth being exact about, because the
    /// check is easy to over-read. The atom compared is the one the *registry*
    /// resolved, so this fires when the atom a `perform` reached is missing from
    /// the entry point's row — an inference/registry disagreement, and a program
    /// footprint that under-reports what the program itself performs. It cannot
    /// fire on a handler that does more than its registration declared: a handler
    /// never names an atom, so it cannot report one. See ADR 0008 §2.
    pub const HOST_FOOTPRINT_ESCAPE: &str = "E0427";
    /// A handler declared `blocking: true` answered a value inline instead of a
    /// pending token. `blocking: true` means "this handler leaves the machine's
    /// thread", and a value returned from `call` is the machine's own thread
    /// having done the work — so the declaration and the behaviour disagree, and
    /// the scheduler's account of which of its threads are free is wrong.
    pub const HOST_BLOCKING_ANSWER: &str = "E0428";
    /// `net.listen_tls` named a credential the binding does not hold. The
    /// diagnostic lists the credentials the run was configured with, because
    /// the fix is a `--tls` argument rather than an edit to the program.
    pub const TLS_CREDENTIAL_UNKNOWN: &str = "E0429";
    /// A `--tls` credential that does not load: the file is unreadable, the PEM
    /// does not parse, it holds no certificate or no private key, or the key
    /// does not match the leaf certificate.
    ///
    /// Raised at bind time, before anything runs. A server that discovers its
    /// certificate is unusable on the first handshake has already told a client
    /// it was listening.
    pub const TLS_CREDENTIAL_INVALID: &str = "E0430";
    /// `--host` bound the postgres driver and the run named no database, named
    /// one that does not parse, asked for an `sslmode` W4 does not configure, or
    /// named a server that could not be reached.
    ///
    /// The connection string is configured beside the run rather than written in
    /// the program, for the reason [`TLS_CREDENTIAL_INVALID`] gives about a
    /// private key: a password in a definition's hash is in a store designed
    /// never to forget.
    pub const DB_NOT_CONFIGURED: &str = "E0431";
    /// Statement text the driver refuses before preparing it: more than one
    /// statement, a construct the table scanner cannot account for, a parameter
    /// or result type outside the pinned mapping, or a nondeterministic function
    /// in the text where a parameter belongs.
    ///
    /// The scanner refuses rather than guessing because its answer is a
    /// footprint. A construct it silently ignored would produce a row that
    /// under-reports, which corrupts scheduling and isolation with a green
    /// result rather than a red one.
    pub const DB_STATEMENT_REFUSED: &str = "E0432";
    /// The server refused to prepare a statement — syntax, an unknown relation,
    /// an unknown column — or its result description has two columns of one name
    /// or lacks a column the row codec requires.
    ///
    /// Not a `Failed` value like a constraint violation: this one is the same
    /// every time and will never succeed on a retry, so making it a value would
    /// invite a program to loop on it.
    pub const DB_PREPARE_FAILED: &str = "E0433";
    /// A statement touches a table outside the declared footprint of the entry
    /// point that reached it — caught at prepare time from the declared
    /// footprint the request carries, and again at answer time from the atoms
    /// the handler reported it touched.
    ///
    /// Deliberately not [`HOST_FOOTPRINT_ESCAPE`], which is Ply's fault: there
    /// the registry-resolved atom disagreed with the row, while here the
    /// registry was right and the row is wrong, because the tables a statement
    /// reaches are a function of its text rather than of the call site's label.
    /// The program is at fault and it is attributed and bisected like any other
    /// program failure.
    ///
    /// The answer-time half is a **detector and not a preventer**: scheduling
    /// happened before the run, so the statement has already executed against a
    /// table the scheduler believed nobody was touching. What it buys is that a
    /// wrong row fails loudly on its first execution instead of quietly forever.
    pub const DB_FOOTPRINT_UNDECLARED: &str = "E0434";
    /// The live database differs from the schema the run named: a missing table
    /// or column, a type outside the mapping, a nullability that disagrees, or a
    /// missing constraint. Raised at bind time, before anything runs, because a
    /// service that discovers its schema is wrong on the first request has
    /// already told a client it was listening.
    pub const DB_SCHEMA_MISMATCH: &str = "E0435";
    /// A database operation performed by a task that does not own the open
    /// transaction scope. Both alternatives are wrong: sharing the connection is
    /// a protocol violation, since a postgres connection carries one
    /// conversation, and quietly acquiring a second connection would put the
    /// statement *outside* the transaction its author believed it was in.
    pub const DB_TRANSACTION_SCOPE: &str = "E0436";
    /// No connection became available within the acquire deadline.
    ///
    /// A diagnostic rather than a value, unlike every SQLSTATE the server
    /// returns. A value is a thing a program is invited to swallow, and a
    /// swallowed pool exhaustion is a service returning wrong answers under
    /// exactly the load that produced it.
    pub const DB_POOL_EXHAUSTED: &str = "E0437";
    /// The live schema carries a trigger, a rewrite rule, or a referential
    /// action that cascades — an effect that makes one statement touch a table
    /// its own text never names, which no scanner can see and no row can report.
    ///
    /// Raised at bind time with no flag to suppress it. A flag that turns a
    /// soundness check off is a flag whose default becomes the one nobody uses.
    pub const DB_UNMODELLED_SIDE_EFFECT: &str = "E0438";
    /// A host operation was handed a value containing a `Secret` and its
    /// registration does not declare that it may receive one.
    ///
    /// Ply's fault in the sense [`HOST_FOOTPRINT_ESCAPE`] is: the boundary's own
    /// account of itself disagrees with what crossed it. Above the boundary a
    /// `Secret` cannot reach a log, a codec, a response or a diagnostic, because
    /// no function that renders takes one; below it nothing is checkable at all,
    /// so which operations may receive a credential is declared, printed by
    /// `ply hosts`, and covered by its digest.
    pub const SECRET_TO_HOST: &str = "E0439";
    /// A configuration source the run named could not be read: a `--config`
    /// file that is unreadable, a line that is not `KEY=VALUE`, an empty key, a
    /// key that is not an identifier, or a `--set` of the same shape.
    ///
    /// The format is `KEY=VALUE` and not TOML or YAML because the effect
    /// returns `Option<String>`: a format richer than the type it feeds is a
    /// format whose extra structure is silently dropped.
    pub const CONFIG_UNAVAILABLE: &str = "E0440";
    /// A key the run's `--config-schema` marks `required` that no source
    /// supplies. Raised at bind time, before anything runs, because a service
    /// that discovers a credential is missing on its first request has already
    /// told a client it was listening.
    pub const CONFIG_MISSING: &str = "E0441";
    /// A resolved configuration value that does not satisfy its declared shape.
    /// The message names the key and the source that won, and **never the
    /// value** when the shape is a secret.
    pub const CONFIG_INVALID: &str = "E0442";
    /// A deployable artifact that does not verify: the header, the section
    /// table, the program digest, a definition body against the hash it is filed
    /// under, or a reference to a hash the artifact does not carry.
    ///
    /// Every body is checked against its own key, so a corrupted transfer is a
    /// refusal naming one definition rather than a plausible wrong program.
    pub const ARTIFACT_INVALID: &str = "E0443";
    /// An artifact built under a different `FRONTEND_VERSION`, `RUNTIME_VERSION`
    /// or `BODY_ENCODING` than the binary loading it.
    ///
    /// Its own code rather than [`ARTIFACT_INVALID`] because the two call for
    /// opposite responses: rebuild the artifact, versus transfer it again.
    pub const ARTIFACT_VERSION: &str = "E0444";
    /// `trace.exit` naming a span that is not open on the performing task's
    /// stack — closed already, never opened, or opened by another task.
    ///
    /// Accepting it silently is how one request's timing lands under another
    /// request's span. A span the program never closed is not this: it is closed
    /// at teardown and reported [`SPAN_ABANDONED`].
    pub const SPAN_UNBALANCED: &str = "E0445";
    /// A value branded with a region's name would outlive the region:
    /// returned from it, stored into a binding that predates it, captured by a
    /// closure that leaves it, or written as a field of a declared type, which
    /// is outside every region there is.
    ///
    /// Reported where the value would escape rather than where it is later
    /// used, and always naming both the value's type and the region it belongs
    /// to. Deliberately not [`TYPE_MISMATCH`]: nothing here disagrees about
    /// what a value *is*, only about how long it lives, and the two call for
    /// different edits.
    pub const REGION_ESCAPE: &str = "E0446";
    /// Two regions in scope at once under one name. The brand is the name, so
    /// the inner region's values would be indistinguishable from the outer's
    /// and closing the inner one would free memory the outer still holds.
    ///
    /// Refused rather than reinterpreted, because there is no reading of the
    /// program under which the two brands mean different things.
    pub const REGION_ALREADY_OPEN: &str = "E0447";
    /// A region declared `unique` across which a continuation capture is
    /// reachable — ADR 0017 §3.
    ///
    /// A refusal rather than a downgrade to `shared`, because the two kinds
    /// differ in what a program *means*: a `unique` region is a bump pointer
    /// with no snapshot, so one resumption would observe another resumption's
    /// writes. Reinterpreting the annotation would leave a reader believing the
    /// cheap kind had been proved where it had only been asked for.
    pub const REGION_KIND_REFUSED: &str = "E0448";
    /// A value reaching a runtime boundary carrying a handle into a region — a
    /// `Cell`, a `Task` or a continuation — where no type is left for
    /// [`REGION_ESCAPE`] to look at: a host operation's argument, a host
    /// handler's answer, or an entry point's argument.
    ///
    /// Deliberately not [`REGION_ESCAPE`], which is a compile error the author
    /// can fix by editing the expression it points at. This one fires while a
    /// program runs, at a boundary a type does not cross, and the reader's next
    /// move is different: ADR 0017 §2's open route — a continuation parked in an
    /// enclosing region's cell, where a constructor's field type erases the
    /// brand — is how a handle gets this far, so the fix is upstream of the
    /// boundary rather than at it.
    pub const REGION_ESCAPE_AT_BOUNDARY: &str = "E0449";
    /// A compiled backend a run asked for cannot be attached: the spec does not
    /// name one, or the engine asked for has no compiled path to attach it to.
    ///
    /// Its own number rather than a usage error, because the two cases a reader
    /// meets are different and the second is the one worth a code: a flag that
    /// is accepted and does nothing is `CONTRIBUTING.md` §"The one rule"'s defect
    /// shape, so `--engine treewalk --backend ..` is refused rather than
    /// silently ignored.
    pub const BACKEND_UNAVAILABLE: &str = "E0450";
    pub const ASSERTION_FAILED: &str = "E0501";
    /// A program-level failure the language defines: `panic`, division by zero,
    /// integer overflow, a resource limit. The program is at fault and the
    /// failure is attributable to a change, so it is bisected like any other.
    pub const RUNTIME_ERROR: &str = "E0502";
    /// Never a warning: the result cache would record whichever engine ran
    /// first and never recompute it.
    pub const ENGINE_DIVERGENCE: &str = "E0503";
    /// A handler clause the tree-walker cannot express. Its own number rather
    /// than `RUNTIME_ERROR` so that a consumer can tell a refusal to run from a
    /// defect while running: the two call for opposite responses.
    pub const MACHINE_ONLY_CLAUSE: &str = "E0504";
    /// Ply broke one of its own invariants. Its own number rather than
    /// `RUNTIME_ERROR` because the two call for opposite responses: a runtime
    /// error is the program's fault and is attributed to whichever change
    /// introduced it, while this one is Ply's fault and there is nothing in the
    /// user's definition graph to attribute it to.
    pub const INTERNAL_ERROR: &str = "E0505";
    /// `W` rather than `E`: cache trouble is never a fault in the user's
    /// program, so these are always warnings.
    pub const CACHE_UNREADABLE: &str = "W0601";
    pub const CACHE_CORRUPT: &str = "W0602";
    pub const CACHE_VERSION_CHANGED: &str = "W0603";
    /// An obligation the system could not decide at any tier — an effect nothing
    /// discharges, a parameter nothing can generate, an evaluation that raised.
    /// A `W` because it is nobody's fault: it is a gap, it is counted, and it
    /// leaves its definition uncovered.
    pub const OBLIGATION_NOT_DISCHARGED: &str = "W0604";
    /// The stdlib shipped with this compiler differs from the one the cache was
    /// written under. Correctness needs no warning — a stdlib definition is
    /// content-addressed like any other, so exactly the tests reaching a changed
    /// one re-run — but an upgrade that silently re-runs work is a mystery, and
    /// the warning is what turns it into a fact with a digest beside it.
    pub const STDLIB_CHANGED: &str = "W0605";
    /// A host runtime could not hand every resource back when an entry point
    /// ended: a transaction scope whose `ROLLBACK` failed, a connection closed
    /// rather than returned to the pool, an operation still in flight when the
    /// bound expired. A `W` because it is the run's own state rather than the
    /// program's — the entry point's verdict is unchanged — and it is printed
    /// because a pool that quietly refills is a pool nobody can size.
    pub const HOST_TEARDOWN: &str = "W0606";
    /// A configuration key supplied explicitly — by `--set` or by a `--config`
    /// file — that the run's schema does not declare. Only the explicit
    /// sources: an environment is full of names that have nothing to do with
    /// this program, while a `--set` is something a person typed on purpose and
    /// a typo in one is the classic silent deploy failure.
    pub const CONFIG_UNDECLARED: &str = "W0607";
    /// The drain deadline expired with connections still in flight. Names how
    /// many were abandoned with no response written and how many transactions
    /// were rolled back at teardown.
    ///
    /// A `W` because the run's own configuration is at fault rather than the
    /// program, and the verdict is carried by the exit code instead: a
    /// deployment must be able to tell a clean stop from one that dropped
    /// requests.
    pub const DRAIN_INCOMPLETE: &str = "W0608";
    /// A value was made to reach itself, so reference counting will never free
    /// it: ADR 0017 §4 does not collect cycles and accepts the leak.
    ///
    /// A `W` because refusing the write would change what a legal program means,
    /// which is the one thing the region model may not do. It names the cell and
    /// the write that closed the loop, so the leak is a fact on the run rather
    /// than something a reader has to infer from memory growth.
    pub const REFERENCE_CYCLE: &str = "W0610";
    /// Spans were still open when an entry point ended, so teardown closed them
    /// rather than the program. The program's fault and attributed like any
    /// other failure, but not an error: the records are written either way, and
    /// the `Abandoned` outcome on them is the signal.
    pub const SPAN_ABANDONED: &str = "W0609";
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: &'static str,
    pub message: String,
    pub labels: Vec<Label>,
    pub notes: Vec<String>,
}

impl Diagnostic {
    pub fn error(code: &'static str, message: impl Into<String>) -> Self {
        Diagnostic {
            severity: Severity::Error,
            code,
            message: message.into(),
            labels: Vec::new(),
            notes: Vec::new(),
        }
    }

    pub fn warning(code: &'static str, message: impl Into<String>) -> Self {
        Diagnostic {
            severity: Severity::Warning,
            ..Self::error(code, message)
        }
    }

    pub fn primary(mut self, span: Span, message: impl Into<String>) -> Self {
        self.labels.push(Label {
            span,
            message: message.into(),
            primary: true,
        });
        self
    }

    pub fn secondary(mut self, span: Span, message: impl Into<String>) -> Self {
        self.labels.push(Label {
            span,
            message: message.into(),
            primary: false,
        });
        self
    }

    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    pub fn primary_span(&self) -> Option<Span> {
        self.labels
            .iter()
            .find(|l| l.primary)
            .or_else(|| self.labels.first())
            .map(|l| l.span)
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

impl std::error::Error for Diagnostic {}

pub mod render;

pub type Result<T> = std::result::Result<T, Vec<Diagnostic>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_col_is_one_based_and_char_counted() {
        let mut sm = SourceMap::new();
        let id = sm.add("t.ply", "abc\nlét x = 1\n");
        let f = sm.get(id).unwrap();
        assert_eq!(f.line_col(0), (1, 1));
        assert_eq!(f.line_col(4), (2, 1));
        // `é` is two bytes; the column after it is still counted in chars.
        assert_eq!(f.line_col(7), (2, 3));
    }

    #[test]
    fn span_to_covers_both_and_ignores_dummy() {
        let s = SourceId(0);
        let a = Span::new(s, 5, 10);
        let b = Span::new(s, 20, 25);
        assert_eq!(a.to(b), Span::new(s, 5, 25));
        assert_eq!(Span::DUMMY.to(b), b);
        assert_eq!(a.to(Span::DUMMY), a);
    }

    /// A code is matched on by tooling long after the release that introduced
    /// it, so a number may never be reused or renumbered.
    #[test]
    fn every_registered_code_has_its_published_number() {
        let registry = [
            ("UNEXPECTED_TOKEN", codes::UNEXPECTED_TOKEN, "E0001"),
            ("UNTERMINATED_STRING", codes::UNTERMINATED_STRING, "E0002"),
            ("UNKNOWN_NAME", codes::UNKNOWN_NAME, "E0101"),
            ("UNKNOWN_TYPE", codes::UNKNOWN_TYPE, "E0102"),
            ("UNKNOWN_EFFECT", codes::UNKNOWN_EFFECT, "E0103"),
            ("UNKNOWN_OPERATION", codes::UNKNOWN_OPERATION, "E0104"),
            ("DUPLICATE_DEFINITION", codes::DUPLICATE_DEFINITION, "E0105"),
            ("UNKNOWN_MODULE", codes::UNKNOWN_MODULE, "E0106"),
            ("PRIVATE_NAME", codes::PRIVATE_NAME, "E0107"),
            ("AMBIGUOUS_IMPORT", codes::AMBIGUOUS_IMPORT, "E0108"),
            ("MODULE_CYCLE", codes::MODULE_CYCLE, "E0109"),
            ("DUPLICATE_IMPORT", codes::DUPLICATE_IMPORT, "E0110"),
            ("INVALID_MODULE_PATH", codes::INVALID_MODULE_PATH, "E0111"),
            (
                "AMBIGUOUS_ENTRY_POINT",
                codes::AMBIGUOUS_ENTRY_POINT,
                "E0112",
            ),
            ("RESERVED_MODULE_NAME", codes::RESERVED_MODULE_NAME, "E0113"),
            ("UNKNOWN_EFFECT_SET", codes::UNKNOWN_EFFECT_SET, "E0114"),
            ("EFFECT_SET_CYCLE", codes::EFFECT_SET_CYCLE, "E0115"),
            ("RECORD_UPDATE_SHAPE", codes::RECORD_UPDATE_SHAPE, "E0116"),
            ("RECORD_UPDATE_FIELD", codes::RECORD_UPDATE_FIELD, "E0117"),
            ("TRY_SCOPE", codes::TRY_SCOPE, "E0118"),
            ("TRY_POSITION", codes::TRY_POSITION, "E0119"),
            ("DEFAULT_NOT_ALLOWED", codes::DEFAULT_NOT_ALLOWED, "E0120"),
            ("DEFAULT_NOT_PURE", codes::DEFAULT_NOT_PURE, "E0121"),
            ("DEFAULT_PRIVATE_NAME", codes::DEFAULT_PRIVATE_NAME, "E0122"),
            (
                "UNKNOWN_ARGUMENT_NAME",
                codes::UNKNOWN_ARGUMENT_NAME,
                "E0123",
            ),
            ("ARGUMENT_ORDER", codes::ARGUMENT_ORDER, "E0124"),
            ("MISSING_ARGUMENT", codes::MISSING_ARGUMENT, "E0125"),
            ("TYPE_MISMATCH", codes::TYPE_MISMATCH, "E0201"),
            ("ARITY_MISMATCH", codes::ARITY_MISMATCH, "E0202"),
            ("OCCURS_CHECK", codes::OCCURS_CHECK, "E0203"),
            ("NOT_A_FUNCTION", codes::NOT_A_FUNCTION, "E0204"),
            ("NON_EXHAUSTIVE_MATCH", codes::NON_EXHAUSTIVE_MATCH, "E0205"),
            ("NOT_DERIVABLE", codes::NOT_DERIVABLE, "E0206"),
            ("UNKNOWN_DERIVER", codes::UNKNOWN_DERIVER, "E0207"),
            ("ORPHAN_DERIVE", codes::ORPHAN_DERIVE, "E0208"),
            ("DECIMAL_DIVISION", codes::DECIMAL_DIVISION, "E0209"),
            ("UNBOUND_ROW_VAR", codes::UNBOUND_ROW_VAR, "E0301"),
            ("EFFECT_NOT_PERMITTED", codes::EFFECT_NOT_PERMITTED, "E0302"),
            ("UNHANDLED_EFFECT", codes::UNHANDLED_EFFECT, "E0303"),
            ("RESOURCE_REQUIRED", codes::RESOURCE_REQUIRED, "E0304"),
            ("NONDET_IN_DET_TEST", codes::NONDET_IN_DET_TEST, "E0412"),
            ("TASK_ESCAPES_SCOPE", codes::TASK_ESCAPES_SCOPE, "E0413"),
            ("DEADLOCK", codes::DEADLOCK, "E0414"),
            (
                "SIMULATION_DIVERGENCE",
                codes::SIMULATION_DIVERGENCE,
                "E0415",
            ),
            ("NESTED_SIMULATION", codes::NESTED_SIMULATION, "E0416"),
            ("EFFECT_IN_SPEC", codes::EFFECT_IN_SPEC, "E0417"),
            ("UNQUANTIFIABLE_TYPE", codes::UNQUANTIFIABLE_TYPE, "E0418"),
            ("OBLIGATION_REFUTED", codes::OBLIGATION_REFUTED, "E0419"),
            ("VACUOUS_OBLIGATION", codes::VACUOUS_OBLIGATION, "E0420"),
            (
                "HOST_OPERATION_UNKNOWN",
                codes::HOST_OPERATION_UNKNOWN,
                "E0421",
            ),
            (
                "HOST_HANDLER_CONFLICT",
                codes::HOST_HANDLER_CONFLICT,
                "E0422",
            ),
            (
                "HOST_DETERMINISM_MISMATCH",
                codes::HOST_DETERMINISM_MISMATCH,
                "E0423",
            ),
            ("HERMETIC_BOUNDARY", codes::HERMETIC_BOUNDARY, "E0424"),
            ("HOST_IN_SIMULATION", codes::HOST_IN_SIMULATION, "E0425"),
            (
                "HOST_CONTINUATION_RESUMED",
                codes::HOST_CONTINUATION_RESUMED,
                "E0426",
            ),
            (
                "HOST_FOOTPRINT_ESCAPE",
                codes::HOST_FOOTPRINT_ESCAPE,
                "E0427",
            ),
            ("HOST_BLOCKING_ANSWER", codes::HOST_BLOCKING_ANSWER, "E0428"),
            (
                "TLS_CREDENTIAL_UNKNOWN",
                codes::TLS_CREDENTIAL_UNKNOWN,
                "E0429",
            ),
            (
                "TLS_CREDENTIAL_INVALID",
                codes::TLS_CREDENTIAL_INVALID,
                "E0430",
            ),
            ("DB_NOT_CONFIGURED", codes::DB_NOT_CONFIGURED, "E0431"),
            ("DB_STATEMENT_REFUSED", codes::DB_STATEMENT_REFUSED, "E0432"),
            ("DB_PREPARE_FAILED", codes::DB_PREPARE_FAILED, "E0433"),
            (
                "DB_FOOTPRINT_UNDECLARED",
                codes::DB_FOOTPRINT_UNDECLARED,
                "E0434",
            ),
            ("DB_SCHEMA_MISMATCH", codes::DB_SCHEMA_MISMATCH, "E0435"),
            ("DB_TRANSACTION_SCOPE", codes::DB_TRANSACTION_SCOPE, "E0436"),
            ("DB_POOL_EXHAUSTED", codes::DB_POOL_EXHAUSTED, "E0437"),
            (
                "DB_UNMODELLED_SIDE_EFFECT",
                codes::DB_UNMODELLED_SIDE_EFFECT,
                "E0438",
            ),
            ("SECRET_TO_HOST", codes::SECRET_TO_HOST, "E0439"),
            ("CONFIG_UNAVAILABLE", codes::CONFIG_UNAVAILABLE, "E0440"),
            ("CONFIG_MISSING", codes::CONFIG_MISSING, "E0441"),
            ("CONFIG_INVALID", codes::CONFIG_INVALID, "E0442"),
            ("ARTIFACT_INVALID", codes::ARTIFACT_INVALID, "E0443"),
            ("ARTIFACT_VERSION", codes::ARTIFACT_VERSION, "E0444"),
            ("SPAN_UNBALANCED", codes::SPAN_UNBALANCED, "E0445"),
            ("REGION_ESCAPE", codes::REGION_ESCAPE, "E0446"),
            ("REGION_ALREADY_OPEN", codes::REGION_ALREADY_OPEN, "E0447"),
            ("REGION_KIND_REFUSED", codes::REGION_KIND_REFUSED, "E0448"),
            (
                "REGION_ESCAPE_AT_BOUNDARY",
                codes::REGION_ESCAPE_AT_BOUNDARY,
                "E0449",
            ),
            ("BACKEND_UNAVAILABLE", codes::BACKEND_UNAVAILABLE, "E0450"),
            ("ASSERTION_FAILED", codes::ASSERTION_FAILED, "E0501"),
            ("RUNTIME_ERROR", codes::RUNTIME_ERROR, "E0502"),
            ("ENGINE_DIVERGENCE", codes::ENGINE_DIVERGENCE, "E0503"),
            ("MACHINE_ONLY_CLAUSE", codes::MACHINE_ONLY_CLAUSE, "E0504"),
            ("INTERNAL_ERROR", codes::INTERNAL_ERROR, "E0505"),
            ("CACHE_UNREADABLE", codes::CACHE_UNREADABLE, "W0601"),
            ("CACHE_CORRUPT", codes::CACHE_CORRUPT, "W0602"),
            (
                "CACHE_VERSION_CHANGED",
                codes::CACHE_VERSION_CHANGED,
                "W0603",
            ),
            (
                "OBLIGATION_NOT_DISCHARGED",
                codes::OBLIGATION_NOT_DISCHARGED,
                "W0604",
            ),
            ("STDLIB_CHANGED", codes::STDLIB_CHANGED, "W0605"),
            ("HOST_TEARDOWN", codes::HOST_TEARDOWN, "W0606"),
            ("CONFIG_UNDECLARED", codes::CONFIG_UNDECLARED, "W0607"),
            ("DRAIN_INCOMPLETE", codes::DRAIN_INCOMPLETE, "W0608"),
            ("SPAN_ABANDONED", codes::SPAN_ABANDONED, "W0609"),
            ("REFERENCE_CYCLE", codes::REFERENCE_CYCLE, "W0610"),
        ];

        for (name, code, expected) in registry {
            assert_eq!(code, expected, "`{name}` moved to a different number");
        }

        let mut numbers: Vec<&str> = registry.iter().map(|(_, code, _)| *code).collect();
        numbers.sort_unstable();
        let before = numbers.len();
        numbers.dedup();
        assert_eq!(
            before,
            numbers.len(),
            "two constants share one number: {numbers:?}"
        );
    }

    #[test]
    fn line_text_strips_terminator() {
        let mut sm = SourceMap::new();
        let id = sm.add("t.ply", "one\ntwo\r\nthree");
        let f = sm.get(id).unwrap();
        assert_eq!(f.line_text(1), "one");
        assert_eq!(f.line_text(2), "two");
        assert_eq!(f.line_text(3), "three");
    }
}
