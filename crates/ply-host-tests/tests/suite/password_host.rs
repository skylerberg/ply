//! `std.password`'s operations, performed by a compiled program against the host's own handlers:
//! a `Secret` crosses the boundary, the hash comes back from the pool, and a cost past the bounds
//! is refused where it is performed.

use ply_eval::{Machine, Span, Symbol, Value};
use std::sync::Arc;

const PROGRAM: &str = r#"
import std.bytes (hex_of)
import std.password
import std.password (kdf, Invalid, Valid, ValidNeedsRehash)
import std.random (entropy)

fn filled(byte: Int, n: Int) -> Bytes / {abort.raise} =
  bytes_concat_all(map(range(0, n), |_: Int| byte_of_int(byte)))

// RFC 9106 section 5.3: Argon2id under a secret and associated data.
pub fn rfc_9106() -> String / {kdf.argon2, abort.raise} =
  hex_of(
    kdf.argon2(
      "argon2id",
      secret_of_bytes(filled(1, 32)),
      filled(2, 16),
      secret_of_bytes(filled(3, 8)),
      filled(4, 12),
      32,
      3,
      4,
      32,
    ),
  )

// A hash made under a salt the host drew verifies, and no other password does.
pub fn round_trip() -> List<Bool> / {entropy.next, kdf.argon2, kdf.scrypt, kdf.bcrypt, kdf.pbkdf2, abort.raise} =
  map([try { password::argon2id(64, 1, 1) }, try { password::scrypt(4, 1, 1) }], |made: Result<password::Params, password::Refusal>|
    match made {
      Err(_) -> false,
      Ok(params) -> {
        let secret = secret_of_string("correct horse");
        let stored = password::hash(secret, params);
        password::verify(stored, secret, params) == Valid
          && password::verify(stored, secret_of_string("correct horsf"), params) == Invalid
          && password::hash(secret, params) != stored
      },
    })

// The reference implementation's own test string, which is not what the preset would write.
pub fn reference() -> Bool / {kdf.argon2, kdf.scrypt, kdf.bcrypt, kdf.pbkdf2, abort.raise} =
  password::verify(
    "$argon2id$v=19$m=65536,t=2,p=1$c29tZXNhbHQ$CTFhFdXPJO1aFaMaO6Mm5c8y7cJHAph8ArZWb2GRPPc",
    secret_of_string("password"),
    password::owasp_argon2id(),
  ) == ValidNeedsRehash

// Four gigabytes, asked of the operation itself.
pub fn four_gigabytes() -> Bytes / {kdf.argon2} =
  kdf.argon2(
    "argon2id",
    secret_of_bytes(b"correct horse"),
    b"saltsalt",
    secret_of_bytes(b""),
    b"",
    4194304,
    1,
    1,
    32,
  )

pub fn cost_thirty_one() -> Bytes / {kdf.bcrypt, abort.raise} =
  kdf.bcrypt(secret_of_bytes(b"correct horse"), filled(0, 16), 31)

fn checked(supplied: String) -> String / {kdf.argon2, kdf.scrypt, kdf.bcrypt, kdf.pbkdf2, abort.raise} =
  match password::verify(
    "$argon2id$v=19$m=19456,t=2,p=1$AAECAwQFBgcICQoLDA0ODw$gYJZtjEAJqjg26xdLmknq8/bB7MiWPrE9hsYuA+SkIU",
    secret_of_string(supplied),
    password::owasp_argon2id(),
  ) {
    Valid -> "valid",
    ValidNeedsRehash -> "rehash",
    Invalid -> "invalid",
  }

// Four tasks hashing at once: each parks on its own token, and each is answered its own hash.
pub fn together() -> List<String> / {task.spawn, task.join, kdf.argon2, kdf.scrypt, kdf.bcrypt, kdf.pbkdf2, abort.raise} = {
  let a = task.spawn(|| checked("correct horse battery staple"));
  let b = task.spawn(|| checked("correct horse battery stapled"));
  let c = task.spawn(|| checked("correct horse battery staple"));
  let d = task.spawn(|| checked(""));
  [task.join(a), task.join(b), task.join(c), task.join(d)]
}
"#;

fn call(entry: &str) -> Result<Value, ply_eval::Diagnostic> {
    let host = Arc::new(ply_host::Host::new());
    let (front, unit) = crate::support::answered::compiled("m", PROGRAM);
    let binding = host
        .registry()
        .bind(&front.check)
        .expect("the declaration and the registration agree");

    let mut machine = Machine::new(&front, ply_eval::Provider::attach(unit))
        .expect("the unit was compiled from this program");
    machine.set_host_binding(Arc::new(binding));
    machine.set_host_runtime({
        let host = Arc::clone(&host);
        Arc::new(move || host.runtime())
    });
    let declared = front
        .check
        .defs
        .get(&Symbol::new(entry))
        .expect("the entry is a definition of the program");
    machine.set_declared_footprint(declared.footprint.clone());
    machine.call(entry, Vec::new(), Span::DUMMY).into_parts().0
}

fn answered(entry: &str) -> Value {
    call(entry).unwrap_or_else(|e| panic!("`{entry}` answers: {e}"))
}

#[test]
fn a_program_is_answered_rfc_9106s_argon2id_tag() {
    assert_eq!(
        answered("m.rfc_9106"),
        Value::str("0d640df58d78766c08c037a34a8b53c9d01ef0452d75b65eb52520e96b01e659")
    );
}

#[test]
fn a_hash_made_through_the_host_verifies_through_it() {
    assert_eq!(
        answered("m.round_trip"),
        Value::list(vec![Value::Bool(true), Value::Bool(true)])
    );
}

#[test]
fn a_hash_another_implementation_made_verifies() {
    assert_eq!(answered("m.reference"), Value::Bool(true));
}

/// A hash parks the task that asked for it and no other, so the runtime watches, collects and
/// parks on this facility's pool as it does on the others'.
#[test]
fn tasks_hashing_at_once_are_each_answered_their_own_hash() {
    let answers = ["valid", "invalid", "valid", "invalid"];
    assert_eq!(
        answered("m.together"),
        Value::list(answers.into_iter().map(Value::str).collect())
    );
}

/// The refusal is the perform's, made before a thread is started, and holds nothing of the
/// password it was handed.
#[test]
fn a_cost_past_the_bounds_is_refused_where_it_is_performed() {
    for (entry, names) in [
        ("m.four_gigabytes", "4194304 KiB of memory"),
        ("m.cost_thirty_one", "a cost of 31"),
    ] {
        let why = format!("{}", call(entry).expect_err("past the bounds"));
        assert!(why.contains(names), "`{entry}` does not say why: {why}");
        assert!(
            !why.contains("correct") && !why.contains("horse"),
            "`{entry}` holds the password: {why}"
        );
    }
}
