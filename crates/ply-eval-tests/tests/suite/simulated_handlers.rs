use crate::fixture::port_check;
use ply_eval::decode::AnswerValue;
use ply_eval::{
    Answer, CheckOutput, EffectInfo, Handlers, SEEDED_OPS, SimType, SourceId, Span, Symbol, TaskId,
    Value,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The prelude's two effects the seeded handlers answer, plus a hand-written handler.
const SOURCE: &str = r#"
nondet effect clock {
  read now() -> Instant
  write sleep(Duration) -> Unit
}

nondet effect random {
  write next() -> Int
  write below(Int) -> Int
}

fn work() -> Int / {clock.read, clock.write, random.write} = {
  let started = match clock.now() { Instant(n) -> n };
  clock.sleep(Duration(40));
  started + random.next() + random.below(6)
}

fn stub() -> Int = handle work() with {
  clock.now() -> Instant(0),
  clock.sleep(nanos) -> (),
  random.next() -> 7,
  random.below(bound) -> 0,
}
"#;

fn checked() -> CheckOutput {
    port_check(&[("sig", SOURCE)])
}

fn effect<'a>(check: &'a CheckOutput, simple: &str) -> &'a EffectInfo {
    check
        .effects
        .get(&Symbol::new(format!("sig.{simple}")))
        .unwrap_or_else(|| panic!("`{simple}` is declared"))
}

/// A declared operation's type as the compiler prints one: a constructor and its arguments.
fn printed(t: AnswerValue<'_>) -> String {
    let ty = t.ctor().expect("a type is a constructor of `types.Type`");
    assert_eq!(
        ty.name(),
        "TyCon",
        "a seeded operation's types are named types"
    );
    let con = ty.arg(0).expect("`TyCon` holds its name and arguments");
    let name = con
        .field("name")
        .and_then(|n| n.utf8())
        .expect("a named type has a name")
        .to_string();
    let args = con
        .field("args")
        .and_then(|a| a.items(|t| Ok(printed(t))))
        .expect("a named type lists its arguments");
    if args.is_empty() {
        name
    } else {
        format!("{name}<{}>", args.join(", "))
    }
}

/// Each operation `sig` declares, with its parameter and answer types as the compiler answered
/// them, by its program-wide `effect.op`: what a seeded handler's signature has to agree with.
fn declared() -> HashMap<String, (Vec<String>, String)> {
    let files = [("sig.ply".to_string(), SOURCE.to_string())];
    let bytes = ply_machine::builds::answered(&files).expect("the builder answers");
    let front = ply_machine::runnable::front_value(&bytes).expect("the answer reads");
    let mut out = HashMap::new();
    let effects = AnswerValue::new("the answer", &front)
        .field("dump")
        .and_then(|d| d.field("effects"))
        .and_then(|e| e.list())
        .expect("the answer declares effects");
    for effect in effects {
        let effect_name = effect
            .field("name")
            .and_then(|n| n.utf8())
            .expect("an effect is named");
        if !effect_name.starts_with("sig.") {
            continue;
        }
        for op in effect
            .field("ops")
            .and_then(|o| o.list())
            .expect("an effect declares operations")
        {
            let name = op.field("name").and_then(|n| n.utf8()).expect("named");
            let params = op
                .field("params")
                .and_then(|p| p.items(|t| Ok(printed(t))))
                .expect("an operation lists its parameters");
            let ret = printed(op.field("ret").expect("an operation answers"));
            out.insert(format!("{effect_name}.{name}"), (params, ret));
        }
    }
    out
}

fn span() -> Span {
    Span::new(SourceId(0), 0, 1)
}

/// What the run would report a value's type as.
fn type_of(value: &Value) -> Option<&'static str> {
    match value {
        Value::Int(_) => Some("Int"),
        Value::Unit => Some("Unit"),
        Value::Ctor { name, .. } if name.as_str() == "Instant" => Some("Instant"),
        _ => None,
    }
}

#[test]
fn the_clause_set_covers_each_declared_effect_exactly() {
    let check = checked();
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
fn every_seeded_operation_has_the_declared_types() {
    let declared = declared();
    for sig in SEEDED_OPS {
        let (params, ret) = &declared[&format!("sig.{sig}")];
        let seeded: Vec<&str> = sig.params.iter().map(|p| p.as_str()).collect();
        assert_eq!(params, &seeded, "`{sig}` disagrees about its parameters");
        assert_eq!(ret, sig.ret.as_str(), "`{sig}` disagrees about its result");
    }
}

#[test]
fn what_the_handlers_answer_has_the_declared_type() {
    let declared = declared();
    let mut handlers = Handlers::new(11);
    for sig in SEEDED_OPS {
        let (params, ret) = &declared[&format!("sig.{sig}")];
        let args: Vec<Value> = params
            .iter()
            .map(|param| match param.as_str() {
                // Positive: `random.below` cannot answer below zero, and a zero sleep is a yield.
                "Int" => Value::Int(3),
                "Duration" => Value::ctor("Duration", vec![Value::Int(3)]),
                other => panic!("`{sig}` takes a `{other}` now"),
            })
            .collect();
        match handlers.dispatch(sig, TaskId(0), &args, span()) {
            Ok(Answer::Value(v)) => {
                assert_eq!(type_of(&v), Some(ret.as_str()), "`{sig}` answered {v:?}")
            }
            // A woken sleeper is resumed with `clock.sleep`'s declared return.
            Ok(Answer::Sleeping { .. }) => {
                assert_eq!(ret, "Unit");
                assert_eq!(sig.ret, SimType::Unit);
            }
            Err(d) => panic!("`{sig}` refused its own declared arguments: {}", d.message),
        }
    }
}

#[test]
fn a_hand_written_handler_and_the_seeded_one_answer_the_same_operations() {
    let check = checked();
    let stub = check
        .defs
        .get(&Symbol::new("sig.stub"))
        .expect("`stub` is defined");
    assert!(
        stub.footprint.is_empty(),
        "a handler over the declared clause set discharges the effects, leaving {}",
        stub.footprint.0.len()
    );

    let (_, handler) = SOURCE
        .split_once("handle work() with {")
        .expect("`stub` is a handler");
    let (clauses, _) = handler.split_once("\n}").expect("the handler is closed");
    let written: Vec<(String, String, usize)> = clauses
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|clause| {
            let (head, _) = clause.split_once(" -> ").expect("a clause has a body");
            let (op, params) = head.split_once('(').expect("a clause names its operation");
            let (effect, op) = op.split_once('.').expect("a clause names its effect");
            let params = params.trim_end_matches(')');
            let count = params.split(',').filter(|p| !p.trim().is_empty()).count();
            (effect.to_string(), op.to_string(), count)
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

#[test]
fn the_evaluator_reads_no_host_clock_and_no_host_entropy() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../ply-eval/src");
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
            "Instant::now",
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

/// A generator crate would put the host's entropy, and its own version, inside a seed's meaning.
#[test]
fn the_crate_depends_on_no_generator_and_no_entropy_source() {
    let manifest = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../ply-eval/Cargo.toml"),
    )
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
