//! [`Corpus`] to Ply source.

use crate::model::*;
use std::fmt::Write;

/// The two modules every generated one leans on.
pub const EFFECTS_PATH: &str = "core/effects.ply";
pub const PRIM_PATH: &str = "core/prim.ply";

pub const EFFECTS_SOURCE: &str = r#"// The capabilities the whole corpus is written against. Resource labels are
// shared program-wide on purpose: conflict is a claim about the world, so two
// modules writing `[orders]` must serialize even though nothing links them.

pub effect db {
  read  all[table]() -> List<Int>
  write save[table](rows: List<Int>) -> Unit
}

pub effect cache {
  read  peek[region]() -> Int
  write poke[region](value: Int) -> Unit
}

pub nondet effect clock {
  read now() -> Int
}

// One operation, so a handler's read-modify-write of the backing cell is one
// step and no schedule can split it. What a concurrent test then measures is the
// *order* its tasks ran in, which is the only thing left to explore.
pub effect counter {
  write bump[shard](by: Int) -> Unit
}
"#;

pub const PRIM_SOURCE: &str = r#"// Arithmetic every generated definition funnels through. `clamp` is what keeps
// an intermediate inside `Int` no matter how deep the call graph goes.

pub type Weighted = { id: Int, weight: Int }

pub fn clamp(x: Int) -> Int = x % 100003

pub fn mix(x: Int, y: Int) -> Int = clamp(x * 31 + y * 17 + 7)

pub fn total(xs: List<Int>) -> Int = fold(xs, 0, |acc, v| clamp(acc + v))

pub fn weigh(w: Weighted) -> Int = clamp(w.id * 11 + w.weight)
"#;

pub struct Emitted {
    pub path: String,
    pub text: String,
}

pub fn emit(corpus: &Corpus) -> Vec<Emitted> {
    let mut out = vec![
        Emitted {
            path: EFFECTS_PATH.to_string(),
            text: EFFECTS_SOURCE.to_string(),
        },
        Emitted {
            path: PRIM_PATH.to_string(),
            text: PRIM_SOURCE.to_string(),
        },
    ];
    for module in &corpus.modules {
        out.push(Emitted {
            path: module.path.clone(),
            text: emit_module(corpus, module),
        });
    }
    out
}

pub fn emit_module(corpus: &Corpus, module: &Module) -> String {
    let mut s = String::with_capacity(4096);

    let _ = writeln!(
        s,
        "// Layer {} of the corpus. {} definitions, {} imported module(s).",
        module.layer,
        module.defs.len(),
        module.imports.len()
    );
    s.push('\n');

    s.push_str("import core.prim\n");
    if module.needs_effects {
        s.push_str("import core.effects\n");
    }
    for &imported in &module.imports {
        let _ = writeln!(s, "import {}", corpus.modules[imported].name);
    }
    s.push('\n');

    let _ = writeln!(
        s,
        "type {} =\n  | {}(Int)\n  | {}\n",
        module.status_type, module.ctor_ready, module.ctor_idle
    );

    let _ = writeln!(
        s,
        "fn {}(x: Int) -> {} =\n  if x % {} == 0 {{ {} }} else {{ {}(prim::clamp(x + {})) }}\n",
        module.helper.name,
        module.status_type,
        module.helper.m,
        module.ctor_idle,
        module.ctor_ready,
        module.helper.b
    );

    for &id in &module.defs {
        s.push_str(&emit_def(corpus, &corpus.defs[id]));
        s.push('\n');
    }

    for specimen in corpus.specimens_in(module.id) {
        s.push_str(&emit_specimen(corpus, specimen));
        s.push('\n');
    }

    for law in corpus.laws_in(module.id) {
        s.push_str(&emit_law(corpus, law));
        s.push('\n');
    }

    if corpus.specimens_in(module.id).next().is_some() {
        s.push_str(&emit_specimen_test(corpus, module));
        s.push('\n');
    }

    for test in corpus.tests.iter().filter(|t| t.module == module.id) {
        s.push_str(&emit_test(corpus, test));
        s.push('\n');
    }

    for test in corpus.concurrent_in(module.id) {
        for task in &test.tasks {
            s.push_str(&emit_task(corpus, task));
            s.push('\n');
        }
        s.push_str(&emit_concurrent_test(corpus, test));
        s.push('\n');
    }
    s
}

fn params(arity: usize) -> &'static str {
    if arity >= 2 {
        "x: Int, y: Int"
    } else {
        "x: Int"
    }
}

fn row(footprint: &Footprint) -> String {
    if footprint.is_empty() {
        return String::new();
    }
    let atoms: Vec<String> = footprint.iter().map(Atom::render).collect();
    format!(" / {{{}}}", atoms.join(", "))
}

/// The largest argument a specified definition's precondition admits.
pub const ARG_BOUND: i64 = 1000;

/// The `requires`/`ensures` pair a claim renders to, or nothing.
fn clauses(def: &Def) -> String {
    if def.claim.is_none() {
        return String::new();
    }
    let mut domain = format!("x > 0 && x < {ARG_BOUND}");
    if def.arity >= 2 {
        domain.push_str(&format!(" && y > 0 && y < {ARG_BOUND}"));
    }
    format!("\n  requires {domain}\n  ensures result < {CLAMP} && result > -{CLAMP}")
}

pub fn emit_def(corpus: &Corpus, def: &Def) -> String {
    let mut s = String::with_capacity(256);
    let visibility = if def.public { "pub " } else { "" };
    let head = format!(
        "{visibility}fn {}({}) -> Int{}{}",
        def.name,
        params(def.arity),
        row(&def.footprint),
        clauses(def)
    );

    let here = def.module;
    match &def.shape {
        Shape::TableAppend { table, a, b } => {
            let t = &corpus.tables[*table];
            let _ = writeln!(s, "{head} {{");
            let _ = writeln!(s, "  let rows = effects::db.all[{t}]();");
            let _ = writeln!(
                s,
                "  effects::db.save[{t}](push(rows, prim::clamp(x * {a})));"
            );
            let core = format!("prim::clamp(len(rows) * {b} + x)");
            let _ = writeln!(s, "  {}", combine(corpus, def, &core));
            s.push_str("}\n");
        }
        Shape::CachePoke { region, a } => {
            let r = &corpus.regions[*region];
            let _ = writeln!(s, "{head} {{");
            let _ = writeln!(s, "  let seen = effects::cache.peek[{r}]();");
            let _ = writeln!(s, "  effects::cache.poke[{r}](prim::clamp(seen + x));");
            let core = format!("prim::clamp(seen * {a} + x)");
            let _ = writeln!(s, "  {}", combine(corpus, def, &core));
            s.push_str("}\n");
        }
        // A `match` cannot be spliced into an argument list, so a `Sum` with extras is rebuilt as a
        // block whose `let` holds the arms.
        Shape::Sum { off, f, idle } => {
            let module = &corpus.modules[here];
            let arms = format!(
                "match {}(x + {off}) {{\n  {}(v) -> {},\n  {} -> {idle},\n}}",
                module.helper.name,
                module.ctor_ready,
                call_expr(corpus, here, *f, "v"),
                module.ctor_idle
            );
            if def.extras.is_empty() {
                let _ = writeln!(s, "{}", assign(&head));
                let _ = writeln!(s, "{}", indent(&arms, 2));
            } else {
                let _ = writeln!(s, "{head} {{");
                let _ = writeln!(s, "  let core = {};", indent_tail(&arms, 2));
                let _ = writeln!(s, "  {}", combine(corpus, def, "core"));
                s.push_str("}\n");
            }
        }
        other => {
            let _ = writeln!(s, "{}", assign(&head));
            let _ = writeln!(
                s,
                "  {}",
                combine(corpus, def, &core_expr(corpus, def, other))
            );
        }
    }
    s
}

/// A spec clause ends a line, so the `=` that starts the body goes on the next one rather than
/// trailing an `ensures`.
fn assign(head: &str) -> String {
    if head.contains('\n') {
        format!("{head}\n=")
    } else {
        format!("{head} =")
    }
}

fn combine(corpus: &Corpus, def: &Def, core: &str) -> String {
    if def.extras.is_empty() {
        return core.to_string();
    }
    let sum: Vec<String> = def
        .extras
        .iter()
        .map(|call| call_expr(corpus, def.module, *call, "x"))
        .collect();
    format!("prim::mix({core}, {})", sum.join(" + "))
}

fn core_expr(corpus: &Corpus, def: &Def, shape: &Shape) -> String {
    let here = def.module;
    match shape {
        Shape::Arith { a, b } => format!("prim::clamp(x * {a} + {b})"),
        Shape::Compose { f, g } => format!(
            "prim::mix({}, {})",
            call_expr(corpus, here, *f, "x"),
            call_expr(corpus, here, *g, "x")
        ),
        Shape::Guard { m, f, b } => format!(
            "if x % {m} == 0 {{ {} }} else {{ prim::clamp(x + {b}) }}",
            call_expr(corpus, here, *f, "x")
        ),
        Shape::Fold { n, k } => {
            format!("fold(range(0, {n}), 0, |acc, v| prim::clamp(acc + v * {k} + x))")
        }
        Shape::Record { m, k } => {
            format!("prim::weigh({{id: x % {m}, weight: prim::clamp(x * {k})}})")
        }
        Shape::Chain { inner, outer, b } => {
            let arg = format!("{} + {b}", call_expr(corpus, here, *inner, "x"));
            call_expr(
                corpus,
                here,
                Call {
                    target: *outer,
                    offset: 0,
                },
                &arg,
            )
        }
        Shape::ListMap { n, k } => {
            format!("prim::total(map(range(0, {n}), |v| prim::clamp(v * {k} + x)))")
        }
        Shape::Pair { f, a, b } => {
            format!(
                "prim::mix({}, y * {a} + {b})",
                call_expr(corpus, here, *f, "x")
            )
        }
        Shape::TableCount { table, a, f } => format!(
            "prim::clamp(len(effects::db.all[{}]()) * {a} + {})",
            corpus.tables[*table],
            call_expr(corpus, here, *f, "x")
        ),
        Shape::TableSum { table, a } => format!(
            "prim::clamp(prim::total(effects::db.all[{}]()) + x * {a})",
            corpus.tables[*table]
        ),
        Shape::CachePeek { region, a } => {
            format!(
                "prim::clamp(effects::cache.peek[{}]() + x * {a})",
                corpus.regions[*region]
            )
        }
        Shape::Now { a } => format!("prim::clamp(effects::clock.now() % {a} + x)"),
        Shape::Sum { .. } | Shape::TableAppend { .. } | Shape::CachePoke { .. } => {
            unreachable!("block-shaped bodies are emitted by `emit_def`")
        }
    }
    .to_string()
}

/// `base` is the caller's expression for the callee's first argument, before the call's own offset
/// is added.
pub fn call_expr(corpus: &Corpus, here: ModuleId, call: Call, base: &str) -> String {
    let callee = &corpus.defs[call.target];
    let qualifier = qualify(corpus, here, call.target);
    let first = if call.offset == 0 {
        base.to_string()
    } else {
        format!("{base} + {}", call.offset)
    };
    if callee.arity >= 2 {
        format!(
            "{qualifier}{}({first}, {})",
            callee.name,
            second_arg(call.offset)
        )
    } else {
        format!("{qualifier}{}({first})", callee.name)
    }
}

/// A module has no binder for itself, so a same-module reference must be bare; every other one is
/// `m::x` and so cannot be captured by a local binder.
fn qualify(corpus: &Corpus, here: ModuleId, target: DefId) -> String {
    let owner = corpus.defs[target].module;
    if owner == here {
        return String::new();
    }
    format!("{}::", corpus.modules[owner].binder())
}

fn emit_test(corpus: &Corpus, test: &Test) -> String {
    let keyword = if test.nondet { "test/nondet" } else { "test" };
    let mut body = Vec::new();

    for (args, expected) in test.calls.iter().zip(&test.expected) {
        let call = format!(
            "{}({})",
            corpus.defs[test.root].name,
            args.iter()
                .map(|a| a.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
        body.push(format!("assert_eq({call}, {expected})"));
    }
    for (table, len) in &test.final_table_len {
        body.push(format!("assert_eq(len(cell_get(t{table})), {len})"));
    }
    for (region, value) in &test.final_region {
        body.push(format!("assert_eq(cell_get(r{region}), {value})"));
    }

    let mut inner = body.join(";\n");

    if !test.granted.is_empty() {
        let clauses = handler_clauses(corpus, test, &test.granted);
        inner = format!(
            "handle {{\n{}\n}} with {{\n{}\n}}",
            indent(&inner, 2),
            clauses
                .iter()
                .map(|c| format!("  {c},"))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    for (table, _) in test.final_table_len.iter().rev() {
        let rows = literal_list(test.world.table(*table));
        inner = format!(
            "with_cell[{}]({rows}) {{ t{table} ->\n{}\n}}",
            corpus.tables[*table],
            indent(&inner, 2)
        );
    }
    for (region, _) in test.final_region.iter().rev() {
        inner = format!(
            "with_cell[{}]({}) {{ r{region} ->\n{}\n}}",
            corpus.regions[*region],
            test.world.region(*region),
            indent(&inner, 2)
        );
    }

    format!(
        "{keyword} \"{}\" {{\n{}\n}}\n",
        escape(&test.label),
        indent(&inner, 2)
    )
}

/// A definition written for its obligation.
pub fn emit_specimen(corpus: &Corpus, specimen: &Specimen) -> String {
    let module = &corpus.modules[specimen.module];
    match specimen.kind {
        SpecimenKind::Linear { a, b } => format!(
            "fn {}(x: Int) -> Int\n  ensures result - {b} == x * {a}\n= x * {a} + {b}\n",
            specimen.name
        ),
        SpecimenKind::Status => format!(
            "fn {}(s: {}) -> Int\n  ensures result == 1 || result == 0\n= match s {{\n    {}(_) -> 1,\n    {} -> 0,\n  }}\n",
            specimen.name, module.status_type, module.ctor_ready, module.ctor_idle
        ),
        SpecimenKind::Length => format!(
            "fn {name}(xs: List<Int>) -> Int\n  ensures result >= 0\n= match xs {{\n    [_, ..rest] -> 1 + {name}(rest),\n    _ -> 0,\n  }}\n",
            name = specimen.name
        ),
    }
}

pub fn emit_law(corpus: &Corpus, law: &Law) -> String {
    let body = match law.kind {
        LawKind::Ground { a, b } => format!("{a} * 3 + {b} == {}", a * 3 + b),
        LawKind::Finite => "(ready && paid) == !(!ready || !paid)".to_string(),
        LawKind::Length { specimen } => {
            format!("{}(xs) >= 0", corpus.specimens[specimen].name)
        }
    };
    let binders = match law.kind {
        LawKind::Ground { .. } => None,
        LawKind::Finite => Some("forall (ready: Bool, paid: Bool)"),
        LawKind::Length { .. } => Some("forall (xs: List<Int>)"),
    };
    let label = escape(&law.label);
    match binders {
        None => format!("law \"{label}\" {{\n  {body}\n}}\n"),
        Some(binders) => {
            format!("law \"{label}\"\n  {binders} {{\n    {body}\n  }}\n")
        }
    }
}

/// The specimens' own test.
pub fn emit_specimen_test(corpus: &Corpus, module: &Module) -> String {
    let mut body = Vec::new();
    for specimen in corpus.specimens_in(module.id) {
        match specimen.kind {
            SpecimenKind::Linear { a, b } => {
                body.push(format!("assert_eq({}(7), {})", specimen.name, 7 * a + b))
            }
            SpecimenKind::Status => body.push(format!(
                "assert_eq({}({}(1)), 1);\nassert_eq({}({}), 0)",
                specimen.name, module.ctor_ready, specimen.name, module.ctor_idle
            )),
            SpecimenKind::Length => body.push(format!(
                "assert_eq({}([4, 5, 6]), 3);\nassert_eq({}([]), 0)",
                specimen.name, specimen.name
            )),
        }
    }
    format!(
        "test \"the specified definitions of {} return what their claims say\" {{\n{}\n}}\n",
        module.name,
        indent(&body.join(";\n"), 2)
    )
}

pub fn emit_task(corpus: &Corpus, task: &TaskBody) -> String {
    let label = &corpus.shards[task.shard];
    let mut s = format!(
        "fn {}() -> Int / {{effects::counter.write[{label}], task.write}} {{\n",
        task.name
    );
    for (i, by) in task.steps.iter().enumerate() {
        if i > 0 {
            s.push_str("  task.yield();\n");
        }
        let _ = writeln!(s, "  effects::counter.bump[{label}]({by});");
    }
    let _ = writeln!(s, "  {}", task.contributed());
    s.push_str("}\n");
    s
}

/// The test itself: one cell per shard, one handler clause per shard, and assertions that hold
/// under every interleaving — the per-shard totals and the values the tasks returned.
pub fn emit_concurrent_test(corpus: &Corpus, test: &ConcurrentTest) -> String {
    let mut body: Vec<String> = test
        .tasks
        .iter()
        .enumerate()
        .map(|(i, task)| format!("let t{i} = task.spawn(|| {}());", task.name))
        .collect();

    let joins: Vec<String> = (0..test.tasks.len())
        .map(|i| format!("task.join(t{i})"))
        .collect();
    body.push(format!(
        "assert_eq({}, {});",
        joins.join(" + "),
        test.total()
    ));
    let asserts: Vec<String> = test
        .shards
        .iter()
        .map(|&shard| format!("assert_eq(cell_get(c{shard}), {})", test.shard_total(shard)))
        .collect();
    body.push(asserts.join(";\n"));

    let clauses: Vec<String> = test
        .shards
        .iter()
        .map(|&shard| {
            format!(
                "  effects::counter.bump[{}](n) -> cell_set(c{shard}, cell_get(c{shard}) + n),",
                corpus.shards[shard]
            )
        })
        .collect();

    let mut inner = format!(
        "handle {{\n  simulate {{\n{}\n  }}\n}} with {{\n{}\n}}",
        indent(&body.join("\n"), 4),
        clauses.join("\n")
    );
    for &shard in test.shards.iter().rev() {
        inner = format!(
            "with_cell[{}](0) {{ c{shard} ->\n{}\n}}",
            corpus.shards[shard],
            indent(&inner, 2)
        );
    }

    format!(
        "test \"{}\" {{\n{}\n}}\n",
        escape(&test.label),
        indent(&inner, 2)
    )
}

/// One clause per atom the root definition may perform.
pub fn handler_clauses(corpus: &Corpus, test: &Test, footprint: &Footprint) -> Vec<String> {
    let mut clauses = Vec::new();
    let written_table = |t: usize| test.final_table_len.iter().any(|(i, _)| *i == t);
    let written_region = |r: usize| test.final_region.iter().any(|(i, _)| *i == r);

    for atom in footprint {
        match (atom.effect, &atom.resource, atom.write) {
            (Eff::Db, Some(label), false) => {
                let index = position(&corpus.tables, label);
                let body = if written_table(index) {
                    format!("cell_get(t{index})")
                } else {
                    literal_list(test.world.table(index))
                };
                clauses.push(format!("effects::db.all[{label}]() -> {body}"));
            }
            (Eff::Db, Some(label), true) => {
                let index = position(&corpus.tables, label);
                clauses.push(format!(
                    "effects::db.save[{label}](rows) -> cell_set(t{index}, rows)"
                ));
            }
            (Eff::Cache, Some(label), false) => {
                let index = position(&corpus.regions, label);
                let body = if written_region(index) {
                    format!("cell_get(r{index})")
                } else {
                    test.world.region(index).to_string()
                };
                clauses.push(format!("effects::cache.peek[{label}]() -> {body}"));
            }
            (Eff::Cache, Some(label), true) => {
                let index = position(&corpus.regions, label);
                clauses.push(format!(
                    "effects::cache.poke[{label}](v) -> cell_set(r{index}, v)"
                ));
            }
            (Eff::Clock, _, _) => {
                clauses.push(format!("effects::clock.now() -> {}", test.world.clock));
            }
            _ => {}
        }
    }
    clauses
}

fn position(labels: &[String], label: &str) -> usize {
    labels.iter().position(|l| l == label).unwrap_or(0)
}

fn literal_list(values: &[i64]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn indent(text: &str, by: usize) -> String {
    let pad = " ".repeat(by);
    text.lines()
        .map(|l| format!("{pad}{l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Indents every line but the first, for text spliced after something already on the line.
fn indent_tail(text: &str, by: usize) -> String {
    let pad = " ".repeat(by);
    let mut lines = text.lines();
    let first = lines.next().unwrap_or_default().to_string();
    std::iter::once(first)
        .chain(lines.map(|l| format!("{pad}{l}")))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn escape(label: &str) -> String {
    label.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Wraps a one-line definition body in another `prim::clamp`.
pub fn wrap_body(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let body = lines.last()?;
    let head = lines.get(lines.len().checked_sub(2)?)?;
    if !head.ends_with('=') {
        return None;
    }
    let mut out: Vec<String> = lines[..lines.len() - 1]
        .iter()
        .map(|l| l.to_string())
        .collect();
    out.push(format!("  prim::clamp({})", body.trim()));
    Some(format!("{}\n", out.join("\n")))
}
