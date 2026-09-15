/// Tarjan's algorithm, iterative so that a deep definition graph cannot blow the Rust stack.
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
