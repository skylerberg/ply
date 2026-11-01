#!/usr/bin/env bash
# scripts/bootstrap.sh [--release]: this checkout's `ply`, at `target/debug/ply` or with `--release`
# at `target/release/ply`. Cargo builds the runtime's library, and `bootstrap-with.sh` links the rest,
# run by the `ply` already there or, where there is none, by the last release (`stage0.sh`), which
# drives the first bootstrap only: the script then runs once more, so the toolchain it made —
# which checks far faster than the release — drives, as CI's two bootstraps have it. A stage0 too
# old to read `ply.work` is fetched again; one behind the release is named. PLY_C_PHASES=1 prints
# what each step of a bootstrap took.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
case "${1:-}" in
  --release) dir=release profile=release cargo=(--release) ;;
  "") dir=debug profile=development cargo=() ;;
  *) echo "usage: scripts/bootstrap.sh [--release]" >&2; exit 2 ;;
esac
out="$root/target/$dir/ply"
ply=$out
driving=checkout
if [ ! -x "$ply" ]; then
  ply="$root/target/stage0/ply"
  driving=stage0
  # A stage0 too old to read `ply.work` is no driver; fetch this machine's release again.
  if [ -x "$ply" ] && ! "$ply" work "$root" >/dev/null 2>&1; then
    echo "stage0: the downloaded release cannot read ply.work; fetching a fresh one" >&2
    rm -f "$ply"
  fi
  [ -x "$ply" ] || "$root/scripts/stage0.sh" "$ply" >/dev/null
  # The release rolls with main: name a stage0 it has moved past.
  if updated=$(gh release view main-latest --repo skylerberg/ply --json assets \
      --jq '.assets[0].updatedAt' 2>/dev/null); then
    released=$(date -u -jf "%Y-%m-%dT%H:%M:%SZ" "$updated" +%s 2>/dev/null \
      || date -d "$updated" +%s 2>/dev/null || echo 0)
    modified=$(stat -f %m "$ply" 2>/dev/null || stat -c %Y "$ply")
    if [ "$released" -gt "$modified" ]; then
      echo "stage0: the release is newer than $ply; scripts/stage0.sh refreshes it" >&2
    fi
  fi
fi
# The cargo package `ply.work` names the runtime.
work=$("$ply" work "$root")
runtime=$(awk '$1 == "runtime" { print $2 }' <<< "$work")
library="$root/target/$dir/$(awk '$1 == "library" { print $2 }' <<< "$work")"
cargo build --locked --quiet --manifest-path "$root/Cargo.toml" -p "$runtime" ${cargo[@]+"${cargo[@]}"}
"$root/scripts/bootstrap-with.sh" "$ply" "$profile" "$library" "$out" >&2
if [ "$driving" = stage0 ]; then
  "$root/scripts/bootstrap-with.sh" "$out" "$profile" "$library" "$out" >&2
fi
echo "$out"
