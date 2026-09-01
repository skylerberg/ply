//! Whether the W6 measurement files still describe the tree they ship in, and whether the decision
//! machinery can be made to answer without checking C3.

use ply_corpus::w6::{self, Alternative, Criteria, Layer, Report, Verdict};
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate lives two levels below the repository root")
        .to_path_buf()
}

/// The shipped report, merged the way `ply-corpus w6` merges it.
fn shipped() -> Report {
    let mut merged = serde_json::Map::new();
    for name in ["benches/w6-ladder.json", "benches/w6-spike.json"] {
        let text = std::fs::read_to_string(repo().join(name))
            .unwrap_or_else(|e| panic!("`{name}` is what the verdict is read from: {e}"));
        let serde_json::Value::Object(fields) =
            serde_json::from_str(&text).unwrap_or_else(|e| panic!("`{name}` is not JSON: {e}"))
        else {
            panic!("`{name}` is not a report object");
        };
        merged.extend(fields);
    }
    serde_json::from_value(serde_json::Value::Object(merged))
        .expect("the merged measurements are a W6 report")
}

/// The interpreter share is what M9's case rests on, and a share summed from nine differences would
/// be an additive fiction the moment one rung's `without` stopped being the rung below it.
#[test]
fn the_interpreter_share_telescopes_to_one_measured_absolute() {
    let report = shipped();
    let ladder = report.ladder().expect("the shipped ladder assembles");

    let in_process: Vec<_> = ladder
        .rungs
        .iter()
        .filter(|r| r.layer.is_interpreter())
        .collect();
    for pair in in_process.windows(2) {
        assert_eq!(
            pair[0].with_micros, pair[1].without_micros,
            "`{}`'s `with` is {}µs and `{}`'s `without` is {}µs; the interpreter rungs do not \
             chain, so their sum is not an absolute anybody measured",
            pair[0].label, pair[0].with_micros, pair[1].label, pair[1].without_micros
        );
    }
    let machine = in_process.last().expect("five interpreter rungs");
    assert_eq!(machine.layer, Layer::Machine);
    assert!(
        (ladder.interpreter_micros - machine.with_micros).abs() < 1e-9,
        "the interpreter total is {:.2}µs and the `machine` rung measured {:.2}µs",
        ladder.interpreter_micros,
        machine.with_micros
    );
}

/// The share's numerator and its denominator are taken in different arenas on different stacks, and
/// the ladder says so in a column.
#[test]
fn a_negative_residue_is_evidence_the_share_is_not_a_lower_bound() {
    let report = shipped();
    let ladder = report.ladder().expect("the shipped ladder assembles");
    if ladder.residue_micros >= 0.0 {
        return;
    }
    let machine = ladder
        .rungs
        .iter()
        .find(|r| r.layer == Layer::Machine)
        .expect("a machine rung");
    let socket = ladder
        .rungs
        .iter()
        .find(|r| r.layer == Layer::Socket)
        .expect("a socket rung");
    let corrected = (ladder.interpreter_micros + ladder.residue_micros) / ladder.total_micros;
    assert!(
        (ladder.conservative_share - corrected).abs() < 1e-9,
        "the ladder charges a negative residue back itself: it reports {:.4} and the correction is \
         {corrected:.4}",
        ladder.conservative_share
    );
    assert!(
        (report.decision(&ladder).interpreter_share - corrected).abs() < 1e-9,
        "and the decision reads the charged share rather than the attributed one"
    );
    println!(
        "the in-process arena reached {:.2}µs at the socket rung against a {:.2}µs served total; \
         the {:.2}µs residue is that seam, and crediting it to the arena it came from moves the \
         share from {:.3} to {:.3}",
        socket.with_micros,
        ladder.total_micros,
        ladder.residue_micros,
        ladder.interpreter_share,
        corrected
    );
    assert!(
        machine.with_micros > 0.0,
        "the numerator is the machine rung's absolute"
    );
    assert!(
        corrected < ladder.interpreter_share,
        "a negative residue can only lower the share once the seam is charged to the arena that \
         produced it; it moved {:.3} to {:.3}",
        ladder.interpreter_share,
        corrected
    );
}

/// **The C3 hole, closed.**
#[test]
fn an_absent_alternatives_list_cannot_advance_m9() {
    let mut report = shipped();
    let ladder = report.ladder().expect("the shipped ladder assembles");
    assert_eq!(
        report.decision(&ladder).verdict,
        Verdict::Defer,
        "the shipped report defers, and it defers on C3"
    );

    report.alternatives.clear();
    let decision = report.decision(&ladder);
    assert_eq!(
        decision.verdict,
        Verdict::Defer,
        "an empty alternatives array prices none of the cheaper levers, so C3 fails at its widest"
    );
    let findings = report.audit();
    for lever in &w6::LEVERS {
        assert!(
            findings.iter().any(|f| f.contains(lever.name)),
            "the audit does not name `{}`: {findings:?}",
            lever.name
        );
    }
}

/// The same hole through the other field: `priced` and `end_to_end` are numbers in a file, so a
/// report could claim all seven levers were priced at 1.00x and advance M9 without any of them
/// having been measured.
#[test]
fn a_measurement_file_cannot_price_a_lever_by_asserting_it() {
    let mut report = shipped();
    let ladder = report.ladder().expect("the shipped ladder assembles");
    for alternative in &mut report.alternatives {
        alternative.priced = true;
        alternative.end_to_end = 1.0;
        alternative.evidence.clear();
    }
    assert_eq!(
        report.decision(&ladder).verdict,
        Verdict::Defer,
        "seven claims with nothing behind them are not seven measurements"
    );
    assert!(
        report.audit().iter().any(|f| f.contains("evidence")),
        "{:?}",
        report.audit()
    );
}

/// What the shipped file *does* price, and that the lever it prices is one of the roster's rather than one
/// of its own invention.
#[test]
fn every_priced_lever_answers_for_a_lever_adr_0016_names() {
    let report = shipped();
    let priced: Vec<&str> = report
        .alternatives
        .iter()
        .filter(|a| a.is_priced())
        .map(|a| a.name.as_str())
        .collect();
    for name in &priced {
        assert!(
            w6::LEVERS.iter().any(|l| l.name == *name),
            "`{name}` is priced and is not in the cheaper levers"
        );
    }
    for alternative in report.alternatives.iter().filter(|a| a.is_priced()) {
        assert!(
            !alternative.evidence.trim().is_empty(),
            "`{}` is priced and says nothing about what the ratio is between",
            alternative.name
        );
    }
    println!(
        "priced: {}; unpriced: {}",
        if priced.is_empty() {
            "none".to_string()
        } else {
            priced.join(", ")
        },
        Alternative::unpriced(&report.alternatives)
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
}

/// The shipped files may not carry a verdict or a threshold.
#[test]
fn neither_shipped_measurement_file_carries_a_verdict_or_a_criterion() {
    for name in ["benches/w6-ladder.json", "benches/w6-spike.json"] {
        let text = std::fs::read_to_string(repo().join(name)).expect("the file is present");
        let value: serde_json::Value = serde_json::from_str(&text).expect("it is JSON");
        let mut stack = vec![&value];
        while let Some(node) = stack.pop() {
            if let serde_json::Value::Object(fields) = node {
                for key in fields.keys() {
                    assert!(
                        !matches!(
                            key.as_str(),
                            "verdict"
                                | "criteria"
                                | "min_share"
                                | "defer_share"
                                | "min_spike"
                                | "defer_spike"
                                | "min_projection"
                                | "gray_spike"
                                | "alternative_margin"
                                | "max_negative_share"
                        ),
                        "`{name}` carries `{key}`, which is a bar a measurement may not supply"
                    );
                }
                stack.extend(fields.values());
            }
            if let serde_json::Value::Array(items) = node {
                stack.extend(items.iter());
            }
        }
    }
}

/// **The staleness guard, and the one this audit exists because nothing had.**
#[test]
fn the_shipped_ladder_still_describes_the_tree_it_ships_in() {
    let report = shipped();
    let ladder = report.ladder().expect("the shipped ladder assembles");
    let fresh = ply_corpus::w6_run::in_process(&repo(), 256, 500, 2)
        .expect("the in-process rungs re-take without a database");

    let taken: Vec<(Layer, f64)> = vec![
        (Layer::Call, fresh.call_micros),
        (Layer::Endpoint, fresh.endpoint_micros),
        (Layer::Framing, fresh.framed_micros),
        (Layer::Routing, fresh.routed_micros),
    ];
    let of = |layer: Layer| -> f64 {
        ladder
            .rungs
            .iter()
            .find(|r| r.layer == layer)
            .unwrap_or_else(|| panic!("the shipped ladder carries a `{}` rung", layer.label()))
            .with_micros
    };
    // Both sides are divided by their own `framing`, so a profile cancels.
    let (shipped_unit, fresh_unit) = (of(Layer::Framing), fresh.framed_micros);
    assert!(
        shipped_unit > 0.0 && fresh_unit > 0.0,
        "a zero framing rung"
    );

    let mut stale = Vec::new();
    println!(
        "rung          shipped `with` µs      re-taken µs     ratio    shape (of framing)  \
         shipped shape"
    );
    for (layer, now) in taken {
        let was = of(layer);
        let ratio = if now > 0.0 { was / now } else { f64::INFINITY };
        let (shape, shipped_shape) = (now / fresh_unit, was / shipped_unit);
        println!(
            "  {:<10} {:>14.3} {:>16.3} {:>9.2}x {:>18.4} {:>14.4}",
            layer.label(),
            was,
            now,
            ratio,
            shape,
            shipped_shape
        );
        let shape_ratio = if shape > 0.0 {
            shipped_shape / shape
        } else {
            f64::INFINITY
        };
        if !(0.25..=4.0).contains(&shape_ratio) {
            stale.push(format!(
                "`{}`: the file has it at {:.4} of the framing rung and the tree has it at {:.4} \
                 ({:.1}x apart), which no build profile explains",
                layer.label(),
                shipped_shape,
                shape,
                shape_ratio
            ));
        }
        if !cfg!(debug_assertions) && !(0.25..=4.0).contains(&ratio) {
            stale.push(format!(
                "`{}`: the file says {:.2}µs and this release build measures {:.2}µs ({:.1}x)",
                layer.label(),
                was,
                now,
                ratio
            ));
        }
    }
    if cfg!(debug_assertions) {
        println!(
            "this is a debug build, so only the shape was checked; the shipped file is a release \
             measurement and `cargo test --release` checks its microseconds too"
        );
    }

    assert!(
        stale.is_empty(),
        "`benches/w6-ladder.json` no longer describes this tree, so the interpreter share, the \
         Amdahl projection and `Decision::reopens_at` computed from it are all about a program \
         that is not here:\n  {}\nRe-take it: `ply-corpus w6-ladder --repo . --db <url> \
         --machine <name> --postgres <version> --out benches/w6-ladder.json`, which writes exactly \
         this file — the alternatives, the limits and the `not_measured` list included.",
        stale.join("\n  ")
    );
}

/// The pinned criteria and the shipped numbers together, so a reader can see which criterion the
/// verdict turned on rather than being told.
#[test]
fn the_shipped_verdict_turns_on_c3() {
    let report = shipped();
    let ladder = report.ladder().expect("the shipped ladder assembles");
    let criteria = Criteria::default();
    let spike = report
        .spike
        .as_ref()
        .expect("the shipped report has a spike");
    let judged = spike.judge();

    assert!(judged.evidence, "C4: {:?}", judged.failures);
    assert!(
        !w6::c3_gaps(&report.alternatives).is_empty(),
        "C3 is what the deferral rests on, so the report needs at least one unpriced lever"
    );

    let share = ladder.conservative_share;
    let e = w6::projected(share, judged.speedup);
    let decision = report.decision(&ladder);
    assert_eq!(decision.verdict, Verdict::Defer, "{:?}", decision.reasons);
    assert!(
        decision.reasons.iter().any(|r| r.contains("not priced")),
        "the deferral names the unpriced levers: {:?}",
        decision.reasons
    );
    println!(
        "S = {:.3} (attributed {:.3}, band {}), k = {:.2}, E = {:.2}, ceiling = {:.2}; C1 {} at \
         {:.2}, C2 {} at {:.2}/{:.2}\n{}",
        share,
        ladder.interpreter_share,
        match (ladder.share_low, ladder.share_high) {
            (Some(low), Some(high)) => format!("{low:.3}–{high:.3}"),
            _ => "one sample".to_string(),
        },
        judged.speedup,
        e,
        w6::ceiling(share),
        if share >= criteria.min_share {
            "passes"
        } else {
            "fails"
        },
        criteria.min_share,
        if judged.speedup >= criteria.min_spike && e >= criteria.min_projection {
            "passes"
        } else {
            "fails"
        },
        criteria.min_spike,
        criteria.min_projection,
        decision.reopens_at
    );
}

/// **The file is the command's output, not a document assembled around it.**
#[test]
fn the_shipped_ladder_is_what_the_command_writes() {
    let path = repo().join("benches/w6-ladder.json");
    let text = std::fs::read_to_string(&path).expect("the shipped ladder is present");
    let report: Report = serde_json::from_str(&text).expect("it is a W6 report");
    let written = format!(
        "{}\n",
        serde_json::to_string_pretty(&report).expect("a report serializes")
    );
    assert_eq!(
        text, written,
        "`benches/w6-ladder.json` is not what `ply-corpus w6-ladder --out` writes, so it was \
         edited by a hand rather than taken by the harness — and the two staleness guards tell a \
         contributor to re-take it, which would silently drop whatever the hand added"
    );
}
