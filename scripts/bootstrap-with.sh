#!/usr/bin/env bash
# scripts/bootstrap-with.sh DRIVER PROFILE LIBRARY OUT: builds this checkout's `ply` at OUT by running
# the checkout's bootstrap inside the `ply` DRIVER, on DRIVER's own runtime, so the checkout's
# compiler writes `ply`'s C for the runtime's library at LIBRARY. A program cannot ask what it is made
# of, so `ply hash` lists that for it; `ply run` then reuses the front end `ply hash` ran. All DRIVER
# needs is `hash` and `run`.
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
