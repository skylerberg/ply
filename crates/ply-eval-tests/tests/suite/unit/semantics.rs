use ply_eval::semantics::*;
use ply_eval::{Closure, ClosureKind, Value, values_equal};
use ply_span::{Span, Symbol};
use std::sync::Arc;

/// A name no other test in this module uses, because the cache is
/// thread-local and a test binary may run two tests on one thread.
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

/// "Built once" as identity rather than as an allocation count: an equal
/// value would mean it was rebuilt.
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

/// The hazard a cache keyed by name has and a fresh build does not: two
/// programs run on one thread can spell one constructor with two arities,
/// and the second must not be handed the first's value.
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
    // A closure has no equality a program can ask for, so this is the
    // statement [`Value::builtin`]'s note rests on instead: the ordering
    // that decides a `Map`'s key order cannot separate two of them.
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

/// The secret invariant at this seam. A cached value has the program's lifetime, so
/// what it may hold is the whole question: a nullary constructor's `args`
/// are empty, so it can hold no [`Value::Cell`] past the region that would
/// reclaim one and no [`Value::Secret`] past the call that made it — and
/// `ctor_value` is reached only from a name resolution, which has no
/// argument to put in one.
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

/// What the cache trades: a `malloc`/`free` pair for a hash of the name and
/// a refcount bump. Unmeasured until this ran, and the value-representation work's
/// `max_time_regression` is what it feeds.
///
/// Both arms are timed in one window inside one process, alternating, and
/// the fastest of each is reported — `benches/README.md` §"Every ratio is
/// taken inside one window" is the reason, and on a machine whose load
/// moves between 3 and 47 it is the only way this resolves at all. The
/// second arm is `ctor_value`'s body from before the constant-value memo, spelled out
/// rather than called, because that function no longer exists.
///
/// **What it measures is a mention in a hot loop, where the allocator's
/// free list is warm and a `malloc`/`free` pair is at its cheapest.** The
/// nullary case comes out near even on that footing; the arity>=1 case does
/// not, because rebuilding a constructor closure allocates 80 bytes rather
/// than 40. Neither is the request path, where 45.0 fewer allocations is
/// what the change is for — `r4_value_construction` is that instrument.
///
/// The bar here is deliberately loose: this is a wall clock on a shared
/// machine, it decides nothing, and a green suite should not depend on one.
/// It fails only if a lookup is *dearer than the allocation it replaced*
/// by more than half, which would mean the trade is the wrong way round.
#[test]
#[ignore = "timing; run with `cargo test -p ply-eval-tests --release --test unit semantics::a_cached_mention_against_the_allocation_it_replaces -- --ignored --nocapture`"]
fn a_cached_mention_against_the_allocation_it_replaces() {
    const MENTIONS: usize = 200_000;
    // A real constructor's program-wide name rather than this module's
    // prefixed one: the cache hashes the name, so its length is a cost.
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

/// What the cache can hold, so its memory cost is a number rather than a
/// hope, and what a program past the bound gets — which is what it got
/// before the cache existed.
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
