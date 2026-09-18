//! `effect set` expansion, which happens inside the parser.

use crate::ast::*;
use indexmap::IndexMap;
use ply_span::{Diagnostic, Symbol, codes};

/// Expands every row in the module and fills in each set's `expansion`.
pub(crate) fn expand(module: &mut Module, diags: &mut Vec<Diagnostic>) {
    let mut sets = Sets::collect(module, diags);
    sets.resolve_includes(diags);
    sets.find_cycles(diags);
    sets.expand_all();
    sets.write_back(module);

    walk_module_rows(module, &mut |row| {
        let mut atoms = Vec::new();
        for alias in &row.aliases {
            // `None` is a refused set, already reported; it splices in nothing.
            if let Some(i) = sets.lookup(alias, diags) {
                atoms.extend(sets.defs[i].expansion.iter().cloned());
            }
        }
        row.atoms.extend(atoms);
    });
}

/// Every `effect set` the module declares, by simple name.
struct Sets {
    defs: Vec<EffectSetDef>,
    by_name: IndexMap<Symbol, usize>,
    /// Parallel to `defs`: each `includes` target, or `None` for one already reported.
    edges: Vec<Vec<Option<usize>>>,
    cyclic: Vec<bool>,
    /// The module's effects, so a member naming one is told it should be an atom.
    effect_names: Vec<Symbol>,
}

impl Sets {
    fn collect(module: &Module, diags: &mut Vec<Diagnostic>) -> Sets {
        let mut defs: Vec<EffectSetDef> = Vec::new();
        let mut by_name: IndexMap<Symbol, usize> = IndexMap::new();
        let mut effect_names = Vec::new();
        for item in &module.items {
            match item {
                Item::Effect(e) => effect_names.push(e.name.name.clone()),
                Item::EffectSet(d) => {
                    if let Some(&first) = by_name.get(&d.name.name) {
                        diags.push(
                            Diagnostic::error(
                                codes::DUPLICATE_DEFINITION,
                                format!("duplicate effect set `{}`", d.name.name),
                            )
                            .primary(d.name.span, "redefined here")
                            .secondary(defs[first].name.span, "first defined here")
                            .note(
                                "rename one of them; every effect set in a module needs a \
                                 distinct name",
                            ),
                        );
                        continue;
                    }
                    by_name.insert(d.name.name.clone(), defs.len());
                    defs.push((**d).clone());
                }
                _ => {}
            }
        }
        let edges = vec![Vec::new(); defs.len()];
        let cyclic = vec![false; defs.len()];
        Sets {
            defs,
            by_name,
            edges,
            cyclic,
            effect_names,
        }
    }

    fn resolve_includes(&mut self, diags: &mut Vec<Diagnostic>) {
        for i in 0..self.defs.len() {
            let includes = std::mem::take(&mut self.defs[i].includes);
            let mut edges = Vec::with_capacity(includes.len());
            for q in &includes {
                edges.push(self.lookup(q, diags));
            }
            self.defs[i].includes = includes;
            self.edges[i] = edges;
        }
    }

    /// The set a member or a row named; `None` when missing, foreign or cyclic.
    fn lookup(&self, q: &QName, diags: &mut Vec<Diagnostic>) -> Option<usize> {
        if !q.is_bare() {
            diags.push(
                Diagnostic::error(
                    codes::UNKNOWN_EFFECT_SET,
                    format!("an `effect set` cannot be named from another module: `{q}`"),
                )
                .primary(q.span, "sets are module-local")
                .note(format!(
                    "declare `effect set {}` in this file and write `{}`",
                    q.symbol(),
                    q.symbol()
                ))
                .note(
                    "a set that expanded across a module boundary would let an edit in the \
                     declaring module change this file's published row while this file's bytes \
                     never moved, and the skipped recheck would leave a footprint that \
                     under-reports",
                ),
            );
            return None;
        }
        match self.by_name.get(q.symbol()) {
            Some(&i) if self.cyclic[i] => None,
            Some(&i) => Some(i),
            None => {
                diags.push(self.unknown(q));
                None
            }
        }
    }

    fn unknown(&self, q: &QName) -> Diagnostic {
        let mut d = Diagnostic::error(
            codes::UNKNOWN_EFFECT_SET,
            format!("no `effect set` named `{}` in this module", q.symbol()),
        )
        .primary(q.span, "not declared here");
        if self.effect_names.contains(q.symbol()) {
            d = d
                .note(format!(
                    "`{}` is an effect, and a member of a set is an atom: write `{}.read[..]` \
                     or `{}.write[..]`",
                    q.symbol(),
                    q.symbol(),
                    q.symbol()
                ))
                .note(
                    "a whole effect is every resource label anywhere in the program, so naming \
                     one would let an unrelated module change this row and therefore this \
                     definition's hash",
                );
        }
        d = if self.by_name.is_empty() {
            d.note(
                "this module declares no `effect set`; a set is module-local and cannot be \
                 imported",
            )
        } else {
            let known: Vec<String> = self.by_name.keys().map(|k| format!("`{k}`")).collect();
            d.note(format!(
                "this module declares {}; a set is module-local and cannot be imported",
                known.join(", ")
            ))
        };
        d
    }

    /// Marks every set on a cycle and reports each cycle.
    fn find_cycles(&mut self, diags: &mut Vec<Diagnostic>) {
        #[derive(Clone, Copy, PartialEq)]
        enum Color {
            White,
            Gray,
            Black,
        }
        let mut color = vec![Color::White; self.defs.len()];
        let mut path: Vec<usize> = Vec::new();

        // Explicit stack: a long chain of sets must not overflow the parser's stack.
        for root in 0..self.defs.len() {
            if color[root] != Color::White {
                continue;
            }
            let mut work: Vec<(usize, usize)> = vec![(root, 0)];
            color[root] = Color::Gray;
            path.push(root);
            while let Some((node, next)) = work.last().copied() {
                let Some(&edge) = self.edges[node].get(next) else {
                    color[node] = Color::Black;
                    work.pop();
                    path.pop();
                    continue;
                };
                work.last_mut()
                    .expect("the frame just read is still there")
                    .1 += 1;
                let Some(target) = edge else { continue };
                match color[target] {
                    Color::White => {
                        color[target] = Color::Gray;
                        path.push(target);
                        work.push((target, 0));
                    }
                    Color::Gray => {
                        let at = path.iter().position(|&n| n == target).unwrap_or(0);
                        diags.push(self.cycle_report(&path[at..]));
                        for &n in &path[at..] {
                            self.cyclic[n] = true;
                        }
                    }
                    Color::Black => {}
                }
            }
        }
    }

    fn cycle_report(&self, cycle: &[usize]) -> Diagnostic {
        let head = &self.defs[cycle[0]];
        let mut names: Vec<String> = cycle
            .iter()
            .map(|&n| format!("`{}`", self.defs[n].name.name))
            .collect();
        names.push(format!("`{}`", head.name.name));
        let mut d = Diagnostic::error(
            codes::EFFECT_SET_CYCLE,
            format!("effect set `{}` contains itself", head.name.name),
        )
        .primary(
            head.name.span,
            if cycle.len() == 1 {
                "this set names itself".to_string()
            } else {
                format!(
                    "expands through {} and back",
                    names[1..cycle.len()].join(", ")
                )
            },
        );
        for &n in &cycle[1..] {
            d = d.secondary(self.defs[n].name.span, "on the cycle");
        }
        d.note(format!("the cycle is {}", names.join(" -> ")))
            .note("expansion is a fixed point, and a cycle has none: break it by inlining the atoms one of these sets needs")
    }

    fn expand_all(&mut self) {
        let mut done = vec![false; self.defs.len()];
        for root in 0..self.defs.len() {
            if done[root] || self.cyclic[root] {
                continue;
            }
            // Post-order over the acyclic remainder, so a set's includes are expanded before it is.
            let mut work = vec![(root, false)];
            while let Some((node, visited)) = work.pop() {
                if done[node] || self.cyclic[node] {
                    continue;
                }
                if !visited {
                    work.push((node, true));
                    for &edge in self.edges[node].iter().flatten() {
                        if !done[edge] && !self.cyclic[edge] {
                            work.push((edge, false));
                        }
                    }
                    continue;
                }
                let mut out: Vec<AtomExpr> = self.defs[node].atoms.clone();
                for &edge in self.edges[node].iter().flatten() {
                    out.extend(self.defs[edge].expansion.iter().cloned());
                }
                canonicalize(&mut out);
                self.defs[node].expansion = out;
                done[node] = true;
            }
        }
    }

    fn write_back(&self, module: &mut Module) {
        let mut seen: IndexMap<Symbol, usize> = IndexMap::new();
        for item in &mut module.items {
            let Item::EffectSet(d) = item else { continue };
            let Some(&i) = self.by_name.get(&d.name.name) else {
                continue;
            };
            // A duplicate binds to the first declaration; the refused second one keeps nothing.
            if seen.insert(d.name.name.clone(), i).is_some() {
                continue;
            }
            d.expansion = self.defs[i].expansion.clone();
        }
    }
}

fn canonicalize(atoms: &mut Vec<AtomExpr>) {
    let key = |a: &AtomExpr| {
        (
            a.effect.module.as_ref().map(|m| m.name.clone()),
            a.effect.name.name.clone(),
            a.mode.as_str(),
            a.resource.as_ref().map(|r| r.name.clone()),
        )
    };
    atoms.sort_by_key(key);
    atoms.dedup_by_key(|a| key(a));
}

fn walk_module_rows(module: &mut Module, f: &mut impl FnMut(&mut RowExpr)) {
    for item in &mut module.items {
        match item {
            Item::Fn(d) => {
                for p in &mut d.params {
                    if let Some(t) = &mut p.ty {
                        walk_type(t, f);
                    }
                }
                if let Some(t) = &mut d.ret {
                    walk_type(t, f);
                }
                if let Some(r) = &mut d.effects {
                    f(r);
                }
                for s in &mut d.spec {
                    walk_expr(&mut s.expr, f);
                }
                walk_expr(&mut d.body, f);
            }
            Item::Type(d) => match &mut d.body {
                TypeDefBody::Alias(t) => walk_type(t, f),
                TypeDefBody::Sum(variants) => {
                    for v in variants {
                        for t in &mut v.fields {
                            walk_type(t, f);
                        }
                    }
                }
            },
            Item::Effect(d) => {
                for op in &mut d.ops {
                    for t in &mut op.params {
                        walk_type(t, f);
                    }
                    walk_type(&mut op.ret, f);
                }
            }
            Item::Test(d) => walk_expr(&mut d.body, f),
            Item::Law(d) => {
                for b in &mut d.binders {
                    walk_type(&mut b.ty, f);
                }
                if let Some(g) = &mut d.guard {
                    walk_expr(g, f);
                }
                walk_expr(&mut d.body, f);
            }
            Item::Derive(_) | Item::EffectSet(_) => {}
        }
    }
}

fn walk_type(t: &mut TypeExpr, f: &mut impl FnMut(&mut RowExpr)) {
    grow(|| match t {
        TypeExpr::Var(_) | TypeExpr::Unit { .. } => {}
        TypeExpr::Con { args, .. } => {
            for a in args {
                walk_type(a, f);
            }
        }
        TypeExpr::Fn {
            params,
            ret,
            effects,
            ..
        } => {
            for p in params {
                walk_type(p, f);
            }
            walk_type(ret, f);
            if let Some(r) = effects {
                f(r);
            }
        }
        TypeExpr::Record { fields, .. } => {
            for (_, t) in fields {
                walk_type(t, f);
            }
        }
    })
}

/// `let` and lambda annotations can hold function types, so a row can appear at any depth.
fn walk_expr(e: &mut Expr, f: &mut impl FnMut(&mut RowExpr)) {
    grow(|| match &mut e.kind {
        ExprKind::Lit(_) | ExprKind::Var(_) => {}
        ExprKind::Binary { lhs, rhs, .. } => {
            walk_expr(lhs, f);
            walk_expr(rhs, f);
        }
        ExprKind::Unary { operand, .. } => walk_expr(operand, f),
        ExprKind::Lambda { params, body, .. } => {
            for p in params {
                if let Some(t) = &mut p.ty {
                    walk_type(t, f);
                }
            }
            walk_expr(body, f);
        }
        ExprKind::App { func, args, named } => {
            walk_expr(func, f);
            for a in args {
                walk_expr(a, f);
            }
            for n in named {
                walk_expr(&mut n.value, f);
            }
        }
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            walk_expr(cond, f);
            walk_expr(then_branch, f);
            walk_expr(else_branch, f);
        }
        ExprKind::Match { scrutinee, arms } => {
            walk_expr(scrutinee, f);
            for arm in arms {
                if let Some(g) = &mut arm.guard {
                    walk_expr(g, f);
                }
                walk_expr(&mut arm.body, f);
            }
        }
        ExprKind::Block { stmts, tail } => {
            for stmt in stmts {
                match stmt {
                    Stmt::Let { ty, value, .. } => {
                        if let Some(t) = ty {
                            walk_type(t, f);
                        }
                        walk_expr(value, f);
                    }
                    Stmt::Expr(e) => walk_expr(e, f),
                }
            }
            if let Some(t) = tail {
                walk_expr(t, f);
            }
        }
        ExprKind::Record { fields } => {
            for (_, v) in fields {
                walk_expr(v, f);
            }
        }
        // Row expansion runs before record-update and `?` expansion, so it walks the sugar.
        ExprKind::RecordUpdate { base, fields } => {
            walk_expr(base, f);
            for (_, v) in fields {
                walk_expr(v, f);
            }
        }
        ExprKind::Field { base, .. } => walk_expr(base, f),
        ExprKind::Try { operand } => walk_expr(operand, f),
        ExprKind::List { items } => {
            for i in items {
                walk_expr(i, f);
            }
        }
        ExprKind::Perform { args, .. } => {
            for a in args {
                walk_expr(a, f);
            }
        }
        ExprKind::Handle {
            body,
            clauses,
            return_clause,
        } => {
            walk_expr(body, f);
            for c in clauses {
                walk_expr(&mut c.body, f);
            }
            if let Some(r) = return_clause {
                walk_expr(&mut r.body, f);
            }
        }
        ExprKind::WithCell { init, body, .. } => {
            walk_expr(init, f);
            walk_expr(body, f);
        }
        ExprKind::WithRegion { body, .. } => walk_expr(body, f),
        ExprKind::Simulate { body } => walk_expr(body, f),
    })
}

pub fn grow<T>(f: impl FnOnce() -> T) -> T {
    const RED_ZONE: usize = 256 * 1024;
    const NEW_SEGMENT: usize = 2 * 1024 * 1024;
    stacker::maybe_grow(RED_ZONE, NEW_SEGMENT, f)
}
