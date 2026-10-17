#!/usr/bin/env bash
# scripts/bootstrap.sh [--release]: this checkout's `ply`, at `target/debug/ply` or with `--release`
# at `target/release/ply`. Cargo builds the runtime's library, and `ply bootstrap` links the rest,
# run by the `ply` already there or, where there is none, by the last release (`stage0.sh`).
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
case "${1:-}" in
  --release) dir=release profile=release cargo=(--release) ;;
  "") dir=debug profile=development cargo=() ;;
  *) echo "usage: scripts/bootstrap.sh [--release]" >&2; exit 2 ;;
esac
out="$root/target/$dir/ply"
ply=$out
if [ ! -x "$ply" ]; then
  ply="$root/target/stage0/ply"
  [ -x "$ply" ] || "$root/scripts/stage0.sh" "$ply" >/dev/null
fi
# The cargo package `ply.work` names the runtime.
runtime=$("$ply" work "$root" | awk '$1 == "runtime" { print $2 }')
cargo build --locked --quiet --manifest-path "$root/Cargo.toml" -p "$runtime" ${cargo[@]+"${cargo[@]}"}
"$ply" bootstrap "$root" --profile "$profile" --out "$out" >&2
echo "$out"
