//! Whether a `reuse fn` keeps its promise, before the program runs. `ply check --costs` reports
//! the same pass, in the program that answers it.

use crate::load::Loaded;
use ply_eval::Value;
use ply_span::{Diagnostic, SourceId, Span, codes};
use std::collections::HashMap;

const ENTRY: &str = "costs.costs_dump";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Reuses,
    Copies,
    Unknown,
}

struct Site {
    span: Span,
    verdict: Verdict,
    reason: String,
    fix: Option<String>,
    /// `Some(k)` when the list is parameter `k` at its last use.
    param: Option<usize>,
}

struct Definition {
    name: String,
    promise: Option<Span>,
    sites: Vec<Site>,
}

/// Only definitions with an append.
struct Report {
    defs: Vec<Definition>,
}

/// A `reuse fn` whose promise the cost checker cannot show stops the run, as under `ply check`.
pub fn broken_promises(loaded: &Loaded) -> Option<crate::load::LoadError> {
    if !loaded.promised {
        return None;
    }
    let diagnostics = promises(loaded);
    (!diagnostics.is_empty()).then(|| crate::load::LoadError {
        sources: loaded.sources.clone(),
        diagnostics,
    })
}

/// Each `reuse fn`'s appends must reuse, except onto its own parameter; else E0127.
pub fn promises(loaded: &Loaded) -> Vec<Diagnostic> {
    if !loaded.promised {
        return Vec::new();
    }
    let report = match report(loaded) {
        Ok(report) => report,
        Err(failed) => return vec![failed],
    };
    let mut out = Vec::new();
    for def in &report.defs {
        let Some(promise) = def.promise else {
            continue;
        };
        for site in &def.sites {
            if site.verdict == Verdict::Reuses || site.param.is_some() {
                continue;
            }
            let what = match site.verdict {
                Verdict::Copies => "copies its list",
                _ => "cannot be shown to reuse its list",
            };
            let d = Diagnostic::error(
                codes::REUSE_BROKEN,
                format!(
                    "`{}` is a `reuse fn`, and this append {what}: {}",
                    def.name, site.reason
                ),
            )
            .primary(site.span, "this append")
            .secondary(promise, "the promise");
            out.push(match &site.fix {
                Some(fix) => d.note(format!("fix: {fix}")),
                None => d.note(
                    "no edit inside this body removes the copy; the promise cannot be kept as \
                     written, so either restructure the append or drop `reuse`",
                ),
            });
        }
    }
    out
}

fn report(loaded: &Loaded) -> Result<Report, Diagnostic> {
    let mut sources = HashMap::new();
    let mut names = Vec::new();
    let mut texts = Vec::new();
    for module in loaded.check.modules.values() {
        let Some(file) = loaded.sources.get(module.source) else {
            return Err(failed(&format!(
                "no source text for module `{}`",
                module.name
            )));
        };
        sources.insert(module.name.as_str().to_string(), module.source);
        names.push(Value::bytes(module.name.as_str().as_bytes()));
        texts.push(Value::bytes(file.text.as_bytes()));
    }
    let (packages, mod_pkg, shelf) =
        ply_codegen::c::producer::package_tables(&loaded.front.packages, &loaded.front.mod_pkg);
    let answer = ply_codegen::c::producer::call(
        ENTRY,
        &[
            Value::list(names),
            Value::list(texts),
            packages,
            mod_pkg,
            shelf,
        ],
    )
    .map_err(|e| failed(&format!("{e:#}")))?;
    let Value::Str(dump) = &answer else {
        return Err(failed(&format!(
            "`{ENTRY}` answered a {} rather than a string",
            answer.type_name()
        )));
    };
    read(dump, &sources).map_err(|e| failed(&e))
}

fn failed(why: &str) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("the compiler could not say where this program's appends copy: {why}"),
    )
    .primary(
        Span::DUMMY,
        "no append was costed, so no promise is claimed kept",
    )
    .note("this is Ply's fault: the compiler's own `costs.ply` is what failed here")
}

fn read(dump: &str, sources: &HashMap<String, SourceId>) -> Result<Report, String> {
    let (head, mut rest) = dump
        .split_once('\n')
        .ok_or("an empty answer: the program did not resolve in the port")?;
    if !head.starts_with("rounds ") {
        return Err(format!(
            "an answer that opens with {head:?}, not `rounds <n>`"
        ));
    }
    let mut defs: Vec<Definition> = Vec::new();
    let mut source = None;
    while !rest.is_empty() {
        let (header, after) = rest
            .split_once('\n')
            .ok_or("an unterminated frame header")?;
        let fields: Vec<&str> = header.split(' ').collect();
        rest = match fields.as_slice() {
            ["def", _kind, start, end, module, name, label] => {
                let (module, after) = take(after, module)?;
                let (name, after) = take(after, name)?;
                let (_label, after) = take(after, label)?;
                let id = *sources
                    .get(module)
                    .ok_or_else(|| format!("a definition in `{module}`, which was not asked"))?;
                source = Some(id);
                let promise = match (*start, *end) {
                    ("-", "-") => None,
                    _ => Some(Span::new(id, number(start)?, number(end)?)),
                };
                defs.push(Definition {
                    name: name.to_string(),
                    promise,
                    sites: Vec::new(),
                });
                after
            }
            ["site", start, end, verdict, param, reason, fix] => {
                let (reason, after) = take(after, reason)?;
                let (fix, after) = take(after, fix)?;
                let (Some(id), Some(def)) = (source, defs.last_mut()) else {
                    return Err("a site before any definition".to_string());
                };
                def.sites.push(Site {
                    span: Span::new(id, number(start)?, number(end)?),
                    verdict: match *verdict {
                        "reuses" => Verdict::Reuses,
                        "copies" => Verdict::Copies,
                        "unknown" => Verdict::Unknown,
                        other => {
                            return Err(format!("a verdict this reader does not know: {other:?}"));
                        }
                    },
                    reason: reason.to_string(),
                    fix: (!fix.is_empty()).then(|| fix.to_string()),
                    param: match *param {
                        "-" => None,
                        k => Some(k.parse().map_err(|_| format!("a parameter {k:?}"))?),
                    },
                });
                after
            }
            _ => {
                return Err(format!(
                    "a frame header this reader does not know: {header:?}"
                ));
            }
        };
    }
    Ok(Report { defs })
}

fn take<'a>(text: &'a str, n: &str) -> Result<(&'a str, &'a str), String> {
    let n: usize = n.parse().map_err(|_| format!("a length {n:?}"))?;
    match (text.get(..n), text.get(n..)) {
        (Some(taken), Some(rest)) => Ok((taken, rest)),
        _ => Err(format!("a text of {n} bytes past the end of the answer")),
    }
}

fn number(text: &str) -> Result<u32, String> {
    text.parse().map_err(|_| format!("an offset {text:?}"))
}
