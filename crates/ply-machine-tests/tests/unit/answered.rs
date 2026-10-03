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
