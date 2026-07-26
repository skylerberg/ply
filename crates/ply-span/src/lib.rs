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
    pub const TYPE_MISMATCH: &str = "E0201";
    pub const ARITY_MISMATCH: &str = "E0202";
    pub const OCCURS_CHECK: &str = "E0203";
    pub const NOT_A_FUNCTION: &str = "E0204";
    pub const NON_EXHAUSTIVE_MATCH: &str = "E0205";
    pub const UNBOUND_ROW_VAR: &str = "E0301";
    pub const EFFECT_NOT_PERMITTED: &str = "E0302";
    pub const UNHANDLED_EFFECT: &str = "E0303";
    pub const RESOURCE_REQUIRED: &str = "E0304";
    pub const NONDET_IN_DET_TEST: &str = "E0412";
    pub const ASSERTION_FAILED: &str = "E0501";
    pub const RUNTIME_ERROR: &str = "E0502";
    /// `W` rather than `E`: cache trouble is never a fault in the user's
    /// program, so these are always warnings.
    pub const CACHE_UNREADABLE: &str = "W0601";
    pub const CACHE_CORRUPT: &str = "W0602";
    pub const CACHE_VERSION_CHANGED: &str = "W0603";
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
            ("TYPE_MISMATCH", codes::TYPE_MISMATCH, "E0201"),
            ("ARITY_MISMATCH", codes::ARITY_MISMATCH, "E0202"),
            ("OCCURS_CHECK", codes::OCCURS_CHECK, "E0203"),
            ("NOT_A_FUNCTION", codes::NOT_A_FUNCTION, "E0204"),
            ("NON_EXHAUSTIVE_MATCH", codes::NON_EXHAUSTIVE_MATCH, "E0205"),
            ("UNBOUND_ROW_VAR", codes::UNBOUND_ROW_VAR, "E0301"),
            ("EFFECT_NOT_PERMITTED", codes::EFFECT_NOT_PERMITTED, "E0302"),
            ("UNHANDLED_EFFECT", codes::UNHANDLED_EFFECT, "E0303"),
            ("RESOURCE_REQUIRED", codes::RESOURCE_REQUIRED, "E0304"),
            ("NONDET_IN_DET_TEST", codes::NONDET_IN_DET_TEST, "E0412"),
            ("ASSERTION_FAILED", codes::ASSERTION_FAILED, "E0501"),
            ("RUNTIME_ERROR", codes::RUNTIME_ERROR, "E0502"),
            ("CACHE_UNREADABLE", codes::CACHE_UNREADABLE, "W0601"),
            ("CACHE_CORRUPT", codes::CACHE_CORRUPT, "W0602"),
            (
                "CACHE_VERSION_CHANGED",
                codes::CACHE_VERSION_CHANGED,
                "W0603",
            ),
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
