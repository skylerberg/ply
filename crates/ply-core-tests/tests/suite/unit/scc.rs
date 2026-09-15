use ply_core::scc::sccs;

#[test]
fn dependencies_come_out_before_their_dependents() {
    let adj = vec![vec![1], vec![2], vec![]];
    assert_eq!(sccs(3, &adj), vec![vec![2], vec![1], vec![0]]);
}

#[test]
fn a_cycle_forms_one_component() {
    let adj = vec![vec![1, 2], vec![0], vec![]];
    let out = sccs(3, &adj);
    assert_eq!(out.len(), 2);
    assert_eq!(out[0], vec![2]);
    assert_eq!(out[1], vec![0, 1]);
}

#[test]
fn self_recursion_is_a_singleton_component() {
    let adj = vec![vec![0]];
    assert_eq!(sccs(1, &adj), vec![vec![0]]);
}

#[test]
fn disconnected_nodes_all_appear_exactly_once() {
    let adj = vec![vec![], vec![], vec![]];
    let out = sccs(3, &adj);
    let mut flat: Vec<usize> = out.into_iter().flatten().collect();
    flat.sort();
    assert_eq!(flat, vec![0, 1, 2]);
}

#[test]
fn a_deep_chain_does_not_overflow_the_stack() {
    let n = 100_000;
    let adj: Vec<Vec<usize>> = (0..n)
        .map(|i| if i + 1 < n { vec![i + 1] } else { vec![] })
        .collect();
    let out = sccs(n, &adj);
    assert_eq!(out.len(), n);
    assert_eq!(out[0], vec![n - 1]);
}
