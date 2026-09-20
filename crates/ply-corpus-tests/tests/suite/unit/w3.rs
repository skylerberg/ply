use ply_corpus::w3::{Loaded, Sample, Service, Transport, Variant, aliases, get, request};
use std::path::PathBuf;
use std::time::Duration;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn both_variants_are_produced_from_the_example_and_typecheck() {
    let service = Service::open(&repo()).expect("the example is where it was");
    for variant in [Variant::Sequential, Variant::TaskPerConn] {
        let source = service.source(variant).unwrap();
        Loaded::parse(&source).expect("the rewritten service typechecks");
    }
    let concurrent = service.source(Variant::TaskPerConn).unwrap();
    assert!(concurrent.contains("task.spawn(|| serve_connection(c, l))"));
    // Everything below the accept loop is the same program.
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
            (Transport::Http, "run_memory(8137, None, 9)"),
            (Transport::Https, "run_memory_tls(8137, \"desk\", None, 9)"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            service
                .project(dir.path(), variant, transport, 8137, 9)
                .unwrap();
            let source = std::fs::read_to_string(dir.path().join("desk.ply")).unwrap();
            assert!(source.contains(entry), "{entry} in {variant:?}");
            assert!(!source.contains("config.get[server]"), "{variant:?}");
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
    let loaded = Loaded::parse(&service.source(Variant::Sequential).unwrap()).unwrap();
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
