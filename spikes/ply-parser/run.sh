#!/usr/bin/env bash
# Everything this spike can check, in one command. See README.md.
#
# Nothing in `harness/` is reached by `cargo build --workspace`,
# `cargo test --workspace` or `cargo clippy --workspace`, which is why this file
# exists -- `CONTRIBUTING.md` §"Things known to be broken" item 1 records what a
# spike outside the workspace cost the last two times.
#
#   ./spikes/ply-parser/run.sh            build, test, compare
#   ./spikes/ply-parser/run.sh --arm      and then run the 16 mutations
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
PLY_BIN="$root/target/release/ply" cargo test --test agreement -- --nocapture --test-threads=2

if [ "${1:-}" = "--arm" ]; then
  echo
  echo "==> arming it: sixteen corruptions of the Ply parser, each seen to go red"
  PLY_BIN="$root/target/release/ply" "$here/arm-harness.sh"
fi
