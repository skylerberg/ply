use ply_prove::prove::egraph::{Classes, Shape, conflict};

#[test]
fn a_class_representative_does_not_depend_on_the_union_order() {
    let mut forward = Classes::new(4);
    forward.union(0, 1);
    forward.union(1, 2);
    let mut backward = Classes::new(4);
    backward.union(2, 1);
    backward.union(1, 0);
    assert_eq!(forward.find(2), backward.find(0));
    assert_eq!(forward.find(2), 0);
}

#[test]
fn an_asserted_disequality_between_equal_terms_contradicts() {
    let mut classes = Classes::new(3);
    classes.distinguish(0, 2);
    classes.check_diseqs();
    assert!(!classes.contradiction);
    classes.union(0, 2);
    classes.check_diseqs();
    assert!(classes.contradiction);
}

#[test]
fn shapes_of_different_kinds_conclude_nothing() {
    assert_eq!(conflict(&Shape::Int(1), &Shape::Bool(true)), None);
    assert_eq!(conflict(&Shape::Nil, &Shape::Int(0)), None);
    assert_eq!(conflict(&Shape::Int(1), &Shape::Int(2)), Some(true));
    assert_eq!(conflict(&Shape::Int(1), &Shape::Int(1)), Some(false));
}
