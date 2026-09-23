use crate::hosts::{Hosts, hosting};
use crate::load::Loaded;
use ply_eval::HostRuntime;
use ply_span::{Diagnostic, SourceId, Span, Symbol, codes};
use ply_test::{Engine, RunReport};
use ply_ty::{DefInfo, HashOutput};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

pub struct Mutant {
    pub definition: Symbol,
    pub span: Span,
    pub from: String,
    pub to: String,
}

pub enum Verdict {
    Killed,
    Survived,
    /// The mutant is not a program, or no test reaches the definition.
    Skipped(&'static str),
    /// The tests neither all passed nor any failed for the mutant's sake.
    Unresolved(&'static str),
}

pub struct Judged {
    pub mutant: Mutant,
    pub verdict: Verdict,
    pub tests: Vec<Symbol>,
}

#[derive(Default)]
pub struct Report {
    pub definitions: usize,
    pub generated: usize,
    pub judged: Vec<Judged>,
    pub unreached: Vec<Symbol>,
    pub budget_spent: bool,
}

impl Report {
    pub fn count(&self, want: fn(&Verdict) -> bool) -> usize {
        self.judged.iter().filter(|j| want(&j.verdict)).count()
    }

    pub fn killed(&self) -> usize {
        self.count(|v| matches!(v, Verdict::Killed))
    }

    pub fn survived(&self) -> usize {
        self.count(|v| matches!(v, Verdict::Survived))
    }

    pub fn skipped(&self) -> usize {
        self.count(|v| matches!(v, Verdict::Skipped(_)))
    }

    pub fn unresolved(&self) -> usize {
        self.count(|v| matches!(v, Verdict::Unresolved(_)))
    }
}

/// The definitions `--mutate DEF` names: every project definition for `*`, else one by its
/// program-wide or unique simple name.
pub fn targets<'a>(loaded: &'a Loaded, query: &str) -> Result<Vec<&'a DefInfo>, Diagnostic> {
    let project: Vec<&DefInfo> = loaded
        .check
        .defs
        .values()
        .filter(|d| !ply_machine::shelf::is_shipped(&d.module))
        .collect();
    if query == "*" {
        return Ok(project);
    }
    let exact: Vec<&DefInfo> = project
        .iter()
        .copied()
        .filter(|d| d.name.as_str() == query)
        .collect();
    if !exact.is_empty() {
        return Ok(exact);
    }
    let simple: Vec<&DefInfo> = project
        .iter()
        .copied()
        .filter(|d| d.simple_name.as_str() == query)
        .collect();
    match simple.len() {
        1 => Ok(simple),
        0 => Err(Diagnostic::error(
            codes::UNKNOWN_NAME,
            format!("no definition is named `{query}`"),
        )
        .primary(Span::DUMMY, "not a definition of this project")
        .note("`--mutate` takes a program-wide name, a simple name unique in the project, or nothing for every definition")),
        _ => Err(Diagnostic::error(
            codes::UNKNOWN_NAME,
            format!("`{query}` names {} definitions", simple.len()),
        )
        .primary(Span::DUMMY, "say which")
        .note(format!(
            "one of: {}",
            simple.iter().map(|d| d.name.as_str()).collect::<Vec<_>>().join(", ")
        ))),
    }
}

/// The tests whose closure holds the definition, by index.
pub fn reaching(loaded: &Loaded, hashes: &HashOutput, name: &Symbol) -> Vec<usize> {
    loaded
        .check
        .tests
        .iter()
        .filter(|t| hashes.closure.get(&t.key).is_some_and(|c| c.contains(name)))
        .map(|t| t.index)
        .collect()
}

struct Site {
    start: usize,
    end: usize,
    to: String,
}

/// One change per operator or literal in the text, outside strings and comments. A site that is
/// not one in the grammar (a `<` opening a type argument) yields a mutant that does not check,
/// which the run skips.
fn sites(text: &str) -> Vec<Site> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let site = |out: &mut Vec<Site>, start: usize, end: usize, to: &str| {
        out.push(Site {
            start,
            end,
            to: to.to_string(),
        })
    };
    while i < b.len() {
        let c = b[i];
        let next = b.get(i + 1).copied().unwrap_or(0);
        match c {
            b'/' if next == b'/' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                i += 1;
            }
            b'+' if next == b'+' => i += 2,
            b'+' => {
                site(&mut out, i, i + 1, "-");
                i += 1;
            }
            b'-' if next == b'>' => i += 2,
            b'-' => {
                site(&mut out, i, i + 1, "+");
                i += 1;
            }
            b'<' if next == b'<' => i += 2,
            b'<' if next == b'=' => {
                site(&mut out, i, i + 2, "<");
                i += 2;
            }
            b'<' => {
                site(&mut out, i, i + 1, "<=");
                i += 1;
            }
            b'>' if next == b'>' => {
                i += if b.get(i + 2) == Some(&b'>') { 3 } else { 2 };
            }
            b'>' if next == b'=' => {
                site(&mut out, i, i + 2, ">");
                i += 2;
            }
            b'>' => {
                site(&mut out, i, i + 1, ">=");
                i += 1;
            }
            b'=' if next == b'=' => {
                site(&mut out, i, i + 2, "!=");
                i += 2;
            }
            b'!' if next == b'=' => {
                site(&mut out, i, i + 2, "==");
                i += 2;
            }
            b'!' => {
                site(&mut out, i, i + 1, "");
                i += 1;
            }
            b'&' if next == b'&' => {
                site(&mut out, i, i + 2, "||");
                i += 2;
            }
            b'|' if next == b'|' => {
                site(&mut out, i, i + 2, "&&");
                i += 2;
            }
            c if c.is_ascii_alphabetic() || c == b'_' => {
                let start = i;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                    i += 1;
                }
                match &text[start..i] {
                    "true" => site(&mut out, start, i, "false"),
                    "false" => site(&mut out, start, i, "true"),
                    _ => {}
                }
            }
            c if c.is_ascii_digit() => {
                let start = i;
                while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') {
                    i += 1;
                }
                let after = b.get(i).copied().unwrap_or(0);
                let literal = &text[start..i];
                let plain = !(after.is_ascii_alphabetic() || after == b'_' || after == b'.');
                if plain
                    && let Ok(n) = literal.replace('_', "").parse::<i64>()
                    && let Some(bumped) = n.checked_add(1)
                {
                    site(&mut out, start, i, &bumped.to_string());
                }
            }
            _ => i += 1,
        }
    }
    out
}

/// Every mutant of `def`, in source order.
pub fn mutants(loaded: &Loaded, def: &DefInfo) -> Vec<Mutant> {
    let source = loaded.sources.snippet(def.span);
    let text: &str = &source;
    sites(text)
        .into_iter()
        .map(|s| Mutant {
            definition: def.name.clone(),
            span: Span::new(
                def.span.source,
                def.span.start + s.start as u32,
                def.span.start + s.end as u32,
            ),
            from: text[s.start..s.end].to_string(),
            to: s.to,
        })
        .collect()
}

/// The program with one span replaced, as the front end reads it.
fn spliced(loaded: &Loaded, mutant: &Mutant) -> (Vec<(String, String)>, Vec<SourceId>) {
    let mut modules: Vec<&ply_ty::ModuleInfo> = loaded.check.modules.values().collect();
    modules.sort_by_key(|m| m.source.0);
    let sources: Vec<(String, String)> = modules
        .iter()
        .map(|m| {
            let text = loaded.sources.get(m.source).map_or("", |f| &*f.text);
            let text = if m.source == mutant.span.source {
                let (start, end) = (mutant.span.start as usize, mutant.span.end as usize);
                format!("{}{}{}", &text[..start], mutant.to, &text[end..])
            } else {
                text.to_string()
            };
            (m.name.to_string(), text)
        })
        .collect();
    let ids = (0..sources.len()).map(|i| SourceId(i as u32)).collect();
    (sources, ids)
}

/// Judges every mutant of every target, cheapest first, up to the budget; the honest program
/// is assumed green, since a survivor means nothing otherwise.
#[allow(clippy::too_many_arguments)]
pub fn run<F>(
    loaded: &Loaded,
    hashes: &HashOutput,
    targets: &[&DefInfo],
    budget: usize,
    search: &ply_eval::sim::Plan,
    engine: &Engine,
    backend: &ply_eval::BackendSpec,
    hosts: &Hosts,
    runtime: &Option<F>,
) -> Report
where
    F: Fn() -> Rc<dyn HostRuntime> + Sync,
{
    let mut report = Report {
        definitions: targets.len(),
        ..Report::default()
    };
    let mut queue: Vec<(Vec<usize>, Mutant)> = Vec::new();
    for def in targets {
        let reached = reaching(loaded, hashes, &def.name);
        if reached.is_empty() {
            report.unreached.push(def.name.clone());
            continue;
        }
        for mutant in mutants(loaded, def) {
            queue.push((reached.clone(), mutant));
        }
    }
    report.generated = queue.len();
    queue.sort_by_key(|(reached, _)| reached.len());
    let scratch = crate::test::Cache::scratch();
    let Ok(mut scratch) = scratch else {
        return report;
    };
    for (n, (reached, mutant)) in queue.into_iter().enumerate() {
        if n >= budget {
            report.budget_spent = true;
            break;
        }
        let tests: Vec<Symbol> = reached
            .iter()
            .map(|&i| loaded.check.tests[i].key.clone())
            .collect();
        let verdict = judge(
            loaded,
            &mutant,
            &reached,
            search,
            engine,
            backend,
            hosts,
            runtime,
            &mut scratch.store,
        );
        report.judged.push(Judged {
            mutant,
            verdict,
            tests,
        });
    }
    report
}

#[allow(clippy::too_many_arguments)]
fn judge<F>(
    loaded: &Loaded,
    mutant: &Mutant,
    reached: &[usize],
    search: &ply_eval::sim::Plan,
    engine: &Engine,
    backend: &ply_eval::BackendSpec,
    hosts: &Hosts,
    runtime: &Option<F>,
    store: &mut ply_store::Store,
) -> Verdict
where
    F: Fn() -> Rc<dyn HostRuntime> + Sync,
{
    let (sources, ids) = spliced(loaded, mutant);
    let Ok(front) = ply_codegen::c::producer::checked_front(&sources, &ids) else {
        return Verdict::Skipped("does not check");
    };
    let texts: std::collections::HashMap<String, String> = sources.iter().cloned().collect();
    let Ok(provider) = ply_codegen::Unit::over_front(&front, texts) else {
        return Verdict::Unresolved("the C backend could not be built");
    };
    let mut selection = ply_test::select(&front.check, &front.hashes, store, search, engine);
    let wanted: BTreeSet<usize> = reached.iter().copied().collect();
    selection.to_run.retain(|i| wanted.contains(i));
    selection
        .groups
        .iter_mut()
        .for_each(|g| g.retain(|i| wanted.contains(i)));
    selection.groups.retain(|g| !g.is_empty());
    let expected = selection.to_run.len();
    let outcome: Result<RunReport, _> = catch_unwind(AssertUnwindSafe(|| {
        let executor = ply_test::InterpExecutor::new(&front)
            .with_search(ply_test::Search::of(&selection))
            .with_hosts(hosting(hosts, runtime))
            .with_backend(provider, backend.clone());
        ply_test::run_with(&selection, &front.check, &front.hashes, store, &executor)
    }));
    match outcome {
        Err(_) => Verdict::Unresolved("the run panicked"),
        Ok(report) if report.failed > 0 => Verdict::Killed,
        Ok(report) if report.passed == expected => Verdict::Survived,
        Ok(_) => Verdict::Unresolved("not every test answered"),
    }
}

pub fn to_json(report: &Report, loaded: &Loaded) -> Value {
    let judged = |want: fn(&Verdict) -> bool| -> Vec<Value> {
        report
            .judged
            .iter()
            .filter(|j| want(&j.verdict))
            .map(|j| {
                json!({
                    "definition": j.mutant.definition.as_str(),
                    "from": j.mutant.from,
                    "to": j.mutant.to,
                    "location": crate::test::location_json(&loaded.sources, j.mutant.span),
                    "tests": j.tests.iter().map(|t| t.as_str()).collect::<Vec<_>>(),
                    "why": match &j.verdict {
                        Verdict::Skipped(why) | Verdict::Unresolved(why) => Value::String((*why).to_string()),
                        _ => Value::Null,
                    },
                })
            })
            .collect()
    };
    json!({
        "definitions": report.definitions,
        "generated": report.generated,
        "run": report.judged.len(),
        "killed": report.killed(),
        "survived": report.survived(),
        "skipped": report.skipped(),
        "unresolved": report.unresolved(),
        "budget_spent": report.budget_spent,
        "unreached": report.unreached.iter().map(|n| n.as_str()).collect::<Vec<_>>(),
        "survivors": judged(|v| matches!(v, Verdict::Survived)),
        "skips": judged(|v| matches!(v, Verdict::Skipped(_) | Verdict::Unresolved(_))),
    })
}

/// Which tests reach each project definition, from the hash closure alone.
pub fn coverage_json(loaded: &Loaded, hashes: &HashOutput) -> Value {
    let mut unreached = Vec::new();
    let definitions: Vec<Value> = loaded
        .check
        .defs
        .values()
        .filter(|d| !ply_machine::shelf::is_shipped(&d.module))
        .map(|d| {
            let tests: Vec<&str> = reaching(loaded, hashes, &d.name)
                .into_iter()
                .map(|i| loaded.check.tests[i].key.as_str())
                .collect();
            if tests.is_empty() {
                unreached.push(d.name.as_str());
            }
            json!({ "name": d.name.as_str(), "reached_by": tests })
        })
        .collect();
    json!({ "definitions": definitions, "unreached": unreached })
}
