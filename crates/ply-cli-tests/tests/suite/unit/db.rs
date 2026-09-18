use ply_cli::db::*;
use ply_span::codes;
use std::collections::BTreeMap;

fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
    let map: BTreeMap<String, String> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    move |key: &str| map.get(key).cloned()
}

fn options(url: Option<&str>) -> DbOptions {
    DbOptions {
        url: url.map(str::to_string),
        ..DbOptions::default()
    }
}

#[test]
fn a_full_url_parses_into_the_fields_the_driver_needs() {
    let url = DbUrl::parse("postgres://ply:hunter2@db.internal:5433/desk?sslmode=disable")
        .expect("a well-formed URL parses");
    assert_eq!(url.user, "ply");
    assert_eq!(url.host, "db.internal");
    assert_eq!(url.port, 5433);
    assert_eq!(url.database, "desk");
    assert_eq!(url.sslmode, SslMode::Disable);
    assert_eq!(url.password().map(Secret::expose), Some("hunter2"));
}

#[test]
fn the_port_and_the_sslmode_default_to_what_postgres_means_by_omitting_them() {
    let url = DbUrl::parse("postgresql://ply@localhost/desk").unwrap();
    assert_eq!(url.port, DEFAULT_PORT);
    assert_eq!(url.sslmode, SslMode::Prefer);
    assert!(!url.has_password());
}

#[test]
fn a_percent_escaped_password_survives_the_delimiters_it_contains() {
    let url = DbUrl::parse("postgres://ply:p%40ss%2Fword@localhost:5432/desk").unwrap();
    assert_eq!(url.password().map(Secret::expose), Some("p@ss/word"));
    assert_eq!(url.host, "localhost");
}

#[test]
fn every_malformed_string_is_refused_with_what_to_write() {
    for (text, expected) in [
        ("", "postgres://"),
        ("desk", "postgres://"),
        ("host=localhost dbname=desk", "keyword/value"),
        ("postgres://localhost/desk", "user@"),
        ("postgres://ply@/desk", "no host"),
        ("postgres://ply@localhost", "no database"),
        ("postgres://ply@localhost:abc/desk", "not a port"),
        ("postgres://ply@localhost:0/desk", "port 0"),
    ] {
        let why = DbUrl::parse(text).expect_err("`{text}` must be refused");
        assert!(
            why.contains(expected),
            "`{text}` was refused with `{why}`, which does not mention `{expected}`"
        );
    }
}

#[test]
fn an_sslmode_stronger_than_prefer_is_refused_by_name() {
    for mode in ["require", "verify-ca", "verify-full", "allow"] {
        let why = DbUrl::parse(&format!("postgres://ply@h:5432/d?sslmode={mode}"))
            .expect_err("`{mode}` is not configurable");
        assert!(why.contains("a word that lies"), "{why}");
    }
    assert!(DbUrl::parse("postgres://ply@h:5432/d?sslmode=prefer").is_ok());
    assert!(DbUrl::parse("postgres://ply@h:5432/d?sslmode=disable").is_ok());
}

#[test]
fn an_unknown_parameter_is_refused_rather_than_ignored() {
    let why = DbUrl::parse("postgres://ply@h/d?connect_timeout=3").unwrap_err();
    assert!(why.contains("--db-connect-ms"), "{why}");
    assert!(DbUrl::parse("postgres://ply@h/d?application_name=desk").is_ok());
}

/// A password reaching a diagnostic reaches the result cache, which never forgets.
#[test]
fn no_rendering_of_a_url_or_a_secret_contains_the_password() {
    let url = DbUrl::parse("postgres://ply:hunter2@localhost:5432/desk").unwrap();
    let renderings = [
        url.to_string(),
        format!("{url:?}"),
        url.redacted(),
        url.password().unwrap().to_string(),
        format!("{:?}", url.password().unwrap()),
        format!(
            "{:?}",
            DbConfig {
                url: url.clone(),
                source: Source::Flag,
                pool: 8,
                acquire_ms: 1,
                statement_ms: 1,
                idle_txn_ms: 1,
                connect_ms: 1,
                statement_cache: 1,
                schema: None,
            }
        ),
    ];
    for rendering in &renderings {
        assert!(
            !rendering.contains("hunter2"),
            "`{rendering}` carries the password"
        );
        assert!(rendering.contains(REDACTED), "`{rendering}` hid nothing");
    }
    assert_eq!(
        url.redacted(),
        "postgres://ply:****@localhost:5432/desk?sslmode=prefer"
    );
}

#[test]
fn the_string_the_driver_connects_with_round_trips_through_the_fields() {
    let url = DbUrl::parse("postgres://ply:p%40ss%2Fw%3Ard@db.internal:5433/desk?sslmode=disable")
        .unwrap();
    let rebuilt =
        DbUrl::parse(url.connection_string().expose()).expect("the rebuilt string parses");
    assert_eq!(rebuilt, url);
    assert_eq!(rebuilt.password().map(Secret::expose), Some("p@ss/w:rd"));
}

/// It returns a `Secret`, so a caller must write `expose`: the word a reviewer greps for.
#[test]
fn the_only_rendering_that_carries_the_password_is_a_secret() {
    let url = DbUrl::parse("postgres://ply:hunter2@localhost:5432/desk").unwrap();
    let carrier = url.connection_string();
    assert!(carrier.expose().contains("hunter2"));
    assert_eq!(carrier.to_string(), REDACTED);
    assert!(!format!("{carrier:?}").contains("hunter2"));
}

/// The caller cannot know whether the operator put a password in the string.
#[test]
fn a_malformed_url_diagnostic_never_echoes_what_it_was_given() {
    let options = options(Some("postgres://ply:hunter2@localhost:5432"));
    let diagnostics = options
        .resolve_with(true, &env_of(&[]))
        .expect_err("a URL with no database is refused");
    let rendered = format!("{:?}", diagnostics);
    assert!(!rendered.contains("hunter2"), "{rendered}");
    assert_eq!(diagnostics[0].code, codes::DB_NOT_CONFIGURED);
}

#[test]
fn the_environment_supplies_the_url_and_never_the_binding() {
    let env = env_of(&[(URL_ENV, "postgres://ply@localhost:5432/desk")]);
    assert!(
        options(None).resolve_with(false, &env).unwrap().is_none(),
        "a hermetic run reads no database whatever the environment holds"
    );
    let config = options(None)
        .resolve_with(true, &env)
        .unwrap()
        .expect("under --host the environment is read");
    assert_eq!(config.source, Source::Environment);
    assert_eq!(config.url.database, "desk");
}

#[test]
fn the_flag_wins_over_the_environment_and_the_report_says_which() {
    let env = env_of(&[(URL_ENV, "postgres://ply@localhost:5432/from_env")]);
    let config = options(Some("postgres://ply@localhost:5432/from_flag"))
        .resolve_with(true, &env)
        .unwrap()
        .unwrap();
    assert_eq!(config.url.database, "from_flag");
    assert_eq!(config.source, Source::Flag);
    assert_eq!(config.source.as_str(), "--db");
}

#[test]
fn the_password_comes_out_of_the_environment_and_not_out_of_ps() {
    let env = env_of(&[(PASSWORD_ENV, "hunter2")]);
    let config = options(Some("postgres://ply@localhost:5432/desk"))
        .resolve_with(true, &env)
        .unwrap()
        .unwrap();
    assert_eq!(config.url.password().map(Secret::expose), Some("hunter2"));
    assert!(!config.url.redacted().contains("hunter2"));
}

#[test]
fn a_password_in_both_places_is_refused_rather_than_resolved() {
    let env = env_of(&[(PASSWORD_ENV, "fromenv")]);
    let diagnostics = options(Some("postgres://ply:fromurl@localhost:5432/desk"))
        .resolve_with(true, &env)
        .unwrap_err();
    assert_eq!(diagnostics[0].code, codes::DB_NOT_CONFIGURED);
    let rendered = format!("{diagnostics:?}");
    assert!(
        !rendered.contains("fromenv") && !rendered.contains("fromurl"),
        "{rendered}"
    );
}

#[test]
fn an_empty_environment_value_is_a_refusal_rather_than_an_absence() {
    let diagnostics = options(None)
        .resolve_with(true, &env_of(&[(URL_ENV, "  ")]))
        .unwrap_err();
    assert_eq!(diagnostics[0].code, codes::DB_NOT_CONFIGURED);
    assert!(diagnostics[0].message.contains(URL_ENV));
}

#[test]
fn naming_no_database_is_not_an_error_until_the_driver_binds() {
    assert!(
        options(None)
            .resolve_with(true, &env_of(&[]))
            .unwrap()
            .is_none()
    );
    let diagnostic = missing(&["db.query[items]".to_string()]);
    assert_eq!(diagnostic.code, codes::DB_NOT_CONFIGURED);
    assert!(
        diagnostic
            .notes
            .iter()
            .any(|n| n.contains("db.query[items]"))
    );
    assert!(diagnostic.notes.iter().any(|n| n.contains(PASSWORD_ENV)));
}

#[test]
fn the_pool_defaults_are_the_contract_s_and_are_reported_as_numbers_somebody_chose() {
    let config = options(Some("postgres://ply@localhost/desk"))
        .resolve_with(true, &env_of(&[]))
        .unwrap()
        .unwrap();
    assert_eq!(config.pool, DEFAULT_POOL);
    assert_eq!(config.acquire_ms, DEFAULT_ACQUIRE_MS);
    assert_eq!(config.statement_ms, DEFAULT_STATEMENT_MS);
    assert_eq!(config.idle_txn_ms, DEFAULT_IDLE_TXN_MS);
    assert_eq!(config.connect_ms, DEFAULT_CONNECT_MS);
    assert_eq!(config.statement_cache, DEFAULT_STATEMENT_CACHE);
}

#[test]
fn a_schema_name_that_is_not_module_dot_fn_is_refused_at_resolution() {
    let mut options = options(Some("postgres://ply@localhost/desk"));
    options.schema = Some("schema".to_string());
    let diagnostics = options.resolve_with(true, &env_of(&[])).unwrap_err();
    assert_eq!(diagnostics[0].code, codes::DB_NOT_CONFIGURED);
    options.schema = Some("desk.schema".to_string());
    assert!(options.resolve_with(true, &env_of(&[])).is_ok());
}

fn config() -> DbConfig {
    options(Some(
        "postgres://ply:secret@127.0.0.1:5433/desk?sslmode=disable",
    ))
    .resolve_with(true, &env_of(&[]))
    .unwrap()
    .unwrap()
}

#[test]
fn a_program_with_no_database_in_reach_reports_no_block_at_all() {
    assert!(Database::of(Vec::new(), None, None, None).is_none());
}

#[test]
fn the_block_names_the_scanner_the_pool_and_the_collation() {
    let database = Database::of(
        vec!["db.query[items]".to_string()],
        Some(config()),
        Some(ServerFacts {
            version: "PostgreSQL 18.3".to_string(),
            database: "desk".to_string(),
            collation: "C".to_string(),
            encoding: "UTF8".to_string(),
        }),
        Some(schema::SchemaView {
            name: "desk.schema".to_string(),
            shape: Some(schema::Shape {
                tables: 2,
                columns: 11,
            }),
            state: schema::State::Verified,
        }),
    )
    .unwrap();
    assert_eq!(
        database.lines(),
        [
            "",
            "database",
            "server     PostgreSQL 18.3 · database desk · collation C · encoding UTF8",
            "pool       8 connections · acquire 5000ms · statement 30000ms · idle-txn 30000ms · connect 5000ms · statements 256",
            "scanner    ply_host::db::scan · select insert update delete values with",
            "schema     desk.schema · 2 tables · 11 columns · verified",
        ]
    );
    assert!(database.is_live());
}

#[test]
fn an_unconnected_run_says_so_and_still_redacts() {
    let database = Database::of(
        vec!["db.query[items]".to_string()],
        Some(config()),
        None,
        None,
    )
    .unwrap();
    let text = database.lines().join("\n");
    assert!(text.contains("not connected"), "{text}");
    assert!(text.contains("configured by --db"), "{text}");
    assert!(!text.contains("secret"), "{text}");
    assert!(text.contains("E0433"), "{text}");
    assert_eq!(
        database.json()["url"],
        "postgres://ply:****@127.0.0.1:5433/desk?sslmode=disable"
    );
    assert_eq!(database.json()["server"], serde_json::Value::Null);
}

/// A halved pool must move the digest a CI check pins, and a server upgrade must not.
#[test]
fn the_digest_covers_the_pool_and_the_schema_name_and_not_the_server() {
    let digest = |database: &Database| {
        let mut hasher = blake3::Hasher::new();
        database.hash_into(&mut hasher);
        hasher.finalize().to_hex().to_string()
    };
    let base = Database::of(vec!["db.query[items]".into()], Some(config()), None, None).unwrap();

    let mut halved = base.clone();
    halved.config.as_mut().unwrap().pool = 4;
    assert_ne!(digest(&base), digest(&halved), "a halved pool is a change");

    let mut upgraded = base.clone();
    upgraded.server = Some(ServerFacts {
        version: "PostgreSQL 19.0".to_string(),
        database: "other".to_string(),
        collation: "C".to_string(),
        encoding: "UTF8".to_string(),
    });
    assert_eq!(
        digest(&base),
        digest(&upgraded),
        "a CI check that broke on a minor server upgrade is one people learn to ignore"
    );

    let mut named = base.clone();
    named.config.as_mut().unwrap().schema = Some("desk.schema".to_string());
    assert_ne!(digest(&base), digest(&named));

    let mut migrated = named.clone();
    migrated.schema = Some(schema::SchemaView {
        name: "desk.schema".to_string(),
        shape: Some(schema::Shape {
            tables: 3,
            columns: 20,
        }),
        state: schema::State::Verified,
    });
    assert_eq!(
        digest(&named),
        digest(&migrated),
        "the table count is a property of the database, not of this binary"
    );
}
