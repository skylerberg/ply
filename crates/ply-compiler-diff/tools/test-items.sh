#!/usr/bin/env bash
# The whole parser's in-language tests.
#
#   ./crates/ply-compiler-diff/tools/test-items.sh
#   ./crates/ply-compiler-diff/tools/test-items.sh --keep       # print and keep the project, for diff-items.py and arm-items.sh
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
ply="${PLY_BIN:-$root/target/release/ply}"
[ -x "$ply" ] || ply="$root/target/debug/ply"
[ -x "$ply" ] || { echo "no ply binary: build ply-cli first" >&2; exit 2; }

keep=0
if [ "${1:-}" = "--keep" ]; then keep=1; shift; fi
work="$(mktemp -d)"
[ "$keep" -eq 1 ] || trap 'rm -rf "$work"' EXIT
# Every module: the emitter imports the resolver and its imports, so less does not check.
cp "$here"/*.ply "$work/"
"$ply" test "$work" --no-cache "$@"
[ "$keep" -eq 1 ] && echo "project kept at $work"
exit 0
