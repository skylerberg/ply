//! `ply doc` — one definition or builtin as an agent reads it: the signature with the parameter
//! names the source wrote, the comment above it, its place, its hash and what it touches.

use super::common::{IND, emit_json, location, report_diagnostics, report_load_error};
use crate::cli::DocArgs;
use crate::load::{Loaded, load};
use crate::style::Style;
use crate::{EXIT_COMPILE_ERROR, EXIT_OK};
use ply_codegen::c::producer::BuiltinInfo;
use ply_span::Symbol;
use ply_ty::{Printer, Scheme, Type, Visibility};
use serde_json::{Value, json};

pub const SCHEMA_VERSION: u32 = 1;

struct Page {
    kind: &'static str,
    name: String,
    signature: String,
    ty: String,
    params: Vec<(String, String)>,
    doc: Vec<String>,
    note: String,
    module: Option<String>,
    location: Option<String>,
    hash: Option<String>,
    footprint: Option<String>,
}

pub fn execute(args: &DocArgs, style: Style) -> i32 {
    let loaded = load(&args.path);
    let resolved = loaded
        .as_ref()
        .ok()
        .map(|l| super::callers::resolve(l, &args.query));
    let page = match resolved {
        Some(Ok(name)) => definition(loaded.as_ref().expect("resolved against it"), &name),
        _ => match builtin(&args.query) {
            Ok(Some(page)) => page,
            Ok(None) => {
                return match (loaded, resolved) {
                    (Err(err), _) => report_load_error("doc", &err, args.json, style),
                    (Ok(l), Some(Err(unknown))) => {
                        let unknown = unknown
                            .note("a builtin is named by its bare name, as in `ply doc map`");
                        report_diagnostics("doc", &[unknown], &l, args.json, style)
                    }
                    (Ok(_), _) => unreachable!("a loaded program resolves or refuses"),
                };
            }
            Err(e) => {
                eprintln!("{}: {e}", style.red("error"));
                return EXIT_COMPILE_ERROR;
            }
        },
    };
    if args.json {
        emit_json(&page_json(&page));
        return EXIT_OK;
    }
    println!();
    println!("{IND}{}", style.bold(&page.signature));
    for line in &page.doc {
        println!("{IND}{IND}{line}");
    }
    if !page.note.is_empty() {
        println!("{IND}{IND}{}", page.note);
    }
    println!();
    if let Some(place) = &page.location {
        println!("{IND}{} {}", style.dim("at"), place);
    }
    if let Some(footprint) = &page.footprint {
        println!("{IND}{} {footprint}", style.dim("touches"));
    }
    if let Some(hash) = &page.hash {
        println!("{IND}{} {hash}", style.dim("hash"));
    }
    if page.location.is_none() {
        println!("{IND}{} {}", style.dim("kind"), page.kind);
    }
    println!();
    EXIT_OK
}

fn page_json(page: &Page) -> Value {
    json!({
        "command": "doc",
        "schema_version": SCHEMA_VERSION,
        "ok": true,
        "exit_code": EXIT_OK,
        "kind": page.kind,
        "name": page.name,
        "signature": page.signature,
        "type": page.ty,
        "params": page.params.iter().map(|(name, ty)| json!({ "name": name, "type": ty })).collect::<Vec<_>>(),
        "doc": page.doc.join("\n"),
        "note": page.note,
        "module": page.module,
        "location": page.location,
        "hash": page.hash,
        "footprint": page.footprint,
    })
}

fn definition(loaded: &Loaded, name: &Symbol) -> Page {
    let d = &loaded.check.defs[name];
    let written = loaded.front.defs_written.get(name);
    let names: Vec<String> = written
        .map(|w| w.params.iter().map(|p| p.name.to_string()).collect())
        .unwrap_or_default();
    let prefix = format!(
        "{}{}fn ",
        match written.map(|w| w.vis) {
            Some(Visibility::Public) => "pub ",
            _ => "",
        },
        if written.is_some_and(|w| w.reuse) {
            "reuse "
        } else {
            ""
        }
    );
    let (signature, params) = signature(&prefix, d.simple_name.as_str(), &d.scheme, &names);
    Page {
        kind: "definition",
        name: d.name.to_string(),
        signature,
        ty: ply_ty::print_scheme(&d.scheme),
        params,
        doc: comment_above(loaded, d.span),
        note: String::new(),
        module: Some(d.module.to_string()),
        location: location(&loaded.sources, d.span),
        hash: loaded.hashes.defs.get(name).map(|h| h.to_hex()),
        footprint: Some(d.footprint.to_string()),
    }
}

fn builtin(query: &str) -> Result<Option<Page>, String> {
    let Some(b) = ply_codegen::c::producer::builtins()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|b: &BuiltinInfo| b.name.as_str() == query)
    else {
        return Ok(None);
    };
    let (signature, params) = signature("", b.name.as_str(), &b.scheme, &b.params);
    Ok(Some(Page {
        kind: "builtin",
        name: b.name.to_string(),
        signature,
        ty: ply_ty::print_scheme(&b.scheme),
        params,
        doc: Vec::new(),
        note: b.note,
        module: None,
        location: None,
        hash: None,
        footprint: None,
    }))
}

/// `prefix name<generics>(p: T, ...) -> R / row`, with the parameters named as the source did.
fn signature(
    prefix: &str,
    name: &str,
    scheme: &Scheme,
    names: &[String],
) -> (String, Vec<(String, String)>) {
    let mut printer = Printer::new();
    let full = printer.scheme(scheme);
    let Type::Fn {
        params,
        ret,
        effects,
    } = &scheme.ty
    else {
        return (format!("{prefix}{name}: {full}"), Vec::new());
    };
    let generics = full.find('(').map(|i| &full[..i]).unwrap_or("");
    let pairs: Vec<(String, String)> = params
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let name = names.get(i).cloned().unwrap_or_else(|| format!("_{i}"));
            (name, printer.ty(t))
        })
        .collect();
    let args: Vec<String> = pairs.iter().map(|(n, t)| format!("{n}: {t}")).collect();
    let row = printer.row(effects);
    let effects = if row == "{}" {
        String::new()
    } else {
        format!(" / {row}")
    };
    let ret = printer.ty(ret);
    (
        format!(
            "{prefix}{name}{generics}({}) -> {ret}{effects}",
            args.join(", ")
        ),
        pairs,
    )
}

/// The `//` lines directly above the definition, stopping at a blank or non-comment line.
fn comment_above(loaded: &Loaded, span: ply_span::Span) -> Vec<String> {
    let Some(file) = loaded.sources.containing(span) else {
        return Vec::new();
    };
    // The span starts at `fn`, after any `pub`, so the walk starts from that line's beginning.
    let head = &file.text[..span.start as usize];
    let before = &head[..head.rfind('\n').map_or(0, |i| i + 1)];
    let mut lines: Vec<String> = Vec::new();
    for line in before.lines().rev() {
        let Some(text) = line.trim().strip_prefix("//") else {
            break;
        };
        lines.push(text.strip_prefix(' ').unwrap_or(text).to_string());
    }
    lines.reverse();
    lines
}
