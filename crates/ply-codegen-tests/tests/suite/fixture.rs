//! The units the suite's tests enter, as the builder makes them.

use ply_codegen::c::Native;
use ply_codegen::source::Source;

/// `PLY_C_CACHE` and the bucket counters are process-wide: a test that changes or counts them takes
/// this for writing, every other build for reading.
pub static CONFIG: std::sync::RwLock<()> = std::sync::RwLock::new(());

/// What the builder makes of `files`, `(path, text)` each: the front end's answer, a refusal's
/// included, and the unit's C, every root offered.
pub fn made(files: &[(String, String)]) -> ply_machine::runnable::Runnable {
    ply_machine::builds::answered_program(files)
        .unwrap_or_else(|d| panic!("the builder answers: {d}"))
}

/// What the builder makes of the standard library's own tree, which a load reads whole: every
/// module at the path its name spells, and each data file one embeds at its name, which is its
/// place beside them.
pub fn standard_library() -> ply_machine::runnable::Runnable {
    use ply_machine::shipped_modules;
    let modules: Vec<(String, &str)> = shipped_modules::sources()
        .into_iter()
        .filter(|(name, _)| name.starts_with("std."))
        .collect();
    let named: Vec<(&str, &str)> = modules
        .iter()
        .map(|(n, text)| (n.as_str(), *text))
        .collect();
    let mut files: Vec<(String, Vec<u8>)> = ply_machine::builds::module_files(&named)
        .into_iter()
        .map(|(path, text)| (path, text.into_bytes()))
        .collect();
    for name in shipped_modules::data_names() {
        let bytes = shipped_modules::data(&name).expect("a listed data file is carried");
        files.push((name, bytes.to_vec()));
    }
    ply_machine::builds::checked_program(&files)
        .unwrap_or_else(|d| panic!("the standard library checks: {d}"))
}

/// What the builder makes of `modules`, `(module name, text)` each, which have to check.
pub fn answered(modules: &[(&str, &str)]) -> ply_machine::runnable::Runnable {
    ply_machine::builds::checked_program(&ply_machine::builds::module_files(modules))
        .unwrap_or_else(|d| panic!("the fixture checks: {d}"))
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
