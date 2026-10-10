#!/usr/bin/env bash
# Parse every Ply module in this tree with the grammar and fail on a syntax error. The grammar is a
# second reading of the language beside `crates/ply-compiler/ply`, so this is what says the two
# still agree: a rule the language gains or drops shows up here as an `ERROR` or a `MISSING` node.
#
# The `phases/` fixtures and `tests/fixtures/` hold inputs written to be refused, and the lexer
# fixtures under `phases/lexer/` are token fragments rather than programs, so none of them is read.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
grammar=$(cd "$here/.." && pwd)
root=$(cd "$grammar/../.." && pwd)

paths=$(mktemp)
trap 'rm -f "$paths"' EXIT

find "$root/crates" "$root/examples" "$root/tests/lang" "$root/benches" \
  -type f -name '*.ply' \
  ! -path '*/target/*' ! -path '*/bootstrap/*' ! -path '*/phases/*' \
  ! -path "$root/tests/fixtures/*" > "$paths"

cd "$grammar"
out=$(npx tree-sitter parse --paths "$paths" 2>&1) || true

if grep -qE '\((ERROR|MISSING)' <<< "$out"; then
  grep -E '\((ERROR|MISSING)' <<< "$out" | head -20 >&2
  echo "the grammar does not parse every Ply module above" >&2
  exit 1
fi

echo "parsed $(wc -l < "$paths" | tr -d " ") Ply modules with no syntax error"
