use crate::codec::{FORMAT, shown, tag};
use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

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

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct SourceId(pub u32);

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Span {
    pub source: SourceId,
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub fn new(source: SourceId, start: u32, end: u32) -> Self {
        Span { source, start, end }
    }

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

    pub fn containing(&self, span: Span) -> Option<&SourceFile> {
        self.get(span.source)
            .filter(|f| span.start <= span.end && span.end as usize <= f.text.len())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Clone, Debug)]
pub struct Label {
    pub span: Span,
    pub message: String,
    pub primary: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub span: Span,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fix {
    pub title: String,
    pub edits: Vec<Edit>,
}

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: &'static str,
    pub message: String,
    pub labels: Vec<Label>,
    pub notes: Vec<String>,
    pub fixes: Sparse<Fix>,
    pub values: Sparse<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
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

    pub fn showing(mut self, values: Vec<Vec<u8>>) -> Self {
        self.values = values.into();
        self
    }

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
                .map_or("a value", |v| told(v));
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

fn told(bytes: &[u8]) -> &'static str {
    let at = FORMAT.len();
    match bytes.get(at).copied() {
        Some(tag::UNIT) => "`()`",
        Some(tag::FALSE | tag::TRUE) => "a `Bool`",
        Some(tag::INT) => "an `Int`",
        Some(tag::FLOAT) => "a `Float`",
        Some(tag::DECIMAL) => "a `Decimal`",
        Some(tag::FIXED) => "a fixed-width integer",
        Some(tag::CHAR) => "a `Char`",
        Some(tag::STR) => "a `String`",
        Some(tag::BYTES) => "a `Bytes`",
        Some(tag::LIST) => "a `List`",
        Some(tag::ARRAY) => "an `Array`",
        Some(tag::RECORD) => "a record",
        Some(tag::CTOR) => "a variant",
        Some(tag::MAP) => "a `Map`",
        Some(tag::FUN) => "a function",
        Some(tag::HANDLE) if bytes.get(at + 1).copied() == Some(shown::SCOPED as u8) => {
            "a `Scoped`"
        }
        Some(tag::HANDLE) => "a `Secret`",
        _ => "a value",
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let titled = match self.severity {
            Severity::Error => "Error",
            Severity::Warning => "Warning",
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
