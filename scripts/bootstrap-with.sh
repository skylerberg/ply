#!/usr/bin/env bash
set -euo pipefail
if [ $# -ne 4 ]; then
  echo "usage: scripts/bootstrap-with.sh DRIVER PROFILE LIBRARY OUT" >&2
  exit 2
fi
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
absolute() { case $1 in /*) printf '%s\n' "$1" ;; *) printf '%s/%s\n' "$PWD" "$1" ;; esac; }
driver=$1 profile=$2 library=$(absolute "$3") out=$(absolute "$4")
program="$root/packages/bootstrap"
hashes=$(mktemp)
trap 'rm -f "$hashes"' EXIT
if ! "$driver" hash --json "$program" > "$hashes"; then
  cat "$hashes" >&2
  exit 1
fi
"$driver" run "$program" --profile release --allow shipped --allow toolchain --fs abs=/ \
  -- "$root" "$profile" "$hashes" "$library" "$out"
