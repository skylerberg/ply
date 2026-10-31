#!/usr/bin/env bash
# scripts/bootstrap.sh [--release]: builds this checkout's `ply` at `target/debug/ply`, or at
# `target/release/ply` with `--release`.
#
# Cargo builds the runtime library, then `ply bootstrap` builds the rest. `ply bootstrap` is run by
# a `ply` that already exists. The first run uses the `ply` already at the output path. If there is
# none, it uses the latest release, which `stage0.sh` downloads into `target/stage0`.
#
# The script then bootstraps a second time, using the `ply` it just built. That `ply` is the
# checkout's own and type-checks faster than the release. CI also bootstraps twice.
#
# If the downloaded release is too old to read `ply.work`, the script downloads it again. If a newer
# release exists than the one downloaded, the script prints a warning.
#
# Set `PLY_C_PHASES=1` to print how long each step of a bootstrap took.
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
  if [ -x "$ply" ] && ! "$ply" work "$root" >/dev/null 2>&1; then
    echo "stage0: the downloaded release cannot read ply.work; fetching a fresh one" >&2
    rm -f "$ply"
  fi
  [ -x "$ply" ] || "$root/scripts/stage0.sh" "$ply" >/dev/null
  # The `main-latest` release is republished on every change to main. Warns when it is newer than
  # the downloaded stage0 `ply`.
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
runtime=$("$ply" work "$root" | awk '$1 == "runtime" { print $2 }')
cargo build --locked --quiet --manifest-path "$root/Cargo.toml" -p "$runtime" ${cargo[@]+"${cargo[@]}"}
"$ply" bootstrap "$root" --profile "$profile" --out "$out" >&2
if [ "$driving" = stage0 ]; then
  "$out" bootstrap "$root" --profile "$profile" --out "$out" >&2
fi
echo "$out"
