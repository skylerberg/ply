#!/usr/bin/env bash
# ci-warm.sh PLY        the `ply` program, built or found once, before a job's tests and corpus
#                       lanes start several processes that would each build it at once. A test of a
#                       scratch project is the cheapest run that emits and compiles a unit; nothing
#                       in it is cached. The emitter's lines say what it read back and was asked.
# ci-warm.sh used MARK  the C cache and the stages cut to what the `ply`s since MARK used: a load
#                       marks what it reads, so the rest is what earlier runs left that this one
#                       did not read, and every job after would restore it for nothing.
set -euo pipefail
cache=${PLY_C_CACHE:-/tmp/ply-c-cache}
stage=${PLY_C_STAGE:-/tmp/ply-c-stage}

megabytes() { du -sm "$@" 2>/dev/null | awk '{ s += $1 } END { print s + 0 }'; }

case "${1:-}" in
  used)
    mark=${2:?usage: ci-warm.sh used MARK}
    [ -f "$mark" ] || { echo "nothing is marked at $mark" >&2; exit 2; }
    before=$(megabytes "$cache" "$stage")
    [ -d "$cache" ] && find "$cache" -type f ! -newer "$mark" -delete
    if [ -d "$stage" ]; then
      for dir in "$stage"/*/; do
        [ -d "$dir" ] || continue
        case "$(basename "$dir")" in
          # Files swept one by one, each a front of its own.
          answered | run-fronts) find "$dir" -type f ! -newer "$mark" -delete ;;
          # A stage goes whole: its `.used` stamp is what a load writes.
          *) [ -n "$(find "$dir" -type f -newer "$mark" -print -quit)" ] || rm -rf "$dir" ;;
        esac
      done
    fi
    find "$cache" "$stage" -mindepth 1 -type d -empty -delete 2>/dev/null || true
    echo "the C cache and stages kept $(megabytes "$cache" "$stage") MB of $before MB"
    ;;
  *)
    ply=${1:?usage: ci-warm.sh PLY | used MARK}
    dir=$(mktemp -d)
    trap 'rm -rf "$dir"' EXIT
    printf 'test "a unit is compiled" {\n  assert(1 + 1 == 2)\n}\n' > "$dir/m.ply"
    PLY_C_PHASES=1 "$ply" test "$dir" > "$dir/out" 2> "$dir/err" || {
      cat "$dir/out" "$dir/err" >&2
      exit 1
    }
    grep '^phases: emitter' "$dir/err" || echo "the emitter was asked nothing: the test compiled nothing" >&2
    ;;
esac
