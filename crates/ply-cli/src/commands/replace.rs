//! `ply replace` — one definition rewritten from new text, formatted, with every other byte of
//! its file kept and no other definition's name or hash moved.

use super::common::{diagnostics_json, emit_json, print_diagnostics, report_load_error};
use super::show::{Place, locate};
use crate::cli::ReplaceArgs;
use crate::load::{Loaded, load};
use crate::style::Style;
use crate::{EXIT_COMPILE_ERROR, EXIT_OK};
use ply_span::{Diagnostic, SourceId, Symbol, codes};
use ply_ty::HashOutput;
use serde_json::json;
use std::collections::BTreeSet;
use std::path::Path;

pub const SCHEMA_VERSION: u32 = 1;

pub fn execute(args: &ReplaceArgs, style: Style) -> i32 {
    let loaded = match load(&args.path) {
        Ok(loaded) => loaded,
        Err(err) => return report_load_error("replace", &err, args.json, style),
    };
    let place = match locate(&loaded, &args.query) {
        Ok(place) => place,
        Err(diagnostic) => return report(&loaded, args, None, false, &[diagnostic], style),
    };
    let refused =
        |diagnostic: Diagnostic| report(&loaded, args, Some(&place), false, &[diagnostic], style);
    let item = match read_item(args.with.as_deref()) {
        Ok(item) => item,
        Err(diagnostic) => return refused(diagnostic),
    };
    let replaced =
        ply_codegen::c::producer::replace_item(&place.file.text, place.simple_name.as_str(), &item);
    let text = match replaced {
        Ok(Ok(text)) => text,
        Ok(Err(why)) => {
            return refused(
                Diagnostic::error(codes::REPLACEMENT_REFUSED, why)
                    .primary(place.span, "the definition to replace")
                    .note("the replacement is one item of the same kind and name"),
            );
        }
        Err(e) => {
            return refused(Diagnostic::error(
                codes::INTERNAL_ERROR,
                format!("the front end could not replace `{}`: {e:#}", place.name),
            ));
        }
    };
    let changed = text.as_str() != &*place.file.text;
    if changed {
        if let Err(diagnostic) = still_the_same_program(&loaded, &place, &text) {
            return refused(diagnostic);
        }
        if !args.check {
            if let Err(e) = std::fs::write(&place.file.path, &text) {
                return refused(Diagnostic::error(
                    codes::RUNTIME_ERROR,
                    format!("could not write `{}`: {e}", place.file.path.display()),
                ));
            }
        }
    }
    report(&loaded, args, Some(&place), changed, &[], style)
}

/// The new item from `--with`, or from stdin.
fn read_item(with: Option<&Path>) -> Result<String, Diagnostic> {
    match with {
        Some(path) => std::fs::read_to_string(path).map_err(|e| {
            Diagnostic::error(
                codes::RUNTIME_ERROR,
                format!("could not read `{}`: {e}", path.display()),
            )
        }),
        None => std::io::read_to_string(std::io::stdin()).map_err(|e| {
            Diagnostic::error(codes::RUNTIME_ERROR, format!("could not read stdin: {e}"))
                .note("`--with FILE` names the file holding the new definition")
        }),
    }
}

/// Checks the program with `text` in place of the file, and refuses it unless it checks, defines
/// the same names, and moves no definition's hash but the one replaced.
fn still_the_same_program(
    loaded: &Loaded,
    place: &Place<'_>,
    text: &str,
) -> Result<(), Diagnostic> {
    let refused = |message: String| {
        Diagnostic::error(codes::REPLACEMENT_REFUSED, message)
            .primary(place.span, "the definition to replace")
            .note("nothing was written")
    };
    let backticked = |names: &[&Symbol]| {
        names
            .iter()
            .map(|s| format!("`{s}`"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let (sources, ids) = spliced(loaded, place.file.id, text);
    let front = ply_codegen::c::producer::checked_front(&sources, &ids)
        .map_err(|e| refused(format!("with `{}` replaced, {e:#}", place.name)))?;
    let names = |h: &HashOutput| -> BTreeSet<Symbol> {
        h.defs.keys().chain(h.decls.keys()).cloned().collect()
    };
    let (before, after) = (names(&loaded.hashes), names(&front.hashes));
    if before != after {
        let added: Vec<&Symbol> = after.difference(&before).collect();
        let removed: Vec<&Symbol> = before.difference(&after).collect();
        let mut what = Vec::new();
        if !added.is_empty() {
            what.push(format!("adds {}", backticked(&added)));
        }
        if !removed.is_empty() {
            what.push(format!("removes {}", backticked(&removed)));
        }
        return Err(refused(format!(
            "the replacement of `{}` {}",
            place.name,
            what.join(" and ")
        )));
    }
    let mut moved: Vec<&Symbol> = loaded
        .hashes
        .own
        .iter()
        .filter(|(name, hash)| *name != &place.name && front.hashes.own.get(*name) != Some(*hash))
        .map(|(name, _)| name)
        .collect();
    // A `type` or `effect` hashes over what it references, so replacing one may move another's
    // hash; nothing references a `fn` by hash, so replacing a `fn` may move no declaration.
    if loaded.hashes.defs.contains_key(&place.name) {
        moved.extend(
            loaded
                .hashes
                .decls
                .iter()
                .filter(|(name, hash)| front.hashes.decls.get(*name) != Some(*hash))
                .map(|(name, _)| name),
        );
    }
    if !moved.is_empty() {
        return Err(refused(format!(
            "the replacement of `{}` also changes {}",
            place.name,
            backticked(&moved)
        )));
    }
    Ok(())
}

/// The program with one file's text replaced, as the front end reads it.
fn spliced(loaded: &Loaded, file: SourceId, text: &str) -> (Vec<(String, String)>, Vec<SourceId>) {
    let mut modules: Vec<&ply_ty::ModuleInfo> = loaded.check.modules.values().collect();
    modules.sort_by_key(|m| m.source.0);
    let sources: Vec<(String, String)> = modules
        .iter()
        .map(|m| {
            let text = if m.source == file {
                text.to_string()
            } else {
                loaded
                    .sources
                    .get(m.source)
                    .map_or("", |f| &*f.text)
                    .to_string()
            };
            (m.name.to_string(), text)
        })
        .collect();
    let ids = (0..sources.len()).map(|i| SourceId(i as u32)).collect();
    (sources, ids)
}

fn report(
    loaded: &Loaded,
    args: &ReplaceArgs,
    place: Option<&Place<'_>>,
    changed: bool,
    diagnostics: &[Diagnostic],
    style: Style,
) -> i32 {
    let exit_code = if diagnostics.is_empty() {
        EXIT_OK
    } else {
        EXIT_COMPILE_ERROR
    };
    let file = place.map(|p| p.file.path.display().to_string());
    if args.json {
        emit_json(&json!({
            "command": "replace",
            "schema_version": SCHEMA_VERSION,
            "ok": exit_code == EXIT_OK,
            "exit_code": exit_code,
            "name": place.map(|p| p.name.as_str()),
            "file": file,
            "changed": changed,
            "diagnostics": diagnostics_json(diagnostics, &loaded.sources),
        }));
        return exit_code;
    }
    if !diagnostics.is_empty() {
        print_diagnostics(diagnostics, &loaded.sources, style);
        return exit_code;
    }
    let (Some(place), Some(file)) = (place, file) else {
        return exit_code;
    };
    if !changed {
        println!("{} in {file} is unchanged", place.name);
    } else if args.check {
        println!("would replace {file}");
    } else {
        println!("replaced {} in {file}", place.name);
    }
    exit_code
}
