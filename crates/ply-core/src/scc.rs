/// Tarjan's algorithm, iterative so that a deep definition graph cannot blow the
/// Rust stack. Components come out in reverse topological order, so a component
/// is always emitted before the components that depend on it — exactly the order
/// generalization needs.
pub fn sccs(n: usize, adj: &[Vec<usize>]) -> Vec<Vec<usize>> {
    const UNVISITED: usize = usize::MAX;
    let mut index = vec![UNVISITED; n];
    let mut low = vec![0usize; n];
    let mut on_stack = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut call: Vec<(usize, usize)> = Vec::new();
    let mut out: Vec<Vec<usize>> = Vec::new();
    let mut next = 0usize;

    for root in 0..n {
        if index[root] != UNVISITED {
            continue;
        }
        index[root] = next;
        low[root] = next;
        next += 1;
        stack.push(root);
        on_stack[root] = true;
        call.push((root, 0));

        while let Some((v, edge)) = call.pop() {
            if edge < adj[v].len() {
                call.push((v, edge + 1));
                let w = adj[v][edge];
                if index[w] == UNVISITED {
                    index[w] = next;
                    low[w] = next;
                    next += 1;
                    stack.push(w);
                    on_stack[w] = true;
                    call.push((w, 0));
                } else if on_stack[w] {
                    low[v] = low[v].min(index[w]);
                }
            } else {
                if low[v] == index[v] {
                    let mut comp = Vec::new();
                    loop {
                        let w = stack.pop().expect("tarjan stack underflow");
                        on_stack[w] = false;
                        comp.push(w);
                        if w == v {
                            break;
                        }
                    }
                    comp.reverse();
                    out.push(comp);
                }
                if let Some(&(parent, _)) = call.last() {
                    low[parent] = low[parent].min(low[v]);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
