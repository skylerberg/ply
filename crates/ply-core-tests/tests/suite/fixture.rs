//! Parse, resolve and check a source, and the stubbed `json` module a derivation test checks
//! against: what the modules in this binary need before they can assert anything.
//!
//! Two shapes, because `derive` expansion is not free and half the fixtures here have nothing to
//! expand: [`compile`] skips it, [`expanded`] runs it as the driver does.

use ply_core::{CheckOutput, check_program};
use ply_span::{Diagnostic, SourceId};
use ply_syntax::ast::ModuleName;
use ply_syntax::resolve::resolve;

/// The signatures a generated `json` dictionary references, and nothing else.
pub const JSON: &str = r#"
pub type Json = Null | Bool(Bool) | Number(Decimal) | Str(String)
              | Array(List<Json>) | Object(Map<String, Json>)
pub type DecodeError = {path: String, message: String}
pub type JsonCodec<a> = {encode: (a) -> Json, decode: (Json) -> Result<a, DecodeError>}

pub fn int_json() -> JsonCodec<Int> = panic("stub")
pub fn bool_json() -> JsonCodec<Bool> = panic("stub")
pub fn string_json() -> JsonCodec<String> = panic("stub")
pub fn bytes_json() -> JsonCodec<Bytes> = panic("stub")
pub fn float_json() -> JsonCodec<Float> = panic("stub")
pub fn decimal_json() -> JsonCodec<Decimal> = panic("stub")
pub fn unit_json() -> JsonCodec<Unit> = panic("stub")

pub fn list_json<a>(a: JsonCodec<a>) -> JsonCodec<List<a>>
  where derivable(json, a) = panic("stub")
pub fn option_json<a>(a: JsonCodec<a>) -> JsonCodec<Option<a>>
  where derivable(json, a) = panic("stub")
pub fn result_json<a, e>(a: JsonCodec<a>, e: JsonCodec<e>) -> JsonCodec<Result<a, e>>
  where derivable(json, a), derivable(json, e) = panic("stub")
pub fn map_json<k, v>(k: JsonCodec<k>, v: JsonCodec<v>) -> JsonCodec<Map<k, v>>
  where derivable(ord, k), derivable(json, k), derivable(json, v) = panic("stub")

pub fn string_map_json<v>(value: JsonCodec<v>) -> JsonCodec<Map<String, v>> = panic("stub")

pub type Member = {key: String, value: Json}
pub type Tagged = {tag: String, values: List<Json>}

pub fn object(fields: List<Member>) -> Json = panic("stub")
pub fn field<a>(j: Json, name: String, codec: JsonCodec<a>) -> Result<a, DecodeError>
  where derivable(json, a) = panic("stub")
pub fn variant(tag: String, values: List<Json>) -> Json = panic("stub")
pub fn variant_of(j: Json) -> Result<Tagged, DecodeError> = panic("stub")
pub fn variant_value(v: Tagged, index: Int) -> Result<Json, DecodeError> = panic("stub")
pub fn unknown_variant<a>(tag: String, expected: List<String>) -> Result<a, DecodeError> =
  panic("stub")
pub fn decode_and_then<a, b>(r: Result<a, DecodeError>, f: (a) -> Result<b, DecodeError>)
  -> Result<b, DecodeError> = panic("stub")
"#;

/// One module named `m`.
pub fn compile(source: &str) -> Result<CheckOutput, Vec<Diagnostic>> {
    let inputs = vec![(SourceId(0), ModuleName::from_dotted("m"), source)];
    let mut program = ply_syntax::parse_program(inputs)?;
    let resolved = resolve(&mut program)?;
    check_program(&program, &resolved)
}

/// One module named `m`, with `derive` expanded first, as the driver does.
pub fn expanded(source: &str) -> Result<CheckOutput, Vec<Diagnostic>> {
    expanded_modules(&[("m", source)])
}

/// Several modules, each one's `SourceId` its position, with `derive` expanded first.
pub fn expanded_modules(modules: &[(&str, &str)]) -> Result<CheckOutput, Vec<Diagnostic>> {
    let inputs: Vec<_> = modules
        .iter()
        .enumerate()
        .map(|(i, (name, src))| (SourceId(i as u32), ModuleName::from_dotted(name), *src))
        .collect();
    let mut program = ply_syntax::parse_program(inputs)?;
    let diags = ply_derive::expand_program(&mut program);
    if !diags.is_empty() {
        return Err(diags);
    }
    let resolved = resolve(&mut program)?;
    check_program(&program, &resolved)
}
