//! Every diagnostic renders two ways from one value: `ariadne` for a terminal and JSON for an
//! agent.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// An interned, cheaply-cloned name.
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

    /// A span usable where no real source location exists (builtins, synthesized nodes).
    pub const DUMMY: Span = Span {
        source: SourceId(u32::MAX),
        start: 0,
        end: 0,
    };

    pub fn is_dummy(&self) -> bool {
        self.source.0 == u32::MAX
    }

    /// Smallest span covering both.
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

/// Stable diagnostic codes.
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
    /// A project file whose path would name a module under a root the language reserves — `std`
    /// today.
    pub const RESERVED_MODULE_NAME: &str = "E0113";
    /// A row, or an `effect set` body, naming a set the module does not declare.
    pub const UNKNOWN_EFFECT_SET: &str = "E0114";
    /// An `effect set` that contains itself, directly or through another.
    pub const EFFECT_SET_CYCLE: &str = "E0115";
    /// The base of a record update `{..b, f: e}` has no record shape this module can name.
    pub const RECORD_UPDATE_SHAPE: &str = "E0116";
    /// A record update names a field the base does not have.
    pub const RECORD_UPDATE_FIELD: &str = "E0117";
    /// A `?` whose enclosing function has no return type this file can read as `Result` or
    /// `Option`, so the expansion has no constructors to name.
    pub const TRY_SCOPE: &str = "E0118";
    /// A `?` written where its early exit would change what runs, or would discard something
    /// written.
    pub const TRY_POSITION: &str = "E0119";
    /// A parameter default written where no call could ever fill it in: on a lambda, on an effect
    /// operation, or on a handler clause.
    pub const DEFAULT_NOT_ALLOWED: &str = "E0120";
    /// A parameter default that is not a pure, closed expression.
    pub const DEFAULT_NOT_PURE: &str = "E0121";
    /// A default on a `pub fn` that mentions a name the callee's module does not export.
    pub const DEFAULT_PRIVATE_NAME: &str = "E0122";
    /// A named argument that names no parameter of the callee, or names one twice, or names one a
    /// positional argument already filled.
    pub const UNKNOWN_ARGUMENT_NAME: &str = "E0123";
    /// A positional argument after a named one.
    pub const ARGUMENT_ORDER: &str = "E0124";
    /// A parameter filled neither positionally nor by name, carrying no default.
    pub const MISSING_ARGUMENT: &str = "E0125";
    /// A top-level `fn` that left a parameter type or its return type to be
    /// inferred.
    ///
    /// A published signature is a claim a human makes, not a summary the
    /// compiler derives: `ply review --changed`'s load-bearing row is
    /// *implementation changed, spec unchanged*, and a signature that moves
    /// with the body it describes cannot hold still for that row to mean
    /// anything. So the type is written and inference checks it.
    ///
    /// Effect rows are the deliberate exception and are **not** covered by this
    /// code: a row is derived from what a body calls rather than chosen, so it
    /// stays inferred unless written, and a written one is checked as an upper
    /// bound. See `docs/GUIDE.md` §5.9.
    ///
    /// The diagnostic names the type inference *would* have given, so the fix
    /// is the text of the error.
    pub const MISSING_SIGNATURE: &str = "E0126";
    pub const TYPE_MISMATCH: &str = "E0201";
    pub const ARITY_MISMATCH: &str = "E0202";
    pub const OCCURS_CHECK: &str = "E0203";
    pub const NOT_A_FUNCTION: &str = "E0204";
    pub const NON_EXHAUSTIVE_MATCH: &str = "E0205";
    /// A type has no derivation for the deriver asked for, and the diagnostic names the field that
    /// blocks it rather than the type as a whole.
    pub const NOT_DERIVABLE: &str = "E0206";
    /// A `derive` or a `where` clause naming something that is not one of the derivers the language
    /// defines.
    pub const UNKNOWN_DERIVER: &str = "E0207";
    /// A `derive` in a module other than the one declaring its target type.
    pub const ORPHAN_DERIVE: &str = "E0208";
    /// `/` applied to `Decimal`.
    pub const DECIMAL_DIVISION: &str = "E0209";
    /// An arithmetic or ordered-comparison operand whose numeric type nothing
    /// determines.
    ///
    /// Ply has three numeric types and no numeric tower, so `a + b` is one of
    /// three different operations. This used to be settled by *defaulting* the
    /// operand to `Int` before generalization — a tiebreak inside the compiler
    /// that landed in a published signature. With signatures written
    /// ([`MISSING_SIGNATURE`]) an operand a definition's own parameters do not
    /// pin is a lambda binder or a `let` nothing constrains, and choosing for
    /// the author there is a guess. The diagnostic asks for the annotation
    /// rather than making it.
    pub const NUMERIC_UNDETERMINED: &str = "E0210";
    pub const UNBOUND_ROW_VAR: &str = "E0301";
    pub const EFFECT_NOT_PERMITTED: &str = "E0302";
    pub const UNHANDLED_EFFECT: &str = "E0303";
    pub const RESOURCE_REQUIRED: &str = "E0304";
    pub const NONDET_IN_DET_TEST: &str = "E0412";
    /// A `Task` in a `simulate` region's result type, or a `join` of a task whose region has
    /// already ended.
    pub const TASK_ESCAPES_SCOPE: &str = "E0413";
    /// A simulated region made no more progress: nothing is enabled and no timer can fire, or the
    /// per-interleaving step budget was spent.
    pub const DEADLOCK: &str = "E0414";
    /// Replaying a seed did not reproduce the recorded schedule.
    pub const SIMULATION_DIVERGENCE: &str = "E0415";
    /// A `simulate` region inside a `simulate` region, lexically or through a call.
    pub const NESTED_SIMULATION: &str = "E0416";
    /// A `requires`, `ensures` or `where` guard whose row is not empty, or a law body whose row is
    /// not a subset of `{sim.read}`.
    pub const EFFECT_IN_SPEC: &str = "E0417";
    /// A `forall` binder whose type cannot be quantified over: no generator (`Cell`, `Task`), a
    /// function type with a non-empty row, or an effect-row variable.
    pub const UNQUANTIFIABLE_TYPE: &str = "E0418";
    /// An obligation was refuted by a counterexample.
    pub const OBLIGATION_REFUTED: &str = "E0419";
    /// The guard admitted no values, so the obligation is trivially valid and says nothing.
    pub const VACUOUS_OBLIGATION: &str = "E0420";
    /// A host registration names an effect, operation or resource the program does not declare.
    pub const HOST_OPERATION_UNKNOWN: &str = "E0421";
    /// Two host registrations claim one atom.
    pub const HOST_HANDLER_CONFLICT: &str = "E0422";
    /// A host handler declares itself nondeterministic for an effect the program did not declare
    /// `nondet`.
    pub const HOST_DETERMINISM_MISMATCH: &str = "E0423";
    /// An operation reached the host boundary with nothing bound.
    pub const HERMETIC_BOUNDARY: &str = "E0424";
    /// A host operation reached from a test the search re-runs — from inside a `simulate` region,
    /// or from the prefix or suffix around one.
    pub const HOST_IN_SIMULATION: &str = "E0425";
    /// A continuation was resumed a second time across an at-most-once host operation.
    pub const HOST_CONTINUATION_RESUMED: &str = "E0426";
    /// A host handler answered an atom outside the declared footprint of the entry point that
    /// reached it.
    pub const HOST_FOOTPRINT_ESCAPE: &str = "E0427";
    /// A handler declared `blocking: true` answered a value inline instead of a pending token.
    pub const HOST_BLOCKING_ANSWER: &str = "E0428";
    /// `net.listen_tls` named a credential the binding does not hold.
    pub const TLS_CREDENTIAL_UNKNOWN: &str = "E0429";
    /// A `--tls` credential that does not load: the file is unreadable, the PEM does not parse, it
    /// holds no certificate or no private key, or the key does not match the leaf certificate.
    pub const TLS_CREDENTIAL_INVALID: &str = "E0430";
    /// `--host` bound the postgres driver and the run named no database, named one that does not
    /// parse, asked for an `sslmode` W4 does not configure, or named a server that could not be
    /// reached.
    pub const DB_NOT_CONFIGURED: &str = "E0431";
    /// Statement text the driver refuses before preparing it: more than one statement, a construct
    /// the table scanner cannot account for, a parameter or result type outside the pinned mapping,
    /// or a nondeterministic function in the text where a parameter belongs.
    pub const DB_STATEMENT_REFUSED: &str = "E0432";
    /// The server refused to prepare a statement — syntax, an unknown relation, an unknown column —
    /// or its result description has two columns of one name or lacks a column the row codec
    /// requires.
    pub const DB_PREPARE_FAILED: &str = "E0433";
    /// A statement touches a table outside the declared footprint of the entry point that reached
    /// it — caught at prepare time from the declared footprint the request carries, and again at
    /// answer time from the atoms the handler reported it touched.
    pub const DB_FOOTPRINT_UNDECLARED: &str = "E0434";
    /// The live database differs from the schema the run named: a missing table or column, a type
    /// outside the mapping, a nullability that disagrees, or a missing constraint.
    pub const DB_SCHEMA_MISMATCH: &str = "E0435";
    /// A database operation performed by a task that does not own the open transaction scope.
    pub const DB_TRANSACTION_SCOPE: &str = "E0436";
    /// No connection became available within the acquire deadline.
    pub const DB_POOL_EXHAUSTED: &str = "E0437";
    /// The live schema carries a trigger, a rewrite rule, or a referential action that cascades —
    /// an effect that makes one statement touch a table its own text never names, which no scanner
    /// can see and no row can report.
    pub const DB_UNMODELLED_SIDE_EFFECT: &str = "E0438";
    /// A host operation was handed a value containing a `Secret` and its registration does not
    /// declare that it may receive one.
    pub const SECRET_TO_HOST: &str = "E0439";
    /// A configuration source the run named could not be read: a `--config` file that is
    /// unreadable, a line that is not `KEY=VALUE`, an empty key, a key that is not an identifier,
    /// or a `--set` of the same shape.
    pub const CONFIG_UNAVAILABLE: &str = "E0440";
    /// A key the run's `--config-schema` marks `required` that no source supplies.
    pub const CONFIG_MISSING: &str = "E0441";
    /// A resolved configuration value that does not satisfy its declared shape.
    pub const CONFIG_INVALID: &str = "E0442";
    /// A deployable artifact that does not verify: the header, the section table, the program
    /// digest, a definition body against the hash it is filed under, or a reference to a hash the
    /// artifact does not carry.
    pub const ARTIFACT_INVALID: &str = "E0443";
    /// An artifact built under a different `FRONTEND_VERSION`, `RUNTIME_VERSION` or `BODY_ENCODING`
    /// than the binary loading it.
    pub const ARTIFACT_VERSION: &str = "E0444";
    /// `trace.exit` naming a span that is not open on the performing task's stack — closed already,
    /// never opened, or opened by another task.
    pub const SPAN_UNBALANCED: &str = "E0445";
    /// A value branded with a region's name would outlive the region: returned from it, stored into
    /// a binding that predates it, captured by a closure that leaves it, or written as a field of a
    /// declared type, which is outside every region there is.
    pub const REGION_ESCAPE: &str = "E0446";
    /// Two regions in scope at once under one name.
    pub const REGION_ALREADY_OPEN: &str = "E0447";
    /// A region declared `unique` across which a continuation capture is reachable — ADR 0017 §3.
    pub const REGION_KIND_REFUSED: &str = "E0448";
    /// A value reaching a runtime boundary carrying a handle into a region — a `Cell`, a `Task` or
    /// a continuation — where no type is left for [`REGION_ESCAPE`] to look at: a host operation's
    /// argument, a host handler's answer, or an entry point's argument.
    pub const REGION_ESCAPE_AT_BOUNDARY: &str = "E0449";
    /// A compiled backend a run asked for cannot be attached: the spec does not name one, or the
    /// engine asked for has no compiled path to attach it to.
    pub const BACKEND_UNAVAILABLE: &str = "E0450";
    pub const ASSERTION_FAILED: &str = "E0501";
    /// A program-level failure the language defines: `panic`, division by zero, integer overflow, a
    /// resource limit.
    pub const RUNTIME_ERROR: &str = "E0502";
    /// A compiled backend answered something the machine would not have.
    pub const ENGINE_DIVERGENCE: &str = "E0503";
    pub const INTERNAL_ERROR: &str = "E0505";
    /// `W` rather than `E`: cache trouble is never a fault in the user's program, so these are
    /// always warnings.
    pub const CACHE_UNREADABLE: &str = "W0601";
    pub const CACHE_CORRUPT: &str = "W0602";
    pub const CACHE_VERSION_CHANGED: &str = "W0603";
    /// An obligation the system could not decide at any tier — an effect nothing discharges, a
    /// parameter nothing can generate, an evaluation that raised.
    pub const OBLIGATION_NOT_DISCHARGED: &str = "W0604";
    /// The stdlib shipped with this compiler differs from the one the cache was written under.
    pub const STDLIB_CHANGED: &str = "W0605";
    /// A host runtime could not hand every resource back when an entry point ended: a transaction
    /// scope whose `ROLLBACK` failed, a connection closed rather than returned to the pool, an
    /// operation still in flight when the bound expired.
    pub const HOST_TEARDOWN: &str = "W0606";
    /// A configuration key supplied explicitly — by `--set` or by a `--config` file — that the
    /// run's schema does not declare.
    pub const CONFIG_UNDECLARED: &str = "W0607";
    /// The drain deadline expired with connections still in flight.
    pub const DRAIN_INCOMPLETE: &str = "W0608";
    /// A value was made to reach itself, so reference counting will never free it: ADR 0017 §4 does
    /// not collect cycles and accepts the leak.
    pub const REFERENCE_CYCLE: &str = "W0610";
    /// Spans were still open when an entry point ended, so teardown closed them rather than the
    /// program.
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

    /// A code is matched on by tooling long after the release that introduced it, so a number may
    /// never be reused or renumbered.
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
            ("MISSING_SIGNATURE", codes::MISSING_SIGNATURE, "E0126"),
            ("TYPE_MISMATCH", codes::TYPE_MISMATCH, "E0201"),
            ("ARITY_MISMATCH", codes::ARITY_MISMATCH, "E0202"),
            ("OCCURS_CHECK", codes::OCCURS_CHECK, "E0203"),
            ("NOT_A_FUNCTION", codes::NOT_A_FUNCTION, "E0204"),
            ("NON_EXHAUSTIVE_MATCH", codes::NON_EXHAUSTIVE_MATCH, "E0205"),
            ("NOT_DERIVABLE", codes::NOT_DERIVABLE, "E0206"),
            ("UNKNOWN_DERIVER", codes::UNKNOWN_DERIVER, "E0207"),
            ("ORPHAN_DERIVE", codes::ORPHAN_DERIVE, "E0208"),
            ("DECIMAL_DIVISION", codes::DECIMAL_DIVISION, "E0209"),
            ("NUMERIC_UNDETERMINED", codes::NUMERIC_UNDETERMINED, "E0210"),
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
