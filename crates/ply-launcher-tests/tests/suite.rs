//! The environment a launched program reads, end to end: a program performs `env.var`,
//! `env.terminal` and `env.binary_version` and the launcher answers.

use ply_eval::host::HostRegistry;
use ply_eval::{BackendKind, BackendSpec, Machine, Provider};
use ply_span::{SourceId, Span};
use ply_ty::Front;
use std::collections::HashMap;
use std::sync::Arc;

/// A program that asks the environment everything it knows to ask.
const ASKER: &str = r#"
nondet effect env {
  read var[e](name: String) -> Option<String>
  read terminal[e](stream: String) -> Bool
  read binary_version[e]() -> String
}

fn main() -> String / {env.var[e], env.terminal[e], env.binary_version[e]} = {
  let found = env.var[e]("PLY_LAUNCHER_TEST_MARK");
  let missing = env.var[e]("PLY_LAUNCHER_TEST_ABSENT");
  let term = env.terminal[e]("stdout");
  let version = env.binary_version[e]();
  let mark = match found { Some(v) -> v, None -> "unset" };
  let miss = match missing { Some(_) -> "present", None -> "absent" };
  mark ++ "|" ++ miss ++ "|" ++ (if term { "terminal" } else { "piped" }) ++ "|" ++ version
}
"#;

fn front_of(source: &str) -> Front {
    let named = vec![("m".to_string(), source.to_string())];
    let ids = vec![SourceId(0)];
    ply_codegen::c::producer::ensure_default();
    ply_codegen::c::producer::checked_front(&named, &ids).expect("the program checks")
}

fn ask() -> String {
    let front = front_of(ASKER);
    let texts: HashMap<String, String> =
        [("m".to_string(), ASKER.to_string())].into_iter().collect();
    let unit = ply_codegen::Unit::over_front(&front, texts).expect("this host has a C toolchain");
    let mut machine = Machine::new(&front);
    machine.set_compiled(unit.attach(&BackendSpec {
        kind: BackendKind::C,
    }));
    let mut registry = HostRegistry::new();
    for (op, handler) in ply_launcher::env::registrations("9.9.9-test") {
        registry.register(op, handler);
    }
    let binding = registry.bind(&front.check).expect("the env ops bind");
    machine.set_host_binding(Arc::new(binding));
    machine
        .call("m.main", Vec::new(), Span::DUMMY)
        .expect("the entry ran")
        .to_string()
}

#[test]
fn a_program_reads_its_environment() {
    // Safety: the test is alone in its process (nextest), so the variable is its own.
    unsafe { std::env::set_var("PLY_LAUNCHER_TEST_MARK", "here") };
    let answer = ask();
    assert_eq!(answer, "\"here|absent|piped|9.9.9-test\"");
    unsafe { std::env::remove_var("PLY_LAUNCHER_TEST_MARK") };
}
