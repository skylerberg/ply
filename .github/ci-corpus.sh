#!/usr/bin/env bash
# The Ply runs (`ci-shards.sh`'s corpus entries), each a `ply test`: over the corpus with the grants
# `benches/corpus.sh` gives the program, or over the CLI's suite with the grants its harness drives
# `ply` with, from a scratch directory of the run's own. A run fails when a test fails, when the run
# refuses, and when its filter selects no test, so a renamed test never stops being run quietly.
#
#   ci-corpus.sh partition K TIMINGS [CUT]
#       every run partition K takes (`ci-shards.sh corpus-for-partition K [CUT]`), each lane one
#       process beside the others. A lane's runs of one package go in one `ply test` with a
#       `--filter` each, so the package's closure is loaded once a lane rather than once a run. Each
#       run's milliseconds are appended to TIMINGS as `corpus <run> <ms>`: a module's are its tests'
#       own, out of the report. Each `ply test` adds `cached <what> <n>`, the tests it took from the
#       cache, which tells `ci-shards.sh timings` whether the run's costs are a cold run's.
#   ci-corpus.sh desks K TIMINGS CUT [ARG...]
#       the desk runs runner K takes (`ci-shards.sh desks-for-runner K CUT`, round robin when CUT is
#       empty) in one `ply test` with ARGs added, their milliseconds onto TIMINGS as a partition's
#   ci-corpus.sh run ID [ARG...]       one run, with ARGs added to its `ply test`
#   ci-corpus.sh mark                  the moment `keep` gathers from
#   ci-corpus.sh keep DIR              the bodies and the compiler's answers `ply` emitted or read
#                                      since `mark`, into DIR for a later run: a body is keyed by its
#                                      definition, the emitter and the runtime's sources, and an answer
#                                      by the emitter and its question, so another tree reuses what
#                                      still applies. Objects stay out: one compiles from its bodies in
#                                      seconds, and they were most of what a lane kept. The stages are
#                                      build-ply's to ship, and the packages' stores are carried apart.
#   ci-corpus.sh pack TAR              what a partition leaves the next run: `keep`'s C, and each package
#                                      store its runs wrote since `mark`, compacted
#   ci-corpus.sh unpack DIR C          every partition's TAR under DIR as one: their C merged into C,
#                                      and each store written over the checkout's, so a later partition
#                                      finds every package's whichever partition the cut gave it
#   ci-corpus.sh restore DIR           a kept DIR merged under what `ply` reads, keeping what is there
#   ci-corpus.sh compact               every package's store compacted before a job saves them: a
#                                      store only grows, and every later job restores what one saves
#   ci-corpus.sh upstream-mark         the moment `upstream-new` gathers from
#   ci-corpus.sh upstream-new TAR      what this job published to `PLY_CACHE_UPSTREAM` since the mark
#   ci-corpus.sh upstream-merge DIR    every job's TAR under DIR merged, keeping this run's runtimes
set -uo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ply="$root/target/debug/ply"
shards="$root/.github/ci-shards.sh"
cli_suite=crates/ply-cli-tests/ply
# The caches `ply` reads, where the workflow restores them.
caches=/tmp
mark=$caches/ply-c-corpus.mark
upstream=$caches/ply-upstream
upstream_mark=$caches/ply-upstream.mark
export PLY_CACHE_UPSTREAM=$upstream
# `ms<TAB>package<TAB>key` per test this job ran, which the job uploads for the run's table of what
# each test cost. A lane writes its own beside its timings, and `partition` gathers them here.
durations=$caches/ply-test-durations.tsv

# What a checks run is given after `ply test PATH`.
grants=(--host --timeout 900000 --steps 0 --json
  --exec "ply=$ply" --allow machine --allow claims --allow shipped --fs work=. --fs "repo=$root")

# What a run of the CLI's suite is given: the `ply` its tests start and the programs they start beside
# it, its scratch directory and the filesystem, the repository, and every family a command drives a
# machine with, since its tests run the commands in this process.
cli_grants=(--host --timeout 900000 --steps 0 --json
  --exec "ply=$ply" --exec sh=/bin/sh --exec "git=$(command -v git)"
  --fs cwd=. --fs abs=/ --fs "repo=$root"
  --allow machine --allow tester --allow claims --allow hosts
  --allow shipped)

# `ply test` over the package at PATH (relative to the repository) with ARGs: the CLI's suite from a
# directory of its own, which its harness empties before each test and refuses without the marker.
tested() {
  local path=$1 status=0 dir
  shift
  if [[ $path == "$cli_suite" ]]; then
    dir=$(mktemp -d)
    touch "$dir/.ply-scratch"
    (cd "$dir" && "$ply" test "$root/$path" "$@" "${cli_grants[@]}") || status=$?
    rm -rf "$dir"
  else
    "$ply" test "$path" "$@" "${grants[@]}" || status=$?
  fi
  return "$status"
}

# What a red run said: each failure's test and diagnostic, and a refused run's diagnostics. The report
# itself is one line too long for a log to show.
red() {
  jq -r '
    (.failures[]? | ("FAILED \(.key // .name): \(.diagnostic.code // "") \(.diagnostic.message // "")",
      (.diagnostic.notes[]? | "  \(.)"))),
    (.diagnostics[]? | "\(.severity // "error") \(.code // ""): \(.message // "")")
  ' "$1" 2>/dev/null || cat "$1"
}

# `ms<TAB>package<TAB>key` per test the report at $1, a run of the package at $2, ran.
timed() {
  jq -r --arg p "$2" '.results[]? | "\((.duration_ms // 0) | floor)\t\($p)\t\(.key // .name)"' "$1" 2>/dev/null
}

# Each test a run ran, with how it ended, its seconds, why it ran (the read that moved, for a
# `changed` one) and `unfiled` for a pass no trace stands in for; and each it took from the cache.
listed() {
  jq -r '(.selection.tests // [] | map({key: .key, value: (.reason + (if .moved then ": " + .moved else "" end))}) | from_entries) as $why
    | (.results[]? | "\(.status)\t\(((.duration_ms // 0) / 100 | floor) / 10)s\t\(.key // .name)\t\($why[.key // .name] // "")\(if .cached == false then " (unfiled)" else "" end)"),
      (.selection.tests[]? | select(.reason == "cached") | "cached\t\t\(.key)")' "$1" 2>/dev/null
}

# The row saying how many tests the report at $1 took from the cache, under $2, onto $3 if one is named.
cached_row() {
  [[ -n $3 ]] || return 0
  printf 'cached\t%s\t%s\n' "$2" "$(jq -s 'map(.summary.cached // 0) | add // 0' "$1" 2>/dev/null || echo 0)" >> "$3"
}

# Where a `ply test` spent its wall clock WALL (ms), from its report: the phases its clock, which
# counts from the process's start, measured, and `other` for what follows the report.
spent() {
  jq -r --argjson wall "$2" '
    def s(ms): (ms / 1000 | floor | tostring) + "s";
    (.phases // {}) as $p
    | "spent: before \(s($p.before // 0)), load \(s($p.load // 0)), promises \(s($p.promises // 0)), store \(s($p.store // 0)), selection \(s($p.selection // 0)), handoff \(s($p.handoff // 0)), unit \(s($p.unit // 0)) (C analysis \(s((.backend.analysis_nanos // 0) / 1e6)), codegen \(s((.backend.codegen_nanos // 0) / 1e6))), bind \(s($p.bind // 0)), tests \(s($p.tests // 0)), conclude \(s($p.conclude // 0)), other \(s([$wall - ($p.total // 0), 0] | max))"
  ' "$1" 2>/dev/null
}

# The run ID with ARGs added, its `cached` row onto TIMINGS when that is not empty.
run_one() {
  local timings=$1 id=$2 line path filter status=0 selected out started
  shift 2
  line=$("$shards" corpus-line "$id") || return 2
  read -r path filter <<< "$line"
  out=$(mktemp)
  started=$(date +%s%3N)
  if [[ $id == package-* || $id == fixture-* ]]; then
    # A package's own suite and a fixture run as `ply test` runs them: the corpus's grants are for
    # the corpus.
    "$ply" test "$path" --json "$@" > "$out" || status=$?
  elif [[ $id == stdlib ]]; then
    "$ply" test --std "$path" --json "$@" > "$out" || status=$?
    local proved
    if ! proved=$("$ply" prove --std "$path" 2>&1); then
      printf '%s\n' "$proved" >&2
      status=1
    fi
  else
    tested "$path" ${filter:+--filter "$filter"} "$@" > "$out" || status=$?
  fi
  listed "$out"
  timed "$out" "$path" >> "$durations"
  spent "$out" "$(($(date +%s%3N) - started))"
  cached_row "$out" "$id" "$timings"
  selected=$(jq -s 'map(.selection.tests // [] | length) | add // 0' "$out" 2>/dev/null || echo 0)
  if [ "$status" -ne 0 ] || [ "$selected" -eq 0 ]; then
    [ "$selected" -gt 0 ] || echo "corpus run $id selected no test (filter: ${filter:-none})" >&2
    red "$out"
    rm -f "$out"
    return 1
  fi
  rm -f "$out"
}

# The runs IDs, all of one package, in one `ply test` with the ARGs after `--` added: each run's
# milliseconds are the summed durations of the tests whose `<module>.<label>` key its filter holds,
# and a run whose filter selected none fails.
run_modules() {
  local timings=$1 id line path filter status=0 out bad=0 n ms i
  local -a filters=() ids=() args=() extra=()
  shift
  while [ $# -gt 0 ]; do
    if [ "$1" = -- ]; then
      shift
      extra=("$@")
      break
    fi
    id=$1
    shift
    line=$("$shards" corpus-line "$id") || return 2
    read -r path filter <<< "$line"
    ids+=("$id")
    filters+=("$filter")
    args+=(--filter "$filter")
  done
  out=$(mktemp)
  started=$(date +%s%3N)
  tested "$path" "${args[@]}" ${extra[@]+"${extra[@]}"} > "$out" || status=$?
  wall=$(($(date +%s%3N) - started))
  listed "$out"
  timed "$out" "$path" >> "$durations"
  spent "$out" "$wall"
  cached_row "$out" "$path" "$timings"
  # The startup the cut charges a lane once per package.
  ran=$(jq '(.summary.duration_ms // 0) | floor' "$out" 2>/dev/null || echo 0)
  printf 'startup\t%s\t%s\n' "$path" "$((wall > ran ? wall - ran : 0))" >> "$timings"
  for i in "${!ids[@]}"; do
    # Every test the filter names, run or cached: a module whose tests all passed before selects them.
    n=$(jq --arg f "${filters[$i]}" '[.selection.tests[]? | select(.key | contains($f))] | length' "$out" 2>/dev/null || echo 0)
    ms=$(jq --arg f "${filters[$i]}" '[.results[]? | select(.key | contains($f)) | .duration_ms] | add // 0 | floor' "$out" 2>/dev/null || echo 0)
    if [ "$n" -eq 0 ]; then
      echo "corpus run ${ids[$i]} selected no test (filter: ${filters[$i]})" >&2
      bad=1
    fi
    printf 'corpus\t%s\t%s\n' "${ids[$i]}" "$ms" >> "$timings"
  done
  if [ "$status" -ne 0 ] || [ "$bad" -ne 0 ]; then
    red "$out"
    rm -f "$out"
    return 1
  fi
  rm -f "$out"
}

# One lane's runs, one after another: the program's own, each fixture's and each package's suite in a
# `ply test` of their own, every checks run in one, and every run of the CLI's suite in one.
lane() {
  local timings=$1 id started failed=0
  local durations=${1%.tsv}.durations
  local -a checks=() cli=()
  shift
  : > "$timings"
  : > "$durations"
  for id in "$@"; do
    case "$id" in
      program | stdlib | package-* | fixture-*)
        echo "::group::corpus $id"
        started=$(date +%s%3N)
        run_one "$timings" "$id" || failed=1
        printf 'corpus\t%s\t%s\n' "$id" "$(($(date +%s%3N) - started))" >> "$timings"
        echo "::endgroup::"
        ;;
      cli-*) cli+=("$id") ;;
      *) checks+=("$id") ;;
    esac
  done
  if [ "${#checks[@]}" -gt 0 ]; then
    echo "::group::corpus checks ${checks[*]}"
    run_modules "$timings" "${checks[@]}" || failed=1
    echo "::endgroup::"
  fi
  if [ "${#cli[@]}" -gt 0 ]; then
    echo "::group::the CLI's suite ${cli[*]}"
    run_modules "$timings" "${cli[@]}" || failed=1
    echo "::endgroup::"
  fi
  return "$failed"
}

marked() {
  [ -f "$mark" ] || { echo "nothing is marked: run 'ci-corpus.sh mark' before the runs" >&2; return 1; }
}

# The emitter's answers `ply` wrote or read since the mark, into DIR.
kept_answers() {
  local dir=$1
  rm -rf "$dir"
  mkdir -p "$dir"
  [ -d "$caches/ply-c-cache/bodies" ] || return 0
  (cd "$caches" && find ply-c-cache/bodies -type f -newer "$mark" ! -name '*.tmp' -print0 |
    tar --null -T - -cf -) | tar -xf - -C "$dir"
}

case "${1:-}" in
  partition)
    shard=${2:?a partition}
    timings=${3:?a file for the durations}
    runs=$("$shards" corpus-for-partition "$shard" "${4:-}") || exit 2
    : > "$timings"
    : > "$durations"
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
      cat "$work/lane-$l.durations" >> "$durations"
    done
    rm -rf "$work"
    exit "$failed"
    ;;
  desks)
    k=${2:?a desk runner}
    timings=${3:?a file for the durations}
    [ $# -ge 4 ] || { echo "usage: ci-corpus.sh desks K TIMINGS CUT [ARG...], CUT empty for none" >&2; exit 2; }
    taken=$("$shards" desks-for-runner "$k" "$4") || exit 2
    shift 4
    : > "$timings"
    : > "$durations"
    if [ -z "$taken" ]; then
      echo "desk runner $k takes no run: there are more runners than desk tests"
      exit 0
    fi
    read -ra runs <<< "$(tr '\n' ' ' <<< "$taken")"
    echo "::group::corpus desks ${runs[*]}"
    failed=0
    run_modules "$timings" "${runs[@]}" -- "$@" || failed=1
    echo "::endgroup::"
    exit "$failed"
    ;;
  run)
    : > "$durations"
    run_one "" "${2:?a corpus entry}" "${@:3}"
    ;;
  mark)
    touch "$mark"
    ;;
  keep)
    dir=${2:?a directory}
    marked || exit 2
    kept_answers "$dir" || exit 1
    du -sh "$dir"
    ;;
  pack)
    tar_out=${2:?a tar file}
    marked || exit 2
    work=$(mktemp -d)
    kept_answers "$work/c" || exit 1
    # A store is one file set: it travels whole, from the partition that wrote it.
    for store in "$root"/crates/*/ply/.ply-cache "$root"/crates/ply-corpus/checks/.ply-cache "$root"/crates/ply-corpus/stdlib/.ply-cache; do
      [ -d "$store" ] && [ -n "$(find "$store" -type f -newer "$mark" -print -quit)" ] || continue
      rel=${store#"$root"/}
      "$ply" cache compact "${store%/.ply-cache}" > /dev/null ||
        echo "the store under ${rel%/.ply-cache} was not compacted" >&2
      mkdir -p "$work/stores/${rel%/.ply-cache}"
      cp -R "$store" "$work/stores/$rel"
      echo "packed the store under ${rel%/.ply-cache}"
    done
    tar -C "$work" -cf "$tar_out" .
    rm -rf "$work"
    du -h "$tar_out"
    ;;
  unpack)
    parts=${2:?a directory of packed tars}
    dir=${3:?a directory for the C}
    rm -rf "$dir"
    mkdir -p "$dir"
    while IFS= read -r part; do
      work=$(mktemp -d)
      tar -C "$work" -xf "$part" || { rm -rf "$work"; exit 1; }
      [ -d "$work/c" ] && { tar -C "$work/c" -cf - . | tar -C "$dir" --skip-old-files -xf -; }
      while IFS= read -r store; do
        rel=${store#"$work/stores/"}
        rm -rf "${root:?}/$rel"
        mkdir -p "$(dirname "$root/$rel")"
        cp -R "$store" "$root/$rel"
        echo "the store under ${rel%/.ply-cache} from ${part#"$parts"/}"
      done < <([ -d "$work/stores" ] && find "$work/stores" -type d -name .ply-cache)
      rm -rf "$work"
    done < <(find "$parts" -type f -name '*.tar' | sort)
    du -sh "$dir"
    ;;
  restore)
    dir=${2:?a directory}
    [ -d "$dir" ] || exit 0
    tar -C "$dir" -cf - . | tar -C "$caches" --skip-old-files -xf -
    ;;
  compact)
    for dir in "$root"/crates/*/ply "$root"/crates/ply-corpus/checks; do
      [ -f "$dir/.ply-cache/store.idx" ] || continue
      echo "=== ${dir#"$root"/}"
      "$ply" cache compact "$dir" || echo "the store under ${dir#"$root"/} was not compacted" >&2
    done
    ;;
  upstream-mark)
    mkdir -p "$upstream"
    touch "$upstream_mark"
    find "$upstream" -type f | wc -l | sed 's/^ */upstream entries restored: /'
    ;;
  upstream-new)
    tar_out=${2:?a tar file}
    [ -f "$upstream_mark" ] || { echo "nothing is marked: run 'ci-corpus.sh upstream-mark' first" >&2; exit 2; }
    mkdir -p "$upstream"
    (cd "$upstream" && find . -type f -newer "$upstream_mark" ! -name '*.tmp' -print0 |
      tar --null -T - -cf "$tar_out")
    tar -tf "$tar_out" | wc -l | sed 's/^ */upstream entries published: /'
    ;;
  upstream-merge)
    parts=${2:?a directory of tars}
    mkdir -p "$upstream"
    published=()
    for part in "$parts"/*.tar; do
      [ -f "$part" ] || continue
      tar -C "$upstream" -xf "$part"
      while IFS= read -r stamp; do published+=("$stamp"); done < <(tar -tf "$part" | awk -F/ '$2 == "v2" && $3 != "" { print $3 }')
    done
    # A pass filed under an older runtime is never read again.
    if [ "${#published[@]}" -gt 0 ] && [ -d "$upstream/v2" ]; then
      for dir in "$upstream"/v2/*; do
        name=${dir##*/}
        keep=no
        for stamp in "${published[@]}"; do [ "$stamp" = "$name" ] && keep=yes; done
        [ "$keep" = yes ] || rm -rf "$dir"
      done
    fi
    find "$upstream" -type f | wc -l | sed 's/^ */upstream entries kept: /'
    ;;
  *)
    echo "usage: ci-corpus.sh partition K TIMINGS [CUT] | desks K TIMINGS CUT [ARG...] | run ID [ARG...] | mark | keep DIR | pack TAR | unpack DIR C | restore DIR | compact | upstream-mark | upstream-new TAR | upstream-merge DIR" >&2
    exit 2
    ;;
esac
