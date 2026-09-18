//! `ply hosts` — the trusted computing base, enumerable in one command.

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
use std::sync::Arc;

pub const SCHEMA_VERSION: u32 = 1;

pub fn execute(args: &HostsArgs, style: Style) -> i32 {
    let loaded = match load(&args.path) {
        Ok(loaded) => loaded,
        Err(err) => return report_load_error("hosts", &err, args.json, style),
    };

    // Whether or not `--host` was passed: a digest that moved with a flag would pin nothing.
    let trace = args.trace.open();
    let shutdown = ply_host::signal::Shutdown::new(args.shutdown.bounds());
    let listing = match hosts::Hosts::preview(&loaded.check, Some(Arc::clone(&trace))) {
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

    // Loaded even without `--host`: this command answers what a run trusts, and whether it starts.
    let credentials = match tls::Credentials::load(&args.tls.tls) {
        Ok(credentials) => credentials,
        Err(diagnostics) => {
            return report_bind_error("hosts", &diagnostics, &loaded.sources, args.json, style);
        }
    };
    // Likewise, so an unresolvable root is `E0454` before the listing overstates what is reached.
    let roots = match ply_host::fs::Roots::load(&args.fs.fs, ply_span::Span::DUMMY) {
        Ok(roots) => roots,
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
    let db = match args.db.resolve(args.host) {
        Ok(db) => db,
        Err(diagnostics) => {
            return report_bind_error("hosts", &diagnostics, &loaded.sources, args.json, style);
        }
    };
    // Built only for a schema: this command runs nothing else.
    let constant = |name: &str| {
        let backend = super::common::prover_backend(None, &loaded)?;
        super::common::enter_constant(backend.map(|(provider, _)| provider), name)
    };
    let schema = match schema_view(&loaded.check, db.as_ref(), &constant) {
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
    let (configuration, config_warnings) =
        match crate::config::Configuration::open(&loaded.check, args.host, &args.config, &constant)
        {
            Ok(resolved) => resolved,
            Err(diagnostics) => {
                return report_bind_error("hosts", &diagnostics, &loaded.sources, args.json, style);
            }
        };
    let disclosures = hosts::Disclosures::of(
        &listing,
        Some(&credentials),
        Some(&roots),
        db,
        schema,
        Some(configuration),
        Some(&trace),
        args.trace.level_name(),
        Some(&shutdown),
    );

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
            "hosts": hosts::rows_json(&listing),
            "diagnostics": Value::Array(Vec::new()),
        });
        if let Some(transport) = &disclosures.transport {
            report["transport"] = transport.json();
        }
        if let Some(filesystem) = &disclosures.filesystem {
            report["filesystem"] = filesystem.json();
        }
        if let Some(database) = &disclosures.database {
            report["database"] = database.json();
        }
        if let Some(configuration) = &disclosures.configuration {
            report["configuration"] = configuration.to_json();
        }
        if let Some(observability) = &disclosures.observability {
            report["observability"] = observability.json();
        }
        if let Some(shutdown) = &disclosures.shutdown {
            report["shutdown"] = shutdown.json();
        }
        report["diagnostics"] = diagnostics_json(&config_warnings, &loaded.sources);
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
    // Rendered as a diagnostic, since a deploy check greps for the code.
    print_diagnostics(&config_warnings, &loaded.sources, style);
    EXIT_OK
}

fn schema_view(
    check: &ply_ty::CheckOutput,
    db: Option<&crate::db::DbConfig>,
    constant: &dyn Fn(&str) -> Result<ply_eval::Value, ply_span::Diagnostic>,
) -> Result<Option<crate::db::schema::SchemaView>, ply_span::Diagnostic> {
    let Some(name) = db.and_then(|c| c.schema.as_deref()) else {
        return Ok(None);
    };
    let resolved = crate::db::schema::resolve(check, name)?;
    let name = resolved.as_str().to_string();
    let shape = super::common::materialise_schema(&name, constant);
    Ok(Some(crate::db::schema::SchemaView {
        name,
        shape,
        state: crate::db::schema::State::Declared,
    }))
}
