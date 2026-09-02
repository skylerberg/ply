#!/usr/bin/env bash
# The resolve phase, assembled with the whole parser it reads trees from.
#
#   ./spikes/ply-parser/test-resolve.sh              # the in-language tests
#   ./spikes/ply-parser/test-resolve.sh --keep       # leave the project behind
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
   "$here"/exprs.ply "$here"/items.ply "$here"/resolve.ply "$here"/rewrite.ply \
   "$here"/derive.ply "$here"/tycore.ply "$here"/infer.ply "$work/"
"$ply" test "$work" --no-cache "$@"
[ "$keep" -eq 1 ] && echo "project kept at $work"
exit 0
