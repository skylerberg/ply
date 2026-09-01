//! A `[x, ..rest]` pattern shares the list rather than copying its tail (ADR 0034 Decision 2).
//!
//! Recursion over a list by pattern is the shape a parser or a fold written by hand takes, and it
//! was quadratic: every match built the rest as a fresh array. The rest is now an offset into the
//! same arrays, so walking a list of `n` elements moves O(n) bytes, and doubling `n` doubles them.

use crate::counting::charge;
use ply_eval::Machine;
use ply_span::{SourceId, SourceMap};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::parse_program;
use ply_syntax::resolve::{Resolved, resolve};

fn load(src: &str) -> (Program, Resolved) {
    let mut map = SourceMap::new();
    let id: SourceId = map.add("walk.ply", src.to_string());
    let mut program = match parse_program([(id, ModuleName::from_dotted("walk"), src)]) {
        Ok(p) => p,
        Err(ds) => panic!("the probe must parse: {ds:#?}\n{src}"),
    };
    let resolved = resolve(&mut program).expect("the probe must resolve");
    (program, resolved)
}

/// Bytes charged to running the probe, the machine built outside the measured region.
fn bytes(src: &str) -> usize {
    let (program, resolved) = load(src);
    let mut machine = Machine::for_program(&program, &resolved);
    let count = machine.test_count();
    assert!(count > 0, "the probe declares no test");
    let (out, _, bytes) = charge(|| {
        let mut ok = true;
        for i in 0..count {
            ok &= machine.eval_test(i).is_ok();
        }
        ok
    });
    assert!(out, "the probe must run, or its bytes measure nothing");
    bytes
}

fn walk(n: usize) -> String {
    format!(
        "fn total(xs: List<Int>, acc: Int) -> Int = match xs {{\n\
         \x20 [] -> acc,\n\
         \x20 [x, ..rest] -> total(rest, acc + x),\n\
         }}\n\
         test \"walk\" {{ assert_eq(total(range(0, {n}), 0), {n} * ({n} - 1) / 2) }}\n"
    )
}

#[test]
fn walking_a_list_by_pattern_moves_bytes_linear_in_its_length() {
    let (b_1k, b_2k) = (bytes(&walk(1000)), bytes(&walk(2000)));
    let ratio = b_2k as f64 / b_1k as f64;
    println!("\n  walk {b_1k:>9} -> {b_2k:>9}  {ratio:.2}x");
    assert!(
        ratio < 3.0,
        "walking twice the list moved {ratio:.2}x the bytes: a `..rest` pattern is copying the \
         tail again"
    );
}
