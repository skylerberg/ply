#!/usr/bin/env bash
# The corpus program built once per source state: `ply build` over `crates/ply-corpus/ply` into OUT,
# named for a digest of PLY, this script and every file under `crates/*/ply`, which hold every
# module and manifest the build reads. The artifact's path is printed; one already built under that
# name is reused, so a run pays the front end once rather than on every `ply run` of the sources.
#
#   benches/corpus-program.sh PLY OUT
set -euo pipefail

ply=$1
out=$2
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

key=$(
  {
    shasum -a 256 <"$ply"
    shasum -a 256 <"${BASH_SOURCE[0]}"
    cd "$root" && find crates/*/ply -type f -print0 | LC_ALL=C sort -z | xargs -0 shasum -a 256
  } | shasum -a 256 | cut -c1-16
)
artifact="$out/corpus-$key.plyx"
if [ ! -f "$artifact" ]; then
  mkdir -p "$out"
  # Renamed into place, so a run never starts a half-written artifact.
  staged="$out/.corpus-$key.$$.plyx"
  trap 'rm -f "$staged"' EXIT
  "$ply" build "$root/crates/ply-corpus/ply" --entry cmd.main -o "$staged" >&2
  mv -f "$staged" "$artifact"
fi
printf '%s\n' "$artifact"
