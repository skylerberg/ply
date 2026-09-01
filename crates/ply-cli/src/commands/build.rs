//! `ply build` — write a deployable artifact, or say what is in one.

use super::common::{
    IND, diagnostic_json, diagnostics_json, emit_json, plural, print_diagnostics, print_warnings,
    report_load_error,
};
use crate::artifact::{self, Built};
use crate::cli::BuildArgs;
use crate::load::{Loaded, load};
use crate::style::Style;
use crate::{EXIT_COMPILE_ERROR, EXIT_OK};
use ply_core::DefInfo;
use ply_span::{Diagnostic, SourceMap, Span, Symbol, codes};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub fn execute(args: &BuildArgs, style: Style) -> i32 {
    // The full path, never the incremental one.
    let loaded = match load(&args.path) {
        Ok(loaded) => loaded,
        Err(err) => return report_load_error("build", &err, args.json, style),
    };

    let entry = match entry_point(&loaded, args.entry.as_deref()) {
        Ok(entry) => entry,
        Err(diagnostic) => return refuse(&loaded.sources, diagnostic, args.json, style),
    };
    // The start-up definitions the deployed artifact has to be able to name.
    let mut startup: Vec<&DefInfo> = Vec::new();
    for (flag, named) in [
        ("--config-schema", args.config_schema.as_deref()),
        ("--db-schema", args.db_schema.as_deref()),
    ] {
        let Some(named) = named else { continue };
        match schema_root(&loaded, flag, named) {
            Ok(def) => startup.push(def),
            Err(diagnostic) => return refuse(&loaded.sources, diagnostic, args.json, style),
        }
    }
    let built = match artifact::build(&loaded, entry, &startup, args.sources) {
        Ok(built) => built,
        Err(diagnostics) => {
            return refuse_all(&loaded.sources, &diagnostics, args.json, style);
        }
    };
    let bytes = built.artifact.encode();

    if let Some(old) = &args.diff {
        return report_diff(args, &built, old, &bytes, style);
    }
    if args.digest {
        println!("{}", built.artifact.digest_short());
        return EXIT_OK;
    }

    let out = args
        .output
        .clone()
        .unwrap_or_else(|| default_output(&built.entry_name));
    if let Some(parent) = out.parent().filter(|p| !p.as_os_str().is_empty())
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        return refuse(&loaded.sources, unwritable(&out, &e), args.json, style);
    }
    if let Err(e) = std::fs::write(&out, &bytes) {
        return refuse(&loaded.sources, unwritable(&out, &e), args.json, style);
    }

    if args.json {
        emit_json(&json!({
            "command": "build",
            "ok": true,
            "exit_code": EXIT_OK,
            "root": loaded.root.display().to_string(),
            "entry": built.entry_name.to_string(),
            "entry_hash": built.artifact.entry.to_hex(),
            "startup": built
                .startup
                .iter()
                .map(|n| n.to_string())
                .collect::<Vec<String>>(),
            "artifact": out.display().to_string(),
            "digest": built.artifact.digest_short(),
            "format": artifact::ARTIFACT_FORMAT,
            "definitions": built.artifact.bodies.len(),
            "names": built.artifact.names.len(),
            "sources": built.artifact.has_sources(),
            "artifact_bytes": bytes.len(),
            "binary_bytes": binary_bytes(),
            "diagnostics": Value::Array(Vec::new()),
        }));
        return EXIT_OK;
    }

    println!(
        "{IND}{} {} · {}",
        style.green("built"),
        built.entry_name,
        built.artifact.digest_short()
    );
    println!(
        "{IND}{} {} {} · {} · {}",
        style.bold("artifact"),
        built.artifact.bodies.len(),
        plural(built.artifact.bodies.len(), "definition"),
        human(bytes.len() as u64),
        out.display(),
    );
    // The ratio §5.1 decided against incremental transfer on.
    match binary_bytes() {
        Some(n) => println!(
            "{IND}{} ply {} · {}",
            style.bold("binary"),
            env!("CARGO_PKG_VERSION"),
            human(n)
        ),
        None => println!(
            "{IND}{} ply {} · size unavailable",
            style.bold("binary"),
            env!("CARGO_PKG_VERSION")
        ),
    }
    // Printed whether or not any were named, because the absence of one is the deploy failure: an
    // artifact with no schema is an artifact that cannot refuse to start on a missing credential.
    match built.startup.as_slice() {
        [] => println!(
            "{IND}{} none — this artifact cannot be run with `--config-schema` or `--db-schema`",
            style.bold("startup")
        ),
        roots => println!(
            "{IND}{} {}",
            style.bold("startup"),
            roots
                .iter()
                .map(|n| n.to_string())
                .collect::<Vec<String>>()
                .join(" · ")
        ),
    }
    if built.artifact.has_sources() {
        println!(
            "{IND}{} {} {} embedded",
            style.bold("sources"),
            built.artifact.sources.len(),
            plural(built.artifact.sources.len(), "file"),
        );
    }
    EXIT_OK
}

/// The names a reader is shown for a set that could be hundreds long.
const SHOWN: usize = 8;

fn report_diff(
    args: &BuildArgs,
    built: &Built,
    old_path: &Path,
    bytes: &[u8],
    style: Style,
) -> i32 {
    let sources = SourceMap::new();
    let (old, warnings) = match artifact::read(old_path) {
        Ok(pair) => pair,
        Err(diagnostic) => return refuse(&sources, diagnostic, args.json, style),
    };
    let diff = artifact::diff(&old, built);

    if args.json {
        emit_json(&json!({
            "command": "build",
            "ok": true,
            "exit_code": EXIT_OK,
            "entry": built.entry_name.to_string(),
            "against": old_path.display().to_string(),
            "was": old.digest_short(),
            "now": built.artifact.digest_short(),
            "added": diff.added,
            "changed": diff.changed,
            "dropped": diff.dropped,
            "unchanged": diff.unchanged,
            "reached": diff.reached,
            "artifact_bytes": bytes.len(),
            "binary_bytes": binary_bytes(),
            "diagnostics": diagnostics_json(&warnings, &sources),
        }));
        return EXIT_OK;
    }

    print_warnings(&warnings, style);
    println!(
        "{IND}{} {} → {}",
        style.bold(built.entry_name.as_str()),
        old.digest_short(),
        built.artifact.digest_short()
    );
    println!();
    row(style.green("added"), &diff.added, style);
    row(style.bold("changed"), &diff.changed, style);
    row(style.red("dropped"), &diff.dropped, style);
    println!(
        "{IND}{:<11}{} {}",
        "unchanged",
        diff.unchanged,
        plural(diff.unchanged, "definition")
    );
    if !diff.reached.is_empty() {
        println!();
        println!(
            "{IND}reached by a changed definition: {} {}",
            diff.reached.len(),
            plural(diff.reached.len(), "definition")
        );
        println!("{IND}  {}", names(&diff.reached));
    }
    EXIT_OK
}

fn row(label: String, names_of: &[String], _style: Style) {
    print!(
        "{IND}{label:<11}{} {}",
        names_of.len(),
        plural(names_of.len(), "definition")
    );
    if names_of.is_empty() {
        println!();
    } else {
        println!("   {}", names(names_of));
    }
}

fn names(all: &[String]) -> String {
    let shown: Vec<&str> = all.iter().take(SHOWN).map(String::as_str).collect();
    match all.len().checked_sub(SHOWN) {
        Some(rest) if rest > 0 => format!("{}, and {rest} more", shown.join(", ")),
        _ => shown.join(", "),
    }
}

fn entry_point<'a>(loaded: &'a Loaded, named: Option<&str>) -> Result<&'a DefInfo, Diagnostic> {
    let Some(named) = named else {
        return super::run::entry_point(loaded);
    };
    let matches: Vec<&DefInfo> = loaded
        .check
        .defs
        .values()
        .filter(|d| d.name.as_str() == named || d.simple_name.as_str() == named)
        .filter(|d| !ply_std::is_std(&d.module))
        .collect();
    match matches.as_slice() {
        // An artifact is run by calling its entry point with nothing, so an entry point that takes
        // an argument is one `ply run` could never start.
        [one] if arity(one) > 0 => Err(Diagnostic::error(
            codes::TYPE_MISMATCH,
            format!(
                "`{}` takes {} {}, and a deployed artifact is started with none",
                one.name,
                arity(one),
                plural(arity(one), "argument"),
            ),
        )
        .primary(one.span, "this cannot be an entry point")
        .note("wrap it in a nullary function and name that instead")),
        [one] => Ok(one),
        [] => Err(Diagnostic::error(
            codes::UNKNOWN_NAME,
            format!("no definition named `{named}`"),
        )
        .primary(Span::DUMMY, "there is nothing to build a closure from")
        .note("`--entry` takes a program-wide name (`app.serve`) or a simple one")),
        several => {
            let mut diagnostic = Diagnostic::error(
                codes::AMBIGUOUS_ENTRY_POINT,
                format!("{} definitions are named `{named}`", several.len()),
            );
            for (i, def) in several.iter().enumerate() {
                let message = format!("`{}` declares it here", def.module);
                diagnostic = if i == 0 {
                    diagnostic.primary(def.span, message)
                } else {
                    diagnostic.secondary(def.span, message)
                };
            }
            Err(diagnostic.note("name it in full, as `<module>.<name>`"))
        }
    }
}

/// A `--config-schema` / `--db-schema` function, resolved as a build root.
fn schema_root<'a>(loaded: &'a Loaded, flag: &str, named: &str) -> Result<&'a DefInfo, Diagnostic> {
    let matches: Vec<&DefInfo> = loaded
        .check
        .defs
        .values()
        .filter(|d| d.name.as_str() == named || d.simple_name.as_str() == named)
        .filter(|d| !ply_std::is_std(&d.module))
        .collect();
    match matches.as_slice() {
        [one] if arity(one) > 0 => Err(Diagnostic::error(
            codes::TYPE_MISMATCH,
            format!(
                "`{}` takes {} {}, and `{flag}` names a nullary function",
                one.name,
                arity(one),
                plural(arity(one), "argument"),
            ),
        )
        .primary(one.span, "this cannot be a schema")),
        [one] => Ok(one),
        [] => Err(Diagnostic::error(
            codes::UNKNOWN_NAME,
            format!("`{flag} {named}` names no definition in this program"),
        )
        .primary(Span::DUMMY, "there is nothing to ship")
        .note(format!(
            "`{flag}` takes a program-wide name (`desk.config`) or a simple one"
        ))
        .note(
            "a schema shipped at build time is what lets the deployed artifact be run with the \
             same flag, and therefore what keeps its start-up refusal",
        )),
        several => {
            let mut diagnostic = Diagnostic::error(
                codes::AMBIGUOUS_ENTRY_POINT,
                format!("{} definitions are named `{named}`", several.len()),
            );
            for (i, def) in several.iter().enumerate() {
                let message = format!("`{}` declares it here", def.module);
                diagnostic = if i == 0 {
                    diagnostic.primary(def.span, message)
                } else {
                    diagnostic.secondary(def.span, message)
                };
            }
            Err(diagnostic.note("name it in full, as `<module>.<name>`"))
        }
    }
}

fn arity(def: &DefInfo) -> usize {
    match &def.scheme.ty {
        ply_core::ty::Type::Fn { params, .. } => params.len(),
        _ => 0,
    }
}

/// `desk.run` becomes `desk.plyx`: the module, not the function, because the module is what a
/// deployment thinks it is shipping.
fn default_output(entry: &Symbol) -> PathBuf {
    let text = entry.as_str();
    let module = text.rsplit_once('.').map_or(text, |(m, _)| m);
    let leaf = module.rsplit('.').next().unwrap_or(module);
    PathBuf::from(format!("{leaf}.{}", artifact::EXTENSION))
}

/// `None` when the running binary cannot be located or measured, which is a fact about the platform
/// and never a reason to fail a build.
fn binary_bytes() -> Option<u64> {
    std::env::current_exe()
        .and_then(std::fs::metadata)
        .map(|m| m.len())
        .ok()
}

fn human(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    match bytes {
        n if n >= MB => format!("{:.1} MB", n as f64 / MB as f64),
        n if n >= KB => format!("{:.1} KB", n as f64 / KB as f64),
        n => format!("{n} B"),
    }
}

fn unwritable(path: &Path, e: &std::io::Error) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("could not write `{}`: {e}", path.display()),
    )
    .primary(Span::DUMMY, "the artifact was built but not stored")
}

fn refuse(sources: &SourceMap, diagnostic: Diagnostic, json: bool, style: Style) -> i32 {
    refuse_all(sources, std::slice::from_ref(&diagnostic), json, style)
}

fn refuse_all(sources: &SourceMap, diagnostics: &[Diagnostic], json: bool, style: Style) -> i32 {
    if json {
        emit_json(&json!({
            "command": "build",
            "ok": false,
            "exit_code": EXIT_COMPILE_ERROR,
            "diagnostics": diagnostics.iter().map(|d| diagnostic_json(d, sources)).collect::<Vec<_>>(),
        }));
    } else {
        print_diagnostics(diagnostics, sources, style);
    }
    EXIT_COMPILE_ERROR
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_output_is_named_after_the_entry_points_module() {
        assert_eq!(
            default_output(&Symbol::new("desk.run")),
            PathBuf::from("desk.plyx")
        );
        assert_eq!(
            default_output(&Symbol::new("store.orders.main")),
            PathBuf::from("orders.plyx")
        );
    }

    #[test]
    fn a_long_name_list_is_counted_rather_than_printed_whole() {
        let all: Vec<String> = (0..20).map(|i| format!("m.d{i}")).collect();
        let line = names(&all);
        assert!(line.ends_with("and 12 more"), "{line}");
        assert_eq!(names(&all[..3]), "m.d0, m.d1, m.d2");
    }

    #[test]
    fn sizes_read_as_sizes() {
        assert_eq!(human(512), "512 B");
        assert_eq!(human(2048), "2.0 KB");
        assert_eq!(human(3 * 1024 * 1024), "3.0 MB");
    }
}
