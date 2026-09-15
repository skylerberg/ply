use ply_corpus::w6::*;

fn point(layer: Layer, with: f64, without: f64) -> Point {
    Point {
        layer,
        taken_on: "/items".to_string(),
        with_micros: with,
        without_micros: without,
        // A 1% spread on every rung, so a band exists to be reasoned about and no test depends
        // on one having been taken once.
        with_worst_micros: Some(with * 1.01),
        without_worst_micros: Some(without * 1.01),
        requests: 1000,
    }
}

/// One rung per layer, summing to 100µs of a 120µs request: 60µs of interpreter (50%), 40µs of
/// host, 20µs of residue.
fn full_points() -> Vec<Point> {
    vec![
        point(Layer::Call, 5.0, 0.0),
        point(Layer::Endpoint, 20.0, 5.0),
        point(Layer::Framing, 45.0, 20.0),
        point(Layer::Routing, 55.0, 45.0),
        point(Layer::Machine, 60.0, 55.0),
        point(Layer::Socket, 80.0, 60.0),
        point(Layer::Tls, 90.0, 80.0),
        point(Layer::Database, 95.0, 90.0),
        point(Layer::Tracing, 100.0, 95.0),
    ]
}

fn spike(speedup: f64) -> Spike {
    Spike {
        function: "std.http::read_line".to_string(),
        chosen_because: "the stage table's hottest pure function".to_string(),
        nodes: 41,
        compile_micros: 900.0,
        inputs: (0..3)
            .map(|i| SpikeInput {
                name: format!("head-{i}"),
                interpreter_best_micros: 10.0,
                interpreter_worst_micros: 12.0,
                spike_best_micros: 10.0 / speedup * 0.9,
                spike_worst_micros: 10.0 / speedup,
                agreed: true,
            })
            .collect(),
    }
}

fn priced(name: &str, ratio: f64) -> Alternative {
    Alternative {
        name: name.to_string(),
        what: "a change".to_string(),
        priced: true,
        end_to_end: ratio,
        evidence: "the served workload with the change and without it".to_string(),
        cost: "one change".to_string(),
    }
}

/// Every cheaper lever priced, with `best` the ratio of the best of them.
fn roster(best: f64) -> Vec<Alternative> {
    LEVERS
        .iter()
        .map(|lever| {
            priced(
                lever.name,
                if lever.name == "Env::lookup" {
                    best
                } else {
                    1.0
                },
            )
        })
        .collect()
}

#[test]
fn every_layer_is_in_the_order_and_carries_its_prose() {
    assert_eq!(Layer::ORDER.len(), 9);
    for (i, layer) in Layer::ORDER.into_iter().enumerate() {
        assert_eq!(layer.rank(), i);
        assert!(!layer.label().is_empty());
        assert!(!layer.isolates().is_empty());
        assert!(!layer.substitution().is_empty());
    }
    let interpreter: Vec<&str> = Layer::ORDER
        .into_iter()
        .filter(|l| l.is_interpreter())
        .map(Layer::label)
        .collect();
    assert_eq!(
        interpreter,
        ["call", "endpoint", "framing", "routing", "machine"]
    );
}

/// The residue is the whole point of the table: a ladder that attributed everything would be
/// hiding what it did not separate.
#[test]
fn a_ladder_reports_its_layers_its_residue_and_a_lower_bound_share() {
    let ladder = Ladder::assemble(4.0, 120.0, &full_points()).unwrap();
    assert_eq!(ladder.rungs.len(), 9);
    assert!((ladder.attributed_micros - 100.0).abs() < 1e-9);
    assert!((ladder.residue_micros - 20.0).abs() < 1e-9);
    assert!((ladder.interpreter_micros - 60.0).abs() < 1e-9);
    assert!((ladder.interpreter_share - 0.5).abs() < 1e-9);
    assert!((ladder.over_floor - 30.0).abs() < 1e-9);
    assert!(ladder.missing().is_empty());
}

#[test]
fn a_ladder_refuses_what_a_share_cannot_be_read_off() {
    let mut duplicated = full_points();
    duplicated.push(point(Layer::Tracing, 105.0, 100.0));
    let err = Ladder::assemble(4.0, 120.0, &duplicated)
        .unwrap_err()
        .to_string();
    assert!(err.contains("twice"), "{err}");

    let out_of_order = vec![
        point(Layer::Framing, 45.0, 20.0),
        point(Layer::Endpoint, 20.0, 5.0),
    ];
    let err = Ladder::assemble(4.0, 120.0, &out_of_order)
        .unwrap_err()
        .to_string();
    assert!(err.contains("request order"), "{err}");

    assert!(Ladder::assemble(4.0, 0.0, &full_points()).is_err());
    assert!(Ladder::assemble(0.0, 120.0, &full_points()).is_err());
    assert!(Ladder::assemble(4.0, 120.0, &[]).is_err());

    let zero = vec![Point {
        requests: 0,
        ..point(Layer::Call, 5.0, 0.0)
    }];
    assert!(Ladder::assemble(4.0, 120.0, &zero).is_err());

    let anonymous = vec![Point {
        taken_on: String::new(),
        ..point(Layer::Call, 5.0, 0.0)
    }];
    let err = Ladder::assemble(4.0, 120.0, &anonymous)
        .unwrap_err()
        .to_string();
    assert!(err.contains("names no route"), "{err}");
}

/// Two rungs on two routes have a difference that is not one layer.
#[test]
fn a_route_change_between_two_rungs_is_an_audit_finding() {
    let mut points = full_points();
    points[1].taken_on = "/health".to_string();
    let report = report(points);
    let findings = report.audit();
    assert!(
        findings
            .iter()
            .any(|f| f.contains("`/health`") && f.contains("route change")),
        "{findings:?}"
    );
}

#[test]
fn a_negative_layer_is_reported_rather_than_clamped() {
    let mut points = full_points();
    points[6] = point(Layer::Tls, 74.0, 80.0);
    let ladder = Ladder::assemble(4.0, 120.0, &points).unwrap();
    assert!(ladder.rungs[6].layer_micros < 0.0);
    assert!((ladder.worst_negative_share - 6.0 / 120.0).abs() < 1e-9);
}

/// A layer whose repeats span zero has not measured its own sign, and two decimals of it are
/// two decimals of the machine it ran on.
#[test]
fn a_layer_narrower_than_its_own_repeats_is_named_rather_than_printed() {
    let mut points = full_points();
    points[8] = Point {
        with_micros: 100.0,
        without_micros: 99.0,
        with_worst_micros: Some(112.0),
        without_worst_micros: Some(110.0),
        ..point(Layer::Tracing, 100.0, 99.0)
    };
    let ladder = Ladder::assemble(4.0, 120.0, &points).unwrap();
    let tracing = ladder.rungs.last().unwrap();
    assert!((tracing.layer_micros - 1.0).abs() < 1e-9);
    assert!(tracing.sign_unresolved(), "{tracing:?}");
    let mut report = report(points);
    report.total_micros = 120.0;
    assert!(
        report
            .audit()
            .iter()
            .any(|f| f.contains("did not resolve its sign")),
        "{:?}",
        report.audit()
    );
}

/// A negative residue is the layers summing to more than the request they were read against,
/// which can only be the in-process arena over-counting.
#[test]
fn a_negative_residue_is_charged_to_the_share_the_decision_reads() {
    let mut points = full_points();
    // 140µs of layers against a 120µs request: a −20µs residue.
    points[7] = point(Layer::Database, 125.0, 90.0);
    points[8] = point(Layer::Tracing, 130.0, 125.0);
    let ladder = Ladder::assemble(4.0, 120.0, &points).unwrap();
    assert!(ladder.residue_micros < 0.0);
    assert!((ladder.interpreter_share - 0.5).abs() < 1e-9);
    assert!(
        ladder.conservative_share < ladder.interpreter_share,
        "{:?} against {:?}",
        ladder.conservative_share,
        ladder.interpreter_share
    );
    let decision = decide(
        &ladder,
        Some(&spike(9.0)),
        &roster(1.0),
        &Criteria::default(),
    );
    assert!(
        (decision.interpreter_share - ladder.conservative_share).abs() < 1e-9,
        "the decision read {:.3} and the conservative share is {:.3}",
        decision.interpreter_share,
        ladder.conservative_share
    );

    // And the other direction: a positive residue is credited to nobody, so the share stays
    // exactly what the rungs attributed.
    let positive = Ladder::assemble(4.0, 120.0, &full_points()).unwrap();
    assert!(positive.residue_micros > 0.0);
    assert!((positive.conservative_share - positive.interpreter_share).abs() < 1e-9);
}

/// The share is one number read off one run, and M9's whole case is on which side of 50% it
/// falls.
#[test]
fn a_share_whose_repeats_straddle_the_bar_decides_nothing() {
    let mut points = full_points();
    points[4] = Point {
        with_worst_micros: Some(66.0),
        ..point(Layer::Machine, 60.0, 55.0)
    };
    let straddling = Ladder::assemble_with(
        4.0,
        120.0,
        &points,
        &Denominators {
            total_worst_micros: Some(132.0),
            ..Denominators::default()
        },
    )
    .unwrap();
    assert!(straddling.share_low.unwrap() < 0.50);
    assert!(straddling.share_high.unwrap() >= 0.50);
    let decision = decide(
        &straddling,
        Some(&spike(3.0)),
        &roster(1.0),
        &Criteria::default(),
    );
    assert_eq!(
        decision.verdict,
        Verdict::Undecided,
        "{:?}",
        decision.reasons
    );
    assert!(decision.reopens_at.contains("repeat the ladder"));

    // C3 is checked before it, because C3 reads no share: an unpriced lever defers whatever the
    // band does.
    let deferred = decide(&straddling, Some(&spike(3.0)), &[], &Criteria::default());
    assert_eq!(deferred.verdict, Verdict::Defer);
}

#[test]
fn amdahl_is_the_projection_and_the_ceiling_is_its_limit() {
    assert!((projected(0.5, 3.0) - 1.5).abs() < 1e-9);
    assert!((projected(1.0, 4.0) - 4.0).abs() < 1e-9);
    assert!((projected(0.0, 100.0) - 1.0).abs() < 1e-9);
    assert!((projected(0.3, 1.0) - 1.0).abs() < 1e-9);
    assert!((ceiling(0.5) - 2.0).abs() < 1e-9);
    assert!((ceiling(0.35) - 1.5384615).abs() < 1e-6);
}

/// A speedup is the weakest input's, and a disagreement or an overlap is not a slower speedup —
/// it is no measurement at all.
#[test]
fn a_spike_is_evidence_only_when_it_agreed_and_separated_on_enough_inputs() {
    let good = spike(4.0);
    let judged = good.judge();
    assert!(judged.evidence, "{:?}", judged.failures);
    assert!((judged.speedup - 4.0).abs() < 1e-9);

    let mut wrong = spike(4.0);
    wrong.inputs[1].agreed = false;
    assert!(!wrong.judge().evidence);
    assert!(wrong.judge().failures[0].contains("disagreed"));

    let mut overlapping = spike(4.0);
    overlapping.inputs[2].spike_worst_micros = 11.0;
    assert!(!overlapping.judge().evidence);
    assert!(overlapping.judge().failures[0].contains("overlap"));

    let mut thin = spike(4.0);
    thin.inputs.truncate(2);
    assert!(!thin.judge().evidence);

    let mut uneven = spike(4.0);
    uneven.inputs[0].spike_worst_micros = 9.0;
    assert!((uneven.judge().speedup - 10.0 / 9.0).abs() < 1e-9);
}

#[test]
fn a_missing_rung_or_a_missing_spike_is_undecided_rather_than_deferred() {
    let partial = Ladder::assemble(4.0, 120.0, &full_points()[..4]).unwrap();
    let decision = decide(&partial, Some(&spike(4.0)), &[], &Criteria::default());
    assert_eq!(decision.verdict, Verdict::Undecided);
    assert!(
        decision.reasons[0].contains("no `machine`, `socket`, `tls`, `database`, `tracing`"),
        "{:?}",
        decision.reasons
    );

    let full = Ladder::assemble(4.0, 120.0, &full_points()).unwrap();
    let decision = decide(&full, None, &[], &Criteria::default());
    assert_eq!(decision.verdict, Verdict::Undecided);
    assert!(decision.reasons[0].contains("no codegen spike"));

    let mut points = full_points();
    points[6] = point(Layer::Tls, 60.0, 80.0);
    let bent = Ladder::assemble(4.0, 120.0, &points).unwrap();
    let decision = decide(&bent, Some(&spike(4.0)), &[], &Criteria::default());
    assert_eq!(decision.verdict, Verdict::Undecided);
    assert!(decision.reasons[0].contains("negative"));
}

/// The withdrawal of the ladder, in code: the ladder answers about what it measured.
#[test]
fn a_verdict_names_the_workload_it_was_taken_on_and_never_names_a_milestone() {
    let full = report(full_points());
    let rendered = rendered(&full).unwrap();
    assert_eq!(rendered.decision.workload, WORKLOAD);
    for verdict in [
        Verdict::Advance,
        Verdict::Conditional,
        Verdict::Defer,
        Verdict::Undecided,
    ] {
        assert!(
            !verdict.label().contains("M9"),
            "`{}` names a milestone; the ladder decides a workload",
            verdict.label()
        );
    }
    assert!(
        !rendered.decision.reopens_at.contains("M9"),
        "the reopen sentence names a milestone: {}",
        rendered.decision.reopens_at
    );

    let text = render(&full);
    assert!(
        text.contains("workload: the served HTTP workload"),
        "the rendered report does not say what its share was taken on:\n{text}"
    );
    assert!(
        !text.contains("M9:"),
        "the rendered report still heads its verdict with a milestone:\n{text}"
    );
}

#[test]
fn an_unpriced_alternative_defers_whatever_the_share_says() {
    let ladder = Ladder::assemble(4.0, 120.0, &full_points()).unwrap();
    let mut alternatives = roster(1.1);
    alternatives[1].priced = false;
    alternatives[1].end_to_end = 0.0;
    let decision = decide(
        &ladder,
        Some(&spike(9.0)),
        &alternatives,
        &Criteria::default(),
    );
    assert_eq!(decision.verdict, Verdict::Defer);
    assert!(
        decision.reasons.iter().any(|r| r.contains("not priced")),
        "{:?}",
        decision.reasons
    );
    // The share and the spike both clear their bars here, so what reopens M9 is the pricing and
    // not either of them: 1 + (1.80 − 1)/2.
    assert!(
        decision.reopens_at.contains("1.40x end to end"),
        "{}",
        decision.reopens_at
    );
}

#[test]
fn an_unpriced_alternative_under_a_small_share_still_names_the_share() {
    let points = vec![
        point(Layer::Call, 1.0, 0.0),
        point(Layer::Endpoint, 4.0, 1.0),
        point(Layer::Framing, 9.0, 4.0),
        point(Layer::Routing, 11.0, 9.0),
        point(Layer::Machine, 12.0, 11.0),
        point(Layer::Socket, 32.0, 12.0),
        point(Layer::Tls, 52.0, 32.0),
        point(Layer::Database, 96.0, 52.0),
        point(Layer::Tracing, 100.0, 96.0),
    ];
    let ladder = Ladder::assemble(4.0, 120.0, &points).unwrap();
    let mut alternatives = roster(1.1);
    alternatives[1].priced = false;
    alternatives[1].end_to_end = 0.0;
    let decision = decide(
        &ladder,
        Some(&spike(9.0)),
        &alternatives,
        &Criteria::default(),
    );
    assert_eq!(decision.verdict, Verdict::Defer);
    assert!(
        decision.reopens_at.contains("reopens"),
        "{}",
        decision.reopens_at
    );
}

/// **C3 is checked against the cheaper levers, not against the file.**
#[test]
fn a_report_that_prices_no_lever_at_all_defers_and_names_all_seven() {
    let ladder = Ladder::assemble(4.0, 120.0, &full_points()).unwrap();
    let decision = decide(&ladder, Some(&spike(9.0)), &[], &Criteria::default());
    assert_eq!(
        decision.verdict,
        Verdict::Defer,
        "an empty list prices nothing: {:?}",
        decision.reasons
    );
    for lever in &LEVERS {
        assert!(
            decision.reasons.iter().any(|r| r.contains(lever.name)),
            "`{}` is unmentioned in {:?}",
            lever.name,
            decision.reasons
        );
    }
    assert_eq!(c3_gaps(&[]).len(), LEVERS.len());
}

/// The same hole through the values rather than through the field: a lever may be claimed as
/// priced, but a claim with nothing behind it is not a measurement and does not answer C3.
#[test]
fn a_lever_priced_without_evidence_is_not_priced() {
    let ladder = Ladder::assemble(4.0, 120.0, &full_points()).unwrap();
    let mut claimed = roster(1.0);
    for alternative in &mut claimed {
        alternative.evidence.clear();
    }
    let decision = decide(&ladder, Some(&spike(9.0)), &claimed, &Criteria::default());
    assert_eq!(decision.verdict, Verdict::Defer, "{:?}", decision.reasons);
    assert!(
        decision.reasons.iter().any(|r| r.contains("evidence")),
        "{:?}",
        decision.reasons
    );
    assert!(Alternative::best(&claimed).is_none());
}

#[test]
fn a_half_interpreter_request_with_a_three_times_spike_advances() {
    let ladder = Ladder::assemble(4.0, 120.0, &full_points()).unwrap();
    let decision = decide(
        &ladder,
        Some(&spike(3.0)),
        &roster(1.1),
        &Criteria::default(),
    );
    assert_eq!(decision.verdict, Verdict::Advance, "{:?}", decision.reasons);
    assert!((decision.projected - 1.5).abs() < 1e-9);
}

#[test]
fn a_small_share_defers_and_names_the_ceiling() {
    // 12µs of interpreter in a 120µs request: 10%.
    let points = vec![
        point(Layer::Call, 1.0, 0.0),
        point(Layer::Endpoint, 4.0, 1.0),
        point(Layer::Framing, 9.0, 4.0),
        point(Layer::Routing, 11.0, 9.0),
        point(Layer::Machine, 12.0, 11.0),
        point(Layer::Socket, 32.0, 12.0),
        point(Layer::Tls, 52.0, 32.0),
        point(Layer::Database, 96.0, 52.0),
        point(Layer::Tracing, 100.0, 96.0),
    ];
    let ladder = Ladder::assemble(4.0, 120.0, &points).unwrap();
    let decision = decide(
        &ladder,
        Some(&spike(20.0)),
        &roster(1.02),
        &Criteria::default(),
    );
    assert_eq!(decision.verdict, Verdict::Defer);
    assert!(
        decision
            .reasons
            .iter()
            .any(|r| r.contains("infinitely fast")),
        "{:?}",
        decision.reasons
    );
    assert!(decision.reopens_at.contains("reopens"));
}

#[test]
fn a_cheaper_lever_within_half_of_the_projection_defers() {
    let ladder = Ladder::assemble(4.0, 120.0, &full_points()).unwrap();
    let decision = decide(
        &ladder,
        Some(&spike(3.0)),
        &roster(1.4),
        &Criteria::default(),
    );
    assert_eq!(decision.verdict, Verdict::Defer);
    assert!(
        decision
            .reasons
            .iter()
            .any(|r| r.contains("permanent surface")),
        "{:?}",
        decision.reasons
    );
}

#[test]
fn the_grey_band_needs_a_much_better_spike() {
    // 48µs of 120µs is 40%: above the defer floor, below the advance bar.
    let points = vec![
        point(Layer::Call, 4.0, 0.0),
        point(Layer::Endpoint, 16.0, 4.0),
        point(Layer::Framing, 36.0, 16.0),
        point(Layer::Routing, 44.0, 36.0),
        point(Layer::Machine, 48.0, 44.0),
        point(Layer::Socket, 68.0, 48.0),
        point(Layer::Tls, 78.0, 68.0),
        point(Layer::Database, 92.0, 78.0),
        point(Layer::Tracing, 100.0, 92.0),
    ];
    let ladder = Ladder::assemble(4.0, 120.0, &points).unwrap();
    assert!((ladder.interpreter_share - 0.4).abs() < 1e-9);

    let modest = decide(
        &ladder,
        Some(&spike(3.0)),
        &roster(1.05),
        &Criteria::default(),
    );
    assert_eq!(modest.verdict, Verdict::Defer);

    let strong = decide(
        &ladder,
        Some(&spike(8.0)),
        &roster(1.05),
        &Criteria::default(),
    );
    assert_eq!(strong.verdict, Verdict::Conditional, "{:?}", strong.reasons);
}

fn report(points: Vec<Point>) -> Report {
    Report {
        provenance: Provenance {
            machine: "an M-series laptop".to_string(),
            profile: "release".to_string(),
            taken: "2026-08-15".to_string(),
            repeats: 3,
            request_head_bytes: 63,
            postgres: Some("postgres 17 on 5433".to_string()),
            not_measured: vec!["cancellation, because W5 has none".to_string()],
        },
        floor_micros: 4.0,
        total_micros: 120.0,
        denominators: Denominators {
            floor_taken_on: "the same bytes over plaintext with no interpreter".to_string(),
            total_taken_on: "/items over postgres over TLS".to_string(),
            total_worst_micros: Some(120.0),
        },
        points,
        spike: Some(spike(3.0)),
        alternatives: roster(1.1),
        offerings: vec![Offering {
            what: "one route, no db".to_string(),
            stack: "twin, http".to_string(),
            head_bytes: 63,
            concurrency: 8,
            per_second: 9000.0,
            p50_micros: 800.0,
            p99_micros: 2400.0,
            floor_per_second: Some(90000.0),
        }],
        limits: vec![Limit {
            what: "one machine is one core".to_string(),
            why: "a Ply value holds `Rc`, so a task cannot move between OS threads".to_string(),
            evidence: None,
        }],
    }
}

/// The audit is what makes the honest account a requirement rather than an intention: a report
/// missing a section says so above its own tables.
#[test]
fn the_audit_names_every_section_the_report_owes() {
    let complete = report(full_points());
    assert!(complete.audit().is_empty(), "{:?}", complete.audit());

    let mut thin = report(full_points());
    thin.spike = None;
    thin.offerings.clear();
    thin.limits.clear();
    thin.provenance.not_measured.clear();
    thin.alternatives.retain(|a| a.name != "response buffering");
    let findings = thin.audit();
    for expected in [
        "no codegen spike",
        "no offering",
        "no limits",
        "not_measured",
        "response buffering",
    ] {
        assert!(
            findings.iter().any(|f| f.contains(expected)),
            "`{expected}` missing from {findings:?}"
        );
    }

    let mut partial = report(full_points()[..5].to_vec());
    partial.total_micros = 120.0;
    assert!(
        partial
            .audit()
            .iter()
            .any(|f| f.contains("no `socket` rung"))
    );
}

/// A measurement file may not carry the bar it is about to clear, so the rendered verdict is
/// recomputed from `Criteria::default` every time.
#[test]
fn a_report_renders_its_tables_and_recomputes_its_verdict() {
    let out = render(&report(full_points()));
    for expected in [
        "the accumulated stack",
        "residue",
        "the codegen spike",
        "the cheaper levers",
        "what this language serves today",
        "where this is genuinely not competitive",
        "what W6 did not measure",
        "verdict: advance a code generator for this workload",
        "workload: the served HTTP workload",
    ] {
        assert!(out.contains(expected), "`{expected}` missing from:\n{out}");
    }
    assert!(!out.contains("this report is incomplete"), "{out}");

    let complete = report(full_points());
    let rendered = rendered(&complete).unwrap();
    assert_eq!(rendered.decision.verdict, Verdict::Advance);
    assert!(rendered.spike.unwrap().evidence);
}

/// A report round-trips as JSON: the two measuring agents produce the halves separately and the
/// decision is taken over the merged file.
#[test]
fn a_report_round_trips_through_json_without_carrying_a_verdict() {
    let original = report(full_points());
    let text = serde_json::to_string(&original).unwrap();
    assert!(
        !text.contains("verdict"),
        "a report may not carry a verdict"
    );
    let back: Report = serde_json::from_str(&text).unwrap();
    let ladder = back.ladder().unwrap();
    assert_eq!(
        back.decision(&ladder).verdict,
        original.decision(&original.ladder().unwrap()).verdict
    );
}
