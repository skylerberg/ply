#!/usr/bin/env bash
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
if ! npx tree-sitter parse --quiet --paths "$paths"; then
  echo "the grammar does not parse every Ply module above" >&2
  exit 1
fi

echo "parsed $(wc -l < "$paths" | tr -d " ") Ply modules with no syntax error"
