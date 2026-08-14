//! Turning a [`CorpusSpec`] and a seed into a [`Corpus`].
//!
//! Three passes: modules and their import DAG, then definitions in dependency
//! order, then tests. Nothing looks forward, so a definition's callees always
//! already have a footprint and a weight.

use crate::model::*;
use crate::rng::Rng;
use crate::spec::CorpusSpec;

const PACKAGES: [&str; 10] = [
    "store", "billing", "auth", "search", "report", "ingest", "sched", "audit", "mailer", "index",
];
const UNITS: [&str; 10] = [
    "core", "api", "model", "rules", "view", "sync", "calc", "state", "query", "admin",
];
const VERBS: [&str; 16] = [
    "apply", "render", "collect", "resolve", "merge", "score", "tally", "expand", "prune", "align",
    "stage", "commit", "gather", "rank", "batch", "flush",
];
const TABLE_WORDS: [&str; 16] = [
    "users",
    "orders",
    "items",
    "invoices",
    "sessions",
    "events",
    "prices",
    "shipments",
    "carts",
    "refunds",
    "coupons",
    "reviews",
    "vendors",
    "payouts",
    "tickets",
    "audits",
];
const REGION_WORDS: [&str; 8] = [
    "hot", "warm", "cold", "edge", "shard", "digest", "window", "bucket",
];
const SHARD_WORDS: [&str; 8] = [
    "lane", "mile", "berth", "slot", "track", "gate", "dock", "aisle",
];

/// How many calls a definition makes beyond what its shape already requires.
const MAX_EXTRAS: usize = 3;

pub fn generate(spec: &CorpusSpec) -> Corpus {
    let tables = labels(&TABLE_WORDS, spec.tables);
    let regions = labels(&REGION_WORDS, spec.regions);
    let shards = labels(&SHARD_WORDS, spec.shards_per_test());
    let root = Rng::new(spec.seed);

    let mut modules = plan_modules(spec, &root);
    let mut corpus = Corpus {
        modules: Vec::new(),
        defs: Vec::new(),
        tests: Vec::new(),
        concurrent: Vec::new(),
        tables,
        regions,
        shards,
    };

    for id in 0..modules.len() {
        let defs = generate_module_defs(spec, &root, &mut corpus, &modules, id);
        modules[id].defs = defs;
        modules[id].needs_effects = modules[id]
            .defs
            .iter()
            .any(|&d| !corpus.defs[d].footprint.is_empty());
        corpus.modules.push(modules[id].clone());
    }

    mark_public(&mut corpus);
    corpus.tests = generate_tests(spec, &root, &corpus);
    corpus.concurrent = generate_concurrent_tests(spec, &root, &corpus);
    for test in &corpus.concurrent {
        corpus.modules[test.module].needs_effects = true;
    }
    corpus
}

/// A label list long enough for `count` entries: the vocabulary first, then
/// numbered extensions of it, so a corpus with 40 tables still reads like one.
fn labels(words: &[&str], count: usize) -> Vec<String> {
    (0..count)
        .map(|i| {
            let word = words[i % words.len()];
            if i < words.len() {
                word.to_string()
            } else {
                format!("{word}{}", i / words.len())
            }
        })
        .collect()
}

fn plan_modules(spec: &CorpusSpec, root: &Rng) -> Vec<Module> {
    let mut planned: Vec<Module> = Vec::with_capacity(spec.modules);

    let hubs: Vec<usize> = (0..spec.hub_modules.min(spec.modules)).collect();

    for id in 0..spec.modules {
        let layer = id * spec.depth / spec.modules;
        let package = PACKAGES[layer % PACKAGES.len()];
        let unit = UNITS[id % UNITS.len()];
        let name = format!("{package}{layer}.{unit}_{id}");
        let path = format!("{package}{layer}/{unit}_{id}.ply");

        let mut rng = root.fork(0x1000 + id as u64);
        let imports = choose_imports(&mut rng, &planned, layer, &hubs);

        planned.push(Module {
            id,
            name,
            path,
            layer,
            imports,
            defs: Vec::new(),
            helper: Helper {
                name: format!("stage_{id}"),
                m: rng.between(2, 5),
                b: rng.between(1, 40),
            },
            status_type: format!("Status{id}"),
            ctor_ready: format!("Ready{id}"),
            ctor_idle: format!("Idle{id}"),
            needs_effects: false,
        });
    }
    planned
}

/// Imports run strictly downward through the layers, which is what makes the
/// module graph acyclic by construction rather than by a check afterwards.
fn choose_imports(rng: &mut Rng, planned: &[Module], layer: usize, hubs: &[usize]) -> Vec<usize> {
    if layer == 0 {
        return Vec::new();
    }
    let previous: Vec<usize> = planned
        .iter()
        .filter(|m| m.layer + 1 == layer)
        .map(|m| m.id)
        .collect();
    let reachable_hubs: Vec<usize> = hubs
        .iter()
        .copied()
        .filter(|&h| planned.get(h).is_some_and(|m| m.layer < layer))
        .collect();

    let mut chosen: Vec<usize> = Vec::new();
    let wanted = rng.between(2, 4) as usize;
    for _ in 0..wanted {
        let from_hub = !reachable_hubs.is_empty() && rng.chance(0.45);
        let pick = if from_hub {
            rng.pick(&reachable_hubs).copied()
        } else if !previous.is_empty() {
            Some(previous[rng.skewed_below(previous.len(), 1)])
        } else {
            rng.pick(&reachable_hubs).copied()
        };
        if let Some(id) = pick
            && !chosen.contains(&id)
        {
            chosen.push(id);
        }
    }
    if chosen.is_empty()
        && let Some(&id) = previous.first().or(reachable_hubs.first())
    {
        chosen.push(id);
    }
    chosen.sort_unstable();
    chosen
}

fn callable(corpus: &Corpus, modules: &[Module], module: usize, own: &[DefId]) -> Vec<DefId> {
    let mut out: Vec<DefId> = own.to_vec();
    for &imported in &modules[module].imports {
        out.extend(corpus.modules[imported].defs.iter().copied());
    }
    out
}

fn generate_module_defs(
    spec: &CorpusSpec,
    root: &Rng,
    corpus: &mut Corpus,
    modules: &[Module],
    module: usize,
) -> Vec<DefId> {
    let mut own: Vec<DefId> = Vec::with_capacity(spec.defs_per_module);

    for index in 0..spec.defs_per_module {
        let id = corpus.defs.len();
        let mut rng = root.fork(0x2000_0000 ^ ((module as u64) << 24) ^ index as u64);
        let pool = callable(corpus, modules, module, &own);

        let arity = if rng.chance(0.12) { 2 } else { 1 };
        let shape = choose_shape(spec, &mut rng, corpus, &pool, arity);
        let extras = choose_extras(spec, &mut rng, corpus, &pool, &shape);

        let mut footprint = shape.own_atoms(&corpus.tables, &corpus.regions);
        let mut weight = 1u32;
        for call in shape.calls().into_iter().chain(extras.iter().copied()) {
            let callee = &corpus.defs[call.target];
            footprint.extend(callee.footprint.iter().cloned());
            weight = weight.saturating_add(callee.weight);
        }

        corpus.defs.push(Def {
            id,
            module,
            name: format!("{}_{id}", VERBS[index % VERBS.len()]),
            arity,
            shape,
            extras,
            footprint,
            weight,
            public: false,
        });
        own.push(id);
    }
    own
}

fn choose_shape(
    spec: &CorpusSpec,
    rng: &mut Rng,
    corpus: &Corpus,
    pool: &[DefId],
    arity: usize,
) -> Shape {
    let budget = spec.max_weight;
    let one = |rng: &mut Rng| affordable(rng, corpus, pool, budget, 1).map(|c| c[0]);
    let two = |rng: &mut Rng| affordable(rng, corpus, pool, budget, 2);

    if rng.chance(spec.effect_fraction) {
        let pick = rng.below(100);
        return match pick {
            0..=29 => {
                let table = rng.below(corpus.tables.len());
                match one(rng) {
                    Some(f) => Shape::TableCount {
                        table,
                        a: rng.between(2, 9),
                        f,
                    },
                    None => Shape::TableSum {
                        table,
                        a: rng.between(2, 9),
                    },
                }
            }
            30..=49 => Shape::TableSum {
                table: rng.below(corpus.tables.len()),
                a: rng.between(2, 9),
            },
            50..=74 => Shape::TableAppend {
                table: rng.below(corpus.tables.len()),
                a: rng.between(2, 9),
                b: rng.between(2, 9),
            },
            75..=84 => Shape::CachePeek {
                region: rng.below(corpus.regions.len()),
                a: rng.between(2, 9),
            },
            85..=96 => Shape::CachePoke {
                region: rng.below(corpus.regions.len()),
                a: rng.between(2, 9),
            },
            _ => Shape::Now {
                a: rng.between(3, 97),
            },
        };
    }

    if arity >= 2 {
        return match one(rng) {
            Some(f) => Shape::Pair {
                f,
                a: rng.between(2, 9),
                b: rng.between(0, 40),
            },
            None => Shape::Arith {
                a: rng.between(2, 9),
                b: rng.between(0, 40),
            },
        };
    }

    match rng.below(100) {
        0..=17 => Shape::Arith {
            a: rng.between(2, 9),
            b: rng.between(0, 40),
        },
        18..=29 => Shape::Fold {
            n: rng.between(2, 8),
            k: rng.between(2, 9),
        },
        30..=39 => Shape::Record {
            m: rng.between(2, 9),
            k: rng.between(2, 9),
        },
        40..=49 => Shape::ListMap {
            n: rng.between(2, 8),
            k: rng.between(2, 9),
        },
        50..=63 => match one(rng) {
            Some(f) => Shape::Guard {
                m: rng.between(2, 6),
                f,
                b: rng.between(0, 40),
            },
            None => Shape::Arith {
                a: rng.between(2, 9),
                b: rng.between(0, 40),
            },
        },
        64..=75 => match one(rng) {
            Some(f) => Shape::Sum {
                off: rng.between(0, 20),
                f,
                idle: rng.between(0, 90),
            },
            None => Shape::Fold {
                n: rng.between(2, 8),
                k: rng.between(2, 9),
            },
        },
        76..=88 => match two(rng) {
            Some(pair) => Shape::Compose {
                f: pair[0],
                g: pair[1],
            },
            None => Shape::Arith {
                a: rng.between(2, 9),
                b: rng.between(0, 40),
            },
        },
        _ => match two(rng) {
            Some(pair) => Shape::Chain {
                inner: pair[0],
                outer: pair[1].target,
                b: rng.between(0, 30),
            },
            None => Shape::ListMap {
                n: rng.between(2, 8),
                k: rng.between(2, 9),
            },
        },
    }
}

/// Picks `n` distinct callees whose combined weight leaves room under `budget`,
/// preferring the front of the pool so hubs form. `None` when nothing fits.
fn affordable(
    rng: &mut Rng,
    corpus: &Corpus,
    pool: &[DefId],
    budget: u32,
    n: usize,
) -> Option<Vec<Call>> {
    if pool.is_empty() {
        return None;
    }
    let mut chosen: Vec<Call> = Vec::with_capacity(n);
    let mut used = 1u32;
    for _ in 0..n {
        let mut best: Option<DefId> = None;
        for _ in 0..4 {
            let candidate = pool[rng.skewed_below(pool.len(), 2)];
            if chosen.iter().any(|c| c.target == candidate) {
                continue;
            }
            let weight = corpus.defs[candidate].weight;
            if used.saturating_add(weight) > budget {
                continue;
            }
            match best {
                Some(current) if corpus.defs[current].weight <= weight => {}
                _ => best = Some(candidate),
            }
        }
        let target = best?;
        used = used.saturating_add(corpus.defs[target].weight);
        chosen.push(Call {
            target,
            offset: rng.between(0, 30),
        });
    }
    Some(chosen)
}

fn choose_extras(
    spec: &CorpusSpec,
    rng: &mut Rng,
    corpus: &Corpus,
    pool: &[DefId],
    shape: &Shape,
) -> Vec<Call> {
    if pool.is_empty() {
        return Vec::new();
    }
    let mut used = 1u32;
    for call in shape.calls() {
        used = used.saturating_add(corpus.defs[call.target].weight);
    }

    let wanted = match rng.below(100) {
        0..=19 => 0,
        20..=54 => 1,
        55..=84 => 2,
        _ => MAX_EXTRAS,
    };

    let mut extras: Vec<Call> = Vec::new();
    for _ in 0..wanted {
        let candidate = pool[rng.skewed_below(pool.len(), 2)];
        if shape.calls().iter().any(|c| c.target == candidate)
            || extras.iter().any(|c| c.target == candidate)
        {
            continue;
        }
        let weight = corpus.defs[candidate].weight;
        if used.saturating_add(weight) > spec.max_weight {
            continue;
        }
        used = used.saturating_add(weight);
        extras.push(Call {
            target: candidate,
            offset: rng.between(0, 30),
        });
    }
    extras
}

/// `pub` is not decoration: a definition is exported exactly when another module
/// reaches it, so removing an import removes an export and the corpus keeps
/// being a real test of visibility.
fn mark_public(corpus: &mut Corpus) {
    let mut exported = vec![false; corpus.defs.len()];
    for def in &corpus.defs {
        for call in def
            .shape
            .calls()
            .into_iter()
            .chain(def.extras.iter().copied())
        {
            if corpus.defs[call.target].module != def.module {
                exported[call.target] = true;
            }
        }
    }
    for (def, is_public) in corpus.defs.iter_mut().zip(exported) {
        def.public = is_public;
    }
}

fn generate_tests(spec: &CorpusSpec, root: &Rng, corpus: &Corpus) -> Vec<Test> {
    let mut tests = Vec::with_capacity(spec.tests);
    if corpus.defs.is_empty() {
        return tests;
    }

    for index in 0..spec.tests {
        let module = index % corpus.modules.len();
        let defs = &corpus.modules[module].defs;
        if defs.is_empty() {
            continue;
        }
        let mut rng = root.fork(0x3000_0000 + index as u64);
        let root_def = defs[(index / corpus.modules.len() + rng.below(defs.len())) % defs.len()];
        tests.push(build_test(spec, &mut rng, corpus, module, root_def, index));
    }
    tests
}

fn build_test(
    spec: &CorpusSpec,
    rng: &mut Rng,
    corpus: &Corpus,
    module: ModuleId,
    root: DefId,
    index: usize,
) -> Test {
    let footprint = &corpus.defs[root].footprint;

    let mut world = World {
        clock: 1_700_000_000 + rng.between(0, 5_000),
        ..World::default()
    };
    for atom in footprint {
        match (atom.effect, &atom.resource) {
            (Eff::Db, Some(label)) => {
                let table = index_of(&corpus.tables, label);
                if !world.tables.iter().any(|(i, _)| *i == table) {
                    let rows: Vec<i64> = (0..rng.between(1, 4))
                        .map(|k| rng.between(1, 90) + k)
                        .collect();
                    world.tables.push((table, rows));
                }
            }
            (Eff::Cache, Some(label)) => {
                let region = index_of(&corpus.regions, label);
                if !world.regions.iter().any(|(i, _)| *i == region) {
                    world.regions.push((region, rng.between(1, 90)));
                }
            }
            _ => {}
        }
    }
    world.tables.sort_by_key(|(i, _)| *i);
    world.regions.sort_by_key(|(i, _)| *i);

    let seeded = world.clone();
    let arity = corpus.defs[root].arity;
    let call_count = rng.between(1, 2) as usize;
    let mut calls = Vec::with_capacity(call_count);
    let mut expected = Vec::with_capacity(call_count);
    let mut live = world.clone();
    for _ in 0..call_count {
        let args: Vec<i64> = (0..arity).map(|_| rng.between(1, 60)).collect();
        expected.push(corpus.eval(root, &args, &mut live));
        calls.push(args);
    }

    let granted = live.touched.clone();
    let written_tables: Vec<usize> = granted
        .iter()
        .filter(|a| a.write && a.effect == Eff::Db)
        .filter_map(|a| a.resource.as_ref())
        .map(|label| index_of(&corpus.tables, label))
        .collect();
    let written_regions: Vec<usize> = granted
        .iter()
        .filter(|a| a.write && a.effect == Eff::Cache)
        .filter_map(|a| a.resource.as_ref())
        .map(|label| index_of(&corpus.regions, label))
        .collect();

    let final_table_len = written_tables
        .iter()
        .map(|&t| (t, live.table(t).len()))
        .collect::<Vec<_>>();
    let final_region = written_regions
        .iter()
        .map(|&r| (r, live.region(r)))
        .collect::<Vec<_>>();

    // A clock atom that no handler discharges survives into the test's own
    // footprint, and a `det` test may not carry one.
    let undischarged_clock = footprint
        .iter()
        .any(|a| a.effect == Eff::Clock && !granted.contains(a));
    let nondet = undischarged_clock || rng.chance(spec.nondet_fraction);

    Test {
        module,
        label: format!(
            "{} {} (case {index})",
            corpus.defs[root].name,
            phrase(rng, &corpus.defs[root].shape)
        ),
        nondet,
        root,
        calls,
        expected,
        world: seeded,
        granted,
        final_table_len,
        final_region,
    }
}

/// Concurrency, generated at a chosen conflict density.
///
/// The shape is fixed and only the contention varies, because a measurement of
/// how exploration scales with contention has to hold everything else still:
/// same task count, same step count, same arithmetic, and the assignment of
/// tasks to shards as the only independent variable. At density 0 no two steps
/// conflict and the search should collapse to one interleaving; at density 1
/// every pair conflicts and there is nothing to prune.
fn generate_concurrent_tests(
    spec: &CorpusSpec,
    root: &Rng,
    corpus: &Corpus,
) -> Vec<ConcurrentTest> {
    let mut out = Vec::with_capacity(spec.concurrent_tests);
    // One task is a sequential program with a `simulate` around it, and a
    // corpus of those measures nothing. `validate` refuses the spec; this is
    // what keeps `generate` alone from emitting a test with no tasks in it.
    if spec.tasks_per_test < 2 || corpus.modules.is_empty() || corpus.shards.is_empty() {
        return out;
    }
    let shard_count = spec.shards_per_test().min(corpus.shards.len());

    for index in 0..spec.concurrent_tests {
        let module = index % corpus.modules.len();
        let mut rng = root.fork(0x4000_0000 + index as u64);

        let tasks: Vec<TaskBody> = (0..spec.tasks_per_test)
            .map(|i| TaskBody {
                name: format!("worker_{index}_{i}"),
                shard: i % shard_count,
                steps: (0..spec.steps_per_task)
                    .map(|_| rng.between(1, 9))
                    .collect(),
            })
            .collect();

        let mut shards: Vec<usize> = tasks.iter().map(|t| t.shard).collect();
        shards.sort_unstable();
        shards.dedup();

        out.push(ConcurrentTest {
            module,
            label: format!(
                "{} tasks over {} shard{} (case {index})",
                tasks.len(),
                shards.len(),
                if shards.len() == 1 { "" } else { "s" }
            ),
            tasks,
            shards,
        });
    }
    out
}

fn phrase(rng: &mut Rng, shape: &Shape) -> &'static str {
    let generic = [
        "agrees with its worked example",
        "holds for a seeded fixture",
        "returns the figure the spec names",
    ];
    match shape {
        Shape::TableAppend { .. } => "appends exactly one row",
        Shape::TableCount { .. } | Shape::TableSum { .. } => "reads the table it declares",
        Shape::CachePoke { .. } => "writes the region back",
        Shape::CachePeek { .. } => "reads the region it declares",
        Shape::Now { .. } => "is stable under a pinned clock",
        _ => generic[rng.below(generic.len())],
    }
}

fn index_of(labels: &[String], label: &str) -> usize {
    labels.iter().position(|l| l == label).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn small() -> CorpusSpec {
        CorpusSpec {
            seed: 5,
            modules: 8,
            defs_per_module: 12,
            tests: 24,
            depth: 3,
            ..CorpusSpec::default()
        }
    }

    fn concurrent(density: f64) -> CorpusSpec {
        CorpusSpec {
            concurrent_tests: 6,
            tasks_per_test: 4,
            steps_per_task: 3,
            conflict_density: density,
            ..small()
        }
    }

    #[test]
    fn no_concurrency_is_asked_for_by_default_and_none_is_generated() {
        assert_eq!(CorpusSpec::default().concurrent_tests, 0);
        assert!(generate(&small()).concurrent.is_empty());
    }

    /// The knob's two ends, which are the two ends of the claim: at 0.0 the
    /// dependence relation has nothing to relate and the search should collapse
    /// to one interleaving, at 1.0 it relates everything and there is nothing to
    /// prune. Anything in between is an interpolation of these.
    #[test]
    fn density_zero_gives_every_task_its_own_shard_and_density_one_gives_them_all_the_same() {
        let disjoint = generate(&concurrent(0.0));
        for test in &disjoint.concurrent {
            let shards: BTreeSet<usize> = test.tasks.iter().map(|t| t.shard).collect();
            assert_eq!(shards.len(), test.tasks.len(), "`{}`", test.label);
            assert_eq!(test.contention(), 0.0);
        }

        let contended = generate(&concurrent(1.0));
        for test in &contended.concurrent {
            let shards: BTreeSet<usize> = test.tasks.iter().map(|t| t.shard).collect();
            assert_eq!(shards.len(), 1, "`{}`", test.label);
            assert_eq!(test.contention(), 1.0);
        }
    }

    #[test]
    fn contention_rises_with_the_density_asked_for() {
        let mean = |d: f64| {
            let corpus = generate(&concurrent(d));
            corpus
                .concurrent
                .iter()
                .map(|t| t.contention())
                .sum::<f64>()
                / corpus.concurrent.len() as f64
        };
        let (low, mid, high) = (mean(0.0), mean(0.5), mean(1.0));
        assert!(low < mid && mid < high, "{low} {mid} {high}");
    }

    /// Only the shard assignment may move with the density, or a measurement
    /// across densities is comparing two different programs and attributing the
    /// difference to contention.
    #[test]
    fn density_changes_the_shard_assignment_and_nothing_else() {
        let disjoint = generate(&concurrent(0.0));
        let contended = generate(&concurrent(1.0));
        assert_eq!(disjoint.concurrent.len(), contended.concurrent.len());
        for (a, b) in disjoint.concurrent.iter().zip(&contended.concurrent) {
            assert_eq!(a.module, b.module);
            assert_eq!(a.total(), b.total());
            for (x, y) in a.tasks.iter().zip(&b.tasks) {
                assert_eq!(x.name, y.name);
                assert_eq!(x.steps, y.steps);
            }
        }
    }

    #[test]
    fn a_module_hosting_a_concurrent_test_imports_the_effect_it_performs() {
        let corpus = generate(&concurrent(0.5));
        assert!(!corpus.concurrent.is_empty());
        for test in &corpus.concurrent {
            assert!(
                corpus.modules[test.module].needs_effects,
                "module {} hosts `{}` and would not import `core.effects`",
                corpus.modules[test.module].name, test.label
            );
        }
    }

    #[test]
    fn a_task_body_is_named_apart_from_every_generated_definition() {
        let corpus = generate(&concurrent(0.5));
        let defs: BTreeSet<&str> = corpus.defs.iter().map(|d| d.name.as_str()).collect();
        let mut workers = BTreeSet::new();
        for test in &corpus.concurrent {
            for task in &test.tasks {
                assert!(!defs.contains(task.name.as_str()), "{}", task.name);
                assert!(workers.insert(task.name.clone()), "{}", task.name);
            }
        }
        assert_eq!(workers.len(), 6 * 4);
    }

    #[test]
    fn the_same_seed_produces_the_same_concurrent_tests() {
        let a = generate(&concurrent(0.5));
        let b = generate(&concurrent(0.5));
        for (x, y) in a.concurrent.iter().zip(&b.concurrent) {
            assert_eq!(x.label, y.label);
            assert_eq!(x.total(), y.total());
            assert_eq!(
                x.tasks.iter().map(|t| t.steps.clone()).collect::<Vec<_>>(),
                y.tasks.iter().map(|t| t.steps.clone()).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn the_same_seed_produces_the_same_corpus() {
        let a = generate(&small());
        let b = generate(&small());
        assert_eq!(a.defs.len(), b.defs.len());
        for (x, y) in a.defs.iter().zip(&b.defs) {
            assert_eq!(x.name, y.name);
            assert_eq!(format!("{:?}", x.shape), format!("{:?}", y.shape));
            assert_eq!(x.footprint, y.footprint);
        }
        for (x, y) in a.tests.iter().zip(&b.tests) {
            assert_eq!(x.label, y.label);
            assert_eq!(x.expected, y.expected);
        }
    }

    #[test]
    fn a_different_seed_produces_a_different_corpus() {
        let a = generate(&small());
        let b = generate(&CorpusSpec { seed: 6, ..small() });
        let same = a
            .defs
            .iter()
            .zip(&b.defs)
            .filter(|(x, y)| format!("{:?}", x.shape) == format!("{:?}", y.shape))
            .count();
        assert!(
            same < a.defs.len(),
            "seed 6 reproduced every shape of seed 5"
        );
    }

    #[test]
    fn every_call_points_backwards_so_the_graph_is_acyclic() {
        let corpus = generate(&small());
        for def in &corpus.defs {
            for call in def
                .shape
                .calls()
                .into_iter()
                .chain(def.extras.iter().copied())
            {
                assert!(call.target < def.id, "{} calls forward", def.name);
            }
        }
    }

    #[test]
    fn a_module_only_imports_from_a_lower_layer() {
        let corpus = generate(&small());
        for module in &corpus.modules {
            for &imported in &module.imports {
                assert!(corpus.modules[imported].layer < module.layer);
            }
        }
    }

    #[test]
    fn a_footprint_contains_everything_its_callees_perform() {
        let corpus = generate(&small());
        for def in &corpus.defs {
            for call in def
                .shape
                .calls()
                .into_iter()
                .chain(def.extras.iter().copied())
            {
                for atom in &corpus.defs[call.target].footprint {
                    assert!(
                        def.footprint.contains(atom),
                        "{} lost {} from {}",
                        def.name,
                        atom.render(),
                        corpus.defs[call.target].name
                    );
                }
            }
        }
    }

    #[test]
    fn no_definition_exceeds_the_weight_cap() {
        let spec = small();
        let corpus = generate(&spec);
        for def in &corpus.defs {
            assert!(
                def.weight <= spec.max_weight,
                "{} weighs {}",
                def.name,
                def.weight
            );
        }
    }

    #[test]
    fn a_definition_is_public_exactly_when_another_module_calls_it() {
        let corpus = generate(&small());
        let mut reached = vec![false; corpus.defs.len()];
        for def in &corpus.defs {
            for call in def
                .shape
                .calls()
                .into_iter()
                .chain(def.extras.iter().copied())
            {
                if corpus.defs[call.target].module != def.module {
                    reached[call.target] = true;
                }
            }
        }
        for (def, expected) in corpus.defs.iter().zip(reached) {
            assert_eq!(
                def.public, expected,
                "{} has the wrong visibility",
                def.name
            );
        }
    }

    #[test]
    fn the_conflict_graph_is_not_all_pure_and_not_one_clique() {
        let corpus = generate(&CorpusSpec {
            tests: 60,
            ..small()
        });
        let effectful = corpus
            .tests
            .iter()
            .filter(|t| !corpus.defs[t.root].footprint.is_empty());
        assert!(
            effectful.count() > 5,
            "a corpus with no effectful tests proves nothing"
        );

        let writers: Vec<&Test> = corpus
            .tests
            .iter()
            .filter(|t| corpus.defs[t.root].footprint.iter().any(|a| a.write))
            .collect();
        assert!(writers.len() > 2, "no test writes anything");
        let distinct: std::collections::BTreeSet<_> = writers
            .iter()
            .flat_map(|t| corpus.defs[t.root].footprint.iter().filter(|a| a.write))
            .map(|a| a.resource.clone())
            .collect();
        assert!(distinct.len() > 1, "every writer touches the same resource");
    }
}
