use ply_eval::Value;
use ply_eval::memo::*;
use ply_span::Symbol;

fn nested(levels: usize, bottom: Value) -> Value {
    (0..levels).fold(bottom, |inner, _| Value::Ctor {
        name: Symbol::new("Node"),
        args: std::sync::Arc::new(vec![Value::list(vec![inner])]),
    })
}

/// A parsed program is a constant deeper than any recursion budget.
#[test]
fn a_constant_is_judged_by_its_leaves_however_deep_they_lie() {
    assert!(world_independent(&nested(2_000, Value::Unit)));
    let mut regions: ply_eval::TaskRegions = ply_eval::TaskRegions::new();
    let cell = Value::Cell(regions.alloc_cell(Value::Unit));
    assert!(!world_independent(&nested(2_000, cell)));
}
