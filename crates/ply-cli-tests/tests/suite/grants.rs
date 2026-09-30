//! `--allow`: which privileged families a run lends the program it drives, by name.

use crate::harness::{ply, project};
use tempfile::TempDir;

/// The outer program: declare `machine`, and drive one. The nested program's module is named
/// `machine`, so the values it hands back carry that module's constructors.
const OUTER: &str = "\
nondet effect machine {
  write configure[m](options: Options) -> Unit
  read load[m](root: String, front: Option<Front>) -> Result<Target, Refusal>
  read reload[m]() -> Result<Target, Refusal>
  read bound[m](entry: String) -> Result<Bound, Refusal>
  write enter[m]() -> Ended
  read call[m](name: String, args: List<Value>) -> Result<Value, Raised>
  read accounting[m]() -> Accounting
  write drop[m]() -> Unit
}

type Front = {
  dump: Bytes,
  files: List<{ path: String, name: String, text: Bytes }>,
  read_ms: Int,
  front_ms: Int,
  file_ms: Int,
  cached: Bool,
}
type Options = Unit
type Ended = Unit
type Target = Unit
type Bound = Unit
type Refusal = Unit
type Counters = { updates: Int, updates_in_place: Int, in_place: Option<Decimal>, cycles: Int }
type Accounting = { steps: Int, micros: Int, counters: Counters }
type Raised = { code: String, message: String }
type Value = | VUnit | VBool(Bool) | VInt(Int) | VStr(String) | VList(List<Value>)

fn main() -> Int / {machine.load[m], machine.bound[m], machine.call[m], machine.accounting[m], machine.drop[m]} = {
  match machine.load[m](\"inner\", None) {
    Ok(_) -> match machine.bound[m](\"inner.main\") {
      Ok(_) -> {
        let doubled = machine.call[m](\"inner.double\", [VInt(21)]);
        let spent = machine.accounting[m]();
        machine.drop[m]();
        match doubled {
          Ok(v) -> match v { VInt(i) -> i + spent.steps, _ -> 0 - 2 },
          Err(_) -> 0 - 1,
        }
      },
      Err(_) -> 0 - 5,
    },
    Err(_) -> 0 - 6,
  }
}
";

const INNER: &str = "\
fn main() -> Int = 0

pub fn double(x: Int) -> Int = x * 2
";

/// A project whose `m.ply` drives a machine over the sibling `inner/` directory it is handed.
fn driving() -> TempDir {
    let dir = project(OUTER);
    std::fs::create_dir(dir.path().join("inner")).unwrap();
    std::fs::write(dir.path().join("inner/inner.ply"), INNER).unwrap();
    dir
}

#[test]
fn a_granted_family_is_bound_and_the_program_drives_a_machine() {
    let dir = driving();
    let out = ply(dir.path())
        .args(["run", "--host", "--allow", "machine", "m.ply"])
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{err}");
    let text = crate::harness::stdout_of(&out);
    let value: i64 = text
        .lines()
        .last()
        .and_then(|l| l.trim().parse().ok())
        .unwrap_or(0);
    assert!(
        value >= 42,
        "the call answered {value}, which is not the doubled value plus the steps it took"
    );
}

#[test]
fn a_family_the_program_does_not_declare_is_refused() {
    let dir = driving();
    let out = ply(dir.path())
        .args(["run", "--host", "--allow", "tester", "m.ply"])
        .output()
        .unwrap();
    assert_ne!(out.status.code(), Some(0), "an undeclared grant ran");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("E0459"), "{err}");
    assert!(err.contains("`tester`"), "{err}");
}

#[test]
fn without_the_grant_the_machine_is_not_bound() {
    let dir = driving();
    let out = ply(dir.path())
        .args(["run", "--host", "m.ply"])
        .output()
        .unwrap();
    assert_ne!(
        out.status.code(),
        Some(0),
        "a program drove a machine with no grant"
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("E0424") || err.contains("machine"), "{err}");
}

/// A program that declares the prover and reaches one of its operations behind a branch never taken:
/// what is under test is whether the grant binds at all. The family is `claims` and its effect is
/// `prover`, so a grant checked against the family's name could never be made.
const PROVING: &str = "\
nondet effect prover {
  write configure[claims](options: Unit, front: Unit) -> Unit
  read collected[claims]() -> Result<Unit, Unit>
  read typed[claims]() -> Result<Unit, Unit>
  read outcomes[claims](keys: List<String>) -> List<Option<String>>
  read discharged[claims](choice: Unit) -> Result<Unit, Unit>
  read replay[claims](index: Int, root: Int, case: Int) -> Result<Unit, Unit>
  read shrink[claims](claim: Int) -> Result<Option<Int>, Unit>
  read offers[claims](i: Int) -> Result<Option<Unit>, Unit>
  read would[claims](i: Int, position: Int) -> Result<Bool, Unit>
  write accept[claims](i: Int, position: Int) -> Result<Unit, Unit>
  read settled[claims]() -> Result<Option<Unit>, Unit>
  read reviewed[claims]() -> Unit
  read accepted[claims]() -> Unit
}

fn main() -> Int / {prover.collected[claims]} =
  if 1 > 2 { match prover.collected[claims]() { Ok(_) -> 1, Err(_) -> 2 } } else { 7 }
";

#[test]
fn a_family_is_granted_to_a_program_declaring_the_effect_it_lends() {
    let dir = project(PROVING);
    let out = ply(dir.path())
        .args(["run", "--host", "--allow", "claims", "m.ply"])
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{err}");
    assert!(
        crate::harness::stdout_of(&out)
            .lines()
            .any(|l| l.trim() == "7"),
        "{err}"
    );
}

#[test]
fn a_refused_grant_names_the_effect_the_program_would_have_to_declare() {
    let dir = project(PROVING);
    let out = ply(dir.path())
        .args(["run", "--host", "--allow", "cache", "m.ply"])
        .output()
        .unwrap();
    assert_ne!(out.status.code(), Some(0), "an undeclared grant ran");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("E0459"), "{err}");
    assert!(
        err.contains("`--allow cache`") && err.contains("no `store` effect"),
        "{err}"
    );
}

#[test]
fn the_grant_requires_a_host() {
    let dir = driving();
    let out = ply(dir.path())
        .args(["run", "--allow", "machine", "m.ply"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("--host"), "{err}");
}
