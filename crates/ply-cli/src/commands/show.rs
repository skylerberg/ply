//! `ply show` — one definition's text as its file holds it: the comment lines above it, its
//! `pub`, and its body through the end of its last line.

use super::common::{emit_json, report_diagnostics, report_load_error};
use crate::EXIT_OK;
use crate::cli::ShowArgs;
use crate::load::{Loaded, load};
use crate::style::Style;
use ply_span::{Diagnostic, SourceFile, Span, Symbol, codes};
use ply_ty::TypeDecl;
use serde_json::json;

pub const SCHEMA_VERSION: u32 = 1;

/// A definition and the bytes of its file that hold it.
pub(crate) struct Place<'a> {
    pub name: Symbol,
    pub simple_name: Symbol,
    pub span: Span,
    pub file: &'a SourceFile,
    pub start: usize,
    pub end: usize,
}

impl Place<'_> {
    pub fn source(&self) -> &str {
        &self.file.text[self.start..self.end]
    }
}

pub fn execute(args: &ShowArgs, style: Style) -> i32 {
    let loaded = match load(&args.path) {
        Ok(loaded) => loaded,
        Err(err) => return report_load_error("show", &err, args.json, style),
    };
    let place = match locate(&loaded, &args.query) {
        Ok(place) => place,
        Err(diagnostic) => {
            return report_diagnostics("show", &[diagnostic], &loaded, args.json, style);
        }
    };
    if args.json {
        emit_json(&json!({
            "command": "show",
            "schema_version": SCHEMA_VERSION,
            "ok": true,
            "exit_code": EXIT_OK,
            "name": place.name.as_str(),
            "file": place.file.path.display().to_string(),
            "start": place.start,
            "end": place.end,
            "source": place.source(),
        }));
    } else {
        print!("{}", place.source());
    }
    EXIT_OK
}

/// The `fn` or `type` `query` names, by its program-wide name or a simple name unique in the
/// program, and where its file holds it.
pub(crate) fn locate<'a>(loaded: &'a Loaded, query: &str) -> Result<Place<'a>, Diagnostic> {
    let (name, simple_name, span) = match super::callers::resolve(loaded, query) {
        Ok(name) => {
            let def = &loaded.check.defs[&name];
            (name, def.simple_name.clone(), def.span)
        }
        Err(unknown) => match type_named(loaded, query) {
            Some(Ok(t)) => (t.name.clone(), t.simple_name.clone(), t.span),
            Some(Err(ambiguous)) => return Err(ambiguous),
            None => return Err(unknown),
        },
    };
    let Some(file) = loaded.sources.containing(span) else {
        return Err(Diagnostic::error(
            codes::INTERNAL_ERROR,
            format!("`{name}` has no source text"),
        )
        .primary(span, "declared here"));
    };
    let (start, end) = ply_codegen::c::producer::item_range(&file.text, simple_name.as_str())
        .map_err(|e| {
            Diagnostic::error(
                codes::INTERNAL_ERROR,
                format!("the front end could not place `{name}`: {e:#}"),
            )
            .primary(span, "declared here")
        })?
        .map_err(|why| {
            Diagnostic::error(codes::UNKNOWN_NAME, format!("`{name}`: {why}"))
                .primary(span, "declared here")
        })?;
    Ok(Place {
        name,
        simple_name,
        span,
        file,
        start,
        end,
    })
}

/// A `type` by its program-wide name or a simple name unique in the program; `None` when no
/// type has the name.
fn type_named<'a>(loaded: &'a Loaded, query: &str) -> Option<Result<&'a TypeDecl, Diagnostic>> {
    let exact = Symbol::new(query);
    if let Some(t) = loaded.front.types.get(&exact) {
        return Some(Ok(t));
    }
    let matching: Vec<&TypeDecl> = loaded
        .front
        .types
        .values()
        .filter(|t| t.simple_name.as_str() == query)
        .collect();
    match matching[..] {
        [] => None,
        [one] => Some(Ok(one)),
        _ => Some(Err(Diagnostic::error(
            codes::UNKNOWN_NAME,
            format!("`{query}` names {} types", matching.len()),
        )
        .primary(Span::DUMMY, "say which")
        .note(format!(
            "one of: {}",
            matching
                .iter()
                .map(|t| t.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )))),
    }
}
