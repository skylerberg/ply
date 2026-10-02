#!/usr/bin/env bash
# ci-warm.sh PLY — the `ply` program and the emitter, built or found once, before a job's tests and
# corpus lanes start several processes that would each build them at once. A test of a scratch
# project is the cheapest run that compiles a unit, so it is the one that reaches the emitter;
# nothing in it is cached. The emitter's line says which way it came.
set -euo pipefail
ply=${1:?usage: ci-warm.sh PLY}
dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT
printf 'test "a unit is compiled" {\n  assert(1 + 1 == 2)\n}\n' > "$dir/m.ply"
PLY_C_PHASES=1 "$ply" test "$dir" > "$dir/out" 2> "$dir/err" || {
  cat "$dir/out" "$dir/err" >&2
  exit 1
}
grep '^phases: emitter' "$dir/err" || echo "no emitter was built: the test compiled nothing" >&2
