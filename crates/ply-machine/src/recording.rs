//! A `simulate` region's recording, as the `sim` package spells it: the seed that names a schedule
//! going in, and each scheduling point and the verdict coming out. Both the prover and the tester
//! hand one interleaving at a time to a search that runs in the program.

use crate::payload::{ctor, field_of, option, record};
use ply_eval::{Diagnostic, Interleaving, Span, Value as PlyValue, codes};

/// Where each type this side marshals is declared, and every case it builds of it.
pub const MARSHALLED: &[(&str, &str, &[&str])] = &[
    ("sim.recording", "Access", &["AAtom", "ACell", "AAlloc"]),
    ("sim.recording", "Verdict", &["Passed", "Failed"]),
];

fn case(ty: &str, name: &str, args: Vec<PlyValue>) -> PlyValue {
    let (home, _, cases) = MARSHALLED
        .iter()
        .find(|(_, declared, _)| *declared == ty)
        .unwrap_or_else(|| panic!("`{ty}` is not a type this side marshals"));
    assert!(
        cases.contains(&name),
        "`{name}` is not a case of `{ty}` this side builds"
    );
    ctor(home, name, args)
}

/// A `sim.plan.Seed`: its root and the choices before the stream decides.
pub fn seed_of(value: &PlyValue, span: Span) -> Result<ply_eval::Seed, Diagnostic> {
    let root = match field_of(value, "root", span)? {
        PlyValue::Fixed(f) => f.bits() as u64,
        _ => return Err(malformed("a seed's root is no `U64`", span)),
    };
    let mut path = Vec::new();
    for choice in field_of(value, "path", span)?.as_list(span, "a seed's path")? {
        path.push(
            u16::try_from(choice.as_int(span, "a choice")?).map_err(|_| {
                malformed(
                    "a seed's choice is past what a scheduling point offers",
                    span,
                )
            })?,
        );
    }
    Ok(ply_eval::Seed::at(root, path))
}

/// A `sim.recording.Interleaving`, failed with `failed` when the run failed.
pub fn interleaving_value(interleaving: &Interleaving, failed: Option<PlyValue>) -> PlyValue {
    record(vec![
        (
            "steps",
            PlyValue::list(interleaving.steps.iter().map(step_value).collect()),
        ),
        (
            "verdict",
            match failed {
                None => case("Verdict", "Passed", Vec::new()),
                Some(why) => case("Verdict", "Failed", vec![why]),
            },
        ),
        ("virtual_time", PlyValue::Int(interleaving.virtual_time)),
    ])
}

/// A `{ module, start, end }` span, as a recording places a step.
pub fn span_value(span: Span) -> PlyValue {
    record(vec![
        ("module", PlyValue::Int(i64::from(span.source.0))),
        ("start", PlyValue::Int(i64::from(span.start))),
        ("end", PlyValue::Int(i64::from(span.end))),
    ])
}

/// What a recording's `{ module, start, end }` names.
pub fn span_of(value: &PlyValue, span: Span) -> Result<Span, Diagnostic> {
    let part = |name: &str| -> Result<u32, Diagnostic> {
        u32::try_from(field_of(value, name, span)?.as_int(span, name)?)
            .map_err(|_| malformed("a span is out of range", span))
    };
    Ok(Span::new(
        ply_eval::SourceId(part("module")?),
        part("start")?,
        part("end")?,
    ))
}

fn step_value(step: &ply_eval::region::Step) -> PlyValue {
    let int = |n: u64| PlyValue::Int(i64::try_from(n).unwrap_or(i64::MAX));
    record(vec![
        ("region", int(u64::from(step.region.0))),
        ("task", int(step.task.0)),
        (
            "enabled",
            PlyValue::list(step.enabled.iter().map(|t| int(t.0)).collect()),
        ),
        ("choice", PlyValue::Int(i64::from(step.choice))),
        (
            "accesses",
            PlyValue::list(step.accesses.accesses().map(access_value).collect()),
        ),
        (
            "site",
            record(vec![
                (
                    "definition",
                    option(step.definition.as_ref().map(|d| PlyValue::str(d.as_str()))),
                ),
                ("span", span_value(step.span)),
            ]),
        ),
        (
            "stamp",
            PlyValue::list(step.stamp.iter().map(|&n| int(u64::from(n))).collect()),
        ),
    ])
}

fn access_value(access: &ply_eval::sim::Access) -> PlyValue {
    use ply_eval::sim::Access;
    match access {
        Access::Atom(atom) => case(
            "Access",
            "AAtom",
            vec![record(vec![
                ("effect", PlyValue::str(atom.effect.as_str())),
                (
                    "resource",
                    option(match &atom.resource {
                        ply_eval::Resource::Named(name) => Some(PlyValue::str(name.as_str())),
                        ply_eval::Resource::Var(n) => Some(PlyValue::str(format!("${n}"))),
                        ply_eval::Resource::Every => Some(PlyValue::str("*")),
                        ply_eval::Resource::Singleton => None,
                    }),
                ),
                ("write", PlyValue::Bool(atom.mode == ply_eval::Mode::Write)),
                (
                    "op",
                    option(atom.op.as_ref().map(|op| PlyValue::str(op.as_str()))),
                ),
            ])],
        ),
        Access::Cell { id, mode } => case(
            "Access",
            "ACell",
            vec![record(vec![
                ("index", PlyValue::Int(i64::from(id.index()))),
                ("generation", PlyValue::Int(i64::from(id.generation()))),
                ("write", PlyValue::Bool(*mode == ply_eval::Mode::Write)),
            ])],
        ),
        Access::Alloc => case("Access", "AAlloc", Vec::new()),
    }
}

fn malformed(why: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("the program's recording is malformed: {why}"),
    )
    .primary(span, "this is what the program sent")
    .note("`sim` and this reader are one program's two halves; this is Ply's fault")
}
