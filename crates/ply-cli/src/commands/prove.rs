//! `ply prove` — every obligation, and the tier it was discharged at.
//!
//! The coverage line is **first** and is not behind a flag. A project with three
//! proved obligations and four hundred unspecified definitions must not print
//! three green ticks and stop, because that artifact is worse than the one M7
//! shipped: it invites a reviewer to stop reading at the moment there is most
//! left to read.

use super::common::{
    IND, build_pool, diagnostic_json, diagnostics_json, emit_json, location, millis, once_each,
    plural, print_diagnostics, print_warnings, report_bind_error, report_load_error,
};
use crate::cli::ProveArgs;
use crate::hosts::Hosts;
use crate::load::{Loaded, load, project_root};
use crate::style::Style;
use crate::{EXIT_COMPILE_ERROR, EXIT_FAILED, EXIT_OK, driver};
use ply_core::CheckOutput;
use ply_prove::{
    Binding, CaseReport, Certificate, Counterexample, Coverage, Discharge, Evidence, Frame, Gap,
    Obligation, ObligationKind, ProvePlan, ProveReport, Rule, Tier, Vacuity, VacuityKind,
};
use ply_span::{Diagnostic, SourceMap, Symbol, codes};
use ply_store::Store;
use ply_test::obligation::{self, Laws, Reason};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub fn execute(args: &ProveArgs, style: Style) -> i32 {
    let mut warnings = Vec::new();
    let root = project_root(&args.path);
    let mut store = match Store::open(&root) {
        Ok(store) => store,
        Err(e) => {
            let diagnostic = Diagnostic::error(
                codes::RUNTIME_ERROR,
                format!("could not open the cache under `{}`: {e:#}", root.display()),
            )
            .note("check the directory's permissions");
            if args.json {
                emit_json(&json!({
                    "command": "prove",
                    "ok": false,
                    "exit_code": EXIT_COMPILE_ERROR,
                    "diagnostics": [diagnostic_json(&diagnostic, &SourceMap::new())],
                }));
            } else {
                print_diagnostics(std::slice::from_ref(&diagnostic), &SourceMap::new(), style);
            }
            return EXIT_COMPILE_ERROR;
        }
    };
    warnings.extend(store.take_warnings());

    let loaded = match load_complete(
        &args.path,
        !args.no_incremental,
        args.json,
        "prove",
        &mut store,
        &mut warnings,
        style,
    ) {
        Ok(loaded) => loaded,
        Err(code) => return code,
    };
    warnings.extend(loaded.frontend.warnings.iter().cloned());

    let plan = crate::simulation::prove_plan(&args.prove, &args.simulation);
    let hashes = loaded.hashes.clone();
    let scoped = crate::obligations::project_view(&loaded.check, args.std);
    let laws = Laws::of(&scoped, &hashes);

    let collected = crate::obligations::collect(&loaded.program, &scoped, &hashes);
    warnings.extend(collected.warnings);
    // Counted before the filter and before anything is discharged: "carries a
    // claim" is a fact about the program, not about what this run chose to look
    // at or managed to establish.
    let specified = obligation::specified(&scoped, &laws, &collected.obligations);
    let (obligations, filtered_out) = filter(collected.obligations, args.filter.as_deref());

    // The row a `law/host` will enter, which is what decides whether this run
    // needs a database at all: a file whose laws are all hermetic binds nothing,
    // exactly as `ply test` binds nothing for a suite that installs the twin.
    let reach = ply_core::ty::Footprint::from_atoms(
        scoped
            .laws
            .iter()
            .filter(|law| law.host)
            .flat_map(|law| law.footprint.atoms().cloned()),
    );
    let db = match args.db.resolve(args.host) {
        Ok(db) => db,
        Err(diagnostics) => {
            return report_bind_error("prove", &diagnostics, &loaded.sources, args.json, style);
        }
    };
    let (configuration, config_warnings) = match crate::config::Configuration::open(
        &loaded.program,
        &loaded.resolved,
        &loaded.check,
        args.host,
        &args.config,
    ) {
        Ok(resolved) => resolved,
        Err(diagnostics) => {
            return report_bind_error("prove", &diagnostics, &loaded.sources, args.json, style);
        }
    };
    warnings.extend(config_warnings);
    let hosts = match Hosts::open(
        &loaded.check,
        args.host,
        &args.tls.tls,
        db,
        configuration,
        &args.trace,
        Some(&reach),
    ) {
        Ok(hosts) => hosts,
        Err(diagnostics) => {
            return report_bind_error("prove", &diagnostics, &loaded.sources, args.json, style);
        }
    };
    let runtime = hosts.runtime_factory();
    let hosting = args.host.then(|| crate::engine::Hosting {
        binding: hosts.binding(),
        runtime: runtime
            .as_ref()
            .map(|f| f as &(dyn Fn() -> std::rc::Rc<dyn ply_eval::host::HostRuntime> + Sync)),
    });
    let (engine, engine_warning) = crate::engine::of(
        &loaded.program,
        &loaded.resolved,
        &loaded.check,
        loaded.complete,
        obligations.len(),
        hosting,
    );
    warnings.extend(engine_warning);
    let (pool, _workers) = build_pool(args.jobs, &mut warnings);
    let discharge = || {
        obligation::prove(
            obligations,
            &scoped,
            &laws,
            &mut store,
            &plan,
            !args.no_cache,
            engine.as_ref(),
        )
    };
    let mut proved = match &pool {
        Some(pool) => pool.install(discharge),
        None => discharge(),
    };
    warnings.append(&mut proved.warnings);
    let report = proved.report;

    if let Err(e) = store.flush() {
        warnings.push(
            Diagnostic::warning(codes::CACHE_UNREADABLE, format!("{e:#}"))
                .note("nothing was recorded; the next run discharges everything again"),
        );
    }
    warnings.extend(store.take_warnings());
    let warnings = once_each(warnings);

    let labels = law_labels(&loaded.check);
    if args.json {
        emit_json(&report_json(
            &loaded,
            &report,
            &labels,
            specified,
            filtered_out,
            &warnings,
        ));
    } else {
        print_human(
            &loaded,
            &report,
            &proved.reasons,
            &labels,
            specified,
            filtered_out,
            args.explain,
            &warnings,
            style,
        );
    }
    if report.failed() {
        EXIT_FAILED
    } else {
        EXIT_OK
    }
}

/// Every module parsed, because a clause the run did not read is a claim nobody
/// checked. Gate 1 may skip a file whose bytes are unchanged, and a skipped file
/// has no clause AST to collect from.
pub(crate) fn load_complete(
    path: &std::path::Path,
    incremental: bool,
    json: bool,
    command: &str,
    store: &mut Store,
    warnings: &mut Vec<Diagnostic>,
    style: Style,
) -> Result<Loaded, i32> {
    let first = if incremental {
        driver::load_incremental(path, store)
    } else {
        load(path)
    };
    let loaded = match first {
        Ok(loaded) => loaded,
        Err(err) => return Err(report_load_error(command, &err, json, style)),
    };
    warnings.extend(store.take_warnings());
    if loaded.complete {
        return Ok(loaded);
    }

    let needed: Vec<ply_syntax::ast::ModuleName> = loaded
        .check
        .modules
        .values()
        .map(|m| m.name.clone())
        .collect();
    match driver::load_to_evaluate(path, store, &needed) {
        Ok(full) => {
            warnings.extend(store.take_warnings());
            Ok(full)
        }
        Err(err) => Err(report_load_error(command, &err, json, style)),
    }
}

fn filter(obligations: Vec<Obligation>, filter: Option<&str>) -> (Vec<Obligation>, usize) {
    let Some(needle) = filter else {
        return (obligations, 0);
    };
    let total = obligations.len();
    let kept: Vec<Obligation> = obligations
        .into_iter()
        .filter(|o| o.owner.as_str().contains(needle))
        .collect();
    let filtered_out = total - kept.len();
    (kept, filtered_out)
}

/// A law is labelled rather than named, so the report prints the label it was
/// written with and not the `<module>.<label>` its key is.
pub(crate) fn law_labels(check: &CheckOutput) -> BTreeMap<Symbol, String> {
    check
        .laws
        .iter()
        .map(|law| (law.key.clone(), law.name.clone()))
        .collect()
}

// --- What each outcome says -------------------------------------------------

/// The left-hand column: what became of this obligation.
pub(crate) fn outcome_of(discharge: &Discharge) -> &'static str {
    match discharge {
        Discharge::Held(evidence) => evidence.tier().as_str(),
        Discharge::Refuted(_) => "refuted",
        Discharge::Vacuous(_) => "vacuous",
        Discharge::Unattempted(_) => "unattempted",
    }
}

/// What an `ensures` gives up when its owner's declared row is wider than its
/// body's.
///
/// The footprint is the frame condition: an `ensures` means *this holds of the
/// result, and every resource outside the footprint's writes is unchanged*. An
/// annotation wider than the body — which an `effect set` makes systematic,
/// since one set is written for a whole service — therefore promises less about
/// less, at the same tier and with no other sign that anything was lost. Here
/// it is a scheduling cost nobody wrote down; on an obligation it is a weakened
/// claim, so `ply prove --explain` names the atoms.
fn weakened_frame(loaded: &Loaded, obligation: &Obligation) -> Vec<String> {
    if obligation.kind == ObligationKind::Law {
        return Vec::new();
    }
    let Some(def) = loaded.check.defs.get(&obligation.owner) else {
        return Vec::new();
    };
    let slack = crate::signature::provenance(def);
    if slack.unperformed.is_empty() {
        return Vec::new();
    }
    crate::signature::fill(
        "frame covers, body never touches: ",
        "  ",
        &slack.unperformed,
        "",
        crate::signature::WIDTH - IND.len() - 4,
    )
}

pub(crate) fn owner_label(obligation: &Obligation, labels: &BTreeMap<Symbol, String>) -> String {
    match obligation.kind {
        ObligationKind::Ensures { index } => {
            format!("{} ensures #{index}", obligation.owner)
        }
        ObligationKind::Law => match labels.get(&obligation.owner) {
            Some(label) => format!("law {label:?}"),
            None => format!("law {}", obligation.owner),
        },
    }
}

/// Why an obligation is `proved`, as the certificate itself says.
///
/// Every rule it names, in application order, deduplicated — a reader is being
/// told which arguments were used, not how many times each fired. The step count
/// is last because it is the cost, not the claim.
pub(crate) fn certificate_summary(certificate: &Certificate) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut unfoldings = 0;
    for rule in &certificate.rules {
        if let Rule::Unfold { .. } = rule {
            unfoldings += 1;
            continue;
        }
        let described = describe_rule(rule);
        if !parts.contains(&described) {
            parts.push(described);
        }
    }
    if unfoldings > 0 {
        parts.push(format!("{unfoldings} {}", plural(unfoldings, "unfolding")));
    }
    if !certificate.sorts.is_empty() {
        let sorts: Vec<&str> = certificate.sorts.iter().map(|s| s.as_str()).collect();
        parts.push(format!("uninterpreted {}", sorts.join(", ")));
    }
    parts.push(format!("{} steps", certificate.steps));
    parts.join(" · ")
}

fn describe_rule(rule: &Rule) -> String {
    match rule {
        Rule::GroundEvaluation => "ground evaluation".to_string(),
        Rule::ExhaustiveEnumeration { domain, points } => {
            format!(
                "exhaustive over {points} {} of {domain}",
                plural(*points as usize, "point")
            )
        }
        Rule::LinearArithmetic => "linear arithmetic".to_string(),
        Rule::Propositional => "propositional".to_string(),
        Rule::CaseSplit { ty, arms } => format!("case analysis over {ty} ({arms} arms)"),
        Rule::Congruence => "congruence".to_string(),
        Rule::Injectivity => "injectivity".to_string(),
        Rule::Unfold { def, depth } => format!("unfold {def} to depth {depth}"),
        Rule::ExhaustiveInterleaving { interleavings } => format!(
            "exhaustive over {interleavings} {}",
            plural(*interleavings as usize, "interleaving")
        ),
    }
}

/// What a sampled run did, in the words that make the tier honest.
///
/// An `example` says how much of its budget the guard threw away, because being
/// told `property, 200 cases` when seven cases ran is exactly the misreport this
/// tier exists to avoid.
pub(crate) fn cases_summary(cases: &CaseReport) -> String {
    let mut line = if cases.kept >= ply_prove::MIN_PROPERTY_CASES {
        format!(
            "{} {} · {} rejected",
            cases.kept,
            plural(cases.kept as usize, "case"),
            cases.rejected
        )
    } else {
        format!(
            "{} of {} cases kept · guard rejected {}",
            cases.kept, cases.generated, cases.rejected
        )
    };
    if cases.roots.len() > 1 {
        line.push_str(&format!(" · {} roots", cases.roots.len()));
    }
    for (var, ty) in &cases.instantiations {
        line.push_str(&format!(" · {var} := {ty}"));
    }
    line
}

pub(crate) fn evidence_summary(evidence: &Evidence) -> String {
    match evidence {
        Evidence::Proof(c) => certificate_summary(c),
        Evidence::Cases(c) => cases_summary(c),
    }
}

pub(crate) fn gap_summary(gap: &Gap) -> String {
    match gap {
        Gap::UnhandledEffect(footprint) => {
            if footprint.is_empty() {
                "no handler for what it performs".to_string()
            } else {
                format!("performs {footprint}: no handler")
            }
        }
        Gap::Ungeneratable { param, ty } => {
            format!("no value of `{ty}` can be generated for `{param}`")
        }
        Gap::Raised { diagnostic, .. } => format!("raised: {}", diagnostic.message),
        Gap::GuardNotSampled { generated, .. } => {
            format!("the guard kept none of {generated} cases, but it does admit a value")
        }
        Gap::ReachesHost(footprint) => {
            format!("reaches the host ({footprint}); run `ply prove --host`")
        }
    }
}

pub(crate) fn vacuity_summary(vacuity: &Vacuity) -> String {
    match vacuity.kind {
        VacuityKind::ProvedUnsatisfiable => "the guard admits no value".to_string(),
        VacuityKind::NoCaseKept { generated } => {
            format!("the guard kept none of {generated} cases")
        }
    }
}

pub(crate) fn frame_summary(frame: &Frame) -> String {
    match frame {
        Frame::Pure => "pure".to_string(),
        Frame::Writes(writes) => {
            let named: Vec<String> = writes
                .iter()
                .map(|(effect, resource)| format!("{effect}[{resource}]"))
                .collect();
            format!("writes {}", named.join(", "))
        }
    }
}

fn render_bindings(bindings: &[Binding]) -> String {
    bindings
        .iter()
        .map(|b| format!("{} = {}", b.name, b.rendered))
        .collect::<Vec<_>>()
        .join(", ")
}

fn quantifier(obligation: &Obligation) -> String {
    if obligation.binders.is_empty() {
        return "no binders".to_string();
    }
    let binders: Vec<String> = obligation
        .binders
        .iter()
        .map(|b| format!("{}: {}", b.name, b.ty))
        .collect();
    format!("forall ({})", binders.join(", "))
}

// --- Diagnostics ------------------------------------------------------------

/// One diagnostic per outcome that is not a hold.
///
/// A refutation leads with the shrunk bindings, because the input is the answer
/// and the search is only the evidence for it.
pub(crate) fn diagnostics(
    report: &ProveReport,
    labels: &BTreeMap<Symbol, String>,
) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for (obligation, discharge) in &report.obligations {
        let what = owner_label(obligation, labels);
        match discharge {
            Discharge::Held(_) => {}
            Discharge::Refuted(counterexample) => {
                out.push(refutation(obligation, counterexample, &what));
            }
            Discharge::Vacuous(vacuity) => out.push(
                Diagnostic::error(
                    codes::VACUOUS_OBLIGATION,
                    format!("{what} is vacuous: {}", vacuity_summary(vacuity)),
                )
                .primary(vacuity.guard, "this guard admits nothing")
                .note("an obligation over an empty domain is trivially valid and says nothing")
                .note("widen the guard, or delete the claim"),
            ),
            Discharge::Unattempted(gap) => {
                let mut diagnostic = Diagnostic::warning(
                    codes::OBLIGATION_NOT_DISCHARGED,
                    format!("{what} was not discharged: {}", gap_summary(gap)),
                )
                .primary(obligation.span, "no tier is claimed for this")
                .note("this definition is not covered; a reader still has to read it");
                if let Gap::Raised { bindings, .. } = gap
                    && !bindings.is_empty()
                {
                    diagnostic = diagnostic.note(format!(
                        "shrunk to {}, which still raises",
                        render_bindings(bindings)
                    ));
                }
                if let Gap::GuardNotSampled { witness, .. } = gap {
                    diagnostic = diagnostic
                        .note(format!("the guard admits {}", render_bindings(witness)))
                        .note("the generator draws from the whole type, so a narrow guard is a gap in the search rather than a defect in the spec");
                }
                out.push(diagnostic);
            }
        }
    }
    out
}

fn refutation(obligation: &Obligation, counterexample: &Counterexample, what: &str) -> Diagnostic {
    let mut diagnostic = Diagnostic::error(
        codes::OBLIGATION_REFUTED,
        format!(
            "{what} is false at {}",
            render_bindings(&counterexample.bindings)
        ),
    )
    .primary(obligation.span, "this claim does not hold")
    .note(quantifier(obligation))
    .note(format!(
        "shrank from {} in {} {}",
        render_bindings(&counterexample.original),
        counterexample.shrinks,
        plural(counterexample.shrinks as usize, "step")
    ))
    .note(format!(
        "root {} · case {}",
        counterexample.root, counterexample.case
    ));
    if let Some(seed) = &counterexample.sim_seed {
        diagnostic = diagnostic.note(format!("replay with `--seed {seed}`"));
    }
    diagnostic
}

// --- Human output -----------------------------------------------------------

/// The two lines every run of either command starts with.
///
/// The count of definitions carrying no obligation is exactly the surface where
/// review still costs what it costs today, so it is here rather than behind a
/// flag, and it is ahead of the results rather than after them.
pub(crate) fn print_coverage(coverage: &Coverage, specified: usize, explain: bool, style: Style) {
    let unspecified = coverage.definitions.saturating_sub(specified);
    println!(
        "{IND}{} {} · {specified} carry an obligation · {unspecified} do not",
        coverage.definitions,
        plural(coverage.definitions, "definition"),
    );
    if coverage.uncovered.is_empty() {
        return;
    }
    // Not the same number as `unspecified`, and the difference is the point: a
    // definition can carry a claim the machine could not establish, and such a
    // definition is one a reviewer still has to read.
    let shown = if explain {
        coverage.uncovered.len()
    } else {
        coverage.uncovered.len().min(6)
    };
    let names: Vec<&str> = coverage.uncovered[..shown]
        .iter()
        .map(|n| n.as_str())
        .collect();
    let rest = coverage.uncovered.len() - shown;
    let tail = if rest > 0 {
        format!(", and {rest} more (`--explain` for all)")
    } else {
        String::new()
    };
    println!(
        "{IND}{}",
        style.dim(&format!(
            "{} not covered by a claim that holds: {}{tail}",
            coverage.uncovered.len(),
            names.join(", ")
        ))
    );
}

#[allow(clippy::too_many_arguments)]
fn print_human(
    loaded: &Loaded,
    report: &ProveReport,
    reasons: &[Reason],
    labels: &BTreeMap<Symbol, String>,
    specified: usize,
    filtered_out: usize,
    explain: bool,
    warnings: &[Diagnostic],
    style: Style,
) {
    print_coverage(&report.coverage, specified, explain, style);

    let held = report.obligations.iter().filter(|(_, d)| d.holds()).count();
    let mut summary = format!(
        "{IND}{} {} · {} proved · {} property · {} example",
        report.obligations.len(),
        plural(report.obligations.len(), "obligation"),
        report.count(Tier::Proved),
        report.count(Tier::Property),
        report.count(Tier::Example),
    );
    if report.unattempted() > 0 {
        summary.push_str(&format!(" · {} unattempted", report.unattempted()));
    }
    summary.push_str(&format!(
        "   {}",
        style.dim(&format!("({:.2}s)", report.duration.as_secs_f64()))
    ));
    println!("{summary}");
    if filtered_out > 0 {
        println!(
            "{IND}{}",
            style.dim(&format!("{filtered_out} filtered out"))
        );
    }
    println!();

    for (index, (obligation, discharge)) in report.obligations.iter().enumerate() {
        print_row(loaded, obligation, discharge, labels, style);
        if explain {
            let reason = reasons.get(index).copied().unwrap_or(Reason::New);
            println!(
                "{IND}    {}",
                style.dim(&format!("from: {}", reason.as_str()))
            );
            for line in weakened_frame(loaded, obligation) {
                println!("{IND}    {}", style.dim(&line));
            }
        }
    }

    if !warnings.is_empty() {
        println!();
        print_warnings(warnings, style);
    }

    println!();
    let refuted = report.refuted();
    let vacuous = report.vacuous();
    if refuted == 0 && vacuous == 0 {
        println!(
            "{IND}{} ({:.2}s)",
            style.green(&format!("{held} held")),
            report.duration.as_secs_f64()
        );
    } else {
        let mut parts = Vec::new();
        if refuted > 0 {
            parts.push(format!("{refuted} refuted"));
        }
        if vacuous > 0 {
            parts.push(format!("{vacuous} vacuous"));
        }
        println!(
            "{IND}{}, {held} held ({:.2}s)",
            style.red(&parts.join(", ")),
            report.duration.as_secs_f64()
        );
    }
}

fn print_row(
    loaded: &Loaded,
    obligation: &Obligation,
    discharge: &Discharge,
    labels: &BTreeMap<Symbol, String>,
    style: Style,
) {
    let what = owner_label(obligation, labels);
    let outcome = outcome_of(discharge);
    match discharge {
        Discharge::Held(evidence) => {
            println!(
                "{IND}{} {:<11} {:<40} {}",
                style.green("✓"),
                outcome,
                what,
                style.dim(&evidence_summary(evidence))
            );
        }
        Discharge::Unattempted(gap) => {
            println!(
                "{IND}{} {:<11} {:<40} {}",
                style.yellow("~"),
                outcome,
                what,
                style.dim(&gap_summary(gap))
            );
            if let Gap::Raised { bindings, .. } = gap
                && !bindings.is_empty()
            {
                println!(
                    "{IND}    {}",
                    style.dim(&format!("shrunk to {}", render_bindings(bindings)))
                );
            }
        }
        Discharge::Vacuous(vacuity) => {
            println!(
                "{IND}{} {:<11} {:<40} {}",
                style.red("✗"),
                outcome,
                what,
                vacuity_summary(vacuity)
            );
        }
        Discharge::Refuted(counterexample) => {
            let at = location(&loaded.sources, obligation.span).unwrap_or_default();
            println!(
                "{IND}{} {:<11} {:<40} {}",
                style.red("✗"),
                outcome,
                what,
                style.dim(&at)
            );
            println!(
                "{IND}    {}  →  {}",
                quantifier(obligation),
                render_bindings(&counterexample.bindings)
            );
            println!(
                "{IND}    {}",
                style.dim(&format!(
                    "shrank from {} in {} {} · root {} · case {}",
                    render_bindings(&counterexample.original),
                    counterexample.shrinks,
                    plural(counterexample.shrinks as usize, "step"),
                    counterexample.root,
                    counterexample.case
                ))
            );
            println!(
                "{IND}    {}",
                style.dim(&format!("frame: {}", frame_summary(&obligation.frame)))
            );
        }
    }
}

// --- JSON -------------------------------------------------------------------

pub(crate) fn coverage_json(coverage: &Coverage) -> Value {
    json!({
        "definitions": coverage.definitions,
        "covered": coverage.covered,
        "uncovered": coverage.uncovered.iter().map(|n| n.as_str()).collect::<Vec<_>>(),
        "by_tier": coverage
            .by_tier
            .iter()
            .map(|(tier, n)| (tier.as_str().to_string(), json!(n)))
            .collect::<serde_json::Map<String, Value>>(),
    })
}

pub(crate) fn plan_json(plan: &ProvePlan) -> Value {
    json!({
        "cases": plan.cases,
        "roots": plan.roots,
        "prove_budget": plan.prove_budget,
        "shrink_budget": plan.shrink_budget,
        "sim": {
            "mode": plan.sim.mode.as_str(),
            "roots": plan.sim.roots,
            "budget": plan.sim.budget,
            "steps": plan.sim.steps,
        },
    })
}

pub(crate) fn obligation_json(
    obligation: &Obligation,
    discharge: &Discharge,
    labels: &BTreeMap<Symbol, String>,
    sources: &SourceMap,
) -> Value {
    let mut out = json!({
        "key": obligation.key.to_hex(),
        "owner": obligation.owner.as_str(),
        "label": owner_label(obligation, labels),
        "kind": match obligation.kind {
            ObligationKind::Ensures { .. } => "ensures",
            ObligationKind::Law => "law",
        },
        "guarded": obligation.guarded,
        "frame": frame_summary(&obligation.frame),
        "location": location(sources, obligation.span),
        "outcome": outcome_of(discharge),
        "tier": discharge.tier().map(|t| t.as_str()),
    });
    let object = out.as_object_mut().expect("a JSON object was just built");
    if let ObligationKind::Ensures { index } = obligation.kind {
        object.insert("index".to_string(), json!(index));
    }
    match discharge {
        Discharge::Held(Evidence::Proof(c)) => {
            object.insert(
                "certificate".to_string(),
                json!({
                    "rules": serde_json::to_value(&c.rules).unwrap_or(Value::Null),
                    "steps": c.steps,
                    "guard_satisfiable": c.guard_satisfiable,
                    "sorts": c.sorts.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                    "summary": certificate_summary(c),
                }),
            );
        }
        Discharge::Held(Evidence::Cases(c)) => {
            object.insert(
                "cases".to_string(),
                json!({
                    "generated": c.generated,
                    "kept": c.kept,
                    "rejected": c.rejected,
                    "roots": c.roots,
                    "instantiations": c
                        .instantiations
                        .iter()
                        .map(|(v, t)| json!({ "var": v.as_str(), "type": t.to_string() }))
                        .collect::<Vec<_>>(),
                    "summary": cases_summary(c),
                }),
            );
        }
        Discharge::Refuted(counterexample) => {
            object.insert(
                "counterexample".to_string(),
                json!({
                    "bindings": bindings_json(&counterexample.bindings),
                    "original": bindings_json(&counterexample.original),
                    "shrinks": counterexample.shrinks,
                    "root": counterexample.root,
                    "case": counterexample.case,
                    "seed": counterexample.sim_seed.as_ref().map(|s| s.to_string()),
                }),
            );
        }
        Discharge::Vacuous(vacuity) => {
            object.insert("vacuity".to_string(), json!(vacuity_summary(vacuity)));
        }
        Discharge::Unattempted(gap) => {
            object.insert("gap".to_string(), json!(gap_summary(gap)));
            if let Gap::Raised { bindings, .. } = gap
                && !bindings.is_empty()
            {
                object.insert("raised_at".to_string(), bindings_json(bindings));
            }
        }
    }
    out
}

fn bindings_json(bindings: &[Binding]) -> Value {
    Value::Array(
        bindings
            .iter()
            .map(|b| json!({ "name": b.name.as_str(), "type": b.ty.to_string(), "value": b.rendered }))
            .collect(),
    )
}

fn report_json(
    loaded: &Loaded,
    report: &ProveReport,
    labels: &BTreeMap<Symbol, String>,
    specified: usize,
    filtered_out: usize,
    warnings: &[Diagnostic],
) -> Value {
    let diagnostics = diagnostics(report, labels);
    json!({
        "schema_version": 1,
        "command": "prove",
        "ok": !report.failed(),
        "exit_code": if report.failed() { EXIT_FAILED } else { EXIT_OK },
        "specified": specified,
        "coverage": coverage_json(&report.coverage),
        "plan": plan_json(&report.plan),
        "cached": report.cached,
        "filtered_out": filtered_out,
        "duration_ms": millis(report.duration),
        "summary": {
            "obligations": report.obligations.len(),
            "proved": report.count(Tier::Proved),
            "property": report.count(Tier::Property),
            "example": report.count(Tier::Example),
            "refuted": report.refuted(),
            "vacuous": report.vacuous(),
            "unattempted": report.unattempted(),
        },
        "obligations": report
            .obligations
            .iter()
            .map(|(o, d)| obligation_json(o, d, labels, &loaded.sources))
            .collect::<Vec<_>>(),
        "diagnostics": diagnostics_json(&diagnostics, &loaded.sources),
        "warnings": diagnostics_json(warnings, &loaded.sources),
    })
}
