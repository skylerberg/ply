use ply_core::Footprint;

/// Greedy colouring of the conflict graph, largest footprint first. Every pair
/// inside a returned group has non-conflicting footprints, so a group runs
/// concurrently without any locking.
///
/// The ordering is the whole trick: a test that conflicts with many others has
/// the fewest colours available to it, so it must choose while the classes are
/// still mostly empty. Colouring in source order routinely produces one more
/// group than it needs to, and a group costs a full round of wall-clock time.
pub fn group_by_conflict(tests: &[(usize, Footprint)]) -> Vec<Vec<usize>> {
    let mut order: Vec<usize> = (0..tests.len()).collect();
    order.sort_by(|&a, &b| {
        tests[b]
            .1
            .0
            .len()
            .cmp(&tests[a].1.0.len())
            .then(tests[a].0.cmp(&tests[b].0))
    });

    let mut classes: Vec<Vec<usize>> = Vec::new();
    for &p in &order {
        let footprint = &tests[p].1;
        // Conflict is not transitive, so a colour class is only safe if the
        // candidate clears every member of it, not just one representative.
        let slot = classes.iter().position(|class| {
            class
                .iter()
                .all(|&q| !footprint.conflicts_with(&tests[q].1))
        });
        match slot {
            Some(k) => classes[k].push(p),
            None => classes.push(vec![p]),
        }
    }

    classes
        .into_iter()
        .map(|class| {
            let mut group: Vec<usize> = class.into_iter().map(|p| tests[p].0).collect();
            group.sort_unstable();
            group
        })
        .collect()
}
