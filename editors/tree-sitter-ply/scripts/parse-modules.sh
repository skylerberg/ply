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
out=$(npx tree-sitter parse --paths "$paths" 2>&1) || true

if grep -qE '\((ERROR|MISSING)' <<< "$out"; then
  grep -E '\((ERROR|MISSING)' <<< "$out" | head -20 >&2
  echo "the grammar does not parse every Ply module above" >&2
  exit 1
fi

echo "parsed $(wc -l < "$paths" | tr -d " ") Ply modules with no syntax error"
