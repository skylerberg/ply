//! The ownership checker, judged against the counters rather than against itself — ADR 0025
//! §Decision 2b.

use ply_eval::costs::{Cause, Costs, DefKind, Definition, Verdict};
use ply_eval::rc;
use ply_eval::{Machine, TaskRegions};
use ply_span::{SourceId, SourceMap, Span};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::parse_program;
use ply_syntax::resolve::{Resolved, resolve};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the crate sits two levels under the workspace root")
        .to_path_buf()
}

struct Loaded {
    program: Program,
    resolved: Resolved,
    map: SourceMap,
    /// The module the target file declares, and its source id.
    target: usize,
    source: SourceId,
}

fn std_imports(id: SourceId, name: &ModuleName, text: &str) -> Vec<ModuleName> {
    let Ok(module) = ply_syntax::parse_module(id, name.clone(), text) else {
        return Vec::new();
    };
    module
        .imports
        .iter()
        .map(|i| i.module_name())
        .filter(ply_std::is_std)
        .collect()
}

/// One file, plus every `std` module it reaches, transitively.
fn load(entry: &ModuleName, path: PathBuf, text: String) -> Option<Loaded> {
    let mut map = SourceMap::new();
    let id = map.add(&path, text.clone());
    let mut loaded = vec![(id, entry.clone(), text)];
    let mut next = 0;
    while next < loaded.len() {
        let (id, name, text) = loaded[next].clone();
        next += 1;
        for module in std_imports(id, &name, &text) {
            if loaded.iter().any(|(_, n, _)| *n == module) {
                continue;
            }
            let Some(source) = ply_std::source(&module) else {
                continue;
            };
            let id = map.add(ply_std::pseudo_path(&module), source.to_string());
            loaded.push((id, module, source.to_string()));
        }
    }
    let inputs: Vec<_> = loaded
        .iter()
        .map(|(id, name, text)| (*id, name.clone(), text.as_str()))
        .collect();
    let mut program = parse_program(inputs).ok()?;
    if !ply_derive::expand_program(&mut program).is_empty() {
        return None;
    }
    let resolved = resolve(&mut program).ok()?;
    let target = program.index_of(entry)?;
    Some(Loaded {
        program,
        resolved,
        map,
        target,
        source: id,
    })
}

fn load_std(module: &str) -> Option<Loaded> {
    let name = ModuleName::from_dotted(module);
    let text = ply_std::source(&name)?.to_string();
    load(&name, ply_std::pseudo_path(&name), text)
}

/// The module name is given rather than derived: `spikes/ply-lexer/` is not an identifier path, so
/// `ModuleName::from_relative_path` refuses it, and a file the brief names as the hardest realistic
/// program in the tree would then have been silently absent from the table.
fn load_file(root: &Path, relative: &str, module: &str) -> Option<Loaded> {
    let path = root.join(relative);
    let text = std::fs::read_to_string(&path).ok()?;
    load(&ModuleName::from_dotted(module), path, text)
}

/// Every target the brief names: the eight shipped `std` modules, the example service, and the
/// hardest realistic program in the tree.
fn targets(root: &Path) -> Vec<(String, Loaded)> {
    let mut out = Vec::new();
    for module in [
        "std.config",
        "std.db",
        "std.http",
        "std.json",
        "std.net",
        "std.router",
        "std.signal",
        "std.trace",
    ] {
        if let Some(loaded) = load_std(module) {
            out.push((module.to_string(), loaded));
        }
    }
    for (file, name) in [
        ("examples/desk.ply", "desk"),
        ("spikes/ply-lexer/lexer.ply", "lexer"),
    ] {
        if let Some(loaded) = load_file(root, file, name) {
            out.push((file.to_string(), loaded));
        }
    }
    out
}

/// What the run said, for the sites in one file.
fn run(loaded: &Loaded) -> Vec<(Span, rc::SiteCount)> {
    let mut machine = Machine::for_program(&loaded.program, &loaded.resolved);
    machine.set_regions(TaskRegions::new());
    rc::record_sites(true);
    for index in 0..machine.test_count() {
        // A test's answer is `differential_corpus`'s business; what it *cost* is this one's.
        let _ = machine.eval_test(index);
    }
    let mut sites: Vec<(Span, rc::SiteCount)> = rc::sites()
        .into_iter()
        .filter(|(span, _)| span.source == loaded.source)
        .collect();
    rc::record_sites(false);
    sites.sort_by_key(|(span, _)| (span.start, span.end));
    sites
}

fn line_of(map: &SourceMap, span: Span) -> u32 {
    map.get(span.source)
        .map(|f| f.line_col(span.start).0)
        .unwrap_or(0)
}

/// One file's counts.
#[derive(Default, Clone)]
struct Tally {
    defs: usize,
    defs_with_push: usize,
    /// Every append in it reuses: no source edit, no annotation, nothing to do.
    defs_clean: usize,
    /// At least one append copies, so something in the source has to move.
    defs_needing_edit: usize,
    /// No append copies but at least one is undecidable.
    defs_undecided: usize,
    /// Of `defs_needing_edit`, the ones whose only copying causes are positional — ADR 0025's P1
    /// and P2 are evaluator changes that remove these with **no source edit at all**.
    edit_position: usize,
    /// Of `defs_needing_edit`, the ones that need a mechanical library migration and nothing else:
    /// `cell_update` / `map_update`.
    edit_mechanical: usize,
    /// Of `defs_needing_edit`, the ones where an author has to restructure — a closure captured the
    /// scope, or a caller keeps what it passes.
    edit_hard: usize,
    reuses: usize,
    copies: usize,
    unknown: usize,
    executed: usize,
    agree: usize,
    disagree: usize,
    unknown_executed: usize,
    /// `Reuses` at a site the run shows copying — the error that certifies a quadratic as linear.
    false_green: usize,
    /// `Copies` at a site the run shows reusing.
    false_red: usize,
    /// Every copying or undecidable site, by what caused it.
    causes: BTreeMap<Cause, usize>,
}

impl Tally {
    fn add(&mut self, other: &Tally) {
        self.defs += other.defs;
        self.defs_with_push += other.defs_with_push;
        self.defs_clean += other.defs_clean;
        self.defs_needing_edit += other.defs_needing_edit;
        self.defs_undecided += other.defs_undecided;
        self.edit_position += other.edit_position;
        self.edit_mechanical += other.edit_mechanical;
        self.edit_hard += other.edit_hard;
        self.reuses += other.reuses;
        self.copies += other.copies;
        self.unknown += other.unknown;
        self.executed += other.executed;
        self.agree += other.agree;
        self.disagree += other.disagree;
        self.unknown_executed += other.unknown_executed;
        self.false_green += other.false_green;
        self.false_red += other.false_red;
        for (cause, n) in &other.causes {
            *self.causes.entry(*cause).or_default() += n;
        }
    }

    /// The number the brief asks for: of the definitions that contain an append, the fraction that
    /// need nothing done to them.
    fn clean_rate(&self) -> Option<f64> {
        (self.defs_with_push > 0).then(|| self.defs_clean as f64 / self.defs_with_push as f64)
    }
}

fn tally(defs: &[Definition], oracle: &[(Span, rc::SiteCount)], only_fns: bool) -> Tally {
    let mut t = Tally::default();
    for def in defs {
        if only_fns && def.kind != DefKind::Fn {
            continue;
        }
        t.defs += 1;
        if def.sites.is_empty() {
            continue;
        }
        t.defs_with_push += 1;
        if def.copies() > 0 {
            t.defs_needing_edit += 1;
            let causes: Vec<Cause> = def
                .sites
                .iter()
                .filter(|s| s.verdict == Verdict::Copies)
                .filter_map(|s| s.cause)
                .collect();
            let hard = causes.iter().any(|c| {
                matches!(
                    c,
                    Cause::Capture | Cause::CallerKeeps | Cause::Element | Cause::Program
                )
            });
            let mechanical = causes
                .iter()
                .any(|c| matches!(c, Cause::Cell | Cause::MapEntry));
            if hard {
                t.edit_hard += 1;
            } else if mechanical {
                t.edit_mechanical += 1;
            } else {
                t.edit_position += 1;
            }
        } else if def.unknown() > 0 {
            t.defs_undecided += 1;
        } else {
            t.defs_clean += 1;
        }
        for site in &def.sites {
            match site.verdict {
                Verdict::Reuses => t.reuses += 1,
                Verdict::Copies => t.copies += 1,
                Verdict::Unknown => t.unknown += 1,
            }
            if let Some(cause) = site.cause {
                *t.causes.entry(cause).or_default() += 1;
            }
            let Some((_, counted)) = oracle.iter().find(|(s, _)| *s == site.span) else {
                continue;
            };
            let Some(rate) = counted.rate() else { continue };
            t.executed += 1;
            match site.verdict {
                Verdict::Reuses if rate >= 0.99 => t.agree += 1,
                Verdict::Copies if rate <= 0.01 => t.agree += 1,
                Verdict::Unknown => t.unknown_executed += 1,
                Verdict::Reuses => {
                    t.disagree += 1;
                    t.false_green += 1;
                }
                Verdict::Copies => {
                    t.disagree += 1;
                    t.false_red += 1;
                }
            }
        }
    }
    t
}

fn header(what: &str) {
    println!(
        "\n{:<28} {:>5} {:>6} {:>6} {:>6} {:>6} | {:>4} {:>5} {:>5} | {:>6} {:>6} {:>4}   no edit",
        "file",
        what,
        "w/push",
        "clean",
        "edit",
        "undec",
        "pos",
        "mech",
        "hard",
        "reuses",
        "COPIES",
        "unk",
    );
}

fn row(label: &str, t: &Tally) {
    println!(
        "{:<28} {:>5} {:>6} {:>6} {:>6} {:>6} | {:>4} {:>5} {:>5} | {:>6} {:>6} {:>4}   {}",
        label,
        t.defs,
        t.defs_with_push,
        t.defs_clean,
        t.defs_needing_edit,
        t.defs_undecided,
        t.edit_position,
        t.edit_mechanical,
        t.edit_hard,
        t.reuses,
        t.copies,
        t.unknown,
        match t.clean_rate() {
            Some(r) => format!("{:.0}%", r * 100.0),
            None => "—".to_string(),
        },
    );
}

#[test]
fn the_checker_is_measured_against_the_counters_over_every_shipped_module() {
    let root = workspace_root();
    let mut fns = Tally::default();
    let mut all = Tally::default();
    let mut sites_executed = 0usize;

    let mut per_file = Vec::new();
    let mut rows_fns = Vec::new();
    let mut rows_all = Vec::new();
    for (label, loaded) in targets(&root) {
        let costs = Costs::new(&loaded.program, &loaded.resolved);
        let report = costs.check();
        assert!(
            report.rounds < 24,
            "{label}: the whole-program fixpoint did not converge in {} rounds, so its \
             parameter answers are the last round's rather than the answer",
            report.rounds
        );
        let defs: Vec<Definition> = report.module(loaded.target).into_iter().cloned().collect();
        let oracle = run(&loaded);
        let t_fns = tally(&defs, &oracle, true);
        let t_all = tally(&defs, &oracle, false);
        sites_executed += t_all.executed;
        fns.add(&t_fns);
        all.add(&t_all);
        rows_fns.push((label.clone(), t_fns));
        rows_all.push((label.clone(), t_all));
        per_file.push((label, defs, oracle, loaded, report));
    }

    println!(
        "\n=== THE ANNOTATION BURDEN ===\n\
         ADR 0025 has no annotation, so its burden is zero by construction and reporting\n\
         that would be vacuous. What is counted here is the honest translation: a FORCED\n\
         SOURCE EDIT. A definition is `clean` when every append in it reuses — nothing to\n\
         write, nothing to move. It needs an `edit` when an append copies, and the cause\n\
         table below says which edit.\n\n\
         The three `edit` columns are what a reader has to see before the total:\n\
           pos  — only positional causes. ADR 0025's P1 and P2 are EVALUATOR changes;\n\
                  they remove these with no source edit at all. MEASURED as copying\n\
                  today, PROJECTED to zero after P1/P2 — the projection is not mine to\n\
                  claim and is labelled as one.\n\
           mech — `push(cell_get(c), x)` and `push(map_get(..), x)`. A one-line\n\
                  mechanical migration to `cell_update`/`map_update` per site.\n\
           hard — a closure captured the scope, or a caller keeps what it passes. These\n\
                  are the ones an author has to think about, and NO scheduled change\n\
                  removes them."
    );

    println!("\n-- `fn` definitions only, which is what a module's interface is made of --");
    header("fns");
    for (label, t) in &rows_fns {
        row(label, t);
    }
    row("TOTAL", &fns);

    println!("\n-- every body, `test` and `law` included --");
    header("defs");
    for (label, t) in &rows_all {
        row(label, t);
    }
    row("TOTAL", &all);

    println!("\n-- what the edit is, over every copying or undecidable site --");
    println!("{:<10} {:>6}   the edit that removes it", "cause", "sites");
    for (cause, n) in &all.causes {
        println!(
            "{:<10} {:>6}   {}",
            cause.as_str(),
            n,
            cause
                .fix()
                .unwrap_or("— no source edit removes it; the copy is what the semantics require"),
        );
    }

    println!("\n=== THE CROSS-CHECK ===\n--- every executed site, checker beside the run ---");
    println!(
        "{:<34} {:<7} {:>8} {:>8} {:>7}  definition",
        "site", "verdict", "in place", "copies", "rate",
    );
    for (label, defs, oracle, loaded, _) in &per_file {
        for def in defs {
            for site in &def.sites {
                let Some((_, counted)) = oracle.iter().find(|(s, _)| *s == site.span) else {
                    continue;
                };
                let Some(rate) = counted.rate() else { continue };
                let agrees = match site.verdict {
                    Verdict::Reuses => rate >= 0.99,
                    Verdict::Copies => rate <= 0.01,
                    Verdict::Unknown => true,
                };
                println!(
                    "{:<34} {:<7} {:>8} {:>8} {:>6.1}% {} {}",
                    format!("{}:{}", short(label), line_of(&loaded.map, site.span)),
                    site.verdict.as_str(),
                    counted.in_place,
                    counted.copies,
                    rate * 100.0,
                    if agrees { " " } else { "MISMATCH" },
                    def.name,
                );
            }
        }
    }

    println!("\n--- what the checker says copies, whether or not a test runs it ---");
    for (label, defs, oracle, loaded, _) in &per_file {
        for def in defs {
            for site in &def.sites {
                if site.verdict != Verdict::Copies {
                    continue;
                }
                let counted = oracle
                    .iter()
                    .find(|(s, _)| *s == site.span)
                    .map(|(_, c)| *c)
                    .unwrap_or_default();
                println!(
                    "{}:{}  {}  [{} in place / {} copied]  cause: {}\n    {}",
                    short(label),
                    line_of(&loaded.map, site.span),
                    def.name,
                    counted.in_place,
                    counted.copies,
                    site.cause.map(Cause::as_str).unwrap_or("—"),
                    site.reason,
                );
            }
        }
    }

    println!("\n--- what the checker cannot decide, and why ---");
    for (label, defs, oracle, loaded, _) in &per_file {
        for def in defs {
            for site in &def.sites {
                if site.verdict != Verdict::Unknown {
                    continue;
                }
                let counted = oracle
                    .iter()
                    .find(|(s, _)| *s == site.span)
                    .map(|(_, c)| *c)
                    .unwrap_or_default();
                println!(
                    "{}:{}  {}  [{} in place / {} copied]  cause: {}\n    {}",
                    short(label),
                    line_of(&loaded.map, site.span),
                    def.name,
                    counted.in_place,
                    counted.copies,
                    site.cause.map(Cause::as_str).unwrap_or("—"),
                    site.reason,
                );
            }
        }
    }

    println!(
        "\n=== ADR 0025 §Decision 2b, measured rather than argued ===\n\
         The ADR registers, before building it: \"every `push` whose list argument the \
         lowering\n marked `Own::Owned` must be counted in place, or the test fails\" \
         — and predicts\n that this \"will fail on the tree as it stands\". Both halves \
         are checked here."
    );
    let mut own_total = 0usize;
    let mut own_executed = 0usize;
    let mut own_in_place = 0usize;
    let mut own_violations: Vec<String> = Vec::new();
    for (label, defs, oracle, loaded, _) in &per_file {
        for def in defs {
            for site in &def.sites {
                if !site.own_marked {
                    continue;
                }
                own_total += 1;
                let Some((_, counted)) = oracle.iter().find(|(s, _)| *s == site.span) else {
                    continue;
                };
                let Some(rate) = counted.rate() else { continue };
                own_executed += 1;
                if rate >= 0.99 {
                    own_in_place += 1;
                } else {
                    own_violations.push(format!(
                        "{}:{}  {}  marked Owned, ran {} in place / {} copied ({:.1}%) \
                         — this checker says {}",
                        short(label),
                        line_of(&loaded.map, site.span),
                        def.name,
                        counted.in_place,
                        counted.copies,
                        rate * 100.0,
                        site.verdict.as_str(),
                    ));
                }
            }
        }
    }
    println!(
        "sites whose list argument is marked `Own::Owned`: {own_total}          ({own_executed} executed, {own_in_place} counted in place)"
    );
    for v in &own_violations {
        println!("  VIOLATION {v}");
    }
    if own_violations.is_empty() {
        println!("  (none — the ADR's predicted failure did not occur)");
    }
    println!(
        "the checker's own verdict covers {} sites; `Own::Owned` covers {own_total}, \
         because only a `Var` node carries it and `push(s.field, x)` is not one",
        all.reuses + all.copies + all.unknown,
    );
    assert!(
        own_executed > 0,
        "no `Own::Owned`-marked append ran, so ADR 0025 §Decision 2b's proposal was not \
         measured and the paragraph above says nothing"
    );

    println!(
        "\n--- appends the RUN counted that the CHECKER never saw ---\n\
         (a site missing here is one the checker is blind to, and blindness \
         flatters every rate above)"
    );
    let mut blind: Vec<String> = Vec::new();
    for (label, defs, oracle, loaded, _) in &per_file {
        let known: std::collections::HashSet<Span> = defs
            .iter()
            .flat_map(|d| d.sites.iter().map(|s| s.span))
            .collect();
        for (span, counted) in oracle {
            if known.contains(span) {
                continue;
            }
            let line = format!(
                "{}:{}  [{} in place / {} copied]",
                short(label),
                line_of(&loaded.map, *span),
                counted.in_place,
                counted.copies,
            );
            println!("{line}");
            blind.push(line);
        }
    }
    if blind.is_empty() {
        println!("(none — every append the corpus executed has a verdict)");
    }

    println!(
        "\n--- parameters the whole-program fixpoint could not keep sole-owned ---\n\
         (the blame behind every `caller` cause above)"
    );
    for (label, _, _, loaded, report) in &per_file {
        let prefix = loaded.program.modules[loaded.target]
            .name
            .as_str()
            .to_string();
        for (name, slot, why) in &report.spoiled {
            if !name.starts_with(&prefix) {
                continue;
            }
            println!("{}  {name} parameter {slot}\n    {why}", short(label));
        }
    }

    println!("\n--- every site the checker got wrong ---");
    let mut false_greens: Vec<String> = Vec::new();
    for (label, defs, oracle, loaded, _) in &per_file {
        for def in defs {
            for site in &def.sites {
                let Some((_, counted)) = oracle.iter().find(|(s, _)| *s == site.span) else {
                    continue;
                };
                let Some(rate) = counted.rate() else { continue };
                let wrong = match site.verdict {
                    Verdict::Reuses => rate < 0.99,
                    Verdict::Copies => rate > 0.01,
                    Verdict::Unknown => false,
                };
                if !wrong {
                    continue;
                }
                let kind = if site.verdict == Verdict::Reuses {
                    "FALSE GREEN"
                } else {
                    "false red "
                };
                let line = format!(
                    "{} {}:{}  {}  said {}, ran {} in place / {} copied ({:.1}%)\n    {}",
                    kind,
                    short(label),
                    line_of(&loaded.map, site.span),
                    def.name,
                    site.verdict.as_str(),
                    counted.in_place,
                    counted.copies,
                    rate * 100.0,
                    site.reason,
                );
                println!("{line}");
                if site.verdict == Verdict::Reuses {
                    false_greens.push(line);
                }
            }
        }
    }
    if false_greens.is_empty() {
        println!("(none in the false-green direction)");
    }

    println!(
        "\nexecuted sites: {}   agree: {}   disagree: {}   unknown: {}   \
         false green: {}   false red: {}",
        all.executed, all.agree, all.disagree, all.unknown_executed, all.false_green, all.false_red,
    );
    let total_sites = all.reuses + all.copies + all.unknown;
    let coverage = if total_sites == 0 {
        0.0
    } else {
        (all.executed + all.unknown_executed) as f64 / total_sites as f64
    };
    println!(
        "coverage: {:.1}% of the checker's {total_sites} sites were executed by some test",
        coverage * 100.0
    );

    assert!(
        blind.is_empty(),
        "the run counted {} append(s) the checker produced no verdict for. Every rate \
         above is computed over the sites it did see, so a blind site inflates all of \
         them.\n{}",
        blind.len(),
        blind.join("\n"),
    );
    assert!(
        sites_executed >= 20,
        "only {sites_executed} push sites ran, so an agreement rate would be one program's"
    );
    let decided = all.agree + all.disagree;
    assert!(decided > 0, "no site was both decided and executed");
    let rate = all.agree as f64 / decided as f64;
    println!("agreement on decided, executed sites: {:.1}%", rate * 100.0);

    // Both bars were fixed in `/tmp/ownership-burden/prereg.md` before the first run of this test,
    // and neither has moved since.
    assert!(
        false_greens.is_empty(),
        "the checker told {} site(s) they reuse where the run copied. That is the \
         error a cost checker may not make: it certifies a quadratic as linear.\n{}",
        false_greens.len(),
        false_greens.join("\n"),
    );
    assert!(
        rate >= 0.80,
        "the checker agrees with the counters at {:.1}% of decided, executed sites, \
         which is below the 0.80 floor a warning needs to be worth raising",
        rate * 100.0,
    );
}

/// `std.json`'s `escape_runs` is the quadratic that shipped, on client-influenced input, and it is
/// the site this whole line of work exists because of.
#[test]
fn the_checker_and_the_counters_agree_on_every_append_in_std_json() {
    let loaded = load_std("std.json").expect("`std.json` must load");
    let costs = Costs::new(&loaded.program, &loaded.resolved);
    let report = costs.check();
    let defs = report.module(loaded.target);
    let oracle = run(&loaded);

    let mut exercised = 0usize;
    let mut disagreements = Vec::new();
    for def in defs.iter() {
        for site in &def.sites {
            let Some((_, counted)) = oracle.iter().find(|(s, _)| *s == site.span) else {
                continue;
            };
            if counted.in_place + counted.copies == 0 {
                continue;
            }
            exercised += 1;
            let agrees = match site.verdict {
                Verdict::Copies => counted.copies > 0,
                Verdict::Reuses => counted.copies == 0,
                _ => true,
            };
            if !agrees {
                disagreements.push(format!(
                    "{} json.ply:{}  said {}  ran [{} in place / {} copied]  {}",
                    short(&def.name),
                    line_of(&loaded.map, site.span),
                    site.verdict.as_str(),
                    counted.in_place,
                    counted.copies,
                    site.reason,
                ));
            }
        }
    }

    // Non-vacuity first: an agreement over nothing agrees with anything, and that is this
    // repository's signature defect.
    assert!(
        exercised >= 5,
        "only {exercised} append site(s) in `std.json` were exercised, against 6 when \
         this was written, so agreement over them says nothing about the checker"
    );
    assert!(
        disagreements.is_empty(),
        "the checker and the run disagree at {} of {exercised} exercised site(s):\n{}",
        disagreements.len(),
        disagreements.join("\n"),
    );
}

fn short(label: &str) -> &str {
    label.rsplit('/').next().unwrap_or(label)
}
