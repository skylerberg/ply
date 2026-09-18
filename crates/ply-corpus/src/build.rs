//! Turning a [`CorpusSpec`] and a seed into a [`Corpus`].

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
        specimens: Vec::new(),
        laws: Vec::new(),
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
    attach_claims(spec, &root, &mut corpus);
    corpus.specimens = generate_specimens(spec, &corpus);
    corpus.laws = generate_laws(&root, &corpus);
    corpus.tests = generate_tests(spec, &root, &corpus);
    corpus.concurrent = generate_concurrent_tests(spec, &root, &corpus);
    for test in &corpus.concurrent {
        corpus.modules[test.module].needs_effects = true;
    }
    corpus
}

/// `count` labels: the vocabulary first, then numbered extensions of it.
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

/// Imports run strictly downward through the layers, so the module graph is acyclic.
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
            claim: None,
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

/// Picks `n` distinct callees fitting `budget`, preferring the pool's front so hubs form.
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

/// A definition is `pub` exactly when another module reaches it, so visibility stays tested.
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

/// Claims are keyed per definition, so raising the density adds claims without moving others.
fn attach_claims(spec: &CorpusSpec, root: &Rng, corpus: &mut Corpus) {
    if spec.spec_fraction <= 0.0 {
        return;
    }
    for def in &mut corpus.defs {
        let mut rng = root.fork(0x5000_0000 + def.id as u64);
        if !rng.chance(spec.spec_fraction) {
            continue;
        }
        let intent = if def.footprint.is_empty() {
            Intent::Sampled
        } else {
            Intent::Gap
        };
        def.claim = Some(Claim { intent });
    }
}

/// One specimen per index, cycling the three shapes.
fn generate_specimens(spec: &CorpusSpec, corpus: &Corpus) -> Vec<Specimen> {
    let mut out = Vec::new();
    for module in &corpus.modules {
        for index in 0..spec.specimens_per_module {
            let id = out.len();
            let kind = match index % 3 {
                0 => SpecimenKind::Linear {
                    a: 2 + ((module.id + index) % 7) as i64,
                    b: 1 + ((id * 7) % 40) as i64,
                },
                1 => SpecimenKind::Status,
                _ => SpecimenKind::Length,
            };
            out.push(Specimen {
                id,
                module: module.id,
                name: format!("{}_{id}", specimen_verb(kind)),
                kind,
            });
        }
    }
    out
}

fn specimen_verb(kind: SpecimenKind) -> &'static str {
    match kind {
        SpecimenKind::Linear { .. } => "quote",
        SpecimenKind::Status => "grade",
        SpecimenKind::Length => "depth",
    }
}

/// One law per specimen.
fn generate_laws(root: &Rng, corpus: &Corpus) -> Vec<Law> {
    let mut out = Vec::with_capacity(corpus.specimens.len());
    for specimen in &corpus.specimens {
        let mut rng = root.fork(0x6000_0000 + specimen.id as u64);
        let kind = match specimen.kind {
            SpecimenKind::Linear { .. } => LawKind::Ground {
                a: rng.between(2, 40),
                b: rng.between(1, 90),
            },
            SpecimenKind::Status => LawKind::Finite,
            SpecimenKind::Length => LawKind::Length {
                specimen: specimen.id,
            },
        };
        out.push(Law {
            module: specimen.module,
            label: format!("{} (case {})", law_phrase(kind), specimen.id),
            kind,
        });
    }
    out
}

fn law_phrase(kind: LawKind) -> &'static str {
    match kind {
        LawKind::Ground { .. } => "the arithmetic closes without running anything",
        LawKind::Finite => "either flag settles the pair",
        LawKind::Length { .. } => "a walk down a list never comes back negative",
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

    // An undischarged clock atom stays in the test's footprint, which a `det` test may not carry.
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
fn generate_concurrent_tests(
    spec: &CorpusSpec,
    root: &Rng,
    corpus: &Corpus,
) -> Vec<ConcurrentTest> {
    let mut out = Vec::with_capacity(spec.concurrent_tests);
    // One task is a sequential program, which measures nothing.
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
