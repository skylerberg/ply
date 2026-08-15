//! `ply review --changed` — the artifact this milestone exists to produce.
//!
//! Per changed definition: whether the implementation changed, whether the
//! specification changed, and whether the obligations still hold. The row that
//! matters is *implementation changed, spec unchanged*, where the review is
//! reading the obligations rather than the diff.
//!
//! The one sentence this command must not get wrong is its summary. **"No
//! specified behaviour changed" is true; "nothing changed" is false**, because a
//! behaviour nobody specified can change without anything here noticing — by
//! construction, not by omission. So the count of changed definitions carrying
//! no obligation is in the same sentence as the claim, and the coverage line is
//! above both.

use super::common::{
    IND, diagnostic_json, diagnostics_json, emit_json, millis, once_each, plural,
    print_diagnostics, print_warnings,
};
use super::prove::{
    coverage_json, diagnostics, evidence_summary, gap_summary, law_labels, load_complete,
    obligation_json, outcome_of, owner_label, plan_json, print_coverage, vacuity_summary,
};
use crate::cli::ReviewArgs;
use crate::load::{Loaded, project_root};
use crate::style::Style;
use crate::{EXIT_COMPILE_ERROR, EXIT_FAILED, EXIT_OK};
use ply_prove::{Discharge, ProveReport};
use ply_span::{Diagnostic, SourceMap, codes};
use ply_store::Store;
use ply_test::obligation::{self, Laws, Moved, ReviewReport, Reviewed};
use serde_json::{Value, json};

pub fn execute(args: &ReviewArgs, style: Style) -> i32 {
    let mut warnings = Vec::new();
    let root = project_root(&args.path);
    let mut store = match Store::open(&root) {
        Ok(store) => store,
        Err(e) => {
            let diagnostic = Diagnostic::error(
                codes::RUNTIME_ERROR,
                format!("could not open the cache under `{}`: {e:#}", root.display()),
            )
            .note("a review baseline lives there, so this run has nothing to compare against");
            if args.json {
                emit_json(&json!({
                    "command": "review",
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
        "review",
        &mut store,
        &mut warnings,
        style,
    ) {
        Ok(loaded) => loaded,
        Err(code) => return code,
    };
    warnings.extend(loaded.frontend.warnings.iter().cloned());

    let hashes = loaded.hashes.clone();
    let scoped = crate::obligations::project_view(&loaded.check, args.std);
    let laws = Laws::of(&scoped, &hashes);
    let collected = crate::obligations::collect(&loaded.program, &scoped, &hashes);
    warnings.extend(collected.warnings);

    if args.accept {
        let accepted = obligation::accept(&scoped, &hashes, &laws, &mut store);
        if let Err(e) = store.flush() {
            warnings.push(
                Diagnostic::error(codes::CACHE_UNREADABLE, format!("{e:#}"))
                    .note("nothing was accepted; the baseline is unchanged"),
            );
            let warnings = once_each(warnings);
            return report_accept(accepted, &loaded, &warnings, args.json, style, EXIT_FAILED);
        }
        warnings.extend(store.take_warnings());
        let warnings = once_each(warnings);
        return report_accept(accepted, &loaded, &warnings, args.json, style, EXIT_OK);
    }

    let plan = crate::simulation::prove_plan(&args.prove, &args.simulation);
    let specified = obligation::specified(&scoped, &laws, &collected.obligations);
    let (engine, engine_warning) = crate::engine::of(
        &loaded.program,
        &loaded.resolved,
        &loaded.check,
        loaded.complete,
        collected.obligations.len(),
        // `ply review` reports what moved; it binds nothing, so a `law/host` is
        // a gap here exactly as it is under a hermetic `ply prove`.
        None,
    );
    warnings.extend(engine_warning);
    let mut proved = obligation::prove(
        collected.obligations,
        &scoped,
        &laws,
        &mut store,
        &plan,
        !args.no_cache,
        engine.as_ref(),
    );
    warnings.append(&mut proved.warnings);
    let report = proved.report;

    let review = obligation::review(&scoped, &hashes, &laws, &store, &report);

    if let Err(e) = store.flush() {
        warnings.push(
            Diagnostic::warning(codes::CACHE_UNREADABLE, format!("{e:#}"))
                .note("nothing was recorded; the next run discharges everything again"),
        );
    }
    warnings.extend(store.take_warnings());
    let warnings = once_each(warnings);

    if args.json {
        emit_json(&report_json(
            &loaded, &report, &review, specified, &warnings,
        ));
    } else {
        print_human(&loaded, &report, &review, specified, &warnings, style);
    }
    if report.failed() {
        EXIT_FAILED
    } else {
        EXIT_OK
    }
}

fn report_accept(
    accepted: usize,
    loaded: &Loaded,
    warnings: &[Diagnostic],
    json: bool,
    style: Style,
    code: i32,
) -> i32 {
    if json {
        emit_json(&json!({
            "schema_version": 1,
            "command": "review",
            "action": "accept",
            "ok": code == EXIT_OK,
            "exit_code": code,
            "accepted": accepted,
            "warnings": diagnostics_json(warnings, &loaded.sources),
        }));
    } else {
        println!(
            "{IND}{accepted} {} accepted as reviewed",
            plural(accepted, "definition")
        );
        println!(
            "{IND}{}",
            style.dim(
                "the baseline is keyed by name, so renaming one loses its baseline and \
                 reports it as unreviewed"
            )
        );
        if !warnings.is_empty() {
            print_warnings(warnings, style);
        }
    }
    code
}

/// What a reviewer should do with this definition, from the four rows of ADR
/// 0007 §9.2's table that are worth printing.
///
/// Every row that tells a reviewer to stop reading the diff is derived from
/// *"the claim is fixed and still holds"*. A definition whose obligations were
/// never discharged has neither half of that, so it falls to the row where
/// review costs what it costs today — and says which of the two reasons it is
/// there for, because "no spec" and "a spec nothing could check" want different
/// things from the reader.
fn advice(entry: &Reviewed) -> &'static str {
    if !entry.specified() {
        return if entry.claimed() {
            "no obligation on this definition holds: read the implementation, line by line"
        } else {
            "read the implementation, line by line, exactly as today"
        };
    }
    match (entry.implementation, entry.spec) {
        (Moved::Changed, Moved::Unchanged) => "review the obligations, not the diff",
        (Moved::Unchanged, Moved::Changed) => {
            "review the spec diff; the implementation did not move"
        }
        (Moved::Never, _) | (_, Moved::Never) => {
            "never accepted: review the spec and the implementation once"
        }
        _ => "review both; the tier says how much the machine already checked",
    }
}

fn row(entry: &Reviewed) -> String {
    if entry.implementation == Moved::Never {
        return format!("{} · never reviewed", entry.name);
    }
    let spec = if entry.claimed() {
        format!("spec {}", entry.spec.as_str())
    } else {
        "no spec".to_string()
    };
    format!(
        "{} · implementation {} · {spec}",
        entry.name,
        entry.implementation.as_str()
    )
}

fn print_human(
    loaded: &Loaded,
    report: &ProveReport,
    review: &ReviewReport,
    specified: usize,
    warnings: &[Diagnostic],
    style: Style,
) {
    print_coverage(&review.coverage, specified, false, style);
    println!(
        "{IND}{} of {} {} changed since the last accepted review · {} of them have a baseline",
        review.changed.len(),
        review.definitions,
        plural(review.definitions, "definition"),
        review.reviewed,
    );
    println!();

    let labels = law_labels(&loaded.check);
    for entry in &review.changed {
        println!("{IND}{}", style.bold(&row(entry)));
        for &index in &entry.obligations {
            let (obligation, discharge) = &report.obligations[index];
            let what = owner_label(obligation, &labels);
            let detail = match discharge {
                Discharge::Held(evidence) => evidence_summary(evidence),
                Discharge::Refuted(_) => "no longer holds".to_string(),
                Discharge::Vacuous(v) => vacuity_summary(v),
                Discharge::Unattempted(gap) => gap_summary(gap),
            };
            let mark = match discharge {
                Discharge::Held(_) => style.green("✓"),
                Discharge::Unattempted(_) => style.yellow("~"),
                _ => style.red("✗"),
            };
            println!(
                "{IND}  {mark} {:<11} {:<40} {}",
                outcome_of(discharge),
                what,
                style.dim(&detail)
            );
        }
        println!("{IND}  {}", style.dim(&format!("→ {}", advice(entry))));
        println!();
    }

    if !warnings.is_empty() {
        print_warnings(warnings, style);
        println!();
    }

    let headline = review.headline();
    if review.broken > 0 {
        println!("{IND}{}", style.red(&headline));
    } else {
        println!("{IND}{headline}");
    }
}

fn report_json(
    loaded: &Loaded,
    report: &ProveReport,
    review: &ReviewReport,
    specified: usize,
    warnings: &[Diagnostic],
) -> Value {
    let labels = law_labels(&loaded.check);
    json!({
        "schema_version": 1,
        "command": "review",
        "action": "changed",
        "ok": !report.failed(),
        "exit_code": if report.failed() { EXIT_FAILED } else { EXIT_OK },
        "definitions": review.definitions,
        "specified": specified,
        "reviewed": review.reviewed,
        "coverage": coverage_json(&review.coverage),
        "plan": plan_json(&report.plan),
        "duration_ms": millis(review.duration),
        // Not `changed`: a definition nobody specified can change without
        // anything here noticing, so the artifact says what the claim is about.
        "headline": review.headline(),
        "specified_changed": review.specified(),
        "unspecified_changed": review.unspecified(),
        "broken": review.broken,
        "undischarged": review.undischarged,
        "changed": review
            .changed
            .iter()
            .map(|entry| json!({
                "name": entry.name.as_str(),
                "implementation": entry.implementation.as_str(),
                "spec": if entry.claimed() { entry.spec.as_str() } else { "none" },
                "specified": entry.specified(),
                "advice": advice(entry),
                "obligations": entry
                    .obligations
                    .iter()
                    .map(|&i| {
                        let (obligation, discharge) = &report.obligations[i];
                        obligation_json(obligation, discharge, &labels, &loaded.sources)
                    })
                    .collect::<Vec<_>>(),
            }))
            .collect::<Vec<_>>(),
        "diagnostics": diagnostics_json(&diagnostics(report, &labels), &loaded.sources),
        "warnings": diagnostics_json(warnings, &loaded.sources),
    })
}
