#!/usr/bin/env bash
# scripts/iterate.sh PATH [FILTER...]: checks and tests the package at PATH without a bootstrap.
#
# It runs `ply check` on the package, then `ply test` with each FILTER passed as `--filter`, which
# selects the tests whose names contain that text. It uses the newest `ply` available:
# `target/debug/ply`, else the downloaded release in `target/stage0`, else the `ply` on PATH.
#
# Inside a checkout, `ply` reads the standard library and the compiler from the checkout's sources.
# An edit to either is therefore checked and tested without a bootstrap. The compiler's tests
# compile their fixtures with the `ply` that runs them, so edits to the parser, checker and
# diagnostics are covered too.
#
# Bootstrap instead when the change affects the `ply` binary itself: a language rule you want to try
# in a real program, the C emitter, the CLI, or a native function that a standard library module
# calls. Also bootstrap to see library changes in the CLI's tests, because the programs those tests
# run use the library embedded in the binary, not the checkout's sources.
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
