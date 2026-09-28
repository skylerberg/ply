use ply_corpus::w3::{Loaded, Sample, Service, Transport, Variant, aliases, get, request};
use std::path::PathBuf;
use std::time::Duration;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn every_variant_and_mode_is_written_down_and_typechecks() {
    let service = Service::open(&repo()).expect("the example is where it was");
    for variant in [Variant::Sequential, Variant::TaskPerConn] {
        for transport in [Transport::Http, Transport::Https] {
            let source = service.source(variant, transport).unwrap();
            Loaded::parse(&source).expect("the corpus's service typechecks");
        }
    }
    // The task-per-connection program is the sequential one with the accept loop spawned: that is
    // the whole of what its variant means, and the rest of the service is the same program.
    let concurrent = service
        .source(Variant::TaskPerConn, Transport::Http)
        .unwrap();
    assert!(concurrent.contains("task.spawn(|| serve_connection(c, l))"));
    for shared in [
        "pub fn serve_connection(c: Int, l: http::Limits) -> Unit\n  / {Serving, net.recv[conn], net.send[conn], net.close[conn]} = {",
        "pub fn answer(req: http::Request) -> Reply / {Serving} =",
        "pub fn table() -> List<router::Route<Endpoint>> = [",
    ] {
        assert!(concurrent.contains(shared), "{shared}");
    }
}

#[test]
fn the_served_project_typechecks_and_drives_the_twin() {
    let service = Service::open(&repo()).unwrap();
    for variant in [Variant::Sequential, Variant::TaskPerConn] {
        for (transport, entry) in [
            (Transport::Http, "run_memory(port, None, count)"),
            (
                Transport::Https,
                "run_memory_tls(port, \"desk\", None, count)",
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            service.project(dir.path(), variant, transport).unwrap();
            let source = std::fs::read_to_string(dir.path().join("desk.ply")).unwrap();
            // The entry point reads its port and connection count from the service's own settings,
            // so one program serves any port: the splice it replaced wrote a port into the source.
            assert!(source.contains(entry), "{entry} in {variant:?}");
            assert!(
                source.contains("configured(port_of(config.get[server](port_key())))"),
                "{variant:?}"
            );
            Loaded::parse(&source).expect("the served project typechecks");
        }
    }
}

#[test]
fn the_explicit_spelling_typechecks_and_names_no_set() {
    let service = Service::open(&repo()).unwrap();
    let (explicit, rewritten) = service.explicit_rows().unwrap();
    assert!(rewritten >= 8, "only {rewritten} rows were rewritten");
    assert!(!explicit.contains("/ {Desk"), "a `/ {{Desk` row survived");
    Loaded::parse(&explicit).expect("the expanded service typechecks");
}

#[test]
fn an_alias_and_its_expansion_are_one_program() {
    let report = aliases(&repo()).unwrap();
    assert_eq!(report.hash_differences, 0);
    assert_eq!(report.body_differences, 0);
    assert_eq!(report.footprint_differences, 0);
    assert_eq!(report.stored_bytes_aliased, report.stored_bytes_explicit);
    assert!(report.definitions_naming_the_set >= 8);
    assert!(report.source_bytes_explicit > report.source_bytes_aliased);
}

#[test]
fn the_client_reads_both_framings_the_service_produces() {
    let service = Service::open(&repo()).unwrap();
    let loaded = Loaded::parse(
        &service
            .source(Variant::Sequential, Transport::Http)
            .unwrap(),
    )
    .unwrap();
    // Buffered then streamed on one connection: the second response must begin exactly where the first ended.
    let script = vec![vec![get("/items"), get("/orders/1/receipt")]];
    let (_, connections) = loaded.over_sim(script).unwrap();
    assert_eq!(connections, 1);
}

#[test]
fn percentiles_are_nearest_rank() {
    let of: Vec<Duration> = (1..=100).map(Duration::from_micros).collect();
    assert_eq!(Sample::percentile(&of, 0.50), Duration::from_micros(50));
    assert_eq!(Sample::percentile(&of, 0.99), Duration::from_micros(99));
    assert_eq!(Sample::percentile(&of, 1.0), Duration::from_micros(100));
    assert_eq!(Sample::percentile(&[], 0.5), Duration::ZERO);
}

#[test]
fn padding_a_value_adds_bytes_and_no_fields() {
    let small = request("GET", "/items", None, false, 0, 0);
    let big = request("GET", "/items", None, false, 4096, 0);
    assert!(big.len() > small.len() * 40);
    assert_eq!(count(&big, b"\r\n"), count(&small, b"\r\n") + 1);
    let fielded = request("GET", "/items", None, false, 0, 8);
    assert_eq!(count(&fielded, b"\r\n"), count(&small, b"\r\n") + 8);
}

fn count(hay: &[u8], needle: &[u8]) -> usize {
    hay.windows(needle.len()).filter(|w| *w == needle).count()
}

/// Every served fixture's entry point answers its own `db` in Ply: since the driver moved in there
/// is no host handler for a `db` atom, so a `main` that performs one is a run that refuses with
/// `E0303`. The TLS fixtures had drifted exactly that way, in a section no CI job runs.
#[test]
fn no_served_fixtures_entry_point_performs_db_itself() {
    let root = repo();
    for variant in ["sequential", "task-per-conn"] {
        for mode in ["http", "https", "memory", "postgres", "tls"] {
            let path = root.join(format!(
                "crates/ply-corpus/fixtures/desk-{variant}-{mode}.ply"
            ));
            let source = std::fs::read_to_string(&path).unwrap();
            let loaded = Loaded::parse(&source)
                .unwrap_or_else(|e| panic!("{} does not check: {e:#}", path.display()));
            let main = loaded
                .check
                .defs
                .values()
                .find(|d| d.simple_name.as_str() == "main" && d.module.to_string() == "desk")
                .unwrap_or_else(|| panic!("{} declares no `desk.main`", path.display()));
            for (which, row) in [
                ("publishes", &main.footprint),
                ("performs", &main.performed),
            ] {
                let row = row.to_string();
                assert!(
                    !row.contains("std.db.db."),
                    "{}: `main` {which} `db` and nothing binds it: {row}",
                    path.display()
                );
            }
        }
    }
}

#[test]
fn the_load_client_is_a_program_that_typechecks() {
    // What the socket measurements point `ply run --host` at, so a run measures a program that was
    // checked where every other program is rather than one only exercised by a benchmark.
    let source =
        std::fs::read_to_string(repo().join("crates/ply-corpus/fixtures/load.ply")).unwrap();
    assert!(source.contains("fn main() -> Int / {Load}"));
    // The settings it reads, which are the `--set` names its caller passes.
    for key in [
        "LOAD_HOST",
        "LOAD_PORT",
        "LOAD_ROUTES",
        "LOAD_CONNECTIONS",
        "LOAD_PER_CONN",
        "LOAD_TLS",
        "LOAD_REPORT",
    ] {
        assert!(source.contains(key), "{key}");
    }
    Loaded::parse(&source).expect("the load client typechecks");
}
