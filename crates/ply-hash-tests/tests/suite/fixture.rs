use ply_span::{Diagnostic, SourceId};
use ply_ty::CheckOutput;

/// `files[i]` is `(module name, text)` for `SourceId(i)`.
#[track_caller]
pub fn port_front(files: &[(&str, &str)]) -> ply_ty::Front {
    let (named, ids) = inputs(files);
    ply_codegen::c::producer::checked_front(&named, &ids)
        .unwrap_or_else(|e| panic!("the program must typecheck: {e:#}"))
}

#[track_caller]
pub fn port_check(files: &[(&str, &str)]) -> CheckOutput {
    port_front(files).check
}

/// Every diagnostic when any is an error; empty when the port accepts.
#[track_caller]
pub fn port_errors(files: &[(&str, &str)]) -> Vec<Diagnostic> {
    let (named, ids) = inputs(files);
    ply_codegen::c::producer::ensure_default();
    let front = ply_codegen::c::producer::front(&named, &ids)
        .unwrap_or_else(|e| panic!("the port answers for the program: {e:#}"));
    if front.has_error() {
        front.diagnostics
    } else {
        Vec::new()
    }
}

fn inputs(files: &[(&str, &str)]) -> (Vec<(String, String)>, Vec<SourceId>) {
    let named = files
        .iter()
        .map(|(name, text)| ((*name).to_string(), (*text).to_string()))
        .collect();
    let ids = (0..files.len()).map(|i| SourceId(i as u32)).collect();
    (named, ids)
}
