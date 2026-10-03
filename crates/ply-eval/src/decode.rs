//! Reading a plain value a Ply entry answered: a record's fields by name, a list's items, a
//! constructor by its simple name. A read that fails says where in the answer it failed.

use crate::limit::grow;
use crate::value::Value;
use std::fmt;
use std::fmt::Write as _;

/// A read that failed: the path from the answer's root to the value, and what was wrong there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub path: String,
    pub message: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}

impl std::error::Error for Error {}

/// One value of an answer, beside the answer it is in.
#[derive(Clone, Copy)]
pub struct AnswerValue<'v> {
    answer: &'v str,
    root: &'v Value,
    value: &'v Value,
}

impl<'v> AnswerValue<'v> {
    /// `answer` names the whole value in every error, as in "`costs.costs`' answer".
    pub fn new(answer: &'v str, value: &'v Value) -> AnswerValue<'v> {
        AnswerValue {
            answer,
            root: value,
            value,
        }
    }

    pub fn value(self) -> &'v Value {
        self.value
    }

    fn inner(self, value: &'v Value) -> AnswerValue<'v> {
        AnswerValue { value, ..self }
    }

    /// The path is searched for only when a read fails, so a read that succeeds pays nothing for it.
    fn place(self) -> String {
        let mut path = self.answer.to_string();
        let mut steps = Vec::new();
        if find(self.root, self.value, &mut steps) {
            for step in &steps {
                step.write(&mut path);
            }
        }
        path
    }

    /// An error about this value, for a check only its reader knows to make.
    pub fn error(self, message: impl Into<String>) -> Error {
        Error {
            path: self.place(),
            message: message.into(),
        }
    }

    fn expected(self, what: &str) -> Error {
        self.error(format!("expected {what}, found {}", sketch(self.value)))
    }

    pub fn field(self, name: &str) -> Result<AnswerValue<'v>, Error> {
        let Value::Record(fields) = self.value else {
            return Err(self.expected(&format!("a record with a `{name}` field")));
        };
        match fields.named(name) {
            Some(value) => Ok(self.inner(value)),
            None => Err(Error {
                path: format!("{}.{name}", self.place()),
                message: format!(
                    "no such field; the record has {}",
                    listed(fields.keys().map(|k| k.as_str()))
                ),
            }),
        }
    }

    pub fn list(self) -> Result<AnswerItems<'v>, Error> {
        match self.value {
            Value::List(items) => Ok(AnswerItems {
                at: self,
                items: items.iter(),
            }),
            _ => Err(self.expected("a list")),
        }
    }

    /// Every item of a list, each read by `read`.
    pub fn items<T>(
        self,
        read: impl FnMut(AnswerValue<'v>) -> Result<T, Error>,
    ) -> Result<Vec<T>, Error> {
        self.list()?.map(read).collect()
    }

    /// A map's entries, in its key order.
    pub fn entries(
        self,
    ) -> Result<impl Iterator<Item = (AnswerValue<'v>, AnswerValue<'v>)>, Error> {
        match self.value {
            Value::Map(m) => Ok(m.iter().map(move |(k, v)| (self.inner(k), self.inner(v)))),
            _ => Err(self.expected("a map")),
        }
    }

    pub fn ctor(self) -> Result<AnswerCtor<'v>, Error> {
        match self.value {
            Value::Ctor { name, args } => Ok(AnswerCtor {
                at: self,
                name: simple(name.as_str()),
                args: args.as_slice(),
            }),
            _ => Err(self.expected("a constructor")),
        }
    }

    pub fn option(self) -> Result<Option<AnswerValue<'v>>, Error> {
        if let Value::Ctor { name, args } = self.value {
            match (simple(name.as_str()), args.as_slice()) {
                ("Some", [x]) => return Ok(Some(self.inner(x))),
                ("None", []) => return Ok(None),
                _ => {}
            }
        }
        Err(self.expected("an `Option`"))
    }

    pub fn result(self) -> Result<Result<AnswerValue<'v>, AnswerValue<'v>>, Error> {
        if let Value::Ctor { name, args } = self.value {
            match (simple(name.as_str()), args.as_slice()) {
                ("Ok", [x]) => return Ok(Ok(self.inner(x))),
                ("Err", [x]) => return Ok(Err(self.inner(x))),
                _ => {}
            }
        }
        Err(self.expected("a `Result`"))
    }

    pub fn bytes(self) -> Result<&'v [u8], Error> {
        match self.value {
            Value::Bytes(b) => Ok(&b[..]),
            _ => Err(self.expected("`Bytes`")),
        }
    }

    /// `Bytes` that must be UTF-8: most of what the compiler answers is text it holds as bytes.
    pub fn utf8(self) -> Result<&'v str, Error> {
        std::str::from_utf8(self.bytes()?)
            .map_err(|e| self.error(format!("`Bytes` that are not UTF-8: {e}")))
    }

    pub fn byte_array<const N: usize>(self) -> Result<[u8; N], Error> {
        let b = self.bytes()?;
        <[u8; N]>::try_from(b)
            .map_err(|_| self.error(format!("expected {N} bytes, found {}", b.len())))
    }

    pub fn str(self) -> Result<&'v str, Error> {
        match self.value {
            Value::Str(s) => Ok(&s[..]),
            _ => Err(self.expected("a `String`")),
        }
    }

    pub fn int(self) -> Result<i64, Error> {
        match self.value {
            Value::Int(n) => Ok(*n),
            _ => Err(self.expected("an `Int`")),
        }
    }

    /// An `Int` that must fit `T`, as a count, an offset or an index does.
    pub fn number<T: TryFrom<i64>>(self) -> Result<T, Error> {
        let n = self.int()?;
        T::try_from(n).map_err(|_| {
            self.error(format!(
                "{n} does not fit a `{}`",
                std::any::type_name::<T>()
            ))
        })
    }

    pub fn bool(self) -> Result<bool, Error> {
        match self.value {
            Value::Bool(b) => Ok(*b),
            _ => Err(self.expected("a `Bool`")),
        }
    }
}

impl fmt::Debug for AnswerValue<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.place(), sketch(self.value))
    }
}

/// A list's items, each beside the answer.
pub struct AnswerItems<'v> {
    at: AnswerValue<'v>,
    items: crate::list::Iter<'v>,
}

impl<'v> Iterator for AnswerItems<'v> {
    type Item = AnswerValue<'v>;

    fn next(&mut self) -> Option<AnswerValue<'v>> {
        self.items.next().map(|v| self.at.inner(v))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.items.size_hint()
    }
}

impl ExactSizeIterator for AnswerItems<'_> {}

/// A constructor, read by its simple name.
#[derive(Clone, Copy)]
pub struct AnswerCtor<'v> {
    at: AnswerValue<'v>,
    name: &'v str,
    args: &'v [Value],
}

impl<'v> AnswerCtor<'v> {
    /// The name after the last `.`: one type's constructors are qualified by the program that built
    /// the value, `emit.Body` in the bundle and `compiler.emit.Body` in a program importing it.
    pub fn name(self) -> &'v str {
        self.name
    }

    pub fn arg(self, i: usize) -> Result<AnswerValue<'v>, Error> {
        match self.args.get(i) {
            Some(value) => Ok(self.at.inner(value)),
            None => Err(self.at.error(format!(
                "`{}` holds {} argument(s), and argument {i} was read",
                self.name,
                self.args.len()
            ))),
        }
    }

    /// The error for a constructor its reader has no case for.
    pub fn unknown(self) -> Error {
        self.at.error(format!(
            "a `{}`, which this reader does not know",
            self.name
        ))
    }
}

fn simple(name: &str) -> &str {
    name.rsplit_once('.').map_or(name, |(_, simple)| simple)
}

#[derive(Clone, Copy)]
enum Step<'v> {
    Field(&'v str),
    Item(usize),
    /// A constructor's argument, written as the constructor alone when it takes just one.
    Arg {
        ctor: &'v str,
        index: usize,
        of: usize,
    },
    Key(&'v Value),
    Entry(&'v Value),
}

impl Step<'_> {
    fn write(&self, out: &mut String) {
        let _ = match self {
            Step::Field(name) => write!(out, ".{name}"),
            Step::Item(i) => write!(out, "[{i}]"),
            Step::Arg { ctor, of: 1, .. } => write!(out, ".{ctor}"),
            Step::Arg { ctor, index, .. } => write!(out, ".{ctor}.{index}"),
            Step::Key(k) => write!(out, ".key({})", sketch(k)),
            Step::Entry(k) => write!(out, "[{}]", sketch(k)),
        };
    }
}

/// Whether `target` is `from` or inside it, `steps` then leading from the one to the other. By
/// address, so a value shared at two places is found at the first.
fn find<'v>(from: &'v Value, target: &Value, steps: &mut Vec<Step<'v>>) -> bool {
    if std::ptr::eq(from, target) {
        return true;
    }
    grow(|| match from {
        Value::Record(fields) => fields
            .iter()
            .any(|(name, v)| descend(Step::Field(name.as_str()), v, target, steps)),
        Value::List(items) => items
            .iter()
            .enumerate()
            .any(|(i, v)| descend(Step::Item(i), v, target, steps)),
        Value::Array(items) => items
            .iter()
            .enumerate()
            .any(|(i, v)| descend(Step::Item(i), v, target, steps)),
        Value::Ctor { name, args } => args.iter().enumerate().any(|(index, v)| {
            let step = Step::Arg {
                ctor: simple(name.as_str()),
                index,
                of: args.len(),
            };
            descend(step, v, target, steps)
        }),
        Value::Map(m) => m.iter().any(|(k, v)| {
            descend(Step::Key(k), k, target, steps) || descend(Step::Entry(k), v, target, steps)
        }),
        _ => false,
    })
}

fn descend<'v>(
    step: Step<'v>,
    value: &'v Value,
    target: &Value,
    steps: &mut Vec<Step<'v>>,
) -> bool {
    steps.push(step);
    let found = find(value, target, steps);
    if !found {
        steps.pop();
    }
    found
}

/// What a value is, in a line, by its shape: only the CLI renders a value.
fn sketch(v: &Value) -> String {
    match v {
        Value::Record(fields) => format!(
            "a record with {}",
            listed(fields.keys().map(|k| k.as_str()))
        ),
        Value::Ctor { name, args } => format!("`{name}` holding {} argument(s)", args.len()),
        Value::List(items) => format!("a list of {}", items.len()),
        Value::Array(items) => format!("an array of {}", items.len()),
        Value::Map(m) => format!("a map of {}", m.size()),
        Value::Str(text) => format!("a `String` of {} characters", text.chars().count()),
        Value::Bytes(b) => format!("a `Bytes` of {} bytes", b.len()),
        other => format!("a `{}`", other.type_name()),
    }
}

fn listed<'a>(names: impl Iterator<Item = &'a str>) -> String {
    let names: Vec<String> = names.map(|n| format!("`{n}`")).collect();
    if names.is_empty() {
        "no fields".to_string()
    } else {
        names.join(", ")
    }
}
