//! The units the suite's tests enter, as the builder makes them.

use ply_codegen::c::Native;
use ply_codegen::source::Source;

/// `PLY_C_CACHE` and the bucket counters are process-wide: a test that changes or counts them takes
/// this for writing, every other build for reading.
pub static CONFIG: std::sync::RwLock<()> = std::sync::RwLock::new(());

/// What the builder makes of `files`, `(path, text)` each: the front end's answer, a refusal's
/// included, and the unit's C, every root offered.
pub fn made(files: &[(String, String)]) -> ply_machine::runnable::Runnable {
    let bytes = ply_machine::builds::answered(files)
        .unwrap_or_else(|d| panic!("the builder answers: {}", d.message));
    ply_machine::runnable::decode(&bytes).unwrap_or_else(|why| panic!("the answer reads: {why}"))
}

/// [`made`] over `modules`, `(module name, text)` each written to the file its name spells,
/// which have to check.
pub fn answered(modules: &[(&str, &str)]) -> ply_machine::runnable::Runnable {
    let files: Vec<(String, String)> = modules
        .iter()
        .map(|(name, text)| {
            (
                format!("{}.ply", name.replace('.', "/")),
                (*text).to_string(),
            )
        })
        .collect();
    let answer = made(&files);
    assert!(
        !answer.front.answer.has_error(),
        "the fixture checks: {:?}",
        answer.front.answer.diagnostics
    );
    answer
}

/// `answer`'s unit loaded over its front end's answer, and the refusals it records; nothing
/// where no C compiler runs.
pub fn loaded(
    answer: ply_machine::runnable::Runnable,
) -> Option<(&'static Source, Native, Vec<ply_codegen::c::Refused>)> {
    let _config = CONFIG.read().unwrap_or_else(|e| e.into_inner());
    load(answer)
}

/// [`loaded`] by a caller that holds [`CONFIG`] already.
pub fn load(
    answer: ply_machine::runnable::Runnable,
) -> Option<(&'static Source, Native, Vec<ply_codegen::c::Refused>)> {
    let front: &'static ply_eval::Analysis = Box::leak(Box::new(answer.front.answer));
    let source: &'static Source = Box::leak(Box::new(Source::from_analysis(front)));
    match ply_codegen::c::load_unit(&answer.unit, Some(source), "unit") {
        Ok((native, refused)) => Some((source, native, refused)),
        Err(e) if e.to_string().contains("could not run") => None,
        Err(e) => panic!("{e}"),
    }
}

pub fn unit(text: &str) -> Option<(&'static Source, Native)> {
    with_refusals(text).map(|(s, n, _)| (s, n))
}

pub fn with_refusals(
    text: &str,
) -> Option<(&'static Source, Native, Vec<ply_codegen::c::Refused>)> {
    loaded(answered(&[("m", text)]))
}
