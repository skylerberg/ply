#!/usr/bin/env bash
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
