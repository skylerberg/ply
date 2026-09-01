//! The seeded handlers against the signatures they claim.

use ply_core::{CheckOutput, EffectInfo, Type, check_program};
use ply_eval::{Answer, Handlers, SEEDED_OPS, SimTy, TaskId, Value};
use ply_span::{SourceId, Span, Symbol};
use ply_syntax::ast::{ExprKind, Item, ModuleName, Program};
use ply_syntax::parse_module;
use ply_syntax::resolve::resolve;
use std::path::{Path, PathBuf};

/// ADR 0006 §1.1's declaration of the two effects the seeded handlers answer, plus a hand-written
/// handler for it.
const SOURCE: &str = r#"
nondet effect clock {
  read now() -> Int
  write sleep(Int) -> Unit
}

nondet effect random {
  write next() -> Int
  write below(Int) -> Int
}

fn work() -> Int / {clock.read, clock.write, random.write} = {
  let started = clock.now();
  clock.sleep(40);
  started + random.next() + random.below(6)
}

fn stub() -> Int = handle work() with {
  clock.now() -> 0,
  clock.sleep(nanos) -> (),
  random.next() -> 7,
  random.below(bound) -> 0,
}
"#;

fn program() -> (Program, CheckOutput) {
    let module = parse_module(SourceId(0), ModuleName::from_dotted("sig"), SOURCE)
        .expect("the declaration parses");
    let mut program = Program::single(module);
    let resolved = resolve(&mut program).expect("one module with no imports resolves");
    let check = check_program(&program, &resolved).expect("the declaration typechecks");
    (program, check)
}

fn effect<'a>(check: &'a CheckOutput, simple: &str) -> &'a EffectInfo {
    check
        .effects
        .get(&Symbol::new(format!("sig.{simple}")))
        .unwrap_or_else(|| panic!("`{simple}` is declared"))
}

fn span() -> Span {
    Span::new(SourceId(0), 0, 1)
}

/// What the run would report a value's type as.
fn type_of(value: &Value) -> Option<Type> {
    match value {
        Value::Int(_) => Some(Type::int()),
        Value::Unit => Some(Type::unit()),
        _ => None,
    }
}

#[test]
fn the_clause_set_covers_each_declared_effect_exactly() {
    let (_, check) = program();
    for name in ["clock", "random"] {
        let info = effect(&check, name);
        assert!(
            info.nondet,
            "`{name}` is nondeterministic until it is handled"
        );
        let declared: Vec<&str> = info.ops.keys().map(|op| op.as_str()).collect();
        let seeded: Vec<&str> = SEEDED_OPS
            .iter()
            .filter(|sig| sig.effect == name)
            .map(|sig| sig.op)
            .collect();
        assert_eq!(
            declared, seeded,
            "the seeded clause set and the declaration of `{name}` name different operations"
        );
    }
}

#[test]
fn every_seeded_operation_has_the_declared_mode_and_types() {
    let (_, check) = program();
    for sig in SEEDED_OPS {
        let info = effect(&check, sig.effect);
        let op = info
            .ops
            .get(&Symbol::new(sig.op))
            .unwrap_or_else(|| panic!("`{sig}` is declared"));
        assert_eq!(op.mode, sig.mode, "`{sig}` disagrees about its mode");
        let params: Vec<Type> = sig.params.iter().map(|p| p.ply()).collect();
        assert_eq!(op.params, params, "`{sig}` disagrees about its parameters");
        assert_eq!(op.ret, sig.ret.ply(), "`{sig}` disagrees about its result");
    }
}

/// The signature is a promise about what a perform site receives, so it is only kept if the value
/// the handler actually produces has the declared type.
#[test]
fn what_the_handlers_answer_has_the_declared_type() {
    let (_, check) = program();
    let mut handlers = Handlers::new(11);
    for sig in SEEDED_OPS {
        let op = effect(&check, sig.effect)
            .ops
            .get(&Symbol::new(sig.op))
            .unwrap_or_else(|| panic!("`{sig}` is declared"))
            .clone();
        let args: Vec<Value> = op
            .params
            .iter()
            .map(|param| {
                assert_eq!(*param, Type::int(), "`{sig}` takes something else now");
                // Positive: `random.below` has no value to answer below zero, and a sleep of zero
                // is a yield rather than a deadline.
                Value::Int(3)
            })
            .collect();
        match handlers.dispatch(sig, TaskId(0), &args, span()) {
            Ok(Answer::Value(v)) => assert_eq!(
                type_of(&v).as_ref(),
                Some(&op.ret),
                "`{sig}` answered {}",
                v.render()
            ),
            // A woken sleeper is resumed with `clock.sleep`'s declared return.
            Ok(Answer::Sleeping { .. }) => {
                assert_eq!(op.ret, Type::unit());
                assert_eq!(sig.ret, SimTy::Unit);
            }
            Err(d) => panic!("`{sig}` refused its own declared arguments: {}", d.message),
        }
    }
}

/// Three handlers for one signature — this stub, the seeded one, and the threaded one M9 will write
/// — and no way for them to drift, because the declaration types all three.
#[test]
fn a_hand_written_handler_and_the_seeded_one_answer_the_same_operations() {
    let (program, check) = program();
    let stub = check
        .defs
        .get(&Symbol::new("sig.stub"))
        .expect("`stub` is defined");
    assert!(
        stub.footprint.is_empty(),
        "a handler over the declared clause set discharges the effects, leaving {}",
        stub.footprint.0.len()
    );

    let module = &program.modules[0];
    let body = module
        .items
        .iter()
        .find_map(|item| match item {
            Item::Fn(f) if f.name.name.as_str() == "stub" => Some(&f.body),
            _ => None,
        })
        .expect("`stub` is a function");
    let ExprKind::Handle { clauses, .. } = &body.kind else {
        panic!("`stub` is a handler");
    };

    let written: Vec<(String, String, usize)> = clauses
        .iter()
        .map(|c| {
            (
                c.effect.symbol().to_string(),
                c.op.name.to_string(),
                c.params.len(),
            )
        })
        .collect();
    let seeded: Vec<(String, String, usize)> = SEEDED_OPS
        .iter()
        .map(|sig| (sig.effect.to_string(), sig.op.to_string(), sig.params.len()))
        .collect();
    assert_eq!(
        written, seeded,
        "the hand-written handler and the seeded one are not clause-for-clause the same handler"
    );
}

fn sources(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("the crate's own sources are readable") {
        let path = entry.expect("a readable directory entry").path();
        if path.is_dir() {
            sources(&path, found);
        } else if path.extension().is_some_and(|e| e == "rs") {
            found.push(path);
        }
    }
}

/// `clock.now()` is the only way a Ply program can ask what time it is, and a `simulate` region
/// handles it.
#[test]
fn the_evaluator_reads_no_host_clock_and_no_host_entropy() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    sources(&src, &mut files);
    files.sort();
    assert!(files.len() > 5, "found no sources to check under {src:?}");

    for path in files {
        if path
            .file_name()
            .is_some_and(|n| n == "tests.rs" || n == "build.rs")
        {
            continue;
        }
        let whole = std::fs::read_to_string(&path).expect("a readable source");
        let text = whole.split("#[cfg(test)]").next().unwrap_or(&whole);
        for banned in [
            "SystemTime",
            "Instant",
            "std::time",
            "rand::",
            "thread_rng",
            "getrandom",
            "RandomState",
            "DefaultHasher",
        ] {
            assert!(
                !text.contains(banned),
                "`{banned}` appears in {}: a simulated run must be a function of \
                 its definitions and its seed, so the evaluator may not reach the \
                 host's clock or entropy. Measure wall clock in `ply-test`, where \
                 no program can observe it.",
                path.display()
            );
        }
    }
}

/// The same rule one level down: a generator crate would put the host's entropy — and its own
/// version — inside a seed's meaning.
#[test]
fn the_crate_depends_on_no_generator_and_no_entropy_source() {
    let manifest =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
            .expect("the crate has a manifest");
    for line in manifest.lines() {
        let Some((key, _)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        assert!(
            !["rand", "fastrand", "nanorand", "getrandom", "oorandom"].contains(&key),
            "`{key}` is a dependency of ply-eval; a seeded run may not draw from one"
        );
    }
}
