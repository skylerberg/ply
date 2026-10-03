//! The programs a test drives, as the builder answers for them: the front end's answer, and the
//! unit's C compiled here.

/// What the builder makes of `source`, which has to check.
#[track_caller]
fn checked_program(module: &str, source: &str) -> ply_machine::runnable::Runnable {
    let files = ply_machine::builds::module_files(&[(module, source)]);
    ply_machine::builds::checked_program(&files)
        .unwrap_or_else(|d| panic!("the fixture checks: {d}"))
}

/// The front end's answer for `source`, which has to check.
#[track_caller]
pub fn checked(module: &str, source: &str) -> ply_eval::Analysis {
    checked_program(module, source).front.answer
}

/// [`checked`], and the unit compiled from it.
#[track_caller]
pub fn compiled(module: &str, source: &str) -> (ply_eval::Analysis, &'static ply_codegen::Unit) {
    let program = checked_program(module, source);
    let front = program.front.answer;
    let unit = ply_codegen::Unit::handed(&front, program.unit).expect("this host has a C compiler");
    (front, unit)
}
