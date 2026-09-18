#!/usr/bin/env bash
# Runs the differentials, and with --arm the mutations that show they can go red.
#
#   ./crates/ply-compiler-diff/tools/arm.sh            build, test, compare
#   ./crates/ply-compiler-diff/tools/arm.sh --arm      and then run the mutations
#
# CI does not run --arm: each mutation re-runs the whole comparison.
set -euo pipefail

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
echo "==> the differential: this parser against crates/ply-syntax"
# A run that executes nothing exits 0, so require a non-zero pass count.
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
echo "==> the fourth differential: derive.ply against crates/ply-derive"
cargo test -p ply-compiler-diff --test suite -- derive:: --nocapture --test-threads=2 |
  tee /tmp/ply-parser-derive.log
grep -Eq 'test result: ok\. [1-9][0-9]* passed' /tmp/ply-parser-derive.log || {
  echo "the derive differential ran no tests at all -- see the note above" >&2
  exit 1
}

echo
echo "==> the fifth differential: hash.ply against crates/ply-hash"
cargo test -p ply-compiler-diff --test suite -- hash:: --nocapture --test-threads=2 |
  tee /tmp/ply-parser-hash.log
grep -Eq 'test result: ok\. [1-9][0-9]* passed' /tmp/ply-parser-hash.log || {
  echo "the hash differential ran no tests at all -- see the note above" >&2
  exit 1
}

echo
echo "==> what compiling effects would have to carry, and the corpus for it"
cargo test -p ply-compiler-diff --test suite -- effects:: --nocapture


if [ "${1:-}" = "--arm" ]; then
  echo
  echo "==> arming it: twenty-two corruptions of the Ply parser, each seen to go red"
  "$here/arm-harness.sh"
fi
