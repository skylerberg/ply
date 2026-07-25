//! [`Corpus`] to Ply source.
//!
//! Every arm here has a twin in [`crate::model::Corpus::eval`]. Change one and
//! the generator's own verification pass fails, which is the intended way to
//! find out.

use crate::model::*;
use std::fmt::Write;

/// The two modules every generated one leans on. Hand-written because they are
/// the corpus's contract with the language, not something worth randomizing.
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

fn emit_module(corpus: &Corpus, module: &Module) -> String {
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

    for test in corpus.tests.iter().filter(|t| t.module == module.id) {
        s.push_str(&emit_test(corpus, test));
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

pub fn emit_def(corpus: &Corpus, def: &Def) -> String {
    let mut s = String::with_capacity(256);
    let visibility = if def.public { "pub " } else { "" };
    let head = format!(
        "{visibility}fn {}({}) -> Int{}",
        def.name,
        params(def.arity),
        row(&def.footprint)
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
        // A `match` cannot be spliced into an argument list, so a `Sum` with
        // extras is rebuilt as a block whose `let` holds the arms.
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
                let _ = writeln!(s, "{head} =");
                let _ = writeln!(s, "{}", indent(&arms, 2));
            } else {
                let _ = writeln!(s, "{head} {{");
                let _ = writeln!(s, "  let core = {};", indent_tail(&arms, 2));
                let _ = writeln!(s, "  {}", combine(corpus, def, "core"));
                s.push_str("}\n");
            }
        }
        other => {
            let _ = writeln!(s, "{head} =");
            let _ = writeln!(
                s,
                "  {}",
                combine(corpus, def, &core_expr(corpus, def, other))
            );
        }
    }
    s
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

/// `base` is the caller's expression for the callee's first argument, before the
/// call's own offset is added. A second parameter is derived from that offset so
/// the emitter and the evaluator agree without carrying an extra field.
fn call_expr(corpus: &Corpus, here: ModuleId, call: Call, base: &str) -> String {
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

/// A module has no binder for itself, so a same-module reference must be bare;
/// every other one is `m::x` and so cannot be captured by a local binder.
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

/// One clause per atom the root definition may perform. A written resource is
/// backed by the enclosing cell so the test observes its own writes; a read-only
/// one is a literal, which is exactly the "no fixture, no teardown" claim.
fn handler_clauses(corpus: &Corpus, test: &Test, footprint: &Footprint) -> Vec<String> {
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

/// Indents every line but the first, for text spliced after something already
/// on the line.
fn indent_tail(text: &str, by: usize) -> String {
    let pad = " ".repeat(by);
    let mut lines = text.lines();
    let first = lines.next().unwrap_or_default().to_string();
    std::iter::once(first)
        .chain(lines.map(|l| format!("{pad}{l}")))
        .collect::<Vec<_>>()
        .join("\n")
}

fn escape(label: &str) -> String {
    label.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Wraps a one-line definition body in another `prim::clamp`. Every generated
/// body already yields a clamped value and `clamp` is idempotent over that
/// range, so the definition's normalized form changes and its value does not —
/// which is the only kind of edit a benchmark can apply without invalidating
/// the expected values baked into every test.
pub fn wrap_body(text: &str) -> Option<String> {
    let mut lines = text.lines();
    let head = lines.next()?;
    let body = lines.next()?;
    if lines.next().is_some() || !head.ends_with('=') {
        return None;
    }
    Some(format!("{head}\n  prim::clamp({})\n", body.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::generate;
    use crate::spec::CorpusSpec;

    fn corpus() -> Corpus {
        generate(&CorpusSpec {
            seed: 3,
            modules: 6,
            defs_per_module: 10,
            tests: 20,
            depth: 3,
            ..CorpusSpec::default()
        })
    }

    #[test]
    fn a_module_imports_effects_exactly_when_it_needs_the_binder() {
        let corpus = corpus();
        for module in &corpus.modules {
            let text = emit_module(&corpus, module);
            let mentions = text.contains("effects::");
            assert_eq!(
                text.contains("import core.effects"),
                mentions,
                "module {} imports and uses `effects` inconsistently",
                module.name
            );
        }
    }

    #[test]
    fn a_reference_is_qualified_exactly_when_it_leaves_its_module() {
        let corpus = corpus();
        let mut crossed = 0;
        for def in &corpus.defs {
            for call in def
                .shape
                .calls()
                .into_iter()
                .chain(def.extras.iter().copied())
            {
                let expr = call_expr(&corpus, def.module, call, "x");
                let owner = corpus.defs[call.target].module;
                if owner == def.module {
                    assert!(!expr.contains("::"), "{expr} qualifies a same-module call");
                } else {
                    crossed += 1;
                    let binder = corpus.modules[owner].binder();
                    assert!(
                        expr.starts_with(&format!("{binder}::")),
                        "{expr} is not qualified"
                    );
                }
            }
        }
        assert!(
            crossed > 0,
            "no call crossed a module, so nothing was checked"
        );
    }

    #[test]
    fn a_definition_is_pub_in_the_source_exactly_when_the_model_says_so() {
        let corpus = corpus();
        for def in &corpus.defs {
            let text = emit_def(&corpus, def);
            assert_eq!(text.starts_with("pub fn "), def.public, "{}", def.name);
        }
    }

    #[test]
    fn a_declared_row_lists_every_atom_the_definition_can_perform() {
        let corpus = corpus();
        for def in corpus.defs.iter().filter(|d| !d.footprint.is_empty()) {
            let text = emit_def(&corpus, def);
            for atom in &def.footprint {
                assert!(
                    text.contains(&atom.render()),
                    "{} omits {}",
                    def.name,
                    atom.render()
                );
            }
        }
    }

    #[test]
    fn a_test_grants_one_clause_per_performed_atom_and_no_more() {
        let corpus = corpus();
        for test in &corpus.tests {
            let clauses = handler_clauses(&corpus, test, &test.granted);
            assert_eq!(clauses.len(), test.granted.len(), "test `{}`", test.label);
            assert!(
                test.granted.is_subset(&corpus.defs[test.root].footprint),
                "test `{}` grants an atom its root never declares",
                test.label
            );
        }
    }

    /// Handlers are granted only for what actually fires, so a definition that
    /// declares more than a given call path performs leaves atoms behind. Without
    /// that, every test is hermetic and the scheduler has one group to make.
    #[test]
    fn some_tests_leave_a_declared_atom_ungranted() {
        let corpus = generate(&CorpusSpec {
            seed: 3,
            modules: 8,
            defs_per_module: 12,
            tests: 60,
            depth: 3,
            tables: 3,
            regions: 2,
            ..CorpusSpec::default()
        });
        let partial = corpus
            .tests
            .iter()
            .filter(|t| t.granted != corpus.defs[t.root].footprint)
            .count();
        assert!(partial > 0, "every test granted its root's whole footprint");
    }

    #[test]
    fn a_label_with_a_quote_in_it_cannot_break_out_of_the_string() {
        assert_eq!(escape(r#"a "b" \c"#), r#"a \"b\" \\c"#);
    }
}
