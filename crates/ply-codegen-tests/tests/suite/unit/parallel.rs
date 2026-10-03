use ply_codegen::Source;
use ply_codegen::c::Native;
use ply_codegen::heap::{Heap, Word, imm};
use ply_eval::{Analysis, SourceId, Symbol, Value, codes};
use std::collections::HashMap;

/// The front and the unit over `text`, module `m`; `None` on a host with no C compiler.
fn built(text: &str) -> Option<(&'static Analysis, Native)> {
    let named = [("m".to_string(), text.to_string())];
    let front: &'static Analysis = Box::leak(Box::new(
        ply_codegen::c::producer::checked_analysis(&named, &[SourceId(0)]).expect("checks"),
    ));
    let source: &'static Source = Box::leak(Box::new(
        Source::from_analysis(front).with_texts(HashMap::from(named)),
    ));
    let names = source.functions();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let _config = super::c::CONFIG.read().unwrap_or_else(|e| e.into_inner());
    match ply_codegen::c::build(source, &refs) {
        Ok((native, refused)) => {
            assert!(refused.is_empty(), "{refused:?}");
            Some((front, native))
        }
        Err(e) if e.to_string().contains("could not run") => None,
        Err(e) => panic!("{e}"),
    }
}

const PROGRAM: &str = r#"
fn fib(n: Int) -> Int = if n < 2 { n } else { fib(n - 1) + fib(n - 2) }

pub fn pair(n: Int) -> (Int, Int) = parallel { fib(n), fib(n + 1) }

fn summed(lo: Int, hi: Int) -> Int =
  if hi - lo <= 64 { fold(range(lo, hi), 0, |acc: Int, i: Int| acc + i) }
  else {
    let mid = (lo + hi) / 2;
    let p = parallel { summed(lo, mid), summed(mid, hi) };
    p._0 + p._1
  }

pub fn total(n: Int) -> Int = summed(0, n)

pub fn lengths(n: Int) -> Int = {
  let words = map(range(0, n), |i: Int| "w" ++ int_to_string(i));
  let p = parallel { len(filter(words, |w: String| string_len(w) > 2)), fold(words, 0, |acc: Int, w: String| acc + string_len(w)) };
  p._0 + p._1 + len(words)
}

pub fn failing(n: Int) -> Int / {abort.raise} = {
  let p = parallel { if fib(n) > 0 { panic("the left branch") } else { 0 }, panic("the right branch") };
  p._0 + p._1
}
"#;

/// `name` over one `Int`, its answer as a value, or the failure's code and message.
fn run(native: &Native, name: &str, n: i64, steps: i64) -> Result<Value, (String, String)> {
    let entry = native.entry(name).expect("the definition compiles");
    let mut ctx = native.context();
    ply_codegen::rt::with_step_budget(steps, || {
        ctx.begin(100_000);
        let answer: Word = unsafe { entry(&mut ctx, [imm(n)].as_ptr()) };
        let out = if ctx.failed == 0 {
            Ok(Heap::to_value(&native.tables().layouts, answer))
        } else {
            let d = ctx.diagnostic.as_ref().expect("a failure says why");
            Err((d.code.to_string(), d.message.clone()))
        };
        ctx.end();
        out
    })
}

fn tuple(values: &[i64]) -> Value {
    Value::Record(std::sync::Arc::new(
        values
            .iter()
            .enumerate()
            .map(|(i, v)| (Symbol::new(format!("_{i}")), Value::Int(*v)))
            .collect(),
    ))
}

#[test]
fn a_block_answers_its_branches_as_a_tuple() {
    let Some((_, native)) = built(PROGRAM) else {
        return;
    };
    assert_eq!(run(&native, "m.pair", 20, 0), Ok(tuple(&[6765, 10946])));
    // Blocks nest: each level's branches run at once, inside a branch of the level above.
    assert_eq!(
        run(&native, "m.total", 100_000, 0),
        Ok(Value::Int(4_999_950_000))
    );
}

/// What a branch allocates outlives it, and what the branches read is shared among them.
#[test]
fn what_branches_build_and_share_survives_them() {
    let Some((_, native)) = built(PROGRAM) else {
        return;
    };
    for _ in 0..8 {
        assert_eq!(
            run(&native, "m.lengths", 2000, 0),
            Ok(Value::Int(1990 + 8890 + 2000))
        );
    }
}

/// The branch to the right fails first in time; evaluating them in turn never reaches it.
#[test]
fn the_leftmost_failure_is_the_blocks_whichever_failed_first() {
    let Some((_, native)) = built(PROGRAM) else {
        return;
    };
    for _ in 0..4 {
        let (code, message) = run(&native, "m.failing", 25, 0).expect_err("both branches panic");
        assert_eq!(code, codes::RUNTIME_ERROR);
        assert!(message.contains("the left branch"), "{message}");
    }
}

/// Each branch fits the budget alone and the two do not: the block spends what evaluating them in
/// turn spends, wherever they ran.
#[test]
fn the_branches_spend_one_budget_between_them() {
    let Some((_, native)) = built(PROGRAM) else {
        return;
    };
    let alone = fib_calls(20);
    let both = alone + fib_calls(21);
    let (code, _) = run(&native, "m.pair", 20, both - alone / 2).expect_err("past the budget");
    assert_eq!(code, codes::STEP_BUDGET);
    assert_eq!(
        run(&native, "m.pair", 20, both + 16),
        Ok(tuple(&[6765, 10946]))
    );
}

/// The calls `fib(n)` makes, itself included.
fn fib_calls(n: i64) -> i64 {
    if n < 2 {
        1
    } else {
        1 + fib_calls(n - 1) + fib_calls(n - 2)
    }
}
