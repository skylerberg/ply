//! `ply explain` — what a diagnostic code means, from the registry the compiler raises from.

use super::common::emit_json;
use crate::cli::ExplainArgs;
use crate::style::Style;
use crate::{EXIT_COMPILE_ERROR, EXIT_OK};
use serde_json::json;

pub const SCHEMA_VERSION: u32 = 1;

pub fn execute(args: &ExplainArgs, style: Style) -> i32 {
    if args.all {
        if args.json {
            emit_json(&json!({
                "command": "explain",
                "schema_version": SCHEMA_VERSION,
                "ok": true,
                "exit_code": EXIT_OK,
                "codes": ply_span::MEANINGS.iter().map(|(code, meaning)| json!({
                    "code": code, "severity": severity(code), "meaning": meaning,
                })).collect::<Vec<_>>(),
            }));
        } else {
            for (code, meaning) in ply_span::MEANINGS {
                println!("{} {meaning}", style.bold(code));
            }
        }
        return EXIT_OK;
    }
    let code = args
        .code
        .as_deref()
        .unwrap_or_default()
        .to_ascii_uppercase();
    match ply_span::meaning(&code) {
        Some(meaning) => {
            if args.json {
                emit_json(&json!({
                    "command": "explain",
                    "schema_version": SCHEMA_VERSION,
                    "ok": true,
                    "exit_code": EXIT_OK,
                    "code": code,
                    "severity": severity(&code),
                    "meaning": meaning,
                }));
            } else {
                println!("{} {meaning}", style.bold(&code));
            }
            EXIT_OK
        }
        None => {
            let message = format!(
                "`{code}` is not a code this compiler raises; `ply explain --all` lists them"
            );
            if args.json {
                emit_json(&json!({
                    "command": "explain",
                    "schema_version": SCHEMA_VERSION,
                    "ok": false,
                    "exit_code": EXIT_COMPILE_ERROR,
                    "code": code,
                    "error": message,
                }));
            } else {
                eprintln!("{}: {message}", style.red("error"));
            }
            EXIT_COMPILE_ERROR
        }
    }
}

fn severity(code: &str) -> &'static str {
    if code.starts_with('W') {
        "warning"
    } else {
        "error"
    }
}
