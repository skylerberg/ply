use crate::unit::build::sp;
use ply_eval::Value;
use ply_span::codes;

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
        ply_eval::values_equal(&chain(deep), &chain(deep), sp()).expect("a legal value compares")
    }));
}

#[test]
fn a_value_past_the_bound_is_a_diagnostic_and_not_an_abort() {
    let (code, message) = on_a_small_stack(|| {
        let deep = ply_eval::MAX_VALUE_DEPTH + 2;
        let d = ply_eval::values_equal(&chain(deep), &chain(deep), sp())
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
        assert!(!ply_eval::values_equal(&actual, &other, sp()).expect("they compare"));
        ply_eval::first_difference(&actual, &other)
    });
    let (path, expected, actual) = found.expect("the difference is located");
    assert!(path.ends_with(".Link.0"), "{path}");
    assert_eq!(expected, "End");
    assert_eq!(actual, "Nil");
}

#[test]
fn every_rendered_byte_lexes_back_to_the_byte_it_came_from() {
    let all: Vec<u8> = (0..=255u8).collect();
    for chunk in all.chunks(32) {
        let rendered = Value::bytes(chunk).render();
        let (tokens, diags) = ply_syntax::lexer::lex(ply_span::SourceId(0), &rendered);
        assert!(diags.is_empty(), "{rendered} did not lex: {diags:?}");
        assert_eq!(
            tokens[0].kind,
            ply_syntax::lexer::TokenKind::Bytes(chunk.to_vec()),
            "{rendered}"
        );
    }
}
