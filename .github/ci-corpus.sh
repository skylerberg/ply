#!/usr/bin/env bash
# The corpus's Ply runs (`ci-shards.sh`'s corpus entries), each `ply test` over the corpus with the
# grants `benches/corpus.sh` gives the program. A run fails when a test fails, when the run refuses,
# and when its filter selects no test, so a renamed test never stops being run quietly.
#
#   ci-corpus.sh partition K TIMINGS   every run partition K takes, one after another; each run's
#                                      milliseconds are appended to TIMINGS as `corpus <run> <ms>`,
#                                      and with the program's own tests go those of the programs
#                                      under `fixtures/` it runs, each of which must pass a test
#   ci-corpus.sh run ID [ARG...]       one run, with ARGs added to its `ply test`
set -uo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ply="$root/target/debug/ply"
shards="$root/.github/ci-shards.sh"

run_one() {
  local id=$1 line path filter status=0 selected out
  shift
  line=$("$shards" corpus-line "$id") || return 2
  read -r path filter <<< "$line"
  out=$(mktemp)
  if [[ $id == package-* ]]; then
    # A package's own suite runs as `ply test` runs it: the corpus's grants are for the corpus.
    "$ply" test "$path" --json "$@" > "$out" || status=$?
  else
    "$ply" test "$path" ${filter:+--filter "$filter"} --host --timeout 900000 --steps 0 --json \
      --exec "ply=$ply" --allow machine --allow claims --fs work=. --fs "repo=$root" "$@" \
      > "$out" || status=$?
  fi
  jq -r '.results[]? | "\(.status)\t\(.name)"' "$out" 2>/dev/null
  selected=$(jq -s 'map(.results // [] | length) | add // 0' "$out" 2>/dev/null || echo 0)
  if [ "$status" -ne 0 ] || [ "$selected" -eq 0 ]; then
    [ "$selected" -gt 0 ] || echo "corpus run $id selected no test (filter: ${filter:-none})" >&2
    cat "$out"
    rm -f "$out"
    return 1
  fi
  rm -f "$out"
}

fixtures() {
  local fixture failed=0
  for fixture in "$root"/crates/ply-corpus/fixtures/*.ply; do
    if ! "$ply" test "$fixture" --no-cache --json > /tmp/fixture.json; then
      cat /tmp/fixture.json
      failed=1
    elif ! jq -e '.summary.passed > 0' /tmp/fixture.json > /dev/null; then
      echo "$fixture tested nothing" >&2
      failed=1
    fi
  done
  return "$failed"
}

case "${1:-}" in
  partition)
    shard=${2:?a partition}
    timings=${3:?a file for the durations}
    runs=$("$shards" corpus-for-partition "$shard") || exit 2
    failed=0
    : > "$timings"
    for id in $runs; do
      echo "::group::corpus $id"
      started=$(date +%s%3N)
      run_one "$id" || failed=1
      if [ "$id" = program ]; then fixtures || failed=1; fi
      printf 'corpus\t%s\t%s\n' "$id" "$(($(date +%s%3N) - started))" >> "$timings"
      echo "::endgroup::"
    done
    exit "$failed"
    ;;
  run)
    run_one "${2:?a corpus entry}" "${@:3}"
    ;;
  *)
    echo "usage: ci-corpus.sh partition K TIMINGS | run ID [ARG...]" >&2
    exit 2
    ;;
esac
