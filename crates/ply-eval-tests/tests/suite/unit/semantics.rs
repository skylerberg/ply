use ply_eval::semantics::*;
use ply_eval::{Closure, ClosureKind, Value, values_equal};
use ply_span::{Span, Symbol};
use std::sync::Arc;

/// Unique per test, because the cache is thread-local and a test binary may run two tests on one thread.
fn name(s: &str) -> Symbol {
    Symbol::new(format!("semantics::tests::{s}"))
}

fn ctor_args(v: &Value) -> Arc<Vec<Value>> {
    match v {
        Value::Ctor { args, .. } => args.clone(),
        other => panic!("expected a `Ctor`, found a `{}`", other.type_name()),
    }
}

fn closure_of(v: &Value) -> Arc<Closure> {
    match v {
        Value::Closure(c) => c.clone(),
        other => panic!("expected a `Closure`, found a `{}`", other.type_name()),
    }
}

/// Identity rather than equality: an equal value would mean it was rebuilt.
#[test]
fn a_nullary_constructors_value_is_built_once_per_thread() {
    let n = name("Red");
    let first = ctor_value(&n, 0);
    let second = ctor_value(&n, 0);
    assert!(
        Arc::ptr_eq(&ctor_args(&first), &ctor_args(&second)),
        "two mentions of one nullary constructor answered with two values, so \
         `ctor_value` built the second rather than sharing the first"
    );
}

#[test]
fn a_constructor_closure_is_built_once_per_thread() {
    let n = name("Box");
    let first = closure_of(&ctor_value(&n, 1));
    let second = closure_of(&ctor_value(&n, 1));
    assert!(
        Arc::ptr_eq(&first, &second),
        "two mentions of one constructor answered with two closures"
    );
    assert_eq!(first.arity(), 1);
}

/// Two programs run on one thread can spell one constructor with two arities.
#[test]
fn a_name_met_at_another_arity_is_not_answered_from_the_cache() {
    let n = name("Same");
    assert!(matches!(ctor_value(&n, 0), Value::Ctor { .. }));
    assert_eq!(closure_of(&ctor_value(&n, 2)).arity(), 2);
    assert_eq!(closure_of(&ctor_value(&n, 2)).arity(), 2);
    assert!(
        matches!(ctor_value(&n, 0), Value::Ctor { .. }),
        "the name went back to arity 0 and the cache still answered with the closure it \
         had been rebuilt at"
    );
}

#[test]
fn a_shared_constructor_value_is_the_value_a_fresh_one_was() {
    let n = name("Green");
    let shared = ctor_value(&n, 0);
    let fresh = Value::ctor(n.clone(), Vec::new());
    assert!(
        values_equal(&shared, &fresh, Span::DUMMY).expect("two `Ctor`s compare"),
        "the shared value is not the value a mention used to build"
    );
    // A closure has no equality a program can ask for, so this pins that the map order cannot separate two.
    let f = ctor_value(&name("Pair"), 2);
    let g = Value::Closure(Arc::new(Closure {
        name: Some(name("Pair")),
        kind: ClosureKind::Ctor {
            name: name("Pair"),
            arity: 2,
        },
    }));
    assert_eq!(f.cmp(&g), std::cmp::Ordering::Equal);
}

#[test]
fn a_cached_constructor_value_holds_nothing() {
    let held = ctor_value(&name("Empty"), 0);
    assert!(
        ctor_args(&held).is_empty(),
        "a cached nullary constructor is holding {} value(s)",
        ctor_args(&held).len()
    );
    for arity in 0..4 {
        let v = ctor_value(&name(&format!("Arity{arity}")), arity);
        assert!(
            !matches!(v, Value::Secret(_)),
            "`ctor_value` answered with a `Secret`, which would give a credential the \
             lifetime of the cache"
        );
    }
}

/// A wall clock on a shared machine, so it fails only when the trade is clearly the wrong way round.
#[test]
#[ignore = "timing; run with `cargo test -p ply-eval-tests --release --test unit semantics::a_cached_mention_against_the_allocation_it_replaces -- --ignored --nocapture`"]
fn a_cached_mention_against_the_allocation_it_replaces() {
    const MENTIONS: usize = 200_000;
    // A real program-wide name, because the cache hashes the name and its length is a cost.
    let n = Symbol::new("m.Red");
    let b = Symbol::new("m.Box");
    let per = |s: f64| 1e9 * s / MENTIONS as f64;
    let time = |f: &dyn Fn()| {
        let t = std::time::Instant::now();
        for _ in 0..MENTIONS {
            f();
        }
        t.elapsed().as_secs_f64()
    };
    let rebuild_closure = || {
        std::hint::black_box(Value::Closure(Arc::new(Closure {
            name: Some(b.clone()),
            kind: ClosureKind::Ctor {
                name: b.clone(),
                arity: 1,
            },
        })));
    };
    let arms: [(&str, &dyn Fn()); 4] = [
        ("nullary, cached", &|| {
            std::hint::black_box(ctor_value(&n, 0));
        }),
        ("nullary, rebuilt", &|| {
            std::hint::black_box(Value::ctor(n.clone(), Vec::new()));
        }),
        ("arity 1, cached", &|| {
            std::hint::black_box(ctor_value(&b, 1));
        }),
        ("arity 1, rebuilt", &rebuild_closure),
    ];
    for (_, arm) in &arms {
        arm();
    }
    let mut best = [f64::MAX; 4];
    for _ in 0..7 {
        for (i, (_, arm)) in arms.iter().enumerate() {
            best[i] = best[i].min(time(*arm));
        }
    }
    for (i, (label, _)) in arms.iter().enumerate() {
        println!("  {label:<18} {:>6.1}ns a mention", per(best[i]));
    }
    for (cached, rebuilt, what) in [
        (0, 1, "a nullary constructor"),
        (2, 3, "a constructor closure"),
    ] {
        let ratio = best[cached] / best[rebuilt];
        println!("  {what}: {ratio:.2}x what rebuilding it costs");
        assert!(
            ratio < 1.5,
            "a cached mention of {what} cost {:.1}ns against {:.1}ns to rebuild it: the \
             lookup is dearer than the allocation it replaced, and the value-representation work's \"what is \
             assumed\" item 1 is failing at this seam",
            per(best[cached]),
            per(best[rebuilt])
        );
    }
}

#[test]
fn past_the_bound_a_mention_is_built_as_it_was_before() {
    let entry = size_of::<Symbol>() + size_of::<usize>() + size_of::<Value>();
    println!(
        "one entry is at least {entry} bytes; the cache keeps at most {CTOR_CACHE_KEEP} of \
         them per thread"
    );
    for i in 0..CTOR_CACHE_KEEP {
        let filler = ctor_value(&name(&format!("Filler{i}")), 0);
        assert!(matches!(filler, Value::Ctor { .. }));
    }
    let held = CTOR_VALUES.with(|c| c.borrow().len());
    assert_eq!(
        held, CTOR_CACHE_KEEP,
        "the cache stopped short of its bound"
    );

    let overflow = name("Overflow");
    let first = ctor_value(&overflow, 0);
    let second = ctor_value(&overflow, 0);
    assert!(
        values_equal(&first, &second, Span::DUMMY).expect("two `Ctor`s compare"),
        "a constructor past the bound answered with two different values"
    );
    assert_eq!(
        CTOR_VALUES.with(|c| c.borrow().len()),
        CTOR_CACHE_KEEP,
        "the cache grew past its own bound"
    );
}
