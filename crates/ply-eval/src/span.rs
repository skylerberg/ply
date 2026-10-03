//! Spans, sources and diagnostics; `crates/ply-cli/ply/diagnostic.ply` renders them.

use crate::Plain;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// A cheaply-cloned name.
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
        line_starts.extend(memchr::memchr_iter(b'\n', text.as_bytes()).map(|i| (i + 1) as u32));
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

    pub fn snippet(&self, span: Span) -> Cow<'_, str> {
        match self.containing(span) {
            Some(f) => String::from_utf8_lossy(&f.text.as_bytes()[span.range()]),
            None => Cow::Borrowed(""),
        }
    }

    /// The file a span is a byte range of. A span that cuts a character in half is a defect in
    /// whoever built it, and the line it points at is worth more to a reader than a dropped
    /// label, so the only bound is the text's length.
    pub fn containing(&self, span: Span) -> Option<&SourceFile> {
        self.get(span.source)
            .filter(|f| span.start <= span.end && span.end as usize <= f.text.len())
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

/// One replacement a fix makes: `text` in place of `span`; an empty span inserts, empty text deletes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edit {
    pub span: Span,
    pub text: String,
}

/// A change that applies as it is and leaves a program the diagnostic no longer holds for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fix {
    pub title: String,
    pub edits: Vec<Edit>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: &'static str,
    pub message: String,
    pub labels: Vec<Label>,
    pub notes: Vec<String>,
    pub fixes: Sparse<Fix>,
    /// What the text names by [`slot`], rendered only by the CLI.
    #[serde(default, skip_serializing_if = "Sparse::is_empty")]
    pub values: Sparse<Plain>,
}

/// A list most diagnostics leave empty, held as one pointer: every runtime `Result` carries a
/// `Diagnostic`, so its size is paid on every call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
#[allow(clippy::box_collection)]
pub struct Sparse<T>(Option<Box<Vec<T>>>);

impl<T> Sparse<T> {
    pub fn new() -> Sparse<T> {
        Sparse(None)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_none()
    }

    pub fn push(&mut self, item: T) {
        self.0.get_or_insert_with(Box::default).push(item);
    }
}

impl<T> Default for Sparse<T> {
    fn default() -> Sparse<T> {
        Sparse::new()
    }
}

impl<T> From<Vec<T>> for Sparse<T> {
    fn from(items: Vec<T>) -> Sparse<T> {
        Sparse((!items.is_empty()).then(|| Box::new(items)))
    }
}

impl<T> FromIterator<T> for Sparse<T> {
    fn from_iter<I: IntoIterator<Item = T>>(items: I) -> Sparse<T> {
        items.into_iter().collect::<Vec<T>>().into()
    }
}

impl<T> std::ops::Deref for Sparse<T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        self.0.as_deref().map_or(&[], Vec::as_slice)
    }
}

impl<'a, T> IntoIterator for &'a Sparse<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<T> IntoIterator for Sparse<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.map(|items| *items).unwrap_or_default().into_iter()
    }
}

/// Where a diagnostic's message, label or note stands its `values[i]`.
pub fn slot(i: usize) -> String {
    format!("\u{1}{i}\u{2}")
}

impl Diagnostic {
    pub fn error(code: &'static str, message: impl Into<String>) -> Self {
        Diagnostic {
            severity: Severity::Error,
            code,
            message: message.into(),
            labels: Vec::new(),
            notes: Vec::new(),
            fixes: Sparse::new(),
            values: Sparse::new(),
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

    pub fn fix(mut self, title: impl Into<String>, edits: Vec<Edit>) -> Self {
        self.fixes.push(Fix {
            title: title.into(),
            edits,
        });
        self
    }

    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    /// Carries `values`, which the text names by [`slot`].
    pub fn showing(mut self, values: Vec<Plain>) -> Self {
        self.values = values.into();
        self
    }

    /// `text` with each slot said as what its value is, for a reader with no renderer.
    pub fn described(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(open) = rest.find('\u{1}') {
            out.push_str(&rest[..open]);
            let after = &rest[open + 1..];
            let Some(close) = after.find('\u{2}') else {
                out.push_str(&rest[open..]);
                return out;
            };
            let said = after[..close]
                .parse::<usize>()
                .ok()
                .and_then(|i| self.values.get(i))
                .map_or("a value", Plain::describe);
            out.push_str(said);
            rest = &after[close + 1..];
        }
        out.push_str(rest);
        out
    }

    pub fn primary_span(&self) -> Option<Span> {
        self.labels
            .iter()
            .find(|l| l.primary)
            .or_else(|| self.labels.first())
            .map(|l| l.span)
    }

    /// For a reader that will not hold `sources`: each label they place becomes a note.
    pub fn placed(self, sources: &SourceMap) -> Diagnostic {
        let notes: Vec<String> = self
            .labels
            .iter()
            .filter_map(|l| {
                let file = sources.containing(l.span)?;
                let (line, col) = file.line_col(l.span.start);
                let at = format!("at {}:{line}:{col}", file.path.display());
                Some(if l.message.is_empty() {
                    at
                } else {
                    format!("{at}: {}", l.message)
                })
            })
            .collect();
        notes.into_iter().fold(self, Diagnostic::note)
    }
}

/// With no source to point into: the heading, then a line per note and per fix.
impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let titled = match self.severity {
            Severity::Error => "Error",
            Severity::Warning => "Warning",
            Severity::Note => "Note",
        };
        write!(
            f,
            "{titled}[{}]: {}",
            self.code,
            self.described(&self.message)
        )?;
        for note in &self.notes {
            write!(f, "\n  = {}", self.described(note))?;
        }
        for fix in &self.fixes {
            write!(f, "\n  = fix: {}", fix.title)?;
        }
        Ok(())
    }
}

impl std::error::Error for Diagnostic {}

/// A code read at run time as a [`Diagnostic`]'s `&'static str`, leaked once per distinct code.
pub fn intern_code(code: &str) -> &'static str {
    static POOL: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut pool = POOL
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(existing) = pool.get(code) {
        return existing;
    }
    let leaked: &'static str = Box::leak(code.to_owned().into_boxed_str());
    pool.insert(leaked);
    leaked
}
