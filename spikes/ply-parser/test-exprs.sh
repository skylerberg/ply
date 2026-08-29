#!/usr/bin/env bash
# The tests for Area 3 — `exprs.ply` — in a project holding only the modules it
# needs. `ply test spikes/ply-parser` typechecks every module in the directory
# and four agents write into it at once, so a module still being written
# elsewhere would otherwise read as this area going red.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
ply="${PLY_BIN:-$root/target/release/ply}"
[ -x "$ply" ] || ply="$root/target/debug/ply"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cp "$here/lexer.ply" "$here/spine.ply" "$here/types.ply" "$here/patterns.ply" \
   "$here/exprs.ply" "$work/"
"$ply" test "$work" --no-cache "$@"
