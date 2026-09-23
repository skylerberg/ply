//! `ply cache` — the runner for the `cache` command of the program in `crates/ply-cli/ply`.

use super::shipped_program::{color, run};
use crate::artifact::Binds;
use crate::cli::CacheAction;
use crate::style::Style;
use std::path::Path;

pub fn execute(action: &CacheAction, style: Style) -> i32 {
    // The store is opened, worked and flushed here, behind the effect the program performs; the
    // program is lent what the action did and reaches no tree of its own.
    let binds = Binds {
        lent: crate::cache::lent(&cache_options(action)),
        ..Binds::default()
    };
    run(
        "cache",
        argv(action, style),
        Path::new("."),
        binds,
        json(action),
        style,
    )
}

/// What `process.args` answers: the command word, then the flags the program reads.
fn argv(action: &CacheAction, style: Style) -> Vec<String> {
    let mut argv = vec![
        "cache".to_string(),
        color(style),
        format!("--action={}", word(action)),
    ];
    if json(action) {
        argv.push("--json".to_string());
    }
    if let CacheAction::Inspect(args) = action {
        argv.push(format!("--query={}", args.query));
    }
    argv
}

fn word(action: &CacheAction) -> &'static str {
    match action {
        CacheAction::Clear(_) => "clear",
        CacheAction::Stats(_) => "stats",
        CacheAction::Compact(_) => "compact",
        CacheAction::Inspect(_) => "inspect",
    }
}

fn json(action: &CacheAction) -> bool {
    match action {
        CacheAction::Clear(scope) | CacheAction::Stats(scope) | CacheAction::Compact(scope) => {
            scope.json
        }
        CacheAction::Inspect(args) => args.json,
    }
}

/// The parsed action as the runtime's plain options.
fn cache_options(action: &CacheAction) -> ply_machine::cache::CacheAction {
    match action {
        CacheAction::Clear(scope) => ply_machine::cache::CacheAction::Clear(scope_of(scope)),
        CacheAction::Stats(scope) => ply_machine::cache::CacheAction::Stats(scope_of(scope)),
        CacheAction::Compact(scope) => ply_machine::cache::CacheAction::Compact(scope_of(scope)),
        CacheAction::Inspect(args) => {
            ply_machine::cache::CacheAction::Inspect(ply_machine::cache::InspectOptions {
                query: args.query.clone(),
                path: args.path.clone(),
                json: args.json,
            })
        }
    }
}

fn scope_of(scope: &crate::cli::CacheScope) -> ply_machine::cache::CacheScope {
    ply_machine::cache::CacheScope {
        path: scope.path.clone(),
        json: scope.json,
    }
}
