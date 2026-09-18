#!/usr/bin/env bash
# exprs.ply tests, in a private project so a half-written neighbour cannot redden them.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
src="$root/crates/ply-compiler/ply"
ply="${PLY_BIN:-$root/target/release/ply}"
[ -x "$ply" ] || ply="$root/target/debug/ply"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cp "$src/lexer.ply" "$src/spine.ply" "$src/types.ply" "$src/patterns.ply" \
   "$src/exprs.ply" "$work/"
"$ply" test "$work" --no-cache "$@"
