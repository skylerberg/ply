//! `ply hosts` — the trusted computing base, enumerable in one command.
//!
//! This is the listing ADR 0008 calls "the change most worth a human's
//! attention in the entire system", so two properties decide the whole
//! implementation: it is ordered deterministically, and it never abbreviates. A
//! handler that claims every table prints one line per table it got, and the
//! digest at the foot covers every column of every row — including the flags,
//! because a handler that quietly became repeatable is exactly the change a
//! diff has to show.

use super::common::{
    IND, diagnostics_json, emit_json, print_diagnostics, report_bind_error, report_load_error,
};
use crate::cli::HostsArgs;
use crate::hosts;
use crate::load::load;
use crate::style::Style;
use crate::{EXIT_COMPILE_ERROR, EXIT_OK};
use ply_host::tls;
use serde_json::{Value, json};

/// The `--json` object's shape. Independent of `ply test`'s, because a consumer
/// pinning a TCB is not the consumer reading a run.
pub const SCHEMA_VERSION: u32 = 1;

pub fn execute(args: &HostsArgs, style: Style) -> i32 {
    let loaded = match load(&args.path) {
        Ok(loaded) => loaded,
        Err(err) => return report_load_error("hosts", &err, args.json, style),
    };

    // Resolved whether or not `--host` was passed: the TCB is a property of the
    // registry and the program, and a digest that moved when a flag moved would
    // pin nothing.
    let listing = match hosts::Hosts::preview(&loaded.check) {
        Ok(listing) => listing,
        Err(diagnostics) => {
            if args.json {
                emit_json(&json!({
                    "command": "hosts",
                    "schema_version": SCHEMA_VERSION,
                    "ok": false,
                    "exit_code": EXIT_COMPILE_ERROR,
                    "root": loaded.root.display().to_string(),
                    "diagnostics": diagnostics_json(&diagnostics, &loaded.sources),
                }));
            } else {
                print_diagnostics(&diagnostics, &loaded.sources, style);
            }
            return EXIT_COMPILE_ERROR;
        }
    };

    // Loaded here rather than only under `--host`, because `ply hosts` is the
    // command that answers "what does this run trust" and a credential that
    // will not load is a run that will not start. `E0430` before anything else
    // is printed, so a listing is never produced over material that is broken.
    let credentials = match tls::Credentials::load(&args.tls.tls) {
        Ok(credentials) => credentials,
        Err(diagnostics) => {
            return report_bind_error("hosts", &diagnostics, &loaded.sources, args.json, style);
        }
    };
    // Resolved here for the same reason the credentials are: `ply hosts` is the
    // command that answers "what does this run trust", and a connection string
    // that will not parse is a run that will not start.
    let db = match args.db.resolve(args.host) {
        Ok(db) => db,
        Err(diagnostics) => {
            return report_bind_error("hosts", &diagnostics, &loaded.sources, args.json, style);
        }
    };
    let schema = match schema_view(&loaded, db.as_ref()) {
        Ok(schema) => schema,
        Err(diagnostic) => {
            return report_bind_error(
                "hosts",
                std::slice::from_ref(&diagnostic),
                &loaded.sources,
                args.json,
                style,
            );
        }
    };
    let disclosures = hosts::Disclosures::of(&listing, Some(&credentials), db, schema);

    if args.digest {
        println!("{}", hosts::digest_short(&listing, &disclosures));
        return EXIT_OK;
    }

    if args.json {
        let mut report = json!({
            "command": "hosts",
            "schema_version": SCHEMA_VERSION,
            "ok": true,
            "exit_code": EXIT_OK,
            "root": loaded.root.display().to_string(),
            "binding": if args.host { "host" } else { "hermetic" },
            "handlers": listing.handlers,
            "operations": listing.rows.len(),
            "digest": hosts::digest_short(&listing, &disclosures),
            // Present in both bindings. `binding` says what this run would use;
            // the rows say what exists, and an agent should not have to invoke
            // the command twice to learn both.
            "hosts": hosts::rows_json(&listing),
            "diagnostics": Value::Array(Vec::new()),
        });
        if let Some(transport) = &disclosures.transport {
            report["transport"] = transport.json();
        }
        if let Some(database) = &disclosures.database {
            report["database"] = database.json();
        }
        emit_json(&report);
        return EXIT_OK;
    }

    println!();
    let lines = if args.host {
        hosts::listing_lines(&listing, &disclosures)
    } else {
        hosts::hermetic_lines(&listing)
    };
    for line in lines {
        if line.is_empty() {
            println!();
        } else {
            println!("{IND}{line}");
        }
    }
    EXIT_OK
}

/// The `--db-schema` function, resolved and materialised.
///
/// `ply hosts` is the one command whose whole output is this block, so it pays
/// for the evaluation that fills in the counts. A name that resolves to nothing
/// is `E0431` here rather than at the first statement.
fn schema_view(
    loaded: &crate::load::Loaded,
    db: Option<&crate::db::DbConfig>,
) -> Result<Option<crate::db::schema::SchemaView>, ply_span::Diagnostic> {
    let Some(name) = db.and_then(|c| c.schema.as_deref()) else {
        return Ok(None);
    };
    let resolved = crate::db::schema::resolve(&loaded.check, name)?;
    let name = resolved.as_str().to_string();
    let shape = super::common::materialise_schema(loaded, &name);
    Ok(Some(crate::db::schema::SchemaView {
        name,
        shape,
        state: crate::db::schema::State::Declared,
    }))
}

#[cfg(test)]
mod tests {
    use crate::cli::{Cli, Command};
    use clap::Parser;
    use std::path::PathBuf;

    #[test]
    fn the_flags_parse_and_default_to_hermetic() {
        let args = match Cli::parse_from(["ply", "hosts"]).command {
            Command::Hosts(args) => args,
            other => panic!("expected `hosts`, got {other:?}"),
        };
        assert!(!args.host, "hermetic is the default");
        assert!(!args.json);
        assert!(!args.digest);
        assert_eq!(args.path, PathBuf::from("."));

        let args = match Cli::parse_from(["ply", "hosts", "src", "--host", "--digest"]).command {
            Command::Hosts(args) => args,
            other => panic!("expected `hosts`, got {other:?}"),
        };
        assert!(args.host);
        assert!(args.digest);
        assert_eq!(args.path, PathBuf::from("src"));
    }

    /// `--digest` is the one-line form a CI check pins, so it may not also carry
    /// a table for a human.
    #[test]
    fn digest_and_json_cannot_both_be_asked_for() {
        assert!(Cli::try_parse_from(["ply", "hosts", "--digest", "--json"]).is_err());
    }
}
