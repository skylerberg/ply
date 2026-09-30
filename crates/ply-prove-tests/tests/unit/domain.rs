//! Decoding a point of a domain the program measured. Which types are finite, and how large, is
//! `proof.domain`'s decision and its tests'; these pin that the runtime builds exactly the values the
//! program counted, once each, in the program's order.

use ply_eval::{Fixed, IntTy, Value};
use ply_prove::domain::{Case, Finite, Shape};
use ply_span::Symbol;

fn scalar(name: &str, size: u64) -> Shape {
    Shape::Scalar {
        name: name.to_string(),
        size,
    }
}

fn kinds() -> Shape {
    let nullary = |name: &str| Case {
        name: Symbol::new(name),
        size: 1,
        fields: Vec::new(),
    };
    Shape::Cases {
        size: 3,
        cases: vec![nullary("Asset"), nullary("Liability"), nullary("Equity")],
    }
}

fn ctor(name: &str, args: Vec<Value>) -> Value {
    Value::ctor(name, args)
}

#[test]
fn a_point_is_every_binder_decoded_the_last_varying_fastest() {
    let finite = Finite::of_shapes(vec![scalar("Bool", 2), kinds()]).expect("six points");
    assert_eq!(finite.points, 6);
    assert_eq!(
        finite.point(0),
        Some(vec![Value::Bool(false), ctor("Asset", vec![])])
    );
    assert_eq!(
        finite.point(4),
        Some(vec![Value::Bool(true), ctor("Liability", vec![])])
    );
    assert_eq!(finite.point(6), None);
    // Every point is named once: a repeated tuple would be a domain walked short.
    let points: Vec<_> = (0..finite.points).map(|i| finite.point(i)).collect();
    for (i, a) in points.iter().enumerate() {
        assert!(a.is_some());
        assert!(!points[i + 1..].contains(a), "point {i} repeats");
    }
}

#[test]
fn a_case_takes_its_share_of_its_type_and_decodes_its_fields() {
    let wrap = Shape::Cases {
        size: 7,
        cases: vec![
            Case {
                name: Symbol::new("Nothing"),
                size: 1,
                fields: Vec::new(),
            },
            Case {
                name: Symbol::new("Held"),
                size: 6,
                fields: vec![kinds(), scalar("Bool", 2)],
            },
        ],
    };
    let finite = Finite::of_shapes(vec![wrap]).expect("seven points");
    assert_eq!(finite.point(0), Some(vec![ctor("Nothing", vec![])]));
    assert_eq!(
        finite.point(1),
        Some(vec![ctor(
            "Held",
            vec![ctor("Asset", vec![]), Value::Bool(false)]
        )])
    );
    assert_eq!(
        finite.point(6),
        Some(vec![ctor(
            "Held",
            vec![ctor("Equity", vec![]), Value::Bool(true)]
        )])
    );
    assert_eq!(finite.point(7), None);
}

#[test]
#[allow(clippy::arc_with_non_send_sync)]
fn a_record_and_a_fixed_width_are_built_from_their_shapes() {
    let record = Shape::Fields {
        size: 512,
        fields: vec![
            (Symbol::new("flag"), scalar("Bool", 2)),
            (Symbol::new("n"), scalar("U8", 256)),
        ],
    };
    let finite = Finite::of_shapes(vec![record]).expect("a record's points");
    let fixed = |n: i128| Value::Fixed(Fixed::of(IntTy::U8, n).expect("a byte"));
    let built = |flag: bool, n: i128| {
        Value::Record(std::sync::Arc::new(
            [
                (Symbol::new("flag"), Value::Bool(flag)),
                (Symbol::new("n"), fixed(n)),
            ]
            .into_iter()
            .collect(),
        ))
    };
    assert_eq!(finite.point(0), Some(vec![built(false, 0)]));
    assert_eq!(finite.point(257), Some(vec![built(true, 1)]));
}

#[test]
fn a_domain_of_no_points_is_no_domain_and_a_ground_claim_has_one() {
    let empty = Shape::Cases {
        size: 0,
        cases: Vec::new(),
    };
    assert!(Finite::of_shapes(vec![scalar("Bool", 2), empty]).is_none());
    let ground = Finite::of_shapes(Vec::new()).expect("one point");
    assert_eq!(ground.points, 1);
    assert_eq!(ground.point(0), Some(Vec::new()));
}
