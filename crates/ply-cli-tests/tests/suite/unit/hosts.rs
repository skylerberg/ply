use self::fixture::{op, receives_secrets, registry};
use ply_cli::config::Configuration;
use ply_cli::db::DbConfig;
use ply_cli::hosts::*;
use ply_codegen::c::producer;
use ply_eval::host::{HostListing, HostRegistry, HostResource, Linearity};
use ply_host::tls;
use ply_span::{SourceId, Symbol};
use ply_ty::CheckOutput;
use ply_ty::ty::{Footprint, Resource};
use serde_json::Value;

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

    pub fn named(label: &str) -> HostResource {
        HostResource::Only(Resource::Named(Symbol::new(label)))
    }

    /// The same registration, declared deterministic, which binds against an effect the program did
    /// not mark `nondet`.
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
    producer::ensure_default();
    let modules = [("m".to_string(), source.to_string())];
    let front =
        producer::front(&modules, &[SourceId(0)]).expect("the port answers for the fixture");
    assert!(
        front.diagnostics.is_empty(),
        "the fixture typechecks: {:?}",
        front.diagnostics
    );
    front.check
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

/// One line per resolved triple, ascending, and an `Any` handler's resources spelled out rather
/// than hidden behind a `*`.
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
    let text = listing_lines(&listing, &Disclosures::default()).join("\n");
    assert!(!text.contains('*'), "a resource was hidden:\n{text}");
}

/// The listing whole, rather than a claim per column.
#[test]
fn the_table_is_exactly_the_shape_the_contract_specifies() {
    let lines = listing_lines(&listing(), &Disclosures::default());
    let (rendered, digest) = lines.split_at(lines.len() - 1);
    assert_eq!(
        rendered.join("\n"),
        "\
3 host handlers · 4 operations · trusted computing base

OPERATION       ATOM              HANDLER                    DET  LINEAR        BLOCKING  SECRETS
clock.now       clock.read        ply_host::clock::now       no   repeatable    no        no
db.get[orders]  db.read[orders]   ply_host::postgres::read   no   at-most-once  yes       no
db.get[users]   db.read[users]    ply_host::postgres::read   no   at-most-once  yes       no
db.put[orders]  db.write[orders]  ply_host::postgres::write  no   at-most-once  yes       no
"
    );
    assert!(digest[0].starts_with("digest: b3:"), "{digest:?}");
}

/// The whole ambition of the listing is a one-line diff in a review, which requires two runs
/// over one program to agree byte for byte.
#[test]
fn the_listing_and_its_digest_are_stable_across_runs() {
    let program = check(DB);
    let once = full().preview(&program).unwrap();
    let twice = full().preview(&program).unwrap();
    assert_eq!(
        listing_lines(&once, &Disclosures::default()),
        listing_lines(&twice, &Disclosures::default())
    );
    assert_eq!(once.digest_short(), twice.digest_short());
    assert_eq!(rows_json(&once), rows_json(&twice));
}

/// A handler that quietly became repeatable, or quietly stopped declaring itself blocking, is
/// exactly the change worth a reviewer's attention.
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

    // The newest column, and the one whose value a reviewer most needs a diff for: a handler
    // that quietly became able to receive a credential is where the secret containment claim's claim stops
    // being enforceable.
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
    let text = listing_lines(&secrets, &Disclosures::default()).join("\n");
    assert!(text.contains("SECRETS"), "{text}");
    assert!(text.lines().any(|l| l.ends_with("yes")), "{text}");
    assert_eq!(row_json(&secrets.rows[0])["secrets"], true);
}

#[test]
fn no_shipped_registration_declares_that_it_may_receive_a_credential() {
    let claiming: Vec<&str> = ply_cli::hosts::registry()
        .ops()
        .chain(ply_host::registry_with_database().ops())
        .filter(|op| op.secrets)
        .map(|op| op.path)
        .collect();
    assert!(claiming.is_empty(), "{claiming:?}");
}

#[test]
fn hermetic_says_so_and_still_reports_what_would_bind() {
    let lines = hermetic_lines(&listing());
    assert_eq!(lines[0], "hermetic — no host handler is bound");
    assert!(lines[2].contains("4 operations would bind"), "{lines:?}");
    assert!(lines[2].contains("--host"));
}

/// An empty listing is indistinguishable from a registry that failed to load, so neither form
/// is allowed to print one and stop.
#[test]
fn an_empty_registry_says_it_is_empty_rather_than_printing_nothing() {
    let empty = HostRegistry::new().preview(&check(DB)).unwrap();
    assert!(hermetic_lines(&empty)[2].contains("no host handler is compiled"));
    assert!(
        listing_lines(&empty, &Disclosures::default())
            .iter()
            .any(|l| l.contains("no host handler is compiled"))
    );

    let idle = registry(vec![op(
        "db",
        "get",
        HostResource::Any,
        Linearity::AtMostOnce,
        true,
        "ply_host::postgres::read",
    )]);
    // A driver linked into a program that declares the effect and never queries is idle, not
    // wrong.
    let quiet = check("nondet effect db {\n  read get[r](key: Int) -> Int\n}\nfn f() -> Int = 1\n");
    let idle = idle.preview(&quiet).unwrap();
    assert!(idle.rows.is_empty());
    assert!(
        hermetic_lines(&idle)[2].contains("none serves an atom"),
        "{idle:?}"
    );
}

#[test]
fn the_json_row_carries_the_declaration_side_of_the_determinism_pair() {
    let listing = listing();
    let rows = rows_json(&listing);
    let clock = &rows[0];
    assert_eq!(clock["triple"], "clock.now");
    assert_eq!(clock["atom"], "clock.read");
    assert_eq!(clock["resource"], Value::Null);
    assert_eq!(clock["linearity"], "repeatable");
    assert_eq!(clock["deterministic"], false);
    assert_eq!(clock["declared_nondet"], true);
    assert_eq!(rows[1]["resource"], "orders");
    assert_eq!(rows[1]["handler"], "ply_host::postgres::read");
    assert_eq!(rows[1]["blocking"], true);
}

/// The default is the point: nothing binds without the flag, and a hermetic binding reaches
/// nothing whatever the registry holds.
#[test]
fn hermetic_is_the_default_and_reaches_nothing() {
    let program = check(DB);
    let hosts = Hosts::open(
        &program,
        false,
        &[],
        &[],
        None,
        Configuration::default(),
        &ply_cli::trace::TraceOptions::silent(),
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

/// A footprint that meets the binding is host-backed and therefore not isolated, however
/// isolated its atoms would otherwise make it.
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

    // The same corpus under a hermetic binding: the host column is empty and every other number
    // is what it was before W1.
    let hermetic = Hosts::open(
        &program,
        false,
        &[],
        &[],
        None,
        Configuration::default(),
        &ply_cli::trace::TraceOptions::silent(),
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

// --- database -----------------------------------------------------------

/// A registry whose `db.*` resolve to the postgres driver's paths, which is how a listing row
/// is recognised as one.
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
    ply_cli::db::DbOptions {
        url: Some("postgres://ply:hunter2@127.0.0.1:5433/desk".to_string()),
        schema: schema.map(str::to_string),
        ..ply_cli::db::DbOptions::default()
    }
    .resolve_with(true, &|_| None)
    .expect("the fixture URL parses")
    .expect("--host and a URL yield a configuration")
}

/// The check `--db` exists for: a program that reaches postgres and a run that named no
/// database is a service that would discover it had nowhere to connect after accepting a
/// request.
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

/// The complement, and the reason the check keys on the *binding*: an HTTP-only program under
/// `--host` binds no postgres handler and must not be made to name a database it will never
/// open.
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

    let text = listing_lines(hosts.listing(), &hosts.disclosures()).join("\n");
    assert!(
        text.contains("ply_host::db::scan · select insert"),
        "{text}"
    );
    assert!(text.contains("8 connections · acquire 5000ms"), "{text}");
    assert!(!text.contains("hunter2"), "{text}");
    assert!(
        !serde_json::to_string(&hosts.summary_json())
            .unwrap()
            .contains("hunter2"),
        "the `--json` object carried the password"
    );
}

/// Transactions as handlers handles `db.rollback` in Ply, inside `transaction`, so a bound one would abort
/// nothing and commit what the program meant to discard.
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

// --- transport ----------------------------------------------------------

/// A registry whose `net.listen_tls` resolves to the real TLS handler, so the listing carries
/// the row `Transport::of` keys on.
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

/// A program that cannot create a TLS listener says nothing about TLS, and its digest is what
/// it was before W3 — which is the whole reason the block is conditional rather than always
/// printed.
#[test]
fn a_plaintext_program_reports_no_transport_and_keeps_its_digest() {
    let listing = listing();
    assert!(Transport::of(&listing, None).is_none());
    assert_eq!(
        digest_short(&listing, &Disclosures::default()),
        listing.digest_short()
    );
    assert!(
        !listing_lines(&listing, &Disclosures::default())
            .join("\n")
            .contains("transport")
    );
}

#[test]
fn a_program_that_can_listen_over_tls_discloses_the_stack_by_name() {
    let listing = tls_listing();
    let transport = Transport::of(&listing, None).expect("the tls handler is in the listing");
    assert_eq!(
        transport.lines(),
        [
            "",
            "transport",
            "tls  rustls 0.23.43 · provider ring · TLS 1.3, TLS 1.2 · alpn http/1.1",
            "",
            "credentials",
            "none — `net.listen_tls` is E0429 until `--tls NAME=CERT,KEY` names one",
        ]
    );
    let text = listing_lines(&listing, &transport_only(transport)).join("\n");
    assert!(text.contains(tls::HANDLER), "{text}");
    assert!(text.contains("alpn http/1.1"), "{text}");
}

/// The `--json` object carries the whole fingerprint; the table carries enough of it to
/// recognise and not enough to push the columns out.
#[test]
fn a_credential_is_listed_by_name_and_fingerprint() {
    let transport = Transport {
        library: tls::LIBRARY,
        version: tls::VERSION,
        provider: tls::PROVIDER,
        versions: &tls::VERSIONS,
        alpn: &tls::ALPN,
        credentials: vec![CredentialView {
            name: "api".to_string(),
            fingerprint: "sha256:9f2c1a4e8b03c7d5e6f70819a2b3c4d5".to_string(),
            certificates: 2,
        }],
    };
    assert_eq!(
        transport.lines().last().unwrap(),
        "api  sha256:9f2c1a4e8b03…  2 certificates"
    );
    assert_eq!(
        transport.json()["credentials"][0]["fingerprint"],
        "sha256:9f2c1a4e8b03c7d5e6f70819a2b3c4d5",
        "the table abbreviates; the object must not"
    );
}

/// The trusted computing base listing: a CI check that broke on every certificate renewal is a CI check people learn
/// to ignore.
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

/// A handshake failure is not the program's fault and not attributable to any definition, so it
/// is never a diagnostic — but silence would be wrong, so it is counted with its reason.
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
