#!/usr/bin/env bash
# The tests for Area 3 — `exprs.ply` — in a project holding only the modules it
# needs. `ply test crates/ply-compiler` typechecks every module in the directory
# and four agents write into it at once, so a module still being written
# elsewhere would otherwise read as this area going red.
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
