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
        let end = offset as usize;
        let col = self
            .text
            .get(line_start..)
            .unwrap_or("")
            .char_indices()
            .take_while(|&(i, _)| line_start + i < end)
            .count();
        (line as u32 + 1, col as u32 + 1)
    }

    pub fn line_text(&self, line: u32) -> &str {
        let idx = (line.saturating_sub(1)) as usize;
        let Some(&start) = self.line_starts.get(idx) else {
            return "";
        };
        let end = self
            .line_starts
            .get(idx + 1)
            .map(|&e| e as usize)
            .unwrap_or(self.text.len());
        self.text
            .get(start as usize..end)
            .unwrap_or("")
            .trim_end_matches(['\n', '\r'])
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
            .and_then(|f| f.text.get(span.range()))
            .unwrap_or("")
    }

    pub fn containing(&self, span: Span) -> Option<&SourceFile> {
        self.get(span.source)
            .filter(|f| f.text.get(span.range()).is_some())
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
    /// A project file whose path would name a module under a reserved root such as `std`.
    pub const RESERVED_MODULE_NAME: &str = "E0113";
    /// A row, or an `effect set` body, naming a set the module does not declare.
    pub const UNKNOWN_EFFECT_SET: &str = "E0114";
    pub const EFFECT_SET_CYCLE: &str = "E0115";
    /// The base of a record update `{..b, f: e}` has no record shape this module can name.
    pub const RECORD_UPDATE_SHAPE: &str = "E0116";
    pub const RECORD_UPDATE_FIELD: &str = "E0117";
    /// A `?` whose enclosing function's return type is not readable as `Result` or `Option`.
    pub const TRY_SCOPE: &str = "E0118";
    /// A `?` whose early exit would change what runs or discard something written.
    pub const TRY_POSITION: &str = "E0119";
    /// A parameter default on a lambda, an effect operation or a handler clause.
    pub const DEFAULT_NOT_ALLOWED: &str = "E0120";
    /// A parameter default that is not a pure, closed expression.
    pub const DEFAULT_NOT_PURE: &str = "E0121";
    /// A default on a `pub fn` that mentions a name the callee's module does not export.
    pub const DEFAULT_PRIVATE_NAME: &str = "E0122";
    /// A named argument naming no parameter, one named twice, or one filled positionally.
    pub const UNKNOWN_ARGUMENT_NAME: &str = "E0123";
    /// A positional argument after a named one.
    pub const ARGUMENT_ORDER: &str = "E0124";
    /// A parameter filled neither positionally nor by name, carrying no default.
    pub const MISSING_ARGUMENT: &str = "E0125";
    /// A top-level `fn` that leaves a parameter or return type to inference.
    pub const MISSING_SIGNATURE: &str = "E0126";
    /// A `reuse fn` with an append the cost checker cannot show reuses its list.
    pub const REUSE_BROKEN: &str = "E0127";
    pub const TYPE_MISMATCH: &str = "E0201";
    pub const ARITY_MISMATCH: &str = "E0202";
    pub const OCCURS_CHECK: &str = "E0203";
    pub const NOT_A_FUNCTION: &str = "E0204";
    pub const NON_EXHAUSTIVE_MATCH: &str = "E0205";
    /// No derivation for the requested deriver; reported at the field that blocks it.
    pub const NOT_DERIVABLE: &str = "E0206";
    pub const UNKNOWN_DERIVER: &str = "E0207";
    /// A `derive` in a module other than the one declaring its target type.
    pub const ORPHAN_DERIVE: &str = "E0208";
    /// `/` applied to `Decimal`.
    pub const DECIMAL_DIVISION: &str = "E0209";
    /// An arithmetic or ordered-comparison operand whose numeric type nothing determines.
    pub const NUMERIC_UNDETERMINED: &str = "E0210";
    /// An integer literal outside the fixed-width type its context gave it.
    pub const LITERAL_OUT_OF_RANGE: &str = "E0211";
    pub const UNBOUND_ROW_VAR: &str = "E0301";
    pub const EFFECT_NOT_PERMITTED: &str = "E0302";
    pub const UNHANDLED_EFFECT: &str = "E0303";
    pub const RESOURCE_REQUIRED: &str = "E0304";
    pub const NONDET_IN_DET_TEST: &str = "E0412";
    /// A `Task` in a `simulate` region's result, or a `join` after its region ended.
    pub const TASK_ESCAPES_SCOPE: &str = "E0413";
    /// A simulated region with nothing enabled and no timer to fire, or out of step budget.
    pub const DEADLOCK: &str = "E0414";
    /// Replaying a seed did not reproduce the recorded schedule.
    pub const SIMULATION_DIVERGENCE: &str = "E0415";
    /// A `simulate` region inside a `simulate` region, lexically or through a call.
    pub const NESTED_SIMULATION: &str = "E0416";
    /// An effectful `requires`/`ensures`/`where` guard, or a law body beyond `{sim.read}`.
    pub const EFFECT_IN_SPEC: &str = "E0417";
    /// A `forall` binder type with no generator, an effectful function type, or a row variable.
    pub const UNQUANTIFIABLE_TYPE: &str = "E0418";
    pub const OBLIGATION_REFUTED: &str = "E0419";
    /// The guard admitted no values, so the obligation says nothing.
    pub const VACUOUS_OBLIGATION: &str = "E0420";
    /// A host registration names an effect, operation or resource the program does not declare.
    pub const HOST_OPERATION_UNKNOWN: &str = "E0421";
    /// Two host registrations claim one atom.
    pub const HOST_HANDLER_CONFLICT: &str = "E0422";
    /// A host handler declared nondeterministic for an effect not declared `nondet`.
    pub const HOST_DETERMINISM_MISMATCH: &str = "E0423";
    /// An operation reached the host boundary with nothing bound.
    pub const HERMETIC_BOUNDARY: &str = "E0424";
    /// A host operation reached from a test the search re-runs, in or around a `simulate`.
    pub const HOST_IN_SIMULATION: &str = "E0425";
    /// A continuation was resumed a second time across an at-most-once host operation.
    pub const HOST_CONTINUATION_RESUMED: &str = "E0426";
    /// A host handler answered an atom outside its entry point's declared footprint.
    pub const HOST_FOOTPRINT_ESCAPE: &str = "E0427";
    /// A handler declared `blocking: true` answered a value inline instead of a pending token.
    pub const HOST_BLOCKING_ANSWER: &str = "E0428";
    /// `net.listen_tls` named a credential the binding does not hold.
    pub const TLS_CREDENTIAL_UNKNOWN: &str = "E0429";
    /// A `--tls` credential that does not load, or whose key does not match its certificate.
    pub const TLS_CREDENTIAL_INVALID: &str = "E0430";
    /// Postgres is bound but the database is unnamed, unparseable, unsupported or unreachable.
    pub const DB_NOT_CONFIGURED: &str = "E0431";
    pub const DB_STATEMENT_REFUSED: &str = "E0432";
    /// The server refused to prepare a statement, or its columns do not fit the row codec.
    pub const DB_PREPARE_FAILED: &str = "E0433";
    /// A statement touches a table outside its entry point's declared footprint.
    pub const DB_FOOTPRINT_UNDECLARED: &str = "E0434";
    pub const DB_SCHEMA_MISMATCH: &str = "E0435";
    /// A database operation by a task that does not own the open transaction scope.
    pub const DB_TRANSACTION_SCOPE: &str = "E0436";
    pub const DB_POOL_EXHAUSTED: &str = "E0437";
    /// The live schema has a trigger, rule or cascade touching tables no statement names.
    pub const DB_UNMODELLED_SIDE_EFFECT: &str = "E0438";
    /// A `Secret` passed to a host operation whose registration does not accept one.
    pub const SECRET_TO_HOST: &str = "E0439";
    /// A `--config` file or `--set` that cannot be read or is not `KEY=VALUE`.
    pub const CONFIG_UNAVAILABLE: &str = "E0440";
    /// A key the run's `--config-schema` marks `required` that no source supplies.
    pub const CONFIG_MISSING: &str = "E0441";
    pub const CONFIG_INVALID: &str = "E0442";
    /// A deployable artifact whose contents do not verify against its own digests.
    pub const ARTIFACT_INVALID: &str = "E0443";
    /// An artifact built under a different frontend, runtime or body-encoding version.
    pub const ARTIFACT_VERSION: &str = "E0444";
    /// `trace.exit` naming a span not open on the performing task's stack.
    pub const SPAN_UNBALANCED: &str = "E0445";
    /// A value branded with a region's name would outlive the region.
    pub const REGION_ESCAPE: &str = "E0446";
    /// Two regions in scope at once under one name.
    pub const REGION_ALREADY_OPEN: &str = "E0447";
    /// A region declared `unique` across which a continuation capture is reachable.
    pub const REGION_KIND_REFUSED: &str = "E0448";
    /// A region handle crossing a runtime boundary, where no type is left to check.
    pub const REGION_ESCAPE_AT_BOUNDARY: &str = "E0449";
    pub const BACKEND_UNAVAILABLE: &str = "E0450";
    /// An `fs` operation named a resource label the run bound no root to.
    pub const FS_ROOT_UNBOUND: &str = "E0451";
    /// A path leaving its label's root via `..`, an absolute path, or a symlink.
    pub const FS_PATH_ESCAPES_ROOT: &str = "E0452";
    pub const FS_FILE_TOO_LARGE: &str = "E0453";
    /// A `--fs NAME=PATH` root that is missing, not a directory, or unresolvable.
    pub const FS_ROOT_INVALID: &str = "E0454";
    pub const ASSERTION_FAILED: &str = "E0501";
    /// A failure the language defines: `panic`, division by zero, overflow, a resource limit.
    pub const RUNTIME_ERROR: &str = "E0502";
    pub const INTERNAL_ERROR: &str = "E0505";
    /// Cache codes are warnings: cache trouble is never a fault in the user's program.
    pub const CACHE_UNREADABLE: &str = "W0601";
    pub const CACHE_CORRUPT: &str = "W0602";
    pub const CACHE_VERSION_CHANGED: &str = "W0603";
    /// An obligation no tier could decide.
    pub const OBLIGATION_NOT_DISCHARGED: &str = "W0604";
    /// The stdlib shipped with this compiler differs from the one the cache was written under.
    pub const STDLIB_CHANGED: &str = "W0605";
    /// A host runtime could not hand every resource back when an entry point ended.
    pub const HOST_TEARDOWN: &str = "W0606";
    /// An explicitly supplied configuration key the run's schema does not declare.
    pub const CONFIG_UNDECLARED: &str = "W0607";
    /// The drain deadline expired with connections still in flight.
    pub const DRAIN_INCOMPLETE: &str = "W0608";
    /// A value was made to reach itself; cycles are not collected, so it leaks.
    pub const REFERENCE_CYCLE: &str = "W0610";
    /// Spans still open when an entry point ended, closed by teardown.
    pub const SPAN_ABANDONED: &str = "W0609";
    /// A definition no `pub` item, `main`, test or law reaches.
    pub const UNUSED_DEFINITION: &str = "W0611";
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

pub mod frames;
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

    /// Tooling matches on codes, so a number is never reused or renumbered.
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
            ("REUSE_BROKEN", codes::REUSE_BROKEN, "E0127"),
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
            ("LITERAL_OUT_OF_RANGE", codes::LITERAL_OUT_OF_RANGE, "E0211"),
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
            ("FS_ROOT_UNBOUND", codes::FS_ROOT_UNBOUND, "E0451"),
            ("FS_PATH_ESCAPES_ROOT", codes::FS_PATH_ESCAPES_ROOT, "E0452"),
            ("FS_FILE_TOO_LARGE", codes::FS_FILE_TOO_LARGE, "E0453"),
            ("FS_ROOT_INVALID", codes::FS_ROOT_INVALID, "E0454"),
            ("ASSERTION_FAILED", codes::ASSERTION_FAILED, "E0501"),
            ("RUNTIME_ERROR", codes::RUNTIME_ERROR, "E0502"),
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
            ("UNUSED_DEFINITION", codes::UNUSED_DEFINITION, "W0611"),
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
