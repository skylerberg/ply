use self::fixture::{op, receives_secrets, registry};
use ply_machine::config::Configuration;
use ply_machine::db::DbConfig;
use ply_machine::hosts::*;
use ply_codegen::c::producer;
use ply_eval::host::{HostListing, HostRegistry, HostResource, Linearity};
use ply_host::tls;
use ply_span::{SourceId, Symbol};
use ply_ty::CheckOutput;
use ply_ty::ty::{Footprint, Resource};

/// A registry whose handlers must never be called, for the tests that only report on a binding.
pub mod fixture {
    use ply_eval::host::{
        Determinism, HostAnswer, HostHandler, HostOp, HostRegistry, HostRequest, HostResource,
        HostRuntime, Linearity,
    };
    use ply_span::{Diagnostic, Symbol, codes};
    use ply_ty::ty::Resource;
    use std::sync::Arc;

    struct Never;

    impl HostHandler for Never {
        fn call(
            &self,
            _: &dyn HostRuntime,
            req: &HostRequest<'_>,
        ) -> Result<HostAnswer, Diagnostic> {
            Err(
                Diagnostic::error(codes::INTERNAL_ERROR, "a reporting test called a handler")
                    .primary(req.span, "here"),
            )
        }
    }

    pub fn op(
        effect: &str,
        name: &str,
        resource: HostResource,
        linearity: Linearity,
        blocking: bool,
        path: &'static str,
    ) -> HostOp {
        HostOp {
            effect: Symbol::new(effect),
            op: Symbol::new(name),
            resource,
            determinism: Determinism::Nondeterministic,
            linearity,
            blocking,
            secrets: false,
            path,
        }
    }

    /// The same registration, declared able to receive a credential.
    pub fn receives_secrets(mut op: HostOp) -> HostOp {
        op.secrets = true;
        op
    }

    #[allow(dead_code)]
    pub fn named(label: &str) -> HostResource {
        HostResource::Only(Resource::Named(Symbol::new(label)))
    }

    /// Declared deterministic, which binds against an effect the program did not mark `nondet`.
    #[allow(dead_code)]
    pub fn deterministic(mut op: HostOp) -> HostOp {
        op.determinism = Determinism::Deterministic;
        op
    }

    pub fn registry(ops: Vec<HostOp>) -> HostRegistry {
        let mut registry = HostRegistry::new();
        for op in ops {
            registry.register(op, Arc::new(Never));
        }
        registry
    }
}

const DB: &str = r#"
nondet effect db {
  read  get[r](key: Int) -> Int
  write put[r](key: Int, value: Int) -> Int
}

fn lookup(k: Int) -> Int / {db.read[users]} = db.get[users](k)

fn other(k: Int) -> Int / {db.read[orders]} = db.get[orders](k)

fn store(k: Int) -> Int / {db.write[orders]} = db.put[orders](k, 1)

fn stamp() -> Int / {clock.read} = clock.now()
"#;

fn check(source: &str) -> CheckOutput {
    producer::checked_front(&[(String::new(), source.to_string())], &[SourceId(0)])
        .expect("the fixture typechecks")
        .check
}

fn full() -> HostRegistry {
    registry(vec![
        op(
            "clock",
            "now",
            HostResource::Only(Resource::Singleton),
            Linearity::Repeatable,
            false,
            "ply_host::clock::now",
        ),
        op(
            "db",
            "get",
            HostResource::Any,
            Linearity::AtMostOnce,
            true,
            "ply_host::postgres::read",
        ),
        op(
            "db",
            "put",
            HostResource::Only(Resource::Named(Symbol::new("orders"))),
            Linearity::AtMostOnce,
            true,
            "ply_host::postgres::write",
        ),
    ])
}

fn listing() -> HostListing {
    full().preview(&check(DB)).expect("the fixture binds")
}

#[test]
fn the_listing_names_every_resource_an_any_handler_got() {
    let listing = listing();
    let triples: Vec<String> = listing.rows.iter().map(|r| r.to_string()).collect();
    assert_eq!(
        triples,
        [
            "clock.now",
            "db.get[orders]",
            "db.get[users]",
            "db.put[orders]"
        ]
    );
    assert_eq!(listing.handlers, 3);
}

#[test]
fn the_listing_and_its_digest_are_stable_across_runs() {
    let program = check(DB);
    let once = full().preview(&program).unwrap();
    let twice = full().preview(&program).unwrap();
    assert_eq!(once.rows, twice.rows);
    assert_eq!(once.digest_short(), twice.digest_short());
}

#[test]
fn the_digest_moves_when_a_flag_alone_moves() {
    let program = check(DB);
    let base = full().preview(&program).unwrap().digest_short();

    let clock = |linearity, blocking| {
        registry(vec![op(
            "clock",
            "now",
            HostResource::Only(Resource::Singleton),
            linearity,
            blocking,
            "ply_host::clock::now",
        )])
        .preview(&program)
        .unwrap()
        .digest_short()
    };

    let one = clock(Linearity::Repeatable, false);
    let linear = clock(Linearity::AtMostOnce, false);
    let blocks = clock(Linearity::Repeatable, true);
    assert_ne!(one, base);
    assert_ne!(one, linear, "linearity alone must move the digest");
    assert_ne!(one, blocks, "blocking alone must move the digest");

    // A handler that quietly became able to receive a credential is where secret containment stops being enforceable.
    let secrets = registry(vec![receives_secrets(op(
        "clock",
        "now",
        HostResource::Only(Resource::Singleton),
        Linearity::Repeatable,
        false,
        "ply_host::clock::now",
    ))])
    .preview(&program)
    .unwrap();
    assert_ne!(
        one,
        secrets.digest_short(),
        "the secrets column alone must move the digest"
    );
    assert!(secrets.rows[0].secrets);
}

#[test]
fn no_shipped_registration_declares_that_it_may_receive_a_credential() {
    let claiming: Vec<&str> = ply_machine::hosts::registry()
        .ops()
        .chain(ply_host::registry_with_database().ops())
        .filter(|op| op.secrets)
        .map(|op| op.path)
        .collect();
    assert!(claiming.is_empty(), "{claiming:?}");
}

/// A registry with nothing in it and one nothing in the program reaches are both empty listings,
/// and the report tells them apart by the handler count.
#[test]
fn an_empty_registry_and_an_idle_one_are_told_apart_by_the_handler_count() {
    let empty = HostRegistry::new().preview(&check(DB)).unwrap();
    assert!(empty.rows.is_empty());
    assert_eq!(empty.handlers, 0);

    let idle = registry(vec![op(
        "db",
        "get",
        HostResource::Any,
        Linearity::AtMostOnce,
        true,
        "ply_host::postgres::read",
    )]);
    // A driver linked into a program that declares the effect and never queries is idle, not wrong.
    let quiet = check("nondet effect db {\n  read get[r](key: Int) -> Int\n}\nfn f() -> Int = 1\n");
    let idle = idle.preview(&quiet).unwrap();
    assert!(idle.rows.is_empty());
    assert_eq!(idle.handlers, 1);
}

#[test]
fn a_row_carries_the_declaration_side_of_the_determinism_pair() {
    let listing = listing();
    let clock = &listing.rows[0];
    assert_eq!(clock.to_string(), "clock.now");
    assert_eq!(clock.resource, Resource::Singleton);
    assert_eq!(clock.linearity, Linearity::Repeatable);
    assert!(!clock.deterministic);
    assert!(clock.declared_nondet);
    assert_eq!(
        listing.rows[1].resource,
        Resource::Named(Symbol::new("orders"))
    );
    assert_eq!(listing.rows[1].path, "ply_host::postgres::read");
    assert!(listing.rows[1].blocking);
}

#[test]
fn hermetic_is_the_default_and_reaches_nothing() {
    let program = check(DB);
    let hosts = Hosts::open(
        &program,
        false,
        &ply_machine::options::TlsOptions::default(),
        &[],
        None,
        Configuration::default(),
        &ply_machine::trace::TraceOptions::silent(),
        None,
    )
    .unwrap();
    assert!(hosts.is_hermetic());
    assert_eq!(hosts.label(), "hermetic");
    assert!(hosts.listing().is_empty());
    for def in program.defs.values() {
        assert!(!hosts.reaches(&def.footprint));
    }
}

#[test]
fn a_host_backed_test_leaves_the_trivially_parallel_count() {
    let program = check(DB);
    let hosts = Hosts::bind(full(), &program, true).unwrap();
    assert_eq!(hosts.label(), "host");

    let reads = program
        .defs
        .values()
        .find(|d| d.simple_name.as_str() == "lookup")
        .unwrap();
    let pure = Footprint::empty();
    assert!(hosts.reaches(&reads.footprint));
    assert!(!hosts.reaches(&pure));

    let counts = Counts::of(
        &hosts,
        [(&reads.footprint, true), (&pure, true), (&pure, false)],
    );
    assert_eq!(counts.total, 3);
    assert_eq!(counts.host, 1);
    assert_eq!(counts.isolated, 1);
    assert_eq!(counts.shared, 1);

    // Under a hermetic binding the host column is empty.
    let hermetic = Hosts::open(
        &program,
        false,
        &ply_machine::options::TlsOptions::default(),
        &[],
        None,
        Configuration::default(),
        &ply_machine::trace::TraceOptions::silent(),
        None,
    )
    .unwrap();
    let counts = Counts::of(
        &hermetic,
        [(&reads.footprint, true), (&pure, true), (&pure, false)],
    );
    assert_eq!(counts.host, 0);
    assert_eq!(counts.isolated, 2);
    assert_eq!(counts.shared, 1);
}

/// `db.*` resolve to the postgres driver's paths, which is how a listing row is recognised as one.
fn postgres(op_name: &'static str, path: &'static str) -> HostRegistry {
    registry(vec![op(
        "db",
        op_name,
        HostResource::Any,
        Linearity::AtMostOnce,
        true,
        path,
    )])
}

fn configured(schema: Option<&str>) -> DbConfig {
    ply_machine::db::DbOptions {
        url: Some("postgres://ply:hunter2@127.0.0.1:5433/desk".to_string()),
        schema: schema.map(str::to_string),
        ..ply_machine::db::DbOptions::default()
    }
    .resolve_with(true, &|_| None)
    .expect("the fixture URL parses")
    .expect("--host and a URL yield a configuration")
}

#[test]
fn reaching_postgres_with_no_database_configured_is_refused_before_anything_runs() {
    let program = check(DB);
    let Err(diagnostics) =
        Hosts::bind_with(postgres("get", "ply_host::db::query"), &program, true, None)
    else {
        panic!("a bound driver with no database must be E0431");
    };
    assert_eq!(diagnostics[0].code, ply_span::codes::DB_NOT_CONFIGURED);
    assert!(
        diagnostics[0]
            .notes
            .iter()
            .any(|n| n.contains("db.get[orders]")),
        "the reader is not told which operations bound: {:?}",
        diagnostics[0].notes
    );
}

/// The check keys on the binding: an HTTP-only program under `--host` binds no postgres handler.
#[test]
fn a_program_that_reaches_no_database_needs_none_and_discloses_none() {
    let program = check(DB);
    let hosts = Hosts::bind(full(), &program, true).expect("net and clock bind without a URL");
    assert!(hosts.database().is_none());
    assert!(!hosts.is_live_database());
    assert!(hosts.disclosures().is_empty());
    assert_eq!(
        digest_short(hosts.listing(), &hosts.disclosures()),
        hosts.listing().digest_short(),
        "a program with no database in reach must hash what it hashed before W4"
    );
}

#[test]
fn a_configured_run_says_it_reached_a_real_database_and_never_says_the_password() {
    let program = check(DB);
    let hosts = Hosts::bind_with(
        postgres("get", "ply_host::db::query"),
        &program,
        true,
        Some(configured(None)),
    )
    .expect("a bound driver with a database binds");
    assert!(hosts.is_live_database());

    let line = database_line(&hosts).expect("a live database is reported");
    assert!(
        line.contains("postgres://ply:****@127.0.0.1:5433/desk"),
        "{line}"
    );
    assert!(line.contains("configured by --db"), "{line}");
    assert!(!line.contains("hunter2"), "{line}");

    let summary = serde_json::to_string(&hosts.summary_json()).unwrap();
    assert!(
        summary.contains("\"connections\":8"),
        "the pool is not disclosed: {summary}"
    );
    assert!(
        !summary.contains("hunter2"),
        "the `--json` object carried the password"
    );
}

/// `db.rollback` is handled in Ply inside `transaction`, so a bound one would abort nothing and commit what was meant to be discarded.
#[test]
fn a_bound_rollback_is_refused_as_a_defect_rather_than_listed() {
    let program = check(
        "nondet effect db {\n  write rollback[r](reason: Int) -> Int\n}\n\
         fn f() -> Int / {db.write[orders]} = db.rollback[orders](1)\n",
    );
    let Err(diagnostics) = Hosts::bind_with(
        postgres("rollback", "ply_host::db::abort"),
        &program,
        true,
        Some(configured(None)),
    ) else {
        panic!("a bound rollback must be refused as a defect");
    };
    assert_eq!(diagnostics[0].code, ply_span::codes::INTERNAL_ERROR);
    assert!(diagnostics[0].message.contains("db.rollback"));
}

#[test]
fn a_db_schema_naming_nothing_is_refused_with_what_the_program_does_have() {
    let program = check(DB);
    let Err(diagnostics) = Hosts::bind_with(
        postgres("get", "ply_host::db::query"),
        &program,
        true,
        Some(configured(Some("desk.schema"))),
    ) else {
        panic!("a schema function that does not exist must be refused");
    };
    assert_eq!(diagnostics[0].code, ply_span::codes::DB_NOT_CONFIGURED);
    assert!(
        diagnostics[0].notes.iter().any(|n| n.contains("E0433")),
        "the reader is not told what dropping the flag costs: {:?}",
        diagnostics[0].notes
    );
}

/// `net.listen_tls` resolves to the real TLS handler, so the listing carries the row `Transport::of` keys on.
fn transport_only(transport: Transport) -> Disclosures {
    Disclosures {
        transport: Some(transport),
        ..Disclosures::default()
    }
}

fn tls_registry() -> HostRegistry {
    registry(vec![op(
        "net",
        "listen_tls",
        HostResource::Any,
        Linearity::AtMostOnce,
        false,
        tls::HANDLER,
    )])
}

const NET: &str = r#"
nondet effect net {
  write listen_tls[s](port: Int, credential: String) -> Int
}

fn serve() -> Int / {net.write[api]} = net.listen_tls[api](443, "api")
"#;

fn tls_listing() -> HostListing {
    tls_registry()
        .preview(&check(NET))
        .expect("the fixture binds")
}

#[test]
fn a_plaintext_program_reports_no_transport_and_keeps_its_digest() {
    let listing = listing();
    assert!(Transport::of(&listing, None).is_none());
    assert_eq!(
        digest_short(&listing, &Disclosures::default()),
        listing.digest_short()
    );
}

#[test]
fn a_program_that_can_listen_over_tls_discloses_the_stack_by_name() {
    let listing = tls_listing();
    let transport = Transport::of(&listing, None).expect("the tls handler is in the listing");
    let json = transport.json();
    assert_eq!(json["library"], "rustls");
    assert_eq!(json["provider"], "ring");
    assert_eq!(json["alpn"][0], "http/1.1");
    assert_eq!(json["roots"]["trusted"], 0);
    assert!(transport.credentials.is_empty());
    assert!(listing.rows.iter().any(|row| row.path == tls::HANDLER));
}

/// The table carries enough of the fingerprint to recognise; the object carries all of it.
#[test]
fn the_object_carries_a_credentials_whole_fingerprint() {
    let transport = Transport {
        library: tls::LIBRARY,
        version: tls::VERSION,
        provider: tls::PROVIDER,
        versions: &tls::VERSIONS,
        alpn: &tls::ALPN,
        roots: tls::ROOTS,
        roots_version: tls::ROOTS_VERSION,
        trusted: 0,
        credentials: vec![CredentialView {
            name: "api".to_string(),
            fingerprint: "sha256:9f2c1a4e8b03c7d5e6f70819a2b3c4d5".to_string(),
            certificates: 2,
        }],
    };
    assert_eq!(
        transport.json()["credentials"][0]["fingerprint"],
        "sha256:9f2c1a4e8b03c7d5e6f70819a2b3c4d5",
        "the table abbreviates; the object must not"
    );
}

/// A CI check that broke on every certificate renewal is one people learn to ignore.
#[test]
fn the_digest_survives_a_rotation_and_moves_when_a_credential_does() {
    let listing = tls_listing();
    let with = |credentials: Vec<CredentialView>| {
        digest_short(
            &listing,
            &transport_only(Transport {
                library: tls::LIBRARY,
                version: tls::VERSION,
                provider: tls::PROVIDER,
                versions: &tls::VERSIONS,
                alpn: &tls::ALPN,
                roots: tls::ROOTS,
                roots_version: tls::ROOTS_VERSION,
                trusted: 0,
                credentials,
            }),
        )
    };
    let credential = |fingerprint: &str| CredentialView {
        name: "api".to_string(),
        fingerprint: fingerprint.to_string(),
        certificates: 1,
    };

    let before = with(vec![credential("sha256:aaaa")]);
    assert_eq!(
        before,
        with(vec![credential("sha256:bbbb")]),
        "a renewed certificate is an operational fact, not a structural one"
    );
    assert_ne!(
        before,
        with(vec![
            credential("sha256:aaaa"),
            CredentialView {
                name: "admin".to_string(),
                fingerprint: "sha256:cccc".to_string(),
                certificates: 1,
            }
        ]),
        "a second credential is a second thing the run can serve"
    );
    assert_ne!(before, with(Vec::new()));
    assert_ne!(
        before,
        listing.digest_short(),
        "a configured credential must not hash as if there were none"
    );
}

/// A handshake failure is attributable to no definition, so it is counted with its reason rather than raised.
#[test]
fn refused_handshakes_are_counted_and_named_rather_than_raised() {
    assert!(handshake_lines(&tls::HandshakeCounts::default()).is_empty());
    let counts = tls::HandshakeCounts {
        completed: 7,
        refused: 3,
        reasons: vec![("no application protocol in common", 2), ("not tls", 1)],
    };
    assert_eq!(
        handshake_lines(&counts),
        [
            "handshakes: 7 completed, 3 refused",
            "  2 no application protocol in common",
            "  1 not tls",
        ]
    );
    let json = handshakes_json(&counts);
    assert_eq!(json["refused"], 3);
    assert_eq!(
        json["reasons"][0]["reason"],
        "no application protocol in common"
    );
}
