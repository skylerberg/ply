#!/usr/bin/env bash
# The Ply runs (`ci-shards.sh`'s), each one `ply test` of a package: over the corpus with the grants
# `benches/corpus.sh` gives the program, over the CLI's suite with the grants its harness drives `ply`
# with, from a scratch directory of the run's own, or a package's own suite as `ply test` runs it;
# the proof runs, the standard library's (`proofs`) and each package's (`laws-<id>`), are a
# `ply prove` each. A run fails when a test fails or a claim does not hold, when the run refuses, and
# when it selects no test or proves no claim, so a renamed test never stops being run quietly; a part
# of the CLI's suite takes the tests whose keys hash to it, which may be none.
#
#   ci-corpus.sh run ID [ARG...]
#       the runs the job ID takes (`ci-shards.sh job-runs ID`), in turn, or one run, or one module
#       of the corpus's checks (`<module>`) or of the CLI's suite (`cli-<module>`), with ARGs added
#       to each `ply test` or `ply prove`. Every `ply` is stopped at PLY_CI_DEADLINE, the epoch
#       second the job set its runs to end by, and none starts after it: the job then fails naming
#       the run it stopped and the ones it never started, and the steps after it still keep what the
#       runs wrote, so a re-run starts from there rather than cold.
#   ci-corpus.sh nightly [MODULE...] [ARG...]
#       the modules of the CLI's suite no pull request runs (`ci-shards.sh`'s CLI_NIGHTLY), every
#       MODULE in one `ply test` with the suite's grants and ARGs such as `--shard K/N` or a case's
#       `--filter` added
#   ci-corpus.sh mark                  the moment `keep` gathers from
#   ci-corpus.sh keep DIR              the bodies and the compiler's answers `ply` emitted or read
#                                      since `mark`, into DIR for a later run: a body is keyed by its
#                                      definition, the emitter and the runtime's sources, and an answer
#                                      by the emitter and its question, so another tree reuses what
#                                      still applies. Objects stay out: one compiles from its bodies in
#                                      seconds. The stages are build-ply's to ship, and the packages'
#                                      stores are carried apart.
#   ci-corpus.sh restore DIR           a kept DIR merged under what `ply` reads, keeping what is there
#   ci-corpus.sh compact               every package's store compacted before a job saves them: a
#                                      store only grows, and the job's next run restores what it saves
#   ci-corpus.sh upstream-mark         the moment `upstream-new` gathers from
#   ci-corpus.sh upstream-new TAR      what this job published to `PLY_CACHE_UPSTREAM` since the mark
#   ci-corpus.sh upstream-merge DIR    every job's TAR under DIR merged, keeping this run's runtimes
set -uo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ply="$root/target/debug/ply"
shards="$root/.github/ci-shards.sh"
cli_suite=crates/ply-cli-tests/ply
stdlib=crates/ply-corpus/stdlib
TAB=$'\t'
# The caches `ply` reads, where the workflow restores them.
caches=/tmp
mark=$caches/ply-c-corpus.mark
upstream=$caches/ply-upstream
upstream_mark=$caches/ply-upstream.mark
export PLY_CACHE_UPSTREAM=$upstream
# `ms<TAB>package<TAB>key` per test this job ran, which the job uploads for the run's table of what
# each test cost.
durations=$caches/ply-test-durations.tsv

# What a checks run is given after `ply test PATH`.
grants=(--host --timeout 900000 --steps 0 --json
  --exec "ply=$ply" --allow machine --allow claims --allow shipped --allow toolchain --fs work=. --fs "repo=$root")

# The directory holding postgres and its tools, for the CLI suite's tests that start a cluster: the
# first on PATH, else the newest of Debian's per-version directories, which are off PATH.
postgres_bin() {
  local found
  found=$(command -v initdb 2>/dev/null || true)
  if [[ -n $found ]]; then dirname "$found"; return; fi
  find /usr/lib/postgresql -maxdepth 2 -name bin -type d 2>/dev/null | sort -V | tail -1
}
pg=$(postgres_bin)

# What a run of the CLI's suite is given: the `ply` its tests start and the programs they start beside
# it, a postgres where there is one, its scratch directory and the filesystem, the repository, and
# every family a command drives a machine with, since its tests run the commands in this process.
cli_grants=(--host --timeout 900000 --steps 0 --json
  --exec "ply=$ply" --exec sh=/bin/sh --exec "git=$(command -v git)"
  ${pg:+--exec "initdb=$pg/initdb" --exec "postgres=$pg/postgres" --exec "psql=$pg/psql"}
  --fs cwd=. --fs abs=/ --fs "repo=$root"
  --allow machine --allow tester --allow claims --allow hosts
  --allow shipped --allow toolchain)

# The epoch second every `ply` this job starts must end by; empty for no end.
deadline=${PLY_CI_DEADLINE:-}
# A run's status when the deadline stopped it, or came before it could start.
STOPPED=124

past_deadline() { [[ -n $deadline ]] && (($(date +%s) >= deadline)); }

# The command, ended when the deadline comes along with everything it started: STOPPED then, and
# when the deadline had already come.
bounded() {
  local status=0
  [[ -n $deadline ]] || { "$@"; return; }
  past_deadline && return "$STOPPED"
  timeout --kill-after=30 "$((deadline - $(date +%s)))" "$@" || status=$?
  if ((status == 124 || status == 137)) && past_deadline; then return "$STOPPED"; fi
  return "$status"
}

# What the run ID says once the deadline ended it, the milliseconds since STARTED into it.
stopped_run() {
  echo "::error::the run $2 was stopped at the job's deadline after $((($(date +%s%3N) - $1) / 1000))s: what it finished is kept, and a re-run of this job starts from it"
}

# `ply test` over the package at PATH (relative to the repository) with ARGs: the CLI's suite from a
# directory of its own, which its harness empties before each test and refuses without the marker,
# and the standard library's fixtures with every shipped module's tests, granted nothing.
tested() {
  local path=$1 status=0 dir
  shift
  if [[ $path == "$cli_suite" ]]; then
    dir=$(mktemp -d)
    touch "$dir/.ply-scratch"
    (cd "$dir" && bounded "$ply" test "$root/$path" "$@" "${cli_grants[@]}") || status=$?
    rm -rf "$dir"
  elif [[ $path == "$stdlib" ]]; then
    bounded "$ply" test --std "$path" --json "$@" || status=$?
  else
    bounded "$ply" test "$path" "$@" "${grants[@]}" || status=$?
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

# Where a `ply test` spent its wall clock WALL (ms), from its report: the phases its clock, which
# counts from the process's start, measured, and `other` for what follows the report.
spent() {
  jq -r --argjson wall "$2" '
    def s(ms): (ms / 1000 | floor | tostring) + "s";
    (.phases // {}) as $p
    | (if .front_end.reused then "spent: a kept answer, read in \(s(.front_end.phases.total // 0)); it was filed by a run that spent " else "spent: " end)
      + "before \(s($p.before // 0)), load \(s($p.load // 0)), promises \(s($p.promises // 0)), store \(s($p.store // 0)), selection \(s($p.selection // 0)), handoff \(s($p.handoff // 0)), unit \(s($p.unit // 0)) (C analysis \(s((.backend.analysis_nanos // 0) / 1e6)), codegen \(s((.backend.codegen_nanos // 0) / 1e6))), bind \(s($p.bind // 0)), tests \(s($p.tests // 0)), conclude \(s($p.conclude // 0)), other \(s([$wall - ($p.total // 0), 0] | max))"
  ' "$1" 2>/dev/null
}

# The package the run ID tests, a `--filter` per substring it picks its tests by and its part, into
# the caller's `path`, `picked` and `part`.
placed() {
  local line rest filter
  line=$("$shards" corpus-line "$1") || return 2
  path=${line%%"$TAB"*}
  rest=${line#*"$TAB"}
  part=${rest#*"$TAB"}
  picked=()
  for filter in ${rest%%"$TAB"*}; do picked+=(--filter "$filter"); done
}

# `ply test` of the run ID with ARGs added: each test it ran, where its time went, and what failed.
# STOPPED when the deadline ended it.
tests_of() {
  local id=$1 path part status=0 selected out started
  local -a picked=()
  shift
  placed "$id" || return 2
  out=$(mktemp)
  started=$(date +%s%3N)
  if [[ $id == package-* || $id == fixture-* ]]; then
    # A package's own suite and a fixture run as `ply test` runs them: the corpus's grants are for
    # the corpus.
    bounded "$ply" test "$path" --json "$@" > "$out" || status=$?
  else
    tested "$path" ${picked[@]+"${picked[@]}"} ${part:+--shard "$part"} "$@" > "$out" || status=$?
  fi
  if ((status == STOPPED)); then
    stopped_run "$started" "$id"
    rm -f "$out"
    return "$STOPPED"
  fi
  listed "$out"
  timed "$out" "$path" >> "$durations"
  spent "$out" "$(($(date +%s%3N) - started))"
  selected=$(jq -s 'map(.selection.tests // [] | length) | add // 0' "$out" 2>/dev/null || echo 0)
  if [ "$selected" -eq 0 ] && [ -n "$part" ]; then
    echo "the run $id selected no test: every test of its suite is in another part"
  elif [ "$selected" -eq 0 ]; then
    echo "the run $id selected no test" >&2
    status=1
  fi
  if [ "$status" -ne 0 ]; then
    red "$out"
    rm -f "$out"
    return 1
  fi
  rm -f "$out"
}

# `ply prove` of the run ID with ARGs added, the library's and its fixtures' claims (`proofs`) or a
# package's (`laws-<id>`); it fails when it proves no claim. STOPPED when the deadline ended it.
proofs_of() {
  local id=$1 path part status=0 out started all cached discharging
  local -a picked=()
  shift
  placed "$id" || return 2
  out=$(mktemp)
  started=$(date +%s%3N)
  if [[ $id == proofs ]]; then
    bounded "$ply" prove --std "$path" "$@" --json > "$out" || status=$?
  else
    bounded "$ply" prove "$path" --json "$@" > "$out" || status=$?
  fi
  if ((status == STOPPED)); then
    stopped_run "$started" "$id"
    rm -f "$out"
    return "$STOPPED"
  fi
  discharging=$(jq '(.duration_ms // 0) | floor' "$out" 2>/dev/null || echo 0)
  all=$(jq '.obligations // [] | length' "$out" 2>/dev/null || echo 0)
  cached=$(jq '.cached // 0' "$out" 2>/dev/null || echo 0)
  echo "proved: $all claims, $cached from the cache, the rest discharged in $((discharging / 1000))s of the $((($(date +%s%3N) - started) / 1000))s the prove took"
  if [ "$all" -eq 0 ]; then
    echo "the run $id proved no claim" >&2
    status=1
  fi
  if [ "$status" -ne 0 ]; then
    disproved "$out"
    rm -f "$out"
    return 1
  fi
  rm -f "$out"
}

# What a red proof run said: each claim nothing proved, how and where, and a refused run's
# diagnostics.
disproved() {
  jq -r '
    (.obligations[]? | select(.outcome != "proved")
      | "FAILED \(.owner) \(.label): \(.outcome)\(if .location then " at \(.location)" else "" end)",
        "  \(del(.key, .owner, .label, .kind, .guarded, .frame, .location, .outcome) | tojson)"),
    (.diagnostics[]? | "\(.severity // "error") \(.code // ""): \(.message // "")")
  ' "$1" 2>/dev/null || cat "$1"
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
  run)
    : > "$durations"
    runs=$("$shards" job-runs "${2:?a job, a run or a module}") || exit 2
    failed=0
    late=()
    for id in $runs; do
      if past_deadline; then
        late+=("$id")
        continue
      fi
      status=0
      echo "::group::$id"
      case "$id" in
        proofs | laws-*) proofs_of "$id" "${@:3}" || status=$? ;;
        *) tests_of "$id" "${@:3}" || status=$? ;;
      esac
      echo "::endgroup::"
      if ((status != 0)); then
        failed=1
        ((status == STOPPED)) || echo "::error::the run $id failed"
      fi
    done
    if [ "${#late[@]}" -gt 0 ]; then
      echo "::error::the job's deadline came before these runs could start: ${late[*]}"
      failed=1
    fi
    exit "$failed"
    ;;
  nightly)
    : > "$durations"
    out=$(mktemp)
    status=0
    # The modules, each a filter, up to the first of what `ply test` is also given.
    args=()
    given=0
    for arg in "${@:2}"; do
      [[ $arg == -* ]] && given=1
      if ((given)); then args+=("$arg"); else args+=(--filter "$arg."); fi
    done
    [ "${#args[@]}" -gt 0 ] || { echo "usage: ci-corpus.sh nightly [MODULE...] [ARG...]" >&2; exit 2; }
    tested "$cli_suite" "${args[@]}" > "$out" || status=$?
    listed "$out"
    selected=$(jq -s 'map(.selection.tests // [] | length) | add // 0' "$out" 2>/dev/null || echo 0)
    if [ "$status" -ne 0 ] || [ "$selected" -eq 0 ]; then
      [ "$selected" -gt 0 ] || echo "the nightly run selected no test" >&2
      red "$out"
      rm -f "$out"
      exit 1
    fi
    rm -f "$out"
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
  restore)
    dir=${2:?a directory}
    [ -d "$dir" ] || exit 0
    tar -C "$dir" -cf - . | tar -C "$caches" --skip-old-files -xf -
    ;;
  compact)
    for dir in "$root"/crates/*/ply "$root"/crates/ply-corpus/checks "$root"/crates/ply-corpus/stdlib "$root"/tests/lang; do
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
    echo "usage: ci-corpus.sh run ID [ARG...] | nightly [MODULE...] [ARG...] | mark | keep DIR | restore DIR | compact | upstream-mark | upstream-new TAR | upstream-merge DIR" >&2
    exit 2
    ;;
esac
