//! Every slot the resolver assigns names the variable it was resolved from — ADR 0034 §4.
//!
//! The runtime still answers every lookup by name, so a wrong slot costs nothing today. It would
//! cost a wrong value the moment the machine reads by index, and that is a change no test can be
//! written after: this is the one that has to exist first.

use ply_eval::slots;
use ply_span::{SourceId, SourceMap, Symbol};
use ply_syntax::ast::{Expr, ExprKind, Item, ModuleName, Stmt as AstStmt};
use ply_syntax::parse_program;
use ply_syntax::resolve::resolve as resolve_names;

fn program_of(src: &str) -> ply_syntax::ast::Program {
    let mut map = SourceMap::new();
    let id: SourceId = map.add("slots.ply", src.to_string());
    let mut program = match parse_program([(id, ModuleName::from_dotted("slots"), src)]) {
        Ok(p) => p,
        Err(ds) => panic!("the probe must parse: {ds:#?}\n{src}"),
    };
    resolve_names(&mut program).expect("the probe must resolve");
    program
}

/// Walks the body again and checks each bare variable against the table it was resolved into.
fn check(params: &[Symbol], body: &Expr, src: &str) -> usize {
    let table = slots::resolve(params, body);
    let mut checked = 0;
    let mut stack = vec![body];
    while let Some(e) = stack.pop() {
        if let ExprKind::Var(q) = &e.kind
            && q.is_bare()
            && let Some((at, slot)) = table.of_var.get(&(std::ptr::from_ref(e) as usize))
        {
            // The slot names this variable in the barrier the occurrence reads from.
            let named = table
                .barriers
                .get(*at as usize)
                .and_then(|b| b.names.get(*slot as usize));
            assert_eq!(
                named,
                Some(q.symbol()),
                "`{}` resolved to slot {slot}, which names {named:?} in its barrier\n{src}",
                q.symbol()
            );
            checked += 1;
        }
        children(e, &mut stack);
    }
    checked
}

fn children<'a>(e: &'a Expr, out: &mut Vec<&'a Expr>) {
    match &e.kind {
        ExprKind::Lit(_) | ExprKind::Var(_) => {}
        ExprKind::Lambda { body, .. }
        | ExprKind::Field { base: body, .. }
        | ExprKind::Try { operand: body }
        | ExprKind::Unary { operand: body, .. }
        | ExprKind::WithRegion { body, .. }
        | ExprKind::Simulate { body, .. } => out.push(body),
        ExprKind::Binary { lhs, rhs, .. } => {
            out.push(lhs);
            out.push(rhs);
        }
        ExprKind::App { func, args, .. } => {
            out.push(func);
            out.extend(args.iter());
        }
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            out.push(cond);
            out.push(then_branch);
            out.push(else_branch);
        }
        ExprKind::Match { scrutinee, arms } => {
            out.push(scrutinee);
            for a in arms {
                out.push(&a.body);
                out.extend(a.guard.iter());
            }
        }
        ExprKind::Block { stmts, tail } => {
            for s in stmts {
                match s {
                    AstStmt::Let { value, .. } => out.push(value),
                    AstStmt::Expr(e) => out.push(e),
                }
            }
            out.extend(tail.iter().map(|t| &**t));
        }
        ExprKind::Record { fields } => out.extend(fields.iter().map(|(_, v)| v)),
        ExprKind::RecordUpdate { base, fields } => {
            out.push(base);
            out.extend(fields.iter().map(|(_, v)| v));
        }
        ExprKind::List { items } => out.extend(items.iter()),
        ExprKind::Perform { args, .. } => out.extend(args.iter()),
        ExprKind::Handle {
            body,
            clauses,
            return_clause,
        } => {
            out.push(body);
            out.extend(clauses.iter().map(|c| &c.body));
            out.extend(return_clause.iter().map(|r| &r.body));
        }
        ExprKind::WithCell { init, body, .. } => {
            out.push(init);
            out.push(body);
        }
    }
}

fn check_source(src: &str) -> usize {
    let program = program_of(src);
    let mut checked = 0;
    for module in &program.modules {
        for item in &module.items {
            let (params, body) = match item {
                Item::Fn(f) => (
                    f.params
                        .iter()
                        .map(|p| p.name.name.clone())
                        .collect::<Vec<_>>(),
                    &f.body,
                ),
                Item::Test(t) => (Vec::new(), &t.body),
                _ => continue,
            };
            checked += check(&params, body, src);
        }
    }
    checked
}

/// Shadowing is the case a flat table gets wrong: one name, two slots, and the occurrence decides.
///
/// Asserting only that *two* slots exist would pass with the resolver picking either one, so this
/// pins which slot each occurrence reads. `x` is bound by the parameter and again by the inner
/// `let`, and the three reads straddle the second binder.
#[test]
fn a_shadowed_name_resolves_to_the_binder_nearest_it() {
    let src = "\
fn go(x: Int) -> Int = {
  let a = x;
  let x = a + 1;
  let b = x;
  a + b + x
}
";
    let program = program_of(src);
    let Item::Fn(f) = &program.modules[0].items[0] else {
        panic!("a function")
    };
    let params: Vec<Symbol> = f.params.iter().map(|p| p.name.name.clone()).collect();
    let table = slots::resolve(&params, &f.body);
    let x = Symbol::new("x");

    let body = &table.barriers[0];
    let of_x: Vec<u32> = body
        .names
        .iter()
        .enumerate()
        .filter(|(_, n)| **n == x)
        .map(|(i, _)| i as u32)
        .collect();
    assert_eq!(
        of_x.len(),
        2,
        "the parameter and the inner `let` are two bindings of one name: {:?}",
        body.names
    );
    let (outer, inner) = (of_x[0], of_x[1]);

    // Every read of `x`, left to right. The first is left of the inner binder and the other two are
    // right of it.
    let mut reads: Vec<(u32, u32)> = Vec::new();
    let mut stack = vec![&f.body];
    while let Some(e) = stack.pop() {
        if let ExprKind::Var(q) = &e.kind
            && q.is_bare()
            && *q.symbol() == x
            && let Some((_, slot)) = table.of_var.get(&(std::ptr::from_ref(e) as usize))
        {
            reads.push((e.span.start, *slot));
        }
        children(e, &mut stack);
    }
    reads.sort_unstable();
    assert_eq!(reads.len(), 3, "three reads of `x`: {reads:?}");
    assert_eq!(
        reads[0].1, outer,
        "the read in `let a = x` is left of the inner binder, so it is the parameter's slot"
    );
    assert_eq!(
        (reads[1].1, reads[2].1),
        (inner, inner),
        "the reads after `let x = ..` are the inner binding's slot, not the parameter's"
    );
}

/// A lambda's body indexes its own table, so an outer name is free rather than a slot of the inner.
#[test]
fn a_lambda_body_does_not_index_the_enclosing_barriers_table() {
    let src = "\
fn go(xs: List<Int>, n: Int) -> List<Int> = map(xs, |y| y + n)
";
    let program = program_of(src);
    let Item::Fn(f) = &program.modules[0].items[0] else {
        panic!("a function")
    };
    let params: Vec<Symbol> = f.params.iter().map(|p| p.name.name.clone()).collect();
    let table = slots::resolve(&params, &f.body);
    assert!(
        table.barriers.len() >= 2,
        "the lambda opens a barrier of its own: {:?}",
        table.barriers.len()
    );
    let y = Symbol::new("y");
    let inner = table
        .barriers
        .iter()
        .find(|b| b.names.contains(&y))
        .expect("the lambda's table");
    assert_eq!(
        inner.names,
        vec![y],
        "`n` is free in the lambda, so it takes no slot there"
    );
}

/// Every function the repository ships, rather than the eight shapes below.
///
/// The hand-written probes pin the cases a flat table gets wrong; this one is the breadth. When the
/// machine starts reading by index, it reads these programs, and a slot that names the wrong
/// binding in any of them is a wrong value.
#[test]
fn every_slot_in_every_shipped_module_names_its_own_variable() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the crate sits two levels under the workspace root");
    let mut sources: Vec<(String, String)> = Vec::new();
    for dir in ["examples", "crates/ply-std/ply"] {
        let Ok(entries) = std::fs::read_dir(root.join(dir)) else {
            continue;
        };
        for e in entries.filter_map(|e| e.ok()) {
            let path = e.path();
            if path.extension().is_some_and(|x| x == "ply")
                && let Ok(text) = std::fs::read_to_string(&path)
            {
                sources.push((path.display().to_string(), text));
            }
        }
    }
    sources.sort();
    assert!(
        sources.len() >= 8,
        "found only {} shipped modules, so this measured almost nothing",
        sources.len()
    );

    let (mut modules, mut checked) = (0usize, 0usize);
    for (name, text) in &sources {
        let mut map = SourceMap::new();
        let id: SourceId = map.add(name, text.clone());
        let Ok(mut program) =
            parse_program([(id, ModuleName::from_dotted("probe"), text.as_str())])
        else {
            continue;
        };
        if resolve_names(&mut program).is_err() {
            continue;
        }
        modules += 1;
        for module in &program.modules {
            for item in &module.items {
                let (params, body) = match item {
                    Item::Fn(f) => (
                        f.params
                            .iter()
                            .map(|p| p.name.name.clone())
                            .collect::<Vec<_>>(),
                        &f.body,
                    ),
                    Item::Test(t) => (Vec::new(), &t.body),
                    _ => continue,
                };
                checked += check(&params, body, name);
            }
        }
    }
    println!("\n  {modules} shipped modules, {checked} variable occurrences resolved");
    assert!(
        modules >= 8 && checked >= 500,
        "{modules} modules and {checked} occurrences is too little breadth to be a check"
    );
}

/// The shapes the corpus is written in, checked together rather than one at a time.
#[test]
fn every_resolved_slot_names_its_own_variable() {
    let sources = [
        "fn go(a: Int, b: Int) -> Int = a + b",
        "fn go(xs: List<Int>) -> Int = { let n = len(xs); let m = n * 2; n + m }",
        "fn go(b: Bool, x: Int) -> Int = match b { true -> x, false -> 0 }",
        "fn go(s: {k: Int, v: Int}) -> Int = { let t = s.k; t + s.v }",
        "fn go(xs: List<Int>) -> List<Int> = fold(xs, [], |acc, y| push(acc, y))",
        "effect amb { read flip[c]() -> Bool }\n\
         fn go() -> Int = handle { if amb.flip[c]() { 1 } else { 2 } } \
         with { amb.flip[c]() -> true, return x -> x }",
        "fn go(n: Int) -> Int = with_cell[r](n) { c -> cell_get(c) + n }",
        "fn go(xs: List<Int>) -> List<Int> = { let ys = push(xs, 1); ys }",
    ];
    let mut total = 0;
    for src in sources {
        total += check_source(src);
    }
    assert!(
        total >= 20,
        "the corpus resolved only {total} variables, which is too few to be checking anything"
    );
}

/// How often the machine carries a scope, against how often it captures one — ADR 0034 §4.
///
/// The rewrite trades these against each other. A persistent chain makes capture cheap, because a
/// continuation shares a pointer; a slot stack makes carrying cheap, because the frame records a
/// base index instead of building anything — and then capture has to *copy* the window it took.
/// Which way that trade goes is a property of real programs, so it is counted over the corpus
/// rather than argued.
#[test]
fn the_corpus_carries_far_more_often_than_it_captures() {
    use ply_eval::{Machine, rc};
    use ply_syntax::resolve::resolve as resolve_names;

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the crate sits two levels under the workspace root")
        .join("examples");
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&root)
        .expect("the repository ships examples")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "ply"))
        .collect();
    files.sort();

    let (mut carries, mut captures, mut frames) = (0u64, 0u64, 0u64);
    for path in &files {
        let text = std::fs::read_to_string(path).expect("an example is readable");
        let mut map = SourceMap::new();
        let id = map.add(path, text.clone());
        let name = ModuleName::from_dotted("probe");
        let Ok(mut program) = parse_program([(id, name, text.as_str())]) else {
            continue;
        };
        let Ok(resolved) = resolve_names(&mut program) else {
            continue;
        };
        rc::census4::reset();
        let mut machine = Machine::for_program(&program, &resolved);
        for i in 0..machine.test_count() {
            let _ = machine.eval_test(i);
        }
        let (c, k, f) = rc::census4::read();
        carries += c;
        captures += k;
        frames += f;
    }

    println!("\n  carries {carries}  captures {captures}  captured frames {frames}");
    assert!(
        carries > 0,
        "the corpus carried nothing, so this measured nothing"
    );
    assert!(
        carries > captures * 10,
        "carries {carries} against captures {captures}: the rewrite's trade only pays if carrying \
         is much the more common of the two, and on this corpus it is not"
    );
}
