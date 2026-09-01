//! The pinned mapping between a Ply value and a postgres wire type, in both directions.

use ply_span::{Diagnostic, Span, codes};
use postgres_protocol::types as wire;
use rust_decimal::Decimal;
use std::error::Error;
use tokio_postgres::types::private::BytesMut;
use tokio_postgres::types::{FromSql, IsNull, Kind, ToSql, Type, to_sql_checked};

/// A JSON document, as the driver holds one.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Json {
    Null,
    Bool(bool),
    Number(Decimal),
    Str(String),
    Array(Vec<Json>),
    /// Insertion order as the document had it.
    Object(Vec<(String, Json)>),
}

/// Deeper than this and the driver is walking a document a peer chose the shape of.
const MAX_JSON_DEPTH: usize = 64;

impl Json {
    /// Parse strict JSON.
    pub fn parse(bytes: &[u8], at: &str) -> Result<Json, String> {
        let text = std::str::from_utf8(bytes)
            .map_err(|_| format!("{at} is not UTF-8, so it is not JSON"))?;
        let mut p = JsonParser {
            src: text.as_bytes(),
            at: 0,
            depth: 0,
        };
        p.space();
        let value = p.value().map_err(|e| format!("{at}: {e}"))?;
        p.space();
        if p.at != p.src.len() {
            return Err(format!("{at}: trailing data at byte {}", p.at));
        }
        Ok(value)
    }

    /// Canonical text: no whitespace, keys in the order held, `Decimal`'s own rendering for a
    /// number so a scale survives the round trip.
    pub fn render(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(true) => out.push_str("true"),
            Json::Bool(false) => out.push_str("false"),
            Json::Number(d) => out.push_str(&d.to_string()),
            Json::Str(s) => escape(s, out),
            Json::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            Json::Object(fields) => {
                out.push('{');
                for (i, (key, value)) in fields.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    escape(key, out);
                    out.push(':');
                    value.write(out);
                }
                out.push('}');
            }
        }
    }
}

fn escape(s: &str, out: &mut String) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

struct JsonParser<'a> {
    src: &'a [u8],
    at: usize,
    depth: usize,
}

impl JsonParser<'_> {
    fn space(&mut self) {
        while matches!(self.src.get(self.at), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    fn value(&mut self) -> Result<Json, String> {
        self.depth += 1;
        if self.depth > MAX_JSON_DEPTH {
            return Err(format!("nesting past {MAX_JSON_DEPTH} at byte {}", self.at));
        }
        let out = match self.src.get(self.at) {
            None => return Err("the document ends where a value was expected".into()),
            Some(b'n') => {
                self.literal("null")?;
                Json::Null
            }
            Some(b't') => {
                self.literal("true")?;
                Json::Bool(true)
            }
            Some(b'f') => {
                self.literal("false")?;
                Json::Bool(false)
            }
            Some(b'"') => Json::Str(self.string()?),
            Some(b'[') => {
                self.at += 1;
                let mut items = Vec::new();
                self.space();
                if self.src.get(self.at) == Some(&b']') {
                    self.at += 1;
                } else {
                    loop {
                        self.space();
                        items.push(self.value()?);
                        self.space();
                        match self.src.get(self.at) {
                            Some(b',') => self.at += 1,
                            Some(b']') => {
                                self.at += 1;
                                break;
                            }
                            _ => return Err(format!("`,` or `]` expected at byte {}", self.at)),
                        }
                    }
                }
                Json::Array(items)
            }
            Some(b'{') => {
                self.at += 1;
                let mut fields = Vec::new();
                self.space();
                if self.src.get(self.at) == Some(&b'}') {
                    self.at += 1;
                } else {
                    loop {
                        self.space();
                        let key = self.string()?;
                        self.space();
                        if self.src.get(self.at) != Some(&b':') {
                            return Err(format!("`:` expected at byte {}", self.at));
                        }
                        self.at += 1;
                        self.space();
                        fields.push((key, self.value()?));
                        self.space();
                        match self.src.get(self.at) {
                            Some(b',') => self.at += 1,
                            Some(b'}') => {
                                self.at += 1;
                                break;
                            }
                            _ => return Err(format!("`,` or `}}` expected at byte {}", self.at)),
                        }
                    }
                }
                Json::Object(fields)
            }
            Some(_) => Json::Number(self.number()?),
        };
        self.depth -= 1;
        Ok(out)
    }

    fn literal(&mut self, word: &str) -> Result<(), String> {
        if self.src[self.at..].starts_with(word.as_bytes()) {
            self.at += word.len();
            Ok(())
        } else {
            Err(format!("`{word}` expected at byte {}", self.at))
        }
    }

    fn string(&mut self) -> Result<String, String> {
        if self.src.get(self.at) != Some(&b'"') {
            return Err(format!("a string was expected at byte {}", self.at));
        }
        self.at += 1;
        let mut out = String::new();
        loop {
            match self.src.get(self.at) {
                None => return Err("a string is never closed".into()),
                Some(b'"') => {
                    self.at += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.at += 1;
                    let escape = *self
                        .src
                        .get(self.at)
                        .ok_or("an escape at the end of the document")?;
                    self.at += 1;
                    match escape {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{08}'),
                        b'f' => out.push('\u{0c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let unit = self.hex4()?;
                            // A surrogate pair is two escapes and one character; pushing the halves
                            // separately would produce text that is not what the document held.
                            if (0xd800..0xdc00).contains(&unit) {
                                if self.src.get(self.at) != Some(&b'\\')
                                    || self.src.get(self.at + 1) != Some(&b'u')
                                {
                                    return Err("a high surrogate with no low surrogate".into());
                                }
                                self.at += 2;
                                let low = self.hex4()?;
                                if !(0xdc00..0xe000).contains(&low) {
                                    return Err(
                                        "a high surrogate followed by a non-surrogate".into()
                                    );
                                }
                                let combined = 0x10000 + ((unit - 0xd800) << 10) + (low - 0xdc00);
                                out.push(char::from_u32(combined).ok_or("an unpaired surrogate")?);
                            } else {
                                out.push(char::from_u32(unit).ok_or("an unpaired surrogate")?);
                            }
                        }
                        other => {
                            return Err(format!("`\\{}` is not a JSON escape", other as char));
                        }
                    }
                }
                Some(b) if *b < 0x20 => {
                    return Err(format!(
                        "an unescaped control character at byte {}",
                        self.at
                    ));
                }
                Some(_) => {
                    let rest = std::str::from_utf8(&self.src[self.at..])
                        .map_err(|_| "the document is not UTF-8".to_string())?;
                    let ch = rest.chars().next().expect("a non-empty remainder");
                    out.push(ch);
                    self.at += ch.len_utf8();
                }
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let end = self.at + 4;
        let slice = self
            .src
            .get(self.at..end)
            .ok_or("a `\\u` escape with fewer than four digits")?;
        let text = std::str::from_utf8(slice).map_err(|_| "a `\\u` escape that is not hex")?;
        let value = u32::from_str_radix(text, 16).map_err(|_| "a `\\u` escape that is not hex")?;
        self.at = end;
        Ok(value)
    }

    fn number(&mut self) -> Result<Decimal, String> {
        let start = self.at;
        if self.src.get(self.at) == Some(&b'-') {
            self.at += 1;
        }
        while self
            .src
            .get(self.at)
            .is_some_and(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-'))
        {
            self.at += 1;
        }
        let text = std::str::from_utf8(&self.src[start..self.at])
            .map_err(|_| "a number that is not UTF-8".to_string())?;
        if text.is_empty() {
            return Err(format!("a value was expected at byte {start}"));
        }
        text.parse::<Decimal>()
            .map_err(|_| format!("`{text}` at byte {start} is outside `Decimal`'s range"))
    }
}

/// What a program hands a `db` operation, decoded and ready to be checked against the type the
/// server described for it.
#[derive(Clone, PartialEq, Debug)]
pub enum Param {
    Null,
    Int(i64),
    Bool(bool),
    Text(String),
    Bytes(Vec<u8>),
    Float(f64),
    Numeric(Decimal),
    Json(Json),
    /// One dimension, homogeneous, and never holding a `Null`.
    Array(Vec<Param>),
}

impl Param {
    /// How a refusal names the constructor the program wrote.
    pub fn what(&self) -> &'static str {
        match self {
            Param::Null => "`PNull`",
            Param::Int(_) => "`PInt`",
            Param::Bool(_) => "`PBool`",
            Param::Text(_) => "`PText`",
            Param::Bytes(_) => "`PBytes`",
            Param::Float(_) => "`PFloat`",
            Param::Numeric(_) => "`PNumeric`",
            Param::Json(_) => "`PJson`",
            Param::Array(_) => "`PArray`",
        }
    }
}

/// What comes back out of a column.
#[derive(Clone, PartialEq, Debug)]
pub enum Datum {
    Null,
    Int(i64),
    Bool(bool),
    Text(String),
    Bytes(Vec<u8>),
    Float(f64),
    Numeric(Decimal),
    Json(Json),
    Array(Vec<Datum>),
}

/// The SQLSTATE the server returned and the object it named.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DbError {
    pub code: String,
    pub constraint: String,
    pub detail: String,
}

impl DbError {
    pub fn new(code: &str, constraint: &str, detail: impl Into<String>) -> DbError {
        DbError {
            code: code.to_string(),
            constraint: constraint.to_string(),
            detail: detail.into(),
        }
    }

    /// The connection went away mid-statement.
    pub fn connection(detail: impl Into<String>) -> DbError {
        DbError::new("08006", "", detail)
    }
}

/// A parameter that failed before anything was sent.
pub enum BindError {
    /// The program's claim and the statement's shape disagree, every time, for this statement text.
    Refused(Diagnostic),
    /// A value the column cannot hold.
    Failed(DbError),
}

/// A parameter checked against the type the server described for it.
#[derive(Clone, Debug)]
pub struct Bound {
    ty: Type,
    value: BoundValue,
}

#[derive(Clone, Debug)]
enum BoundValue {
    Null,
    Int2(i16),
    Int4(i32),
    Int8(i64),
    Bool(bool),
    Text(String),
    Bytes(Vec<u8>),
    Float8(f64),
    Numeric(Decimal),
    /// Serialized once here rather than per retry.
    Json(String),
    Array(Vec<Bound>),
}

impl ToSql for Bound {
    fn to_sql(
        &self,
        _ty: &Type,
        out: &mut BytesMut,
    ) -> Result<IsNull, Box<dyn Error + Sync + Send>> {
        match &self.value {
            BoundValue::Null => Ok(IsNull::Yes),
            BoundValue::Int2(v) => {
                wire::int2_to_sql(*v, out);
                Ok(IsNull::No)
            }
            BoundValue::Int4(v) => {
                wire::int4_to_sql(*v, out);
                Ok(IsNull::No)
            }
            BoundValue::Int8(v) => {
                wire::int8_to_sql(*v, out);
                Ok(IsNull::No)
            }
            BoundValue::Bool(v) => {
                wire::bool_to_sql(*v, out);
                Ok(IsNull::No)
            }
            BoundValue::Text(v) => {
                wire::text_to_sql(v, out);
                Ok(IsNull::No)
            }
            BoundValue::Bytes(v) => {
                wire::bytea_to_sql(v, out);
                Ok(IsNull::No)
            }
            BoundValue::Float8(v) => {
                wire::float8_to_sql(*v, out);
                Ok(IsNull::No)
            }
            BoundValue::Numeric(v) => v.to_sql(&Type::NUMERIC, out),
            BoundValue::Json(text) => {
                if self.ty == Type::JSONB {
                    // The jsonb binary format is a version byte and then the text.
                    out.extend_from_slice(&[1u8]);
                }
                out.extend_from_slice(text.as_bytes());
                Ok(IsNull::No)
            }
            // One dimension, framed by `postgres-types` itself: a hand-rolled framer here would be
            // a second implementation of a format the decoder already reads through theirs.
            BoundValue::Array(items) => items.to_sql(&self.ty, out),
        }
    }

    /// Every `Bound` was built against the type the server described, so this is the check having
    /// already happened rather than one being skipped.
    fn accepts(_ty: &Type) -> bool {
        true
    }

    to_sql_checked!();
}

/// Check and encode every parameter against the types the server described.
pub fn bind(params: &[Param], types: &[Type], span: Span) -> Result<Vec<Bound>, BindError> {
    if params.len() != types.len() {
        return Err(BindError::Refused(
            Diagnostic::error(
                codes::DB_STATEMENT_REFUSED,
                format!(
                    "this statement takes {} parameter{} and was given {}",
                    types.len(),
                    if types.len() == 1 { "" } else { "s" },
                    params.len()
                ),
            )
            .primary(span, "this statement reaches the database driver")
            .note("a parameter is bound by position: `$1` is the first element of the list"),
        ));
    }
    let mut out = Vec::with_capacity(params.len());
    for (index, (param, ty)) in params.iter().zip(types).enumerate() {
        out.push(bind_one(param, ty, index + 1, span)?);
    }
    Ok(out)
}

fn bind_one(param: &Param, ty: &Type, position: usize, span: Span) -> Result<Bound, BindError> {
    let value = match (param, ty) {
        (Param::Null, _) => BoundValue::Null,
        (Param::Bool(v), &Type::BOOL) => BoundValue::Bool(*v),
        (Param::Int(v), &Type::INT8) => BoundValue::Int8(*v),
        // An `Int` is always an `Int`; the column decides the width.
        (Param::Int(v), &Type::INT4) => match i32::try_from(*v) {
            Ok(n) => BoundValue::Int4(n),
            Err(_) => return Err(BindError::Failed(out_of_range(*v, "integer", position))),
        },
        (Param::Int(v), &Type::INT2) => match i16::try_from(*v) {
            Ok(n) => BoundValue::Int2(n),
            Err(_) => return Err(BindError::Failed(out_of_range(*v, "smallint", position))),
        },
        (Param::Text(v), &Type::TEXT | &Type::VARCHAR | &Type::BPCHAR | &Type::NAME) => {
            BoundValue::Text(v.clone())
        }
        (Param::Text(v), &Type::UUID) => match parse_uuid(v) {
            Some(bytes) => BoundValue::Bytes(bytes.to_vec()),
            None => {
                return Err(BindError::Failed(DbError::new(
                    "22P02",
                    "",
                    format!("parameter ${position} is not a uuid: `{v}`"),
                )));
            }
        },
        (Param::Bytes(v), &Type::BYTEA) => BoundValue::Bytes(v.clone()),
        (Param::Float(v), &Type::FLOAT8) => BoundValue::Float8(*v),
        // The type mapping maps `Float` to `float8` **as a parameter** and to `float4` or `float8` only
        // as a *result*, so a `float4` parameter is outside the pinned mapping.
        (Param::Float(_), &Type::FLOAT4) => {
            return Err(BindError::Refused(
                Diagnostic::error(
                    codes::DB_STATEMENT_REFUSED,
                    format!("parameter ${position} is a `Float` and the statement wants `float4`"),
                )
                .primary(span, "this statement reaches the database driver")
                .note("the mapping sends every `Float` as `float8`: `float4` is a *result* type in it and not a parameter type")
                .note("`float4` holds 24 bits of mantissa, so binding one here would round a value the program never asked to round, and a value past its range would become an infinity")
                .note(format!("write the narrowing where a reader can see it — `${position}::float8::float4` — or declare the column `float8`")),
            ));
        }
        (Param::Numeric(v), &Type::NUMERIC) => BoundValue::Numeric(*v),
        // An `Int` bound to a `numeric` parameter, which is what a statement assigning to a
        // `numeric` column describes.
        (Param::Int(v), &Type::NUMERIC) => BoundValue::Numeric(Decimal::from(*v)),
        (Param::Json(v), &Type::JSON) | (Param::Json(v), &Type::JSONB) => {
            BoundValue::Json(v.render())
        }
        (Param::Array(items), ty) if matches!(ty.kind(), Kind::Array(_)) => {
            let Kind::Array(member) = ty.kind() else {
                unreachable!("checked by the guard")
            };
            let mut bound = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                if matches!(item, Param::Null) {
                    return Err(BindError::Refused(
                        Diagnostic::error(
                            codes::DB_STATEMENT_REFUSED,
                            format!("parameter ${position} is an array with a `PNull` at index {i}"),
                        )
                        .primary(span, "this statement reaches the database driver")
                        .note("`Datum` has no shape for a `NULL` array element, so a row holding one could not be read back")
                        .note("model an optional element as its own row, or as a sentinel the schema declares"),
                    ));
                }
                if matches!(item, Param::Array(_)) {
                    return Err(BindError::Refused(
                        Diagnostic::error(
                            codes::DB_STATEMENT_REFUSED,
                            format!("parameter ${position} is a nested array"),
                        )
                        .primary(span, "this statement reaches the database driver")
                        .note("W4 maps `List<a>` to a one-dimensional `a[]` and nothing else")
                        .note("postgres's multi-dimensional arrays are rectangular, which `List<List<a>>` is not, so the mapping would be partial in a direction a program cannot see"),
                    ));
                }
                bound.push(bind_one(item, member, position, span)?);
            }
            BoundValue::Array(bound)
        }
        (param, ty) => {
            let mut diagnostic = Diagnostic::error(
                    codes::DB_STATEMENT_REFUSED,
                    format!(
                        "parameter ${position} is {} and the statement wants `{ty}`",
                        param.what()
                    ),
                )
                .primary(span, "this statement reaches the database driver")
                .note(format!(
                    "the mapping is pinned: {}",
                    "Int↔int8/int4/int2, Bool↔bool, String↔text/varchar/bpchar/name/uuid, Bytes↔bytea, Float↔float8/float4, Decimal↔numeric, Json↔json/jsonb, List<a>↔a[]"
                ))
                .note("a column of a type outside it — a timestamp, an interval, an enum — is refused rather than rendered to text, because Ply has no value that would mean the same thing");
            if let Some(advice) = advice(ty) {
                diagnostic = diagnostic.note(advice.to_string());
            }
            return Err(BindError::Refused(diagnostic));
        }
    };
    Ok(Bound {
        ty: ty.clone(),
        value,
    })
}

#[cold]
fn out_of_range(value: i64, what: &str, position: usize) -> DbError {
    DbError::new(
        "22003",
        "",
        format!("parameter ${position} is {value}, which is outside the range of {what}"),
    )
}

fn parse_uuid(text: &str) -> Option<[u8; 16]> {
    let hex: Vec<u8> = text
        .bytes()
        .filter(|b| *b != b'-')
        .map(|b| match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        })
        .collect::<Option<Vec<u8>>>()?;
    if hex.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, pair) in hex.chunks_exact(2).enumerate() {
        out[i] = (pair[0] << 4) | pair[1];
    }
    Some(out)
}

impl<'a> FromSql<'a> for Datum {
    fn from_sql(ty: &Type, raw: &'a [u8]) -> Result<Datum, Box<dyn Error + Sync + Send>> {
        Ok(match *ty {
            Type::BOOL => Datum::Bool(wire::bool_from_sql(raw)?),
            Type::INT2 => Datum::Int(i64::from(wire::int2_from_sql(raw)?)),
            Type::INT4 => Datum::Int(i64::from(wire::int4_from_sql(raw)?)),
            Type::INT8 => Datum::Int(wire::int8_from_sql(raw)?),
            Type::FLOAT4 => Datum::Float(f64::from(wire::float4_from_sql(raw)?)),
            Type::FLOAT8 => Datum::Float(wire::float8_from_sql(raw)?),
            Type::TEXT | Type::VARCHAR | Type::BPCHAR | Type::NAME | Type::UNKNOWN => {
                Datum::Text(wire::text_from_sql(raw)?.to_string())
            }
            Type::UUID => Datum::Text(render_uuid(raw)?),
            Type::BYTEA => Datum::Bytes(wire::bytea_from_sql(raw).to_vec()),
            Type::NUMERIC => Datum::Numeric(numeric_from_sql(raw)?),
            Type::JSON => Datum::Json(Json::parse(raw, "this column").map_err(as_error)?),
            Type::JSONB => {
                let (version, body) = raw.split_first().ok_or("an empty jsonb value")?;
                if *version != 1 {
                    return Err(
                        format!("jsonb version {version} is not one this driver reads").into(),
                    );
                }
                Datum::Json(Json::parse(body, "this column").map_err(as_error)?)
            }
            _ => match ty.kind() {
                // `Vec<T>`'s own decoder refuses more than one dimension, and `Element`'s refuses a
                // `NULL` element — which is the shape `List<a>` has nowhere to put, so it is a
                // decode failure naming the column rather than a hole in the list.
                Kind::Array(_) => Datum::Array(
                    Vec::<Element>::from_sql(ty, raw)?
                        .into_iter()
                        .map(|Element(datum)| datum)
                        .collect(),
                ),
                _ => return Err(format!("`{ty}` is outside the pinned type mapping").into()),
            },
        })
    }

    fn from_sql_null(_: &Type) -> Result<Datum, Box<dyn Error + Sync + Send>> {
        Ok(Datum::Null)
    }

    fn accepts(ty: &Type) -> bool {
        mapped(ty)
    }
}

/// One element of an array column.
struct Element(Datum);

impl<'a> FromSql<'a> for Element {
    fn from_sql(ty: &Type, raw: &'a [u8]) -> Result<Element, Box<dyn Error + Sync + Send>> {
        Ok(Element(Datum::from_sql(ty, raw)?))
    }

    fn from_sql_null(_: &Type) -> Result<Element, Box<dyn Error + Sync + Send>> {
        Err("this column is an array with a NULL element, which `List<a>` has no shape for".into())
    }

    fn accepts(ty: &Type) -> bool {
        <Datum as FromSql>::accepts(ty)
    }
}

fn as_error(message: String) -> Box<dyn Error + Sync + Send> {
    message.into()
}

/// `numeric`, decoded from the wire rather than through `rust_decimal`'s own `FromSql`.
fn numeric_from_sql(raw: &[u8]) -> Result<Decimal, Box<dyn Error + Sync + Send>> {
    fn i16_at(raw: &[u8], at: usize) -> Result<i16, Box<dyn Error + Sync + Send>> {
        let bytes: [u8; 2] = raw
            .get(at..at + 2)
            .ok_or("a numeric shorter than its own header")?
            .try_into()
            .expect("two bytes");
        Ok(i16::from_be_bytes(bytes))
    }

    let count = i16_at(raw, 0)? as usize;
    let weight = i16_at(raw, 2)?;
    let sign = i16_at(raw, 4)? as u16;
    let scale = i16_at(raw, 6)?;

    // `Decimal` has no representation for these and substituting zero is the silent-wrong-answer
    // shape, so they are a decode failure naming the column.
    match sign {
        0x0000 | 0x4000 => {}
        0xC000 => return Err("this column holds `NaN`, which `Decimal` has no value for".into()),
        0xD000 | 0xF000 => {
            return Err("this column holds an infinity, which `Decimal` has no value for".into());
        }
        other => return Err(format!("a numeric with sign 0x{other:04x}").into()),
    }
    if scale < 0 {
        return Err("a numeric with a negative display scale".into());
    }

    let mut digits = Vec::with_capacity(count);
    for i in 0..count {
        digits.push(i16_at(raw, 8 + i * 2)?);
    }

    let mut text = String::new();
    if sign == 0x4000 {
        text.push('-');
    }
    if weight < 0 {
        text.push('0');
    } else {
        for i in 0..=weight {
            let digit = digits.get(i as usize).copied().unwrap_or(0);
            if i == 0 {
                text.push_str(&digit.to_string());
            } else {
                text.push_str(&format!("{digit:04}"));
            }
        }
    }
    if scale > 0 {
        text.push('.');
        let mut fraction = String::new();
        let mut at = i32::from(weight) + 1;
        while fraction.len() < scale as usize {
            let digit = if at >= 0 {
                digits.get(at as usize).copied().unwrap_or(0)
            } else {
                0
            };
            fraction.push_str(&format!("{digit:04}"));
            at += 1;
        }
        fraction.truncate(scale as usize);
        text.push_str(&fraction);
    }

    Decimal::from_str_exact(&text).map_err(|_| {
        format!(
            "this column holds `{text}`, which is outside `Decimal`'s 96-bit mantissa and scale 0..=28"
        )
        .into()
    })
}

fn render_uuid(raw: &[u8]) -> Result<String, Box<dyn Error + Sync + Send>> {
    if raw.len() != 16 {
        return Err("a uuid that is not sixteen bytes".into());
    }
    let hex: String = raw.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    ))
}

/// Whether a postgres type is in the pinned mapping at all.
pub fn mapped(ty: &Type) -> bool {
    match *ty {
        Type::BOOL
        | Type::INT2
        | Type::INT4
        | Type::INT8
        | Type::FLOAT4
        | Type::FLOAT8
        | Type::TEXT
        | Type::VARCHAR
        | Type::BPCHAR
        | Type::NAME
        | Type::UUID
        | Type::BYTEA
        | Type::NUMERIC
        | Type::JSON
        | Type::JSONB => true,
        _ => match ty.kind() {
            // One dimension is a property of the value rather than of the type, so the refusal for
            // a two-dimensional one is at decode.
            Kind::Array(member) => !matches!(member.kind(), Kind::Array(_)) && mapped(member),
            _ => false,
        },
    }
}

/// What a program should write instead, for the types W4 deliberately does not map.
pub fn advice(ty: &Type) -> Option<&'static str> {
    match *ty {
        Type::TIMESTAMP | Type::TIMESTAMPTZ => Some(
            "Ply has no time type; store it as `int8` microseconds since the epoch and pass `clock.now()` as a parameter",
        ),
        Type::DATE => Some("Ply has no time type; store it as `int4` days since the epoch"),
        Type::TIME | Type::TIMETZ | Type::INTERVAL => {
            Some("Ply has no time type; store the component parts the program actually compares")
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests;
