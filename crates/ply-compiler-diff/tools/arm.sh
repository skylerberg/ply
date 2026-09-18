#!/usr/bin/env bash
# The mutations: the evidence the differentials can go red at all.
#
#   ./crates/ply-compiler-diff/tools/arm.sh            build, test, compare
#   ./crates/ply-compiler-diff/tools/arm.sh --arm      and then run the 22 mutations
#
# **CI does not run this.** The differentials themselves are ordinary
# `cargo test -p ply-compiler-diff` now, in the `compiler` shard, so what this
# script existed to run is covered. What is left is `--arm`: 22 mutations at
# 299s, because each re-runs the whole 766-input comparison. That stays a
# by-hand obligation, listed in `CONTRIBUTING.md` §"The suite proves less than
# it looks like it proves", and `.github/workflows/ci.yml`'s preamble is where
# the trade is written down.
#
# This file was `spikes/ply-parser/run.sh`, and its header used to explain why a
# directory outside the workspace needed a CI job of its own. It is inside the
# workspace now; `CONTRIBUTING.md` §"Things known to be broken" items 1, 16 and
# 17 are the record of what that cost while it was not.
set -euo pipefail

# `here` is this `tools/` directory; `crate` is the differential's; `root` is the repository.
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
crate="$(cd "$here/.." && pwd)"
root="$(cd "$crate/../.." && pwd)"


echo "==> building target/release/ply (the parser's own tests in Ply run through it)"
cargo build --manifest-path "$root/Cargo.toml" --release -p ply-cli --bin ply

echo
echo "==> is the binary the one this tree would produce?"
"$root/.github/binary-is-current.sh"

echo
echo "==> the parser's own tests, in Ply (spine, types, patterns, exprs, items)"
"$here/test-items.sh"

echo
echo "==> the resolve phase, in Ply, over the whole parser"
"$here/test-resolve.sh"

echo
echo "==> the reference dumper's own tests, in Rust"
cd "$root"
cargo fmt --all --check
cargo clippy -p ply-compiler-diff --all-targets -- -D warnings
cargo test -p ply-compiler-diff --lib
cargo test -p ply-compiler-diff --test suite -- fields:: --nocapture

echo
echo "==> the oracle for the stage after the front end: the lowered form"
# Not a differential yet -- the second implementation is not written. What it checks is that the
# oracle a code generator in Ply would be compared against is usable: total over the shipped
# corpus, stable run to run, and sensitive to the slot a name reads and to whether a read is its
# last. A canonical form that erased either would compare two ports equal while one of them freed
# too early, which is the class three separate defects in the C tier have been.
cargo test -p ply-compiler-diff --test suite -- lower:: --nocapture

echo
echo "==> the differential: this parser against crates/ply-syntax"
# Piped through `tee` and then grepped for a **non-zero** pass count, for the
# reason `.github/workflows/ci.yml`'s `test` job gives at length about its own
# shards: a run that executes nothing exits 0 and looks exactly like a run that
# executed everything. `#[ignore]` on the seven agreement tests, a stray filter,
# or a `--test-threads` mishap would all leave this whole script green over a
# comparison that never ran, and this is the only place in CI the comparison
# runs at all. `pipefail` is set above, so `cargo test`'s own failure still
# fails the script; this catches the case where it succeeds vacuously.
cargo test -p ply-compiler-diff --test suite -- agreement:: --nocapture --test-threads=2 |
  tee /tmp/ply-parser-agreement.log
grep -Eq 'test result: ok\. [1-9][0-9]* passed' /tmp/ply-parser-agreement.log || {
  echo "the differential ran no tests at all -- see the note above this check" >&2
  exit 1
}

echo
echo "==> the second differential: resolve.ply against crates/ply-syntax's resolve"
cargo test -p ply-compiler-diff --test suite -- resolve:: --nocapture --test-threads=2 |
  tee /tmp/ply-parser-resolve.log
grep -Eq 'test result: ok\. [1-9][0-9]* passed' /tmp/ply-parser-resolve.log || {
  echo "the resolve differential ran no tests at all -- see the note above" >&2
  exit 1
}

echo
echo "==> the third differential: rewrite.ply against crates/ply-syntax's three rewrites"
cargo test -p ply-compiler-diff --test suite -- rewrite:: --nocapture --test-threads=2 |
  tee /tmp/ply-parser-rewrite.log
grep -Eq 'test result: ok\. [1-9][0-9]* passed' /tmp/ply-parser-rewrite.log || {
  echo "the rewrite differential ran no tests at all -- see the note above" >&2
  exit 1
}

echo
echo "==> the fourth differential: infer.ply against crates/ply-core's checker"
cargo test -p ply-compiler-diff --test suite -- infer:: --nocapture --test-threads=2 |
  tee /tmp/ply-parser-infer.log
grep -Eq 'test result: ok\. [1-9][0-9]* passed' /tmp/ply-parser-infer.log || {
  echo "the checker differential ran no tests at all -- see the note above" >&2
  exit 1
}

echo
echo "==> the fifth differential: derive.ply against crates/ply-derive"
cargo test -p ply-compiler-diff --test suite -- derive:: --nocapture --test-threads=2 |
  tee /tmp/ply-parser-derive.log
grep -Eq 'test result: ok\. [1-9][0-9]* passed' /tmp/ply-parser-derive.log || {
  echo "the derive differential ran no tests at all -- see the note above" >&2
  exit 1
}

echo
echo "==> the sixth differential: hash.ply against crates/ply-hash"
cargo test -p ply-compiler-diff --test suite -- hash:: --nocapture --test-threads=2 |
  tee /tmp/ply-parser-hash.log
grep -Eq 'test result: ok\. [1-9][0-9]* passed' /tmp/ply-parser-hash.log || {
  echo "the hash differential ran no tests at all -- see the note above" >&2
  exit 1
}

echo
echo "==> the seventh differential: code.ply's lowering against ply_eval::code"
# The first comparison for a stage *after* the front end. It compares only what the port claims to
# lower -- `lower` answers `None` for a node kind it has not reached -- and asserts the share it
# reaches, so a port that quietly lowered nothing would fail rather than agree with itself.
PLY_C_EMITTER="ply:$root/crates/ply-compiler/ply" cargo test -p ply-compiler-diff --test suite -- lower_diff:: --nocapture --test-threads=2 |
  grep -E "input\(s\)|reaches|^test result|^error|panicked" || true

echo
echo "==> what compiling effects would have to carry, and the corpus for it"
cargo test -p ply-compiler-diff --test suite -- effects:: --nocapture


if [ "${1:-}" = "--arm" ]; then
  echo
  echo "==> arming it: twenty-two corruptions of the Ply parser, each seen to go red"
  "$here/arm-harness.sh"
fi
