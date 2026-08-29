#!/usr/bin/env bash
# The Items area, assembled with everything it depends on: the spine, and the
# three sorts below it in the AST's DAG. `items.ply` reaches all three, so
# unlike `test-spine.sh` this is the whole parser.
#
#   ./spikes/ply-parser/test-items.sh              # 112 in-language tests
#   ./spikes/ply-parser/test-items.sh --keep       # leave the project behind
#
# With --keep it prints the directory, which `diff-items.py` and `arm-items.sh`
# both take as their argument.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
ply="${PLY_BIN:-$root/target/release/ply}"
[ -x "$ply" ] || ply="$root/target/debug/ply"
[ -x "$ply" ] || { echo "no ply binary: build ply-cli first" >&2; exit 2; }

keep=0
if [ "${1:-}" = "--keep" ]; then keep=1; shift; fi
work="$(mktemp -d)"
[ "$keep" -eq 1 ] || trap 'rm -rf "$work"' EXIT
cp "$here"/lexer.ply "$here"/spine.ply "$here"/types.ply "$here"/patterns.ply \
   "$here"/exprs.ply "$here"/items.ply "$work/"
"$ply" test "$work" --no-cache "$@"
[ "$keep" -eq 1 ] && echo "project kept at $work"
exit 0
