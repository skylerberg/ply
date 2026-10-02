//! Whether a `reuse fn` keeps its promise, before the program runs. `ply check --costs` reports
//! the same pass, in the program that answers it.

use crate::load::Loaded;
use ply_eval::decode::{self, At};
use ply_eval::{Diagnostic, SourceId, Span, Value, codes};
use std::collections::HashMap;

const ENTRY: &str = "costs.costs";

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
    let embeds = ply_codegen::c::producer::embeds_of(&loaded.front)
        .map_err(|e| failed(&format!("{e:#}")))?;
    let answer = ply_codegen::c::producer::call(
        ENTRY,
        &[
            Value::list(names),
            Value::list(texts),
            packages,
            mod_pkg,
            shelf,
            embeds,
        ],
    )
    .map_err(|e| failed(&format!("{e:#}")))?;
    let what = format!("`{ENTRY}`'s answer");
    read(At::new(&what, &answer), &sources).map_err(|e| failed(&e.to_string()))
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

/// A `costs.Report`; its offsets are into the module each definition names.
fn read(answer: At<'_>, sources: &HashMap<String, SourceId>) -> Result<Report, decode::Error> {
    if !answer.field("ok")?.bool()? {
        return Err(answer.error("the program did not resolve in the port"));
    }
    let span = |id: SourceId, at: At<'_>| -> Result<Span, decode::Error> {
        Ok(Span::new(
            id,
            at.field("start")?.number()?,
            at.field("end")?.number()?,
        ))
    };
    let defs = answer.field("defs")?.items(|def| {
        let module = def.field("module_name")?;
        let id = *sources
            .get(module.utf8()?)
            .ok_or_else(|| module.error("a definition in a module that was not asked"))?;
        Ok(Definition {
            name: def.field("name")?.utf8()?.to_string(),
            promise: match def.field("reuse")?.option()? {
                Some(reuse) => Some(span(id, reuse)?),
                None => None,
            },
            sites: def.field("sites")?.items(|site| {
                let verdict = site.field("verdict")?;
                let fix = site.field("fix")?.utf8()?;
                let param = site.field("param")?;
                Ok(Site {
                    span: span(id, site)?,
                    verdict: match verdict.utf8()? {
                        "reuses" => Verdict::Reuses,
                        "copies" => Verdict::Copies,
                        "unknown" => Verdict::Unknown,
                        other => return Err(verdict.error(format!("`{other}` is no verdict"))),
                    },
                    reason: site.field("reason")?.utf8()?.to_string(),
                    fix: (!fix.is_empty()).then(|| fix.to_string()),
                    param: if param.int()? < 0 {
                        None
                    } else {
                        Some(param.number()?)
                    },
                })
            })?,
        })
    })?;
    Ok(Report { defs })
}
