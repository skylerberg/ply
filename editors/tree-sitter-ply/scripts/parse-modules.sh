#!/usr/bin/env bash
# Parse every Ply module of the trees `ply work` places with the grammar and fail on a syntax error.
# The grammar is a second reading of the language beside the compiler's, so this is what says the
# two still agree: a rule the language gains or drops shows up here as an `ERROR` or a `MISSING` node.
#
# The fixtures hold inputs written to be refused, so none of them is read. `ply.work` is read with
# the `ply` this checkout built, else the last release (`scripts/stage0.sh`).
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
grammar=$(cd "$here/.." && pwd)
root=$(cd "$grammar/../.." && pwd)

paths=$(mktemp)
trap 'rm -f "$paths"' EXIT

for ply in "$root/target/debug/ply" "$root/target/release/ply" "$root/target/stage0/ply" ""; do
  [ -z "$ply" ] || [ -x "$ply" ] && break
done
[ -n "$ply" ] || { echo "no ply reads ply.work: run scripts/stage0.sh" >&2; exit 2; }
trees=()
while read -r role dir; do
  case "$role" in
    std | std-tests | compiler | cli | cli-tests | package | suite) trees+=("$root/$dir") ;;
  esac
done < <("$ply" work "$root")
find "${trees[@]}" -type f -name '*.ply' ! -path '*/target/*' ! -path '*/bootstrap/*' ! -path '*/.ply-cache/*' |
  sort -u > "$paths"

cd "$grammar"
# `--quiet` prints only a module that has an `ERROR` or `MISSING` node, as it is parsed, and the
# exit status says whether any had one.
if ! npx tree-sitter parse --quiet --paths "$paths"; then
  echo "the grammar does not parse every Ply module above" >&2
  exit 1
fi

echo "parsed $(wc -l < "$paths" | tr -d " ") Ply modules with no syntax error"
