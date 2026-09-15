use ply_corpus::serve::{Endpoint, Parser, Program, Sample, rust_floor};
use std::path::PathBuf;
use std::time::Duration;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The example's code with its `//` comments removed, so that a claim about which builtins a
/// variant *calls* is not answered by prose describing them.
fn code_only(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    for line in source.lines() {
        let bytes = line.as_bytes();
        let mut in_string = false;
        let mut cut = line.len();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'\\' if in_string => i += 1,
                b'"' => in_string = !in_string,
                b'/' if !in_string && bytes.get(i + 1) == Some(&b'/') => {
                    cut = i;
                    break;
                }
                _ => {}
            }
            i += 1;
        }
        out.push_str(&line[..cut]);
        out.push('\n');
    }
    out
}

/// The harness rewrites the example, and a rewrite that silently matched nothing would measure
/// a server on the example's own port.
#[test]
fn the_w1_reconstruction_replaces_every_scan_and_nothing_else() {
    let endpoint = Endpoint::open(&repo()).unwrap();
    let native = endpoint.benchable(Parser::Native).unwrap();
    let folds = endpoint.benchable(Parser::W1Folds).unwrap();
    let folds_code = code_only(&folds);
    for builtin in [
        "bytes_scan",
        "bytes_index_of",
        "bytes_starts_with",
        "bytes_ends_with",
        "bytes_split",
        "bytes_position",
    ] {
        assert!(
            !folds_code.contains(builtin),
            "`{builtin}` survived the rewrite"
        );
    }
    assert!(
        code_only(&native).contains("bytes_scan"),
        "the native variant stopped calling the builtins it is measuring"
    );
    for shared in [
        "fn parse(head: Bytes) -> Parsed {",
        "fn request_line(line: Bytes) -> Parsed {",
        "fn answer(head: Bytes) -> Bytes =",
        "fn response(status: String, body: String) -> Bytes =",
    ] {
        assert!(native.contains(shared), "{shared}");
        assert!(folds.contains(shared), "{shared}");
    }

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("hello.ply"), &folds).unwrap();
    Program::load(dir.path()).expect("the reconstructed endpoint typechecks");
}

#[test]
fn both_shapes_are_produced_from_the_example_and_typecheck() {
    let endpoint = Endpoint::open(&repo()).expect("the example is where it was");
    // Both parsers, because the load table serves the reconstruction too: W2's claim about
    // requests per second needs a before taken the same way as its after, not a before quoted
    // from a milestone ago.
    for parser in Parser::all() {
        for source in [
            endpoint.sequential(parser, 19000, 3).unwrap(),
            endpoint.concurrent(parser, 19001, 3).unwrap(),
        ] {
            assert!(source.contains("fn connections() -> Int = 3"));
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("hello.ply"), &source).unwrap();
            Program::load(dir.path()).expect("the rewritten endpoint typechecks");
        }
    }
}

/// The concurrent variant must differ from the sequential one in `serve` and in the three rows
/// above it, and nowhere else: if it changed `serve_one` or the parser, the two would not be
/// measuring one endpoint.
#[test]
fn the_concurrent_variant_changes_only_the_accept_loop() {
    let endpoint = Endpoint::open(&repo()).unwrap();
    let sequential = endpoint.sequential(Parser::Native, 19002, 1).unwrap();
    let concurrent = endpoint.concurrent(Parser::Native, 19002, 1).unwrap();
    assert!(concurrent.contains("task.spawn(|| serve_one(c))"));
    for shared in [
        "fn serve_one(c: Int) -> Unit / {net.write[conn]} {",
        "fn answer(head: Bytes) -> Bytes =",
        "fn head_end(head: Bytes) -> Int =",
    ] {
        assert!(sequential.contains(shared), "{shared}");
        assert!(concurrent.contains(shared), "{shared}");
    }
}

/// Nearest-rank, and the tail is the number this milestone reports, so an off-by-one here is an
/// off-by-one in the answer.
#[test]
fn percentiles_are_nearest_rank_over_the_sample() {
    let sample = Sample {
        latencies: (1..=100).map(Duration::from_micros).collect(),
        failures: Vec::new(),
    };
    assert_eq!(sample.percentile(0.50), Duration::from_micros(50));
    assert_eq!(sample.percentile(0.95), Duration::from_micros(95));
    assert_eq!(sample.percentile(0.99), Duration::from_micros(99));
    assert_eq!(sample.percentile(1.0), Duration::from_micros(100));

    let one = Sample {
        latencies: vec![Duration::from_micros(7)],
        failures: Vec::new(),
    };
    assert_eq!(one.percentile(0.99), Duration::from_micros(7));
    assert_eq!(Sample::default().percentile(0.5), Duration::ZERO);
}

/// A server that answered some of the requests is not a slower server, it is a different
/// measurement, so the shortfall has to stop the run.
#[test]
fn a_short_sample_is_an_error_rather_than_a_smaller_denominator() {
    let sample = Sample {
        latencies: vec![Duration::from_micros(1); 3],
        failures: vec!["connection reset".to_string()],
    };
    let err = sample.require(4).unwrap_err().to_string();
    assert!(err.contains("3 of 4"), "{err}");
    assert!(err.contains("connection reset"), "{err}");
    assert!(sample.require(3).is_ok());
}

/// The floor is the denominator every interpreter number is read against, so it has to actually
/// serve.
#[test]
fn the_rust_floor_answers_every_request() {
    assert!(rust_floor(8).unwrap() > Duration::ZERO);
}
