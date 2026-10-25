#!/usr/bin/env bash
# scripts/iterate.sh PATH [FILTER...]: the edit loop that needs no bootstrap: `ply check` the
# package at PATH, then `ply test` it with each FILTER as a --filter substring of a test key, run
# by the newest `ply` the checkout already has (the bootstrap in target/, else the last release,
# else the one on PATH). A load inside the checkout reads the standard library and the compiler
# from it, so an edit to either is checked and tested here, and the compiler's tests compile their
# fixtures with the `ply` that runs them, so a parser, checker or diagnostic change is too; a
# bootstrap is for what `ply` itself must do anew: a language rule in a real program, the emitter,
# the CLI, a native a shipped module calls, and what a test's own loads read of the library, as
# the CLI's suite's do.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
case "${1:-}" in
  ""|-h|--help) echo "usage: scripts/iterate.sh PATH [FILTER...]" >&2; exit 2 ;;
esac
path=$1
shift
ply="$root/target/debug/ply"
if [ ! -x "$ply" ]; then ply="$root/target/stage0/ply"; fi
if [ ! -x "$ply" ]; then ply="$(command -v ply)"; fi
echo "iterate: $ply" >&2
"$ply" check "$root/$path"
args=()
for f in "$@"; do args+=(--filter "$f"); done
"$ply" test "$root/$path" ${args[@]+"${args[@]}"}
