#!/usr/bin/env bash
# spine.ply tests, in a private project holding only lexer.ply and spine.ply.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
src="$root/crates/ply-compiler/ply"
ply="${PLY_BIN:-$root/target/release/ply}"
[ -x "$ply" ] || ply="$root/target/debug/ply"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cp "$src/lexer.ply" "$src/spine.ply" "$work/"
"$ply" test "$work" --no-cache "$@"
