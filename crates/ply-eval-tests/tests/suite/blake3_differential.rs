//! `std.hash.blake3` against the `blake3` crate, at every structural boundary.

use ply_eval::{Machine, TaskRegions};
use ply_span::{SourceMap, Symbol};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::parse_program;
use ply_syntax::resolve::{Resolved, resolve};

/// Byte `i` is `i % 251`, which is what the published vectors use and what the probe below builds
/// in Ply.
fn input(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i % 251) as u8).collect()
}

fn escaped(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("\\x{b:02x}")).collect()
}

/// A probe that hashes `n` bytes and asserts the answer the crate gives.
fn probe(n: usize, expected: &[u8]) -> String {
    format!(
        r#"
import std.hash

fn input(n: Int) -> Bytes =
  bytes_concat_all(map(range(0, n), |i: Int| byte_of_int(i % 251)))

test "agrees at {n}" {{
  assert_eq(hash::blake3(input({n})), b"{expected}")
}}
"#,
        n = n,
        expected = escaped(expected),
    )
}

fn load(src: &str) -> (Program, Resolved) {
    let mut map = SourceMap::new();
    let probe_id = map.add("probe.ply", src.to_string());
    let mut sources: Vec<(ply_span::SourceId, ModuleName, &'static str)> = Vec::new();
    let mut queue = vec![ModuleName::from_dotted("std.hash")];
    while let Some(name) = queue.pop() {
        if sources.iter().any(|(_, n, _)| *n == name) {
            continue;
        }
        let text = ply_std::source(&name).expect("this module ships with the compiler");
        let id = map.add(ply_std::pseudo_path(&name), text.to_string());
        let module =
            ply_syntax::parse_module(id, name.clone(), text).expect("a shipped module parses");
        queue.extend(
            module
                .imports
                .iter()
                .map(|i| i.module_name())
                .filter(ply_std::is_std),
        );
        sources.push((id, name, text));
    }
    let mut program = parse_program(
        std::iter::once((probe_id, ModuleName::from_dotted("probe"), src))
            .chain(sources.iter().map(|(id, n, t)| (*id, n.clone(), *t))),
    )
    .expect("the probe and the modules it imports must parse");
    assert!(
        ply_derive::expand_program(&mut program).is_empty(),
        "derive expansion must not diagnose"
    );
    let resolved = resolve(&mut program).expect("the probe must resolve");
    (program, resolved)
}

/// The sizes, and what each one is for.
const BOUNDARIES: &[(usize, &str)] = &[
    (0, "one chunk, one empty block"),
    (1, "the shortest real block"),
    (63, "one byte short of a block"),
    (64, "exactly one block"),
    (65, "the first byte of a second block"),
    (1023, "one byte short of a chunk"),
    (1024, "exactly one chunk, still not a tree"),
    (1025, "the first input that is a tree"),
    (2048, "two whole chunks under one parent"),
    (3072, "three chunks, where the split is not the middle"),
    (4096, "four chunks, two parents and a root"),
];

#[test]
fn the_ply_blake3_agrees_with_the_crate_at_every_structural_boundary() {
    let mut disagreed: Vec<String> = Vec::new();
    for (n, why) in BOUNDARIES {
        let bytes = input(*n);
        let expected = blake3::hash(&bytes);
        let (program, resolved) = load(&probe(*n, expected.as_bytes()));
        let mut machine = Machine::for_program(&program, &resolved);
        machine.set_regions(TaskRegions::new());
        // The probe holds exactly one test, and `std.hash`'s own tests are in a different module,
        // so ordinal 0 in `probe` is unambiguous.
        if let Err(d) = machine.eval_test_in(&Symbol::from("probe"), 0) {
            disagreed.push(format!(
                "{n} bytes ({why}): the Ply implementation did not answer \
                 {}\n{d:#?}",
                expected.to_hex()
            ));
        }
    }
    assert!(
        disagreed.is_empty(),
        "the hash written in Ply's threshold T1 is zero disagreements, and this is {} of them.\n\
         A disagreement here refutes the decision to write the hash in Ply \
         rather than the code that failed: the reversal is a builtin, with this \
         module kept as the readable definition and this test kept as what holds \
         the two together.\n\n{}",
        disagreed.len(),
        disagreed.join("\n\n"),
    );
}

/// The differential above can only fail if the two implementations differ, and two implementations
/// that are both wrong in the same way would pass it.
#[test]
fn the_published_vectors_hold() {
    // From the BLAKE3 reference test vectors.
    let empty = "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262";
    let one = "2d3adedff11b61f14c886e35afa036736dcd87a74d27b5c1510225d0f592e213";
    assert_eq!(blake3::hash(&[]).to_hex().as_str(), empty);
    assert_eq!(blake3::hash(&[0]).to_hex().as_str(), one);
    // And the Ply module answers them, which is what `std.hash`'s own two tests assert in the
    // language.
    for (n, hex) in [(0usize, empty), (1, one)] {
        let bytes = input(n);
        let (program, resolved) = load(&probe(n, blake3::hash(&bytes).as_bytes()));
        let mut machine = Machine::for_program(&program, &resolved);
        machine.set_regions(TaskRegions::new());
        machine
            .eval_test_in(&Symbol::from("probe"), 0)
            .unwrap_or_else(|d| panic!("the module should answer {hex} for {n} bytes: {d:#?}"));
    }
}
