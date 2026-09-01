//! the secret containment claim at the type level: every route a credential could take out of a program, and the
//! diagnostic that closes it.

use ply_core::{CheckOutput, check_program, print_type};
use ply_span::{Diagnostic, SourceId, Symbol, codes};
use ply_syntax::ast::ModuleName;
use ply_syntax::resolve::resolve;

/// The signatures a generated `json` dictionary references.
const JSON: &str = r#"
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

fn compile(modules: &[(&str, &str)]) -> Result<CheckOutput, Vec<Diagnostic>> {
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

fn ok(source: &str) -> CheckOutput {
    match compile(&[("std.json", JSON), ("m", source)]) {
        Ok(out) => out,
        Err(d) => panic!("expected this to check:\n{source}\ngot {d:#?}"),
    }
}

fn errors(source: &str) -> Vec<Diagnostic> {
    match compile(&[("std.json", JSON), ("m", source)]) {
        Ok(_) => panic!("expected a diagnostic from:\n{source}"),
        Err(d) => d,
    }
}

/// The first diagnostic carrying `code`, and a readable failure when there is none — a route that
/// opened usually opens by producing *no* diagnostic at all.
fn code(source: &str, code: &str) -> Diagnostic {
    let diags = errors(source);
    match diags.iter().find(|d| d.code == code) {
        Some(d) => d.clone(),
        None => panic!("expected {code} from:\n{source}\ngot {diags:#?}"),
    }
}

fn says(d: &Diagnostic, text: &str) -> bool {
    d.message.contains(text)
        || d.notes.iter().any(|n| n.contains(text))
        || d.labels.iter().any(|l| l.message.contains(text))
}

fn sig(out: &CheckOutput, name: &str) -> String {
    print_type(&out.defs[&Symbol::new(format!("m.{name}"))].scheme.ty)
}

// --- what is meant to work --------------------------------------------------

#[test]
fn the_three_builtins_have_the_types_the_contract_states() {
    let out = ok("fn mint(s: String) -> Secret<String> = secret_of_string(s)
fn check(s: Secret<String>, guess: String) -> Bool = secret_verify(s, guess)
fn blank(s: Secret<String>) -> Bool = secret_is_empty(s)");
    assert_eq!(sig(&out, "mint"), "(String) -> Secret<String>");
    assert_eq!(sig(&out, "check"), "(Secret<String>, String) -> Bool");
    assert_eq!(sig(&out, "blank"), "(Secret<String>) -> Bool");
}

/// The operations a program is *supposed* to be able to do with a credential: hold it in a record,
/// pass it, compare two of them, and answer a `Bool`.
#[test]
fn a_secret_survives_the_operations_that_are_meant_to_work() {
    ok(r#"type Login = {user: String, password: Secret<String>}

fn login(user: String, password: String) -> Login =
  {user: user, password: secret_of_string(password)}

fn authenticate(l: Login, offered: String) -> Bool =
  secret_verify(l.password, offered)

fn configured(l: Login) -> Bool = !secret_is_empty(l.password)

fn same(a: Login, b: Login) -> Bool = a.password == b.password

fn in_a_list(ls: List<Login>) -> Int = len(ls)

fn in_an_option(l: Login) -> Option<Secret<String>> = Some(l.password)

fn keyed(m: Map<String, Secret<String>>, k: String) -> Option<Secret<String>> =
  map_get(m, k)"#);
}

/// A `Secret` is a value in every ordinary sense: it goes in an `Option`, a `List`, a record and a
/// map *value*, and it comes back out.
#[test]
fn derive_eq_accepts_a_secret_field() {
    ok("type Login = {user: String, password: Secret<String>}
derive eq for Login");
}

/// Presence is deliberately observable, so a start-up can tell a missing credential from
/// a wrong one.
#[test]
fn presence_is_observable_and_answers_a_bool() {
    let out = ok("fn present(s: Secret<String>) -> Bool = !secret_is_empty(s)");
    assert_eq!(sig(&out, "present"), "(Secret<String>) -> Bool");
}

// --- the mechanism ----------------------------------------------------------

#[test]
fn secret_is_a_builtin_type_no_module_may_claim() {
    let d = code("type Secret<a> = {value: a}", codes::DUPLICATE_DEFINITION);
    assert!(says(&d, "builtin type"), "{d:#?}");
}

#[test]
fn there_is_no_pattern_that_binds_the_payload() {
    let d = code(
        "fn leak(s: Secret<String>) -> String = match s { Secret(plain) -> plain }",
        codes::UNKNOWN_NAME,
    );
    assert!(d.message.contains("unknown constructor `Secret`"), "{d:#?}");
    // The general "constructors come from a `type` declaration" note would send the reader looking
    // for a declaration that cannot exist, so the absence is named as the mechanism it is.
    assert!(says(&d, "declares none"), "{d:#?}");
    assert!(says(&d, "secret_verify"), "{d:#?}");
}

/// An alias is transparent, so the payload is not reachable through one either.
#[test]
fn an_alias_to_a_secret_is_still_a_secret() {
    let d = code(
        "type Password = Secret<String>
fn leak(p: Password) -> String = p",
        codes::TYPE_MISMATCH,
    );
    assert!(says(&d, "Secret"), "{d:#?}");
}

#[test]
fn a_secret_takes_exactly_one_type_argument() {
    let diags = errors("fn f(s: Secret) -> Bool = secret_is_empty(s)");
    assert!(
        diags.iter().any(|d| d.code == codes::ARITY_MISMATCH),
        "{diags:#?}"
    );
}

// --- route by route ---------------------------------------------------------

/// `++` is `String`-only, so the concatenation route is a type error rather than a review item.
#[test]
fn concatenation_with_a_string_is_refused() {
    code(
        r#"fn leak(s: Secret<String>) -> String = "token=" ++ s"#,
        codes::TYPE_MISMATCH,
    );
}

#[test]
fn a_secret_is_not_a_string_anywhere_a_string_is_wanted() {
    for source in [
        "fn leak(s: Secret<String>) -> String = string_upper(s)",
        "fn leak(s: Secret<String>) -> Int = string_len(s)",
        "fn leak(s: Secret<String>) -> Bytes = bytes_of_string(s)",
        "fn leak(s: Secret<String>) -> List<String> = string_split(s, \"\")",
        "fn leak(s: Secret<String>) -> Bool = string_contains(s, \"a\")",
    ] {
        code(source, codes::TYPE_MISMATCH);
    }
}

/// The panic payload.
#[test]
fn a_panic_payload_cannot_carry_one() {
    code(
        r#"fn boom(s: Secret<String>) -> Int = panic("token " ++ s)"#,
        codes::TYPE_MISMATCH,
    );
    code(
        "fn boom(s: Secret<String>) -> Int = panic(s)",
        codes::TYPE_MISMATCH,
    );
}

/// A derived JSON document is the route the milestone's headline is about, and the refusal names
/// the field rather than the type.
#[test]
fn derive_json_refuses_a_secret_field_and_names_it() {
    let d = code(
        "import std.json
type Login = {user: String, password: Secret<String>}
derive json for Login",
        codes::NOT_DERIVABLE,
    );
    assert!(says(&d, "password") || says(&d, "Secret"), "{d:#?}");
    assert!(says(&d, "credential"), "{d:#?}");
}

#[test]
fn derive_json_refuses_a_secret_inside_a_variant_a_list_and_an_option() {
    for source in [
        "import std.json
type Held = Nothing | Held(Secret<String>)
derive json for Held",
        "import std.json
type Bag = {keys: List<Secret<String>>}
derive json for Bag",
        "import std.json
type Bag = {key: Option<Secret<String>>}
derive json for Bag",
        "import std.json
type Bag = {keys: Map<String, Secret<String>>}
derive json for Bag",
    ] {
        code(source, codes::NOT_DERIVABLE);
    }
}

/// The alias case is what `ply_core`'s walk over the *solved* type exists for: the syntactic walk
/// in `ply_derive` sees only `Password`.
#[test]
fn derive_json_refuses_a_secret_behind_an_alias() {
    code(
        "import std.json
type Password = Secret<String>
type Login = {user: String, password: Password}
derive json for Login",
        codes::NOT_DERIVABLE,
    );
}

/// Equality leaks one bit per call; an ordering leaks a bit of *position* per call and recovers the
/// whole value in calls proportional to its length.
#[test]
fn derive_ord_refuses_what_derive_eq_accepts() {
    let d = code(
        "type Login = {user: String, password: Secret<String>}
derive ord for Login",
        codes::NOT_DERIVABLE,
    );
    assert!(says(&d, "position"), "{d:#?}");
    assert!(says(&d, "secret_verify"), "{d:#?}");
}

/// A secret as a map key would be an ordering oracle with a data structure attached, and a `Map`
/// key needs `derivable(ord, k)`.
#[test]
fn a_secret_is_not_a_map_key() {
    let d = code(
        "fn index(m: Map<Secret<String>, Int>, k: Secret<String>) -> Option<Int> = map_get(m, k)",
        codes::NOT_DERIVABLE,
    );
    assert!(says(&d, "Secret"), "{d:#?}");
}

#[test]
fn a_secret_is_not_a_map_key_behind_an_alias_or_inside_a_record() {
    for source in [
        "type Password = Secret<String>
fn index(m: Map<Password, Int>) -> Int = map_len(m)",
        "type Pair = {user: String, password: Secret<String>}
fn index(m: Map<Pair, Int>) -> Int = map_len(m)",
    ] {
        code(source, codes::NOT_DERIVABLE);
    }
}

/// `compare` and `compare_values` both carry `where derivable(ord, ·)`, so the ordering oracle is
/// refused at the call and not only at a derivation.
#[test]
fn compare_refuses_a_secret_at_the_call_site() {
    code(
        "fn order(a: Secret<String>, b: Secret<String>) -> Ordering = compare(a, b)",
        codes::NOT_DERIVABLE,
    );
}

/// A generator that minted credentials and a shrinker that printed counterexamples is a leak by
/// construction, and the code for both exists.
#[test]
fn a_law_cannot_quantify_over_a_secret() {
    let d = code(
        r#"law "nothing" forall (s: Secret<String>) { secret_is_empty(s) == secret_is_empty(s) }"#,
        codes::UNQUANTIFIABLE_TYPE,
    );
    assert!(says(&d, "credential"), "{d:#?}");
}

#[test]
fn a_law_cannot_quantify_over_a_record_holding_a_secret() {
    code(
        r#"type Login = {user: String, password: Secret<String>}
law "nothing" forall (l: Login) { secret_is_empty(l.password) == secret_is_empty(l.password) }"#,
        codes::UNQUANTIFIABLE_TYPE,
    );
}

/// `secret_of_string` is the only introduction, and it takes a `String`.
#[test]
fn the_only_introduction_takes_a_string() {
    code(
        "fn mint(b: Bytes) -> Secret<Bytes> = secret_of_string(b)",
        codes::TYPE_MISMATCH,
    );
    code(
        "fn nest(s: Secret<String>) -> Secret<Secret<String>> = secret_of_string(s)",
        codes::TYPE_MISMATCH,
    );
}

/// There is no `secret_expose`, no `secret_len`, no `secret_map` and no `secret_slice`.
#[test]
fn the_eliminations_that_do_not_exist_do_not_resolve() {
    for name in [
        "secret_expose",
        "secret_len",
        "secret_map",
        "secret_slice",
        "secret_concat",
        "secret_to_string",
    ] {
        let source = format!("fn leak(s: Secret<String>) -> String = {name}(s)");
        let diags = errors(&source);
        assert!(
            diags
                .iter()
                .any(|d| d.code == codes::UNKNOWN_NAME || d.code == codes::TYPE_MISMATCH),
            "`{name}` resolved: {diags:#?}"
        );
    }
}

/// The `trace.event[c](.., fields)` route, written without depending on `std.trace`'s own text: a
/// sum type with no `Secret` variant cannot be handed one, whatever its variants are called.
#[test]
fn a_sum_type_with_no_secret_variant_cannot_hold_one() {
    code(
        r#"type Field = FInt(Int) | FBool(Bool) | FText(String)
fn record(s: Secret<String>) -> Field = FText(s)"#,
        codes::TYPE_MISMATCH,
    );
}

/// The SQL parameter route, same shape: `Param` has no `PSecret`, so a credential cannot be bound
/// into a statement.
#[test]
fn a_parameter_type_with_no_secret_case_cannot_hold_one() {
    code(
        r#"type Param = PText(String) | PInt(Int)
fn bind(s: Secret<String>) -> Param = PText(s)"#,
        codes::TYPE_MISMATCH,
    );
}

/// The one way a program observes a credential: a handler clause is handed the whole `Secret` and
/// answers something derived from it.
#[test]
fn a_redacting_handler_clause_is_how_a_secret_is_observed() {
    ok(r#"effect vault {
  read fetch[k](name: String) -> Secret<String>
}

fn present(name: String) -> Bool / {vault.read[keys]} =
  !secret_is_empty(vault.fetch[keys](name))

test "the clause sees the credential and answers a Bool" {
  let seen = handle present("api") with {
    vault.fetch[keys](n) -> secret_of_string("hunter2"),
    return x -> x
  };
  assert(seen)
}"#);
}

/// `secret_verify` answers a `Bool` and `secret_is_empty` answers a `Bool`.
#[test]
fn no_builtin_over_a_secret_returns_its_payload() {
    code(
        "fn leak(s: Secret<String>) -> String = secret_verify(s, \"x\")",
        codes::TYPE_MISMATCH,
    );
    code(
        "fn leak(s: Secret<String>) -> String = secret_is_empty(s)",
        codes::TYPE_MISMATCH,
    );
}
