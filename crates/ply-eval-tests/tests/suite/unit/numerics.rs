use ply_eval::{Span, Value, values_equal};

/// `values_equal` is the language's `==` and `Value::cmp` is the map's order.
#[test]
fn the_language_equality_and_the_map_order_part_only_at_nan_and_signed_zero() {
    let nan = Value::Float(f64::NAN);
    let zero = Value::Float(0.0);
    let negative_zero = Value::Float(-0.0);

    assert!(!values_equal(&nan, &nan, Span::DUMMY).unwrap());
    assert_eq!(nan.cmp(&nan), std::cmp::Ordering::Equal);

    assert!(values_equal(&zero, &negative_zero, Span::DUMMY).unwrap());
    assert_ne!(zero.cmp(&negative_zero), std::cmp::Ordering::Equal);

    // Everywhere else the two agree.
    for a in [1.0f64, -1.0, f64::INFINITY, f64::MAX, 0.5] {
        for b in [1.0f64, -1.0, f64::INFINITY, f64::MAX, 0.5] {
            let (x, y) = (Value::Float(a), Value::Float(b));
            assert_eq!(
                values_equal(&x, &y, Span::DUMMY).unwrap(),
                x.cmp(&y) == std::cmp::Ordering::Equal,
                "{a} vs {b}"
            );
        }
    }
}

#[test]
fn a_floats_text_cannot_be_read_as_an_int() {
    assert_eq!(ply_eval::render_float(1.0), "1.0");
    assert_eq!(ply_eval::render_float(-0.0), "-0.0");
    assert_eq!(ply_eval::render_float(f64::NAN), "NaN");
    assert_eq!(ply_eval::render_float(f64::INFINITY), "Infinity");
    assert_eq!(ply_eval::render_float(f64::NEG_INFINITY), "-Infinity");
    assert_eq!(Value::Float(1.0).type_name(), "Float");
}
