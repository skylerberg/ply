//! The meaning of a node, independent of any strategy for reaching it.
//!
//! A literal, a binary operator, a constructor value, a pattern match and the
//! diagnostics they raise are the same whether a machine steps to them or a
//! lowering folds them away. They live here rather than in `machine.rs` because
//! `builtins.rs`, `handler.rs` and `sim.rs` all raise from the same set, and a
//! second spelling of "takes 2 arguments, but 3 were given" is a divergence in
//! the only surface a user reads.

use crate::handler::OpDecl;
use crate::value::{Closure, ClosureKind, Decimal, Value, type_error, values_equal};
use ply_span::{Diagnostic, Span, Symbol, codes};
use ply_syntax::ast::{BinOp, Ident, Lit, Mode, QName};
use rustc_hash::FxHashMap;
use std::cell::RefCell;
use std::sync::Arc;

/// An operation's declaration, by program-wide effect name and operation name.
pub(crate) type OpTable = FxHashMap<(Symbol, Symbol), (bool, Mode)>;

/// One spelling of a mis-declared operation, wherever it is noticed.
pub(crate) fn op_decl(ops: &OpTable, effect: &Symbol, op: &Symbol) -> OpDecl {
    match ops.get(&(effect.clone(), op.clone())) {
        Some(&(resource_param, mode)) => OpDecl::Declared {
            resource_param,
            mode,
        },
        None if ops.keys().any(|(e, _)| e == effect) => OpDecl::NoSuchOp,
        None => OpDecl::UnknownEffect,
    }
}

pub(crate) fn literal(lit: &Lit) -> Value {
    match lit {
        Lit::Int(i) => Value::Int(*i),
        Lit::Bool(b) => Value::Bool(*b),
        Lit::Str(s) => Value::str(s),
        Lit::Bytes(b) => Value::bytes(b),
        Lit::Float(f) => Value::Float(*f),
        Lit::Decimal { mantissa, scale } => Value::Decimal(decimal_lit(*mantissa, *scale)),
        Lit::Unit => Value::Unit,
    }
}

/// A `Decimal` literal's value.
///
/// Total because both producers of a `Lit::Decimal` already enforce the type's
/// range — the lexer refuses a mantissa past 96 bits or a scale past 28, and the
/// body decoder refuses the same bytes — so the fallback is a shape no stream
/// this evaluator is handed can carry.
pub(crate) fn decimal_lit(mantissa: i128, scale: u32) -> Decimal {
    Decimal::try_from_i128_with_scale(mantissa, scale).unwrap_or(Decimal::ZERO)
}

/// A literal pattern against a value, shared by the machine and the lowering so a
/// both` divergence cannot be the two of them disagreeing about a NaN.
///
/// `Float` matches by IEEE `==`, so a `NaN` pattern matches nothing at all —
/// including a NaN scrutinee. A pattern that answered otherwise would be a
/// second equality on the type, and nobody wrote that one down.
pub(crate) fn lit_matches(lit: &Lit, value: &Value) -> bool {
    match (lit, value) {
        (Lit::Int(a), Value::Int(b)) => a == b,
        (Lit::Bool(a), Value::Bool(b)) => a == b,
        (Lit::Str(a), Value::Str(b)) => a.as_str() == b.as_ref(),
        (Lit::Bytes(a), Value::Bytes(b)) => a.as_slice() == b.as_ref(),
        (Lit::Float(a), Value::Float(b)) => a == b,
        // By numeric value, matching `==`: a `1.50m` pattern matches `1.5m`.
        (Lit::Decimal { mantissa, scale }, Value::Decimal(b)) => {
            decimal_lit(*mantissa, *scale) == *b
        }
        (Lit::Unit, Value::Unit) => true,
        _ => false,
    }
}

/// Constructor values kept per thread.
///
/// Bounded for [`crate::pool`]'s reason and stated here so the cost is a number:
/// a thread that has run a program with more constructors than this keeps the
/// first [`CTOR_CACHE_KEEP`] it met and builds the rest per mention, which is
/// what it did before this cache existed. Past the bound the cost degrades to
/// the old one rather than to a cliff, which is why nothing is evicted.
const CTOR_CACHE_KEEP: usize = 4096;

thread_local! {
    /// Keyed by the constructor's program-wide name, holding the arity the
    /// value was built at: two programs run on one thread can spell one name
    /// with two arities, and the second must not read the first's value. See
    /// [`ctor_value`].
    static CTOR_VALUES: RefCell<FxHashMap<Symbol, (usize, Value)>> =
        RefCell::new(FxHashMap::default());
}

/// The value a mention of a constructor evaluates to: one per constructor per
/// thread, built on first mention.
///
/// A mention is a compile-time constant — the value is a function of the name
/// and the arity and of nothing else — and rebuilding it per mention cost 21.0
/// nullary `Value::Ctor`s and 24.0 `Arc<Closure>`s per `/health` request,
/// measured by `cargo test -p ply-corpus --release --test r4_value_construction
/// -- --nocapture`. [`Value::builtin`] has shared a builtin's closure since W6
/// for the same reason and its note carries the argument that sharing one is
/// invisible: a `Closure` is immutable and [`Value::cmp`] answers `Equal` for
/// any two of them, so there is no identity to observe. The nullary case adds
/// one clause to that argument — a shared `Ctor` is immutable because its
/// `args` are empty, so it holds no [`Value::Cell`] past the region that would
/// reclaim one, and it can never be a [`Value::Secret`], which is built by no
/// path that reaches here.
///
/// [`Value::builtin`]: crate::value::Value::builtin
/// [`Value::cmp`]: crate::value::Value
pub(crate) fn ctor_value(name: &Symbol, arity: usize) -> Value {
    let fresh = || {
        if arity == 0 {
            Value::ctor(name.clone(), Vec::new())
        } else {
            Value::Closure(Arc::new(Closure {
                name: Some(name.clone()),
                kind: ClosureKind::Ctor {
                    name: name.clone(),
                    arity,
                },
            }))
        }
    };
    // `try_with`, because a `Value` dropped during thread-local teardown can
    // reach here after the cache is gone, and building a fresh one is the right
    // answer there rather than an abort. [`Value::builtin`] takes it for the
    // same reason.
    CTOR_VALUES
        .try_with(|cache| {
            let mut cache = cache.borrow_mut();
            match cache.get(name) {
                Some((at, value)) if *at == arity => value.clone(),
                Some(_) => {
                    let value = fresh();
                    cache.insert(name.clone(), (arity, value.clone()));
                    value
                }
                None => {
                    let value = fresh();
                    if cache.len() < CTOR_CACHE_KEEP {
                        cache.insert(name.clone(), (arity, value.clone()));
                    }
                    value
                }
            }
        })
        .unwrap_or_else(|_| fresh())
}

#[inline(never)]
pub(crate) fn strict_binary(
    op: BinOp,
    l: &Value,
    r: &Value,
    lspan: Span,
    rspan: Span,
    span: Span,
) -> Result<Value, Diagnostic> {
    match op {
        BinOp::Eq => Ok(Value::Bool(values_equal(l, r, span)?)),
        BinOp::Ne => Ok(Value::Bool(!values_equal(l, r, span)?)),
        BinOp::Concat => {
            let a = l.as_str(lspan, "`++`")?;
            let b = r.as_str(rspan, "`++`")?;
            Ok(Value::str(format!("{a}{b}")))
        }
        // `Float` answers by IEEE, where `NaN < x` and `NaN >= x` are both
        // false — so a comparison is not the negation of its converse. That is
        // what the type says, and smoothing it over here would make the operator
        // disagree with `==` on the same two values.
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
            if let (Value::Float(a), Value::Float(b)) = (l, r) {
                return Ok(Value::Bool(match op {
                    BinOp::Lt => a < b,
                    BinOp::Le => a <= b,
                    BinOp::Gt => a > b,
                    _ => a >= b,
                }));
            }
            let ordering = match (l, r) {
                (Value::Int(a), Value::Int(b)) => a.cmp(b),
                (Value::Str(a), Value::Str(b)) => a.as_ref().cmp(b.as_ref()),
                (Value::Decimal(a), Value::Decimal(b)) => a.cmp(b),
                (Value::Int(_) | Value::Str(_) | Value::Decimal(_) | Value::Float(_), other) => {
                    return Err(type_error(rspan, "a comparison", l.type_name(), other));
                }
                (other, _) => {
                    return Err(type_error(
                        lspan,
                        "a comparison",
                        "Int, String, Float or Decimal",
                        other,
                    ));
                }
            };
            Ok(Value::Bool(match op {
                BinOp::Lt => ordering.is_lt(),
                BinOp::Le => ordering.is_le(),
                BinOp::Gt => ordering.is_gt(),
                _ => ordering.is_ge(),
            }))
        }
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
            match (l, r) {
                (Value::Float(a), Value::Float(b)) => return float_arithmetic(op, *a, *b, span),
                (Value::Decimal(a), Value::Decimal(b)) => {
                    return decimal_arithmetic(op, *a, *b, rspan, span);
                }
                _ => {}
            }
            let a = l.as_int(lspan, "arithmetic")?;
            let b = r.as_int(rspan, "arithmetic")?;
            let (result, what) = match op {
                BinOp::Add => (a.checked_add(b), "addition"),
                BinOp::Sub => (a.checked_sub(b), "subtraction"),
                BinOp::Mul => (a.checked_mul(b), "multiplication"),
                BinOp::Div if b == 0 => return Err(err_zero_divisor(rspan, "division")),
                BinOp::Div => (a.checked_div(b), "division"),
                _ if b == 0 => return Err(err_zero_divisor(rspan, "remainder")),
                _ => (a.checked_rem(b), "remainder"),
            };
            match result {
                Some(n) => Ok(Value::Int(n)),
                None => Err(err_overflow(span, what, a, b)),
            }
        }
        // The two's-complement bit pattern of the `Int`, and nothing else: the checker
        // refused every other operand type.
        BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor => {
            let a = l.as_int(lspan, "a bit operator")?;
            let b = r.as_int(rspan, "a bit operator")?;
            Ok(Value::Int(match op {
                BinOp::BitAnd => a & b,
                BinOp::BitOr => a | b,
                _ => a ^ b,
            }))
        }

        // A count outside `0..=63` raises for the reason a zero divisor does, while `<<`
        // itself discards rather than raising, because a mixing step is defined to drop
        // the bits that leave.
        BinOp::Shl | BinOp::Shr | BinOp::Ushr => {
            let a = l.as_int(lspan, "a shift")?;
            let n = r.as_int(rspan, "a shift")?;
            if !(0..64).contains(&n) {
                return Err(err_shift_count(rspan, n));
            }
            let n = n as u32;
            Ok(Value::Int(match op {
                BinOp::Shl => ((a as u64) << n) as i64,
                // Both shifts exist because `Int` is signed and there is no `UInt`.
                BinOp::Shr => a >> n,
                _ => ((a as u64) >> n) as i64,
            }))
        }

        BinOp::And | BinOp::Or => Err(Diagnostic::error(
            codes::INTERNAL_ERROR,
            "internal error: a short-circuiting operator reached strict evaluation",
        )
        .primary(span, "please report this")),
    }
}

/// IEEE-754, unmodified. There is no overflow error and no zero-divisor error:
/// `1.0 / 0.0` is `Infinity` and `0.0 / 0.0` is `NaN`, and those are values the
/// standard defines rather than failures. Refusing them would make `Float` a
/// worse `Decimal` instead of a different type — and the cost is stated in the
/// type, which is why nothing about a `Float` may be `proved`.
fn float_arithmetic(op: BinOp, a: f64, b: f64, span: Span) -> Result<Value, Diagnostic> {
    Ok(Value::Float(match op {
        BinOp::Add => a + b,
        BinOp::Sub => a - b,
        BinOp::Mul => a * b,
        BinOp::Div => a / b,
        BinOp::Rem => a % b,
        _ => {
            return Err(Diagnostic::error(
                codes::INTERNAL_ERROR,
                "internal error: a non-arithmetic operator reached float arithmetic",
            )
            .primary(span, "please report this"));
        }
    }))
}

/// Exact, or a diagnostic. Never a silent wrap and never a silent rounding: a
/// total that quietly lost a cent is the failure this type exists to prevent.
///
/// `/` never arrives — inference refuses it with `E0209`, because the exact
/// quotient of two decimals is not in general a decimal and an operator would
/// have to round. `%` does, and is exact: the *remainder* of a decimal division
/// is a decimal even when the quotient is not.
fn decimal_arithmetic(
    op: BinOp,
    a: Decimal,
    b: Decimal,
    rspan: Span,
    span: Span,
) -> Result<Value, Diagnostic> {
    let (result, what) = match op {
        BinOp::Add => (a.checked_add(b), "addition"),
        BinOp::Sub => (a.checked_sub(b), "subtraction"),
        // Exact while the result's scale fits, and half-to-even at scale 28
        // otherwise. `checked_mul` is what applies that rule; a mantissa that
        // leaves 96 bits is `None` and is reported rather than rounded.
        BinOp::Mul => (a.checked_mul(b), "multiplication"),
        BinOp::Rem => {
            if b.is_zero() {
                return Err(err_zero_divisor(rspan, "remainder"));
            }
            (a.checked_rem(b), "remainder")
        }
        BinOp::Div => {
            return Err(Diagnostic::error(
                codes::DECIMAL_DIVISION,
                "`/` is not defined on `Decimal`",
            )
            .primary(span, "the exact quotient of two decimals is not a decimal")
            .note("call `decimal_div(a, b, scale, HalfEven)` and say how to round"));
        }
        _ => {
            return Err(Diagnostic::error(
                codes::INTERNAL_ERROR,
                "internal error: a non-arithmetic operator reached decimal arithmetic",
            )
            .primary(span, "please report this"));
        }
    };
    match result {
        Some(d) => Ok(Value::Decimal(d)),
        None => Err(Diagnostic::error(
            codes::RUNTIME_ERROR,
            format!("`Decimal` overflow in {what}"),
        )
        .primary(
            span,
            format!("{a} and {b} need more than 96 bits of mantissa"),
        )
        .note("`Decimal` is exact and bounded; it will not round to make room")),
    }
}

pub(crate) fn arity_error(span: Span, what: &str, expected: usize, got: usize) -> Diagnostic {
    Diagnostic::error(
        codes::ARITY_MISMATCH,
        format!(
            "{what} takes {expected} argument{}, but {got} were given",
            plural(expected)
        ),
    )
    .primary(span, format!("{got} argument{} here", plural(got)))
}

#[cold]
#[inline(never)]
pub(crate) fn err_unknown_name(q: &QName) -> Diagnostic {
    Diagnostic::error(
        codes::UNKNOWN_NAME,
        format!("cannot find `{q}` in this scope"),
    )
    .primary(q.span, "not bound here")
}

#[cold]
#[inline(never)]
pub(crate) fn err_not_a_function(span: Span, v: &Value) -> Diagnostic {
    Diagnostic::error(
        codes::NOT_A_FUNCTION,
        format!("cannot call a value of type {}", v.type_name()),
    )
    .primary(span, format!("this is {}", v.render()))
}

#[cold]
#[inline(never)]
pub(crate) fn err_non_exhaustive(span: Span, v: &Value) -> Diagnostic {
    Diagnostic::error(
        codes::NON_EXHAUSTIVE_MATCH,
        "no match arm applied to the scrutinee",
    )
    .primary(span, format!("this evaluated to {}", v.render()))
    .note("add an arm covering this value, or a `_` catch-all")
}

#[cold]
#[inline(never)]
pub(crate) fn err_let_mismatch(span: Span, v: &Value) -> Diagnostic {
    Diagnostic::error(
        codes::NON_EXHAUSTIVE_MATCH,
        "`let` pattern did not match the bound value",
    )
    .primary(span, format!("value was {}", v.render()))
    .note("use `match` when the pattern can fail")
}

#[cold]
#[inline(never)]
pub(crate) fn err_no_such_field(field: &Ident, fields: &crate::value::Fields) -> Diagnostic {
    let known: Vec<String> = fields.keys().map(|k| format!("`{k}`")).collect();
    Diagnostic::error(
        codes::UNKNOWN_NAME,
        format!("record has no field `{}`", field.name),
    )
    .primary(field.span, "no such field")
    .note(if known.is_empty() {
        "the record is empty".to_string()
    } else {
        format!("available fields: {}", known.join(", "))
    })
}

#[cold]
#[inline(never)]
pub(crate) fn err_shift_count(span: Span, n: i64) -> Diagnostic {
    Diagnostic::error(codes::RUNTIME_ERROR, "shift count out of range")
        .primary(span, format!("{n} is not in 0..=63"))
        .note("an `Int` is 64 bits, so no other count names a shift of it")
}

#[cold]
#[inline(never)]
pub(crate) fn err_zero_divisor(span: Span, what: &str) -> Diagnostic {
    Diagnostic::error(codes::RUNTIME_ERROR, format!("{what} by zero"))
        .primary(span, "this divisor is 0")
}

#[cold]
#[inline(never)]
pub(crate) fn err_overflow(span: Span, what: &str, a: i64, b: i64) -> Diagnostic {
    let detail = if what == "negation" {
        format!("-{a} does not fit in Int")
    } else {
        format!("{a} and {b} overflow Int")
    };
    Diagnostic::error(codes::RUNTIME_ERROR, format!("integer overflow in {what}"))
        .primary(span, detail)
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    #[ignore = "timing; run with `cargo test -p ply-eval --release --lib semantics::tests::a_cached_mention_against_the_allocation_it_replaces -- --ignored --nocapture`"]
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
}
