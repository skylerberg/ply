//! The programs a test drives, as the builder answers for them: the front end's answer, and the
//! unit's C compiled here.

/// What the builder makes of `source`, the module `module` names.
#[track_caller]
pub fn answered(module: &str, source: &str) -> ply_machine::runnable::Runnable {
    let files = [(
        format!("{}.ply", module.replace('.', "/")),
        source.to_string(),
    )];
    let bytes = ply_machine::builds::answered(&files)
        .unwrap_or_else(|d| panic!("the builder answers: {}", d.message));
    ply_machine::runnable::decode(&bytes).unwrap_or_else(|why| panic!("the answer reads: {why}"))
}

/// The front end's answer for `source`, which has to check.
#[track_caller]
pub fn checked(module: &str, source: &str) -> ply_eval::Analysis {
    let front = answered(module, source).front.answer;
    let errors: Vec<String> = front
        .diagnostics
        .iter()
        .filter(|d| d.severity == ply_eval::Severity::Error)
        .map(|d| format!("{}: {}", d.code, d.message))
        .collect();
    assert!(errors.is_empty(), "the fixture checks: {errors:?}");
    front
}

/// [`checked`], and the unit compiled from it.
#[track_caller]
pub fn tiered(module: &str, source: &str) -> (ply_eval::Analysis, &'static ply_codegen::Unit) {
    let answer = answered(module, source);
    let front = answer.front.answer;
    assert!(
        !front.has_error(),
        "the fixture checks: {:?}",
        front.diagnostics
    );
    let unit = ply_codegen::Unit::handed(&front, answer.unit).expect("this host has a C compiler");
    (front, unit)
}
