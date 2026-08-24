#!/usr/bin/env bash
# Everything this spike can check, in one command. See README.md.
#
# Nothing here is reached by `cargo build --workspace`, `cargo test --workspace`
# or `cargo clippy --workspace`, which is why this file exists.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"

echo "==> building target/release/ply (the harness shells out to it)"
cargo build --manifest-path "$root/Cargo.toml" --release -p ply-cli --bin ply

echo
echo "==> the lexer's own tests, in Ply"
"$root/target/release/ply" test "$here/lexer.ply" --no-cache

echo
echo "==> the differential harness: this lexer against crates/ply-syntax"
cd "$here/harness"
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
PLY_BIN="$root/target/release/ply" cargo test -- --nocapture
