use ply_eval::{Plain, Span, Value, codes};

/// A small stack, where unbounded host recursion aborts the whole test binary.
fn on_a_small_stack<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> R {
    std::thread::Builder::new()
        .stack_size(1024 * 1024)
        .spawn(f)
        .expect("failed to spawn")
        .join()
        .expect("the value walk overflowed the thread stack")
}

fn chain(depth: usize) -> Value {
    let mut v = Value::ctor("Nil", Vec::new());
    for _ in 0..depth {
        v = Value::ctor("Link", vec![v]);
    }
    v
}

#[test]
fn a_value_the_call_bound_permits_compares_and_drops_on_a_small_stack() {
    assert!(on_a_small_stack(|| {
        let deep = ply_eval::MAX_VALUE_DEPTH - 1;
        ply_eval::values_equal(&chain(deep), &chain(deep), Span::DUMMY)
            .expect("a legal value compares")
    }));
}

#[test]
fn a_value_past_the_bound_is_a_diagnostic_and_not_an_abort() {
    let (code, message) = on_a_small_stack(|| {
        let deep = ply_eval::MAX_VALUE_DEPTH + 2;
        let d = ply_eval::values_equal(&chain(deep), &chain(deep), Span::DUMMY)
            .expect_err("past the bound is an error");
        (d.code, d.message)
    });
    assert_eq!(code, codes::RUNTIME_ERROR);
    assert!(message.contains("recursion limit"), "{message}");
    assert!(message.contains("nested values"), "{message}");
}

#[test]
fn the_first_difference_of_two_deep_values_is_found_on_a_small_stack() {
    let found = on_a_small_stack(|| {
        let deep = ply_eval::MAX_VALUE_DEPTH - 1;
        let mut other = Value::ctor("End", Vec::new());
        for _ in 0..deep {
            other = Value::ctor("Link", vec![other]);
        }
        let actual = chain(deep);
        assert!(!ply_eval::values_equal(&actual, &other, Span::DUMMY).expect("they compare"));
        let d = ply_eval::first_difference(&actual, &other).expect("the difference is located");
        let last_step = matches!(
            d.path.last(),
            Some(ply_eval::PathStep::Arg(ctor, 0)) if ctor.as_str() == "Link"
        );
        (last_step, Plain::of(&d.expected), Plain::of(&d.actual))
    });
    assert_eq!(
        found,
        (
            true,
            Plain::Ctor("End".to_string(), Vec::new()),
            Plain::Ctor("Nil".to_string(), Vec::new())
        )
    );
}
