#!/usr/bin/env bash
# The corpus's Ply runs (`ci-shards.sh`'s corpus entries), each `ply test` over the corpus with the
# grants `benches/corpus.sh` gives the program. A run fails when a test fails, when the run refuses,
# and when its filter selects no test, so a renamed test never stops being run quietly.
#
#   ci-corpus.sh partition K TIMINGS [CUT]
#       every run partition K takes (`ci-shards.sh corpus-for-partition K [CUT]`), each lane one
#       process beside the others. A lane's checks go in one `ply test` with a `--filter` each, so
#       the checks package's closure is loaded once a lane rather than once a run. Each run's
#       milliseconds are appended to TIMINGS as `corpus <run> <ms>`: a check's are its tests' own,
#       out of the report. With the program's own tests go those of the programs under `fixtures/`
#       it runs, each of which must pass a test.
#   ci-corpus.sh run ID [ARG...]       one run, with ARGs added to its `ply test`
#   ci-corpus.sh mark                  the moment `keep` gathers from
#   ci-corpus.sh keep DIR              the C `ply` emitted and compiled since `mark`, into DIR for a
#                                      later run: a body is keyed by its definition, the emitter and
#                                      the runtime's sources, so another tree reuses what still
#                                      applies. The packages' stores stay out: they hold test results,
#                                      which a runtime a later run builds could not vouch for.
#   ci-corpus.sh restore DIR           a kept DIR merged under what `ply` reads, keeping what is there
set -uo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ply="$root/target/debug/ply"
shards="$root/.github/ci-shards.sh"
# The caches `ply` reads, where the workflow restores them.
caches=/tmp
mark=$caches/ply-c-corpus.mark

# What a checks run is given after `ply test PATH`.
grants=(--host --timeout 900000 --steps 0 --json
  --exec "ply=$ply" --allow machine --allow claims --fs work=. --fs "repo=$root")

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
    "$ply" test "$path" ${filter:+--filter "$filter"} "${grants[@]}" "$@" > "$out" || status=$?
  fi
  jq -r '.results[]? | "\(.status)\t\(.key // .name)"' "$out" 2>/dev/null
  selected=$(jq -s 'map(.results // [] | length) | add // 0' "$out" 2>/dev/null || echo 0)
  if [ "$status" -ne 0 ] || [ "$selected" -eq 0 ]; then
    [ "$selected" -gt 0 ] || echo "corpus run $id selected no test (filter: ${filter:-none})" >&2
    cat "$out"
    rm -f "$out"
    return 1
  fi
  rm -f "$out"
}

# The checks runs IDs, in one `ply test`: each run's milliseconds are the summed durations of the
# tests whose `<module>.<label>` key its filter holds, and a run whose filter selected none fails.
run_checks() {
  local timings=$1 id line path filter status=0 out bad=0 n ms i
  local -a filters=() ids=() args=()
  shift
  for id in "$@"; do
    line=$("$shards" corpus-line "$id") || return 2
    read -r path filter <<< "$line"
    ids+=("$id")
    filters+=("$filter")
    args+=(--filter "$filter")
  done
  out=$(mktemp)
  "$ply" test "$path" "${args[@]}" "${grants[@]}" > "$out" || status=$?
  jq -r '.results[]? | "\(.status)\t\(.key // .name)"' "$out" 2>/dev/null
  for i in "${!ids[@]}"; do
    n=$(jq --arg f "${filters[$i]}" '[.results[]? | select(.key | contains($f))] | length' "$out" 2>/dev/null || echo 0)
    ms=$(jq --arg f "${filters[$i]}" '[.results[]? | select(.key | contains($f)) | .duration_ms] | add // 0 | floor' "$out" 2>/dev/null || echo 0)
    if [ "$n" -eq 0 ]; then
      echo "corpus run ${ids[$i]} selected no test (filter: ${filters[$i]})" >&2
      bad=1
    fi
    printf 'corpus\t%s\t%s\n' "${ids[$i]}" "$ms" >> "$timings"
  done
  if [ "$status" -ne 0 ] || [ "$bad" -ne 0 ]; then
    cat "$out"
    rm -f "$out"
    return 1
  fi
  rm -f "$out"
}

fixtures() {
  local fixture failed=0 out
  out=$(mktemp)
  for fixture in "$root"/crates/ply-corpus/fixtures/*.ply; do
    if ! "$ply" test "$fixture" --no-cache --json > "$out"; then
      cat "$out"
      failed=1
    elif ! jq -e '.summary.passed > 0' "$out" > /dev/null; then
      echo "$fixture tested nothing" >&2
      failed=1
    fi
  done
  rm -f "$out"
  return "$failed"
}

# One lane's runs, one after another: the program's own and each package's suite in a `ply test` of
# their own, and every checks run in one.
lane() {
  local timings=$1 id started failed=0
  local -a checks=()
  shift
  : > "$timings"
  for id in "$@"; do
    case "$id" in
      program | package-*)
        echo "::group::corpus $id"
        started=$(date +%s%3N)
        run_one "$id" || failed=1
        if [ "$id" = program ]; then fixtures || failed=1; fi
        printf 'corpus\t%s\t%s\n' "$id" "$(($(date +%s%3N) - started))" >> "$timings"
        echo "::endgroup::"
        ;;
      *) checks+=("$id") ;;
    esac
  done
  if [ "${#checks[@]}" -gt 0 ]; then
    echo "::group::corpus checks ${checks[*]}"
    run_checks "$timings" "${checks[@]}" || failed=1
    echo "::endgroup::"
  fi
  return "$failed"
}

case "${1:-}" in
  partition)
    shard=${2:?a partition}
    timings=${3:?a file for the durations}
    runs=$("$shards" corpus-for-partition "$shard" "${4:-}") || exit 2
    : > "$timings"
    work=$(mktemp -d)
    lanes=$(cut -d' ' -f1 <<< "$runs" | sort -un)
    pids=()
    for l in $lanes; do
      read -ra ids <<< "$(awk -v l="$l" '$1 == l { printf "%s ", $2 }' <<< "$runs")"
      lane "$work/lane-$l.tsv" "${ids[@]}" > "$work/lane-$l.log" 2>&1 &
      pids+=("$!")
    done
    failed=0
    for pid in "${pids[@]}"; do wait "$pid" || failed=1; done
    for l in $lanes; do
      echo "=== corpus lane $l"
      cat "$work/lane-$l.log"
      cat "$work/lane-$l.tsv" >> "$timings"
    done
    rm -rf "$work"
    exit "$failed"
    ;;
  run)
    run_one "${2:?a corpus entry}" "${@:3}"
    ;;
  mark)
    touch "$mark"
    ;;
  keep)
    dir=${2:?a directory}
    [ -f "$mark" ] || { echo "nothing is marked: run 'ci-corpus.sh mark' before the runs" >&2; exit 2; }
    rm -rf "$dir"
    mkdir -p "$dir"
    (cd "$caches" && find ply-c-cache ply-c-stage -type f -newer "$mark" -print0 2>/dev/null |
      tar --null -T - -cf -) | tar -xf - -C "$dir" || exit 1
    du -sh "$dir"
    ;;
  restore)
    dir=${2:?a directory}
    [ -d "$dir" ] || exit 0
    tar -C "$dir" -cf - . | tar -C "$caches" --skip-old-files -xf -
    ;;
  *)
    echo "usage: ci-corpus.sh partition K TIMINGS [CUT] | run ID [ARG...] | mark | keep DIR | restore DIR" >&2
    exit 2
    ;;
esac
