#!/usr/bin/env bash
# Everything this spike can check, in one command. See README.md.
#
# Nothing in `harness/` is reached by `cargo build --workspace`,
# `cargo test --workspace` or `cargo clippy --workspace`, which is why this file
# exists -- `CONTRIBUTING.md` §"Things known to be broken" items 1, 16 and 17
# record what a spike outside the workspace has cost, three times now.
#
#   ./spikes/ply-parser/run.sh            build, test, compare
#   ./spikes/ply-parser/run.sh --arm      and then run the 22 mutations
#
# **Since 2026-08-30 this is also a CI job**: `parser-spike` in
# `.github/workflows/ci.yml`, required through the `ci` aggregate's `needs:`.
# The two sentences above are kept because they predicted the bit-rot that then
# happened -- item 17 is the record. What CI runs is this script WITHOUT
# `--arm`: the 22 mutations cost 299s and stay a by-hand obligation, listed in
# `CONTRIBUTING.md` §"The suite proves less than it looks like it proves".
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"

echo "==> building target/release/ply (the harness shells out to it)"
cargo build --manifest-path "$root/Cargo.toml" --release -p ply-cli --bin ply

echo
echo "==> is the binary the one this tree would produce?"
"$root/.github/binary-is-current.sh"

echo
echo "==> the parser's own tests, in Ply (spine, types, patterns, exprs, items)"
"$here/test-items.sh"

echo
echo "==> the reference dumper's own tests, in Rust"
cd "$here/harness"
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --lib
cargo test --test fields -- --nocapture

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
PLY_BIN="$root/target/release/ply" cargo test --test agreement -- --nocapture --test-threads=2 |
  tee /tmp/ply-parser-agreement.log
grep -Eq 'test result: ok\. [1-9][0-9]* passed' /tmp/ply-parser-agreement.log || {
  echo "the differential ran no tests at all -- see the note above this check" >&2
  exit 1
}

if [ "${1:-}" = "--arm" ]; then
  echo
  echo "==> arming it: twenty-two corruptions of the Ply parser, each seen to go red"
  PLY_BIN="$root/target/release/ply" "$here/arm-harness.sh"
fi
