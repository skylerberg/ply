#!/usr/bin/env bash
# The Ply runs (`ci-shards.sh`'s corpus entries), each a `ply test`: over the corpus with the grants
# `benches/corpus.sh` gives the program, or over the CLI's suite with the grants its harness drives
# `ply` with, from a scratch directory of the run's own; the proof runs, the standard library's
# (`proofs`) and each package's (`laws-<id>`), are a `ply prove` each, on a runner of its own. A run
# fails when a test fails or a claim does not hold, when the run refuses, and when it selects no test
# or proves no claim, so a renamed test never stops being run quietly; a part of a run (`run#K/N`)
# takes the tests whose keys hash to it, which may be none.
#
#   ci-corpus.sh partition K TIMINGS [CUT]
#       every run partition K takes (`ci-shards.sh corpus-for-partition K [CUT]`), each lane one
#       process beside the others. A lane's runs of one package go in one `ply test` with a
#       `--filter` each, so the package's closure is loaded once a lane rather than once a run. Each
#       run's milliseconds are appended to TIMINGS as `corpus <run> <ms>`: a module's are its tests'
#       own, out of the report. Each `ply test` adds `cached <what> <n>`, the tests it took from the
#       cache, which tells `ci-shards.sh timings` whether the run's costs are a cold run's. A lane
#       prints each run whole as it ends, so the log of a job stopped short shows every run it
#       finished.
#
#   Every `ply` the partition, desk and lone runs start is stopped at PLY_CI_DEADLINE, the epoch
#   second the job set its runs to end by, and none starts after it: the job then fails naming the
#   runs it stopped and the ones it never started, and the steps after it still keep what the runs
#   wrote, so a re-run starts from there rather than cold.
#   ci-corpus.sh desks K TIMINGS CUT [ARG...]
#       the desk runs runner K takes (`ci-shards.sh desks-for-runner K CUT`, round robin when CUT is
#       empty) in one `ply test` with the desk's grants and ARGs added, their milliseconds onto
#       TIMINGS as a partition's
#   ci-corpus.sh run ID [ARG...]       one run, with ARGs added to its `ply test` or `ply prove`
#   ci-corpus.sh sweep [ARG...]        the edit sweep (`edit_sweep` of the CLI's suite) with the suite's
#                                      grants: the cases its `--filter`s name, or else, with ARGs such as
#                                      `--shard K/N` added, every case
#   ci-corpus.sh select [CUT]
#       the jobs a run has to start, as `KEY=VALUE` lines for the workflow's outputs: `partitions`
#       and `corpus`, the matrices of the partitions, desk runners and runs alone holding a run no
#       kept answer stands for, and `lanes` and `solo`, whether either holds one. Every `ply test`
#       is the one `partition`, `desks` or `run` starts, asked only what an earlier run kept
#       (`--kept`), so a job none of whose runs has work is never started. No kept answer stands for
#       a proof run.
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
stdlib=crates/ply-corpus/stdlib
TAB=$'\t'
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

# What a desk run is given beside the checks' grants: the floors the served tables are compared with,
# built where the workflow builds them, and the database the desk's schema is in.
desk_grants=(--exec "http_floor=$root/target/http-floor" --exec "pg_floor=$root/target/pg-floor"
  --set CORPUS_DB=postgres://postgres@127.0.0.1:5432/desk)

# What every `ply test` a lane starts is also asked: `select` asks only what an earlier run kept,
# and takes each run's exit code as its answer.
asked=()
selecting=0

# The epoch second every `ply` this job starts must end by; empty for no end.
deadline=${PLY_CI_DEADLINE:-}
# A run's status when the deadline stopped it, or came before it could start.
STOPPED=124
# Where the runs the deadline stopped or left unstarted are listed, one a line, when a job lists them.
stopped=
# The lock the lanes of a partition print through, when they print beside each other.
lock=

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

# `stopped_run SECONDS ID...`: what the runs the deadline ended after SECONDS say, and their entries
# in the list.
stopped_run() {
  local seconds=$1
  shift
  echo "::error::corpus $* stopped at the job's deadline after ${seconds}s"
  [[ -z $stopped ]] || printf 'stopped %s\n' "$@" >> "$stopped"
}

# `not_started ID...`, likewise for runs the deadline came before.
not_started() {
  echo "corpus $* not started: the job's deadline had come"
  [[ -z $stopped ]] || printf 'not-started %s\n' "$@" >> "$stopped"
}

# FILE's lines, all at once, then FILE gone: a run's lines are written whole once it ends, so the
# lane beside it never cuts into them.
said() {
  if [[ -n $lock ]]; then flock "$lock" cat "$1"; else cat "$1"; fi
  rm -f "$1"
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
    | (if .front_end.reused then "spent: a kept answer, read in \(s(.front_end.phases.total // 0)); it was filed by a run that spent " else "spent: " end)
      + "before \(s($p.before // 0)), load \(s($p.load // 0)), promises \(s($p.promises // 0)), store \(s($p.store // 0)), selection \(s($p.selection // 0)), handoff \(s($p.handoff // 0)), unit \(s($p.unit // 0)) (C analysis \(s((.backend.analysis_nanos // 0) / 1e6)), codegen \(s((.backend.codegen_nanos // 0) / 1e6))), bind \(s($p.bind // 0)), tests \(s($p.tests // 0)), conclude \(s($p.conclude // 0)), other \(s([$wall - ($p.total // 0), 0] | max))"
  ' "$1" 2>/dev/null
}

# The run ID with ARGs added, its `cached` row onto TIMINGS when that is not empty. STOPPED when the
# deadline ended it.
run_one() {
  local timings=$1 id=$2 line path filter shard status=0 selected out started
  shift 2
  if [[ $id == proofs ]]; then
    run_proofs "$timings" "$@"
    return
  fi
  if [[ $id == laws-* ]]; then
    package_proofs "$timings" "$id" "$@"
    return
  fi
  line=$("$shards" corpus-line "$id") || return 2
  fields "$line"
  out=$(mktemp)
  started=$(date +%s%3N)
  if [[ $id == package-* || $id == fixture-* ]]; then
    # A package's own suite and a fixture run as `ply test` runs them: the corpus's grants are for
    # the corpus.
    bounded "$ply" test "$path" --json ${shard:+--shard "$shard"} "$@" > "$out" || status=$?
  else
    tested "$path" ${filter:+--filter "$filter"} ${shard:+--shard "$shard"} "$@" > "$out" || status=$?
  fi
  if ((selecting)); then
    rm -f "$out"
    return "$status"
  fi
  if ((status == STOPPED)); then
    stopped_run "$((($(date +%s%3N) - started) / 1000))" "$id"
    rm -f "$out"
    return "$STOPPED"
  fi
  listed "$out"
  timed "$out" "$path" >> "$durations"
  spent "$out" "$(($(date +%s%3N) - started))"
  cached_row "$out" "$id" "$timings"
  selected=$(jq -s 'map(.selection.tests // [] | length) | add // 0' "$out" 2>/dev/null || echo 0)
  if [ "$selected" -eq 0 ] && [ -n "$shard" ]; then
    echo "corpus run $id selected no test: every test of its run is in another part"
  elif [ "$selected" -eq 0 ]; then
    echo "corpus run $id selected no test (filter: ${filter:-none})" >&2
    status=1
  fi
  if [ "$status" -ne 0 ]; then
    red "$out"
    rm -f "$out"
    return 1
  fi
  rm -f "$out"
}

# A corpus line, `path<TAB>filter<TAB>shard`, into the caller's `path`, `filter` and `shard`, either
# of the last two empty.
fields() {
  local rest=${1#*"$TAB"}
  path=${1%%"$TAB"*}
  filter=${rest%%"$TAB"*}
  shard=${rest#*"$TAB"}
}

# `ID... [-- ARG...]` into the caller's `ids`, `filters`, `cuts`, `extra` and `path`: the runs, the
# filter and part of each, the ARGs, and the package the runs are all of.
parsed_runs() {
  local lines line filter shard
  while [ $# -gt 0 ]; do
    if [ "$1" = -- ]; then
      shift
      extra=("$@")
      break
    fi
    ids+=("$1")
    shift
  done
  lines=$("$shards" corpus-line "${ids[@]}") || return 2
  while IFS= read -r line; do
    fields "$line"
    filters+=("$filter")
    cuts+=("$shard")
  done <<< "$lines"
}

# The runs IDs, all of one package, in one `ply test` with the ARGs after `--` added: each run's
# milliseconds are the summed durations of the tests whose `<module>.<label>` key its filter holds,
# and a run whose filter selected none fails. STOPPED when the deadline ended them.
run_modules() {
  local timings=$1 path status=0 out bad=0 n ms i started key
  local -a filters=() cuts=() ids=() args=() extra=()
  shift
  parsed_runs "$@" || return 2
  for i in "${!filters[@]}"; do
    args+=(--filter "${filters[$i]}")
    [[ -z ${cuts[$i]} ]] || args+=(--shard "${cuts[$i]}")
  done
  # A part of a run is a `ply test` of its own, and the cut charges its startup apart, as its run's:
  # every part loads the same closure.
  key=$path
  [[ -z ${cuts[0]} ]] || key=$path#${ids[0]%%#*}
  # What an earlier run kept answers each run of several apart, whichever runs it was batched with:
  # only the ones it does not answer are run.
  started=$(date +%s%3N)
  if ((!selecting)) && [ "${#ids[@]}" -gt 1 ]; then
    local asking=0 left kept_ids=() kept_filters=() kept_cuts=() answered=()
    out=$(mktemp)
    tested "$path" "${args[@]}" ${extra[@]+"${extra[@]}"} --kept > "$out" || asking=$?
    left=$(jq -r '.unanswered[]?' "$out" 2>/dev/null)
    rm -f "$out"
    if [ "$asking" -eq "$STOPPED" ]; then
      stopped_run "$((($(date +%s%3N) - started) / 1000))" "${ids[@]}"
      return "$STOPPED"
    fi
    if [ "$asking" -eq 0 ]; then
      echo "answered by what earlier runs kept: ${ids[*]}"
      return 0
    fi
    if [ "$asking" -eq 4 ] && [ -n "$left" ]; then
      args=()
      for i in "${!ids[@]}"; do
        if grep -qxF -- "${filters[$i]}" <<< "$left"; then
          kept_ids+=("${ids[$i]}")
          kept_filters+=("${filters[$i]}")
          kept_cuts+=("${cuts[$i]}")
          args+=(--filter "${filters[$i]}")
          [[ -z ${cuts[$i]} ]] || args+=(--shard "${cuts[$i]}")
        else
          answered+=("${ids[$i]}")
        fi
      done
      [ "${#answered[@]}" -eq 0 ] || echo "answered by what earlier runs kept: ${answered[*]}"
      ids=("${kept_ids[@]}")
      filters=("${kept_filters[@]}")
      cuts=("${kept_cuts[@]}")
    fi
  fi
  out=$(mktemp)
  started=$(date +%s%3N)
  tested "$path" "${args[@]}" ${extra[@]+"${extra[@]}"} > "$out" || status=$?
  if ((selecting)); then
    rm -f "$out"
    return "$status"
  fi
  if ((status == STOPPED)); then
    stopped_run "$((($(date +%s%3N) - started) / 1000))" "${ids[@]}"
    rm -f "$out"
    return "$STOPPED"
  fi
  wall=$(($(date +%s%3N) - started))
  listed "$out"
  timed "$out" "$path" >> "$durations"
  spent "$out" "$wall"
  cached_row "$out" "$path" "$timings"
  # The startup the cut charges a lane once per package.
  ran=$(jq '(.summary.duration_ms // 0) | floor' "$out" 2>/dev/null || echo 0)
  printf 'startup\t%s\t%s\n' "$key" "$((wall > ran ? wall - ran : 0))" >> "$timings"
  # Per filter, every test it names, run or cached (a module whose tests all passed before selects
  # them), and the milliseconds of those that ran: one read of a report that can hold thousands.
  local -a counted=()
  read -ra counted <<< "$(jq -r '
    [.selection.tests[]? | .key // ""] as $selected | [.results[]? | [.key // "", .duration_ms // 0]] as $ran
    | [$ARGS.positional[] as $f
        | ([$selected[] | select(contains($f))] | length),
          ([$ran[] | select(.[0] | contains($f)) | .[1]] | add // 0 | floor)]
    | map(tostring) | join(" ")
  ' "$out" --args "${filters[@]}" 2>/dev/null)"
  for i in "${!ids[@]}"; do
    n=${counted[$((2 * i))]:-0}
    ms=${counted[$((2 * i + 1))]:-0}
    if [ "$n" -eq 0 ] && [ -n "${cuts[$i]}" ]; then
      echo "corpus run ${ids[$i]} selected no test: every test of its run is in another part"
    elif [ "$n" -eq 0 ]; then
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

# The run `proofs`, every claim of the library and of the fixtures beside it in one `ply prove` with
# ARGs added, its `cached` row onto TIMINGS when that is not empty; it fails when it proves no claim.
# Nothing an earlier run kept answers a proof, so a selecting run always has one to run. STOPPED when
# the deadline ended it.
run_proofs() {
  local timings=$1 status=0 out all cached unattempted spent started wall
  shift
  ((!selecting)) || return 1
  out=$(mktemp)
  started=$(date +%s%3N)
  bounded "$ply" prove --std "$stdlib" "$@" --json > "$out" || status=$?
  if ((status == STOPPED)); then
    stopped_run "$((($(date +%s%3N) - started) / 1000))" proofs
    rm -f "$out"
    return "$STOPPED"
  fi
  wall=$(($(date +%s%3N) - started))
  spent=$(jq '(.duration_ms // 0) | floor' "$out" 2>/dev/null || echo 0)
  all=$(jq '.obligations // [] | length' "$out" 2>/dev/null || echo 0)
  cached=$(jq '.cached // 0' "$out" 2>/dev/null || echo 0)
  # Only a claim that held is filed, so an unattempted one is discharged again by every later run.
  unattempted=$(jq '.summary.unattempted // 0' "$out" 2>/dev/null || echo 0)
  echo "proved: $all claims, $cached from the cache, $unattempted unattempted, the rest discharged in $((spent / 1000))s of the $((wall / 1000))s the prove took"
  [[ -z $timings ]] || printf 'cached\tproofs\t%s\n' "$cached" >> "$timings"
  if [ "$all" -eq 0 ]; then
    echo "corpus run proofs proved no claim" >&2
    status=1
  fi
  if [ "$status" -ne 0 ]; then
    disproved "$out"
    rm -f "$out"
    return 1
  fi
  rm -f "$out"
}

# The run `laws-<id>`, a `ply prove` of the package with ARGs added, its `cached` row onto TIMINGS when
# that is not empty; it fails when it proves no claim. A selecting run always has it to run, as it has
# the library's proofs. STOPPED when the deadline ended it.
package_proofs() {
  local timings=$1 id=$2 line path filter shard status=0 out started all cached
  shift 2
  ((!selecting)) || return 1
  line=$("$shards" corpus-line "$id") || return 2
  fields "$line"
  out=$(mktemp)
  started=$(date +%s%3N)
  bounded "$ply" prove "$path" --json "$@" > "$out" || status=$?
  if ((status == STOPPED)); then
    stopped_run "$((($(date +%s%3N) - started) / 1000))" "$id"
    rm -f "$out"
    return "$STOPPED"
  fi
  all=$(jq '.obligations // [] | length' "$out" 2>/dev/null || echo 0)
  cached=$(jq '.cached // 0' "$out" 2>/dev/null || echo 0)
  echo "proved: $all claims, $cached from the cache, in $((($(date +%s%3N) - started) / 1000))s"
  [[ -z $timings ]] || printf 'cached\t%s\t%s\n' "$id" "$cached" >> "$timings"
  if [ "$all" -eq 0 ]; then
    echo "corpus run $id proved no claim" >&2
    status=1
  fi
  if [ "$status" -ne 0 ]; then
    disproved "$out"
    rm -f "$out"
    return 1
  fi
  rm -f "$out"
}

# What a red proof run said: each claim that does not hold, how and where, and a refused run's
# diagnostics.
disproved() {
  jq -r '
    (.obligations[]? | select(.outcome | IN("refuted", "outgrown", "vacuous", "defect"))
      | "FAILED \(.owner) \(.label): \(.outcome)\(if .location then " at \(.location)" else "" end)",
        "  \(del(.key, .owner, .label, .kind, .guarded, .frame, .location, .outcome, .tier) | tojson)"),
    (.diagnostics[]? | "\(.severity // "error") \(.code // ""): \(.message // "")")
  ' "$1" 2>/dev/null || cat "$1"
}

# A line, out whole beside the lanes printing next to it.
say() {
  if [[ -n $lock ]]; then flock "$lock" printf '%s\n' "$*"; else printf '%s\n' "$*"; fi
}

# Whether the deadline has come before the runs IDs of the lane NAME titled WHAT could start, which
# then says so in a block of its own.
too_late() {
  local name=$1 what=$2 block
  shift 2
  past_deadline || return 1
  block=$(mktemp)
  { echo "::group::$name: $what $*"; not_started "$@"; echo "::endgroup::"; } > "$block" 2>&1
  said "$block"
}

# The run ID as `run_one` runs it, in a block of the lane NAME, its wall clock onto TIMINGS.
single() {
  local name=$1 timings=$2 id=$3 status=0 started block
  too_late "$name" corpus "$id" && return 1
  say "$name: started corpus $id"
  block=$(mktemp)
  started=$(date +%s%3N)
  {
    echo "::group::$name: corpus $id"
    run_one "$timings" "$id" ${asked[@]+"${asked[@]}"} || status=$?
    echo "::endgroup::"
  } > "$block" 2>&1
  ((status == STOPPED)) || printf 'corpus\t%s\t%s\n' "$id" "$(($(date +%s%3N) - started))" >> "$timings"
  said "$block"
  return "$((status != 0))"
}

# The runs IDs of one package as RUNNER (`run_modules`) runs them, in a block of the
# lane NAME titled WHAT.
batch() {
  local name=$1 what=$2 runner=$3 timings=$4 status=0 block
  shift 4
  too_late "$name" "$what" "$@" && return 1
  say "$name: started $what $*"
  block=$(mktemp)
  {
    echo "::group::$name: $what $*"
    "$runner" "$timings" "$@" -- ${asked[@]+"${asked[@]}"} || status=1
    echo "::endgroup::"
  } > "$block" 2>&1
  said "$block"
  return "$status"
}

# One lane's runs, one after another: the program's own, each fixture's and each package's suite in a
# `ply test` of their own, every checks run in one, every run of the CLI's suite in one, the library's
# tests in one, and each part of a run cut into parts in one of its own. Each prints as it ends, a
# block under the lane's NAME, and once the deadline has come none starts.
lane() {
  local name=$1 timings=$2 id failed=0
  local durations=${2%.tsv}.durations
  local -a checks=() cli=() library=()
  shift 2
  : > "$timings"
  : > "$durations"
  for id in "$@"; do
    case "$id" in
      program | program#* | package-* | fixture-*) single "$name" "$timings" "$id" || failed=1 ;;
      *#*) batch "$name" "part ${id##*#} of ${id%#*}" run_modules "$timings" "$id" || failed=1 ;;
      cli-*) cli+=("$id") ;;
      stdlib:*) library+=("$id") ;;
      *) checks+=("$id") ;;
    esac
  done
  if [ "${#checks[@]}" -gt 0 ]; then
    batch "$name" "corpus checks" run_modules "$timings" "${checks[@]}" || failed=1
  fi
  if [ "${#cli[@]}" -gt 0 ]; then
    batch "$name" "the CLI's suite" run_modules "$timings" "${cli[@]}" || failed=1
  fi
  if [ "${#library[@]}" -gt 0 ]; then
    batch "$name" "the library's tests" run_modules "$timings" "${library[@]}" || failed=1
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
    lock=$work/lock
    stopped=$work/stopped
    : > "$lock"
    : > "$stopped"
    lanes=$(cut -d' ' -f1 <<< "$runs" | sort -un)
    pids=()
    for l in $lanes; do
      read -ra ids <<< "$(awk -v l="$l" '$1 == l { printf "%s ", $2 }' <<< "$runs")"
      lane "lane $l" "$work/lane-$l.tsv" "${ids[@]}" &
      pids+=("$!")
    done
    failed=0
    for pid in "${pids[@]}"; do wait "$pid" || failed=1; done
    for l in $lanes; do
      cat "$work/lane-$l.tsv" >> "$timings"
      cat "$work/lane-$l.durations" >> "$durations"
    done
    if [ -s "$stopped" ]; then
      echo "::error::partition $shard reached its deadline with $(grep -c '^stopped ' "$stopped") runs stopped and $(grep -c '^not-started ' "$stopped") not started: what the runs finished is kept, and a re-run of this job starts from it"
    fi
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
    run_modules "$timings" "${runs[@]}" -- "${desk_grants[@]}" "$@" || failed=1
    echo "::endgroup::"
    exit "$failed"
    ;;
  select)
    cut=${2:-}
    selecting=1
    asked=(--kept)
    work=$(mktemp -d)
    : > "$durations"
    # Each partition's lanes as `partition` runs them, each desk runner's runs as `desks` does and
    # each run alone as `run` does, all at once.
    pids=()
    asking=()
    for k in $(jq -r '.include[].shard' <<< "$("$shards" partitions)"); do
      (
        placed=$("$shards" corpus-for-partition "$k" "$cut") || exit 2
        for l in $(cut -d' ' -f1 <<< "$placed" | sort -un); do
          read -ra ids <<< "$(awk -v l="$l" '$1 == l { printf "%s ", $2 }' <<< "$placed")"
          lane "lane $l" "$work/lane-$k-$l.tsv" "${ids[@]}" > /dev/null 2>&1 || exit 1
        done
      ) &
      pids+=("$!")
      asking+=("partition $k")
    done
    for id in $(jq -r '.include[].id' <<< "$("$shards" corpus-matrix)"); do
      case "$id" in
        desks-*)
          taken=$("$shards" desks-for-runner "${id#desks-}" "$cut") || exit 2
          [ -n "$taken" ] || { echo "$id: takes no run" >&2; continue; }
          read -ra runs <<< "$(tr '\n' ' ' <<< "$taken")"
          run_modules "$work/$id.tsv" "${runs[@]}" -- "${desk_grants[@]}" --kept > /dev/null 2>&1 &
          ;;
        *) run_one "" "$id" --kept > /dev/null 2>&1 & ;;
      esac
      pids+=("$!")
      asking+=("$id")
    done
    working=()
    ids=()
    for i in "${!pids[@]}"; do
      if wait "${pids[$i]}"; then
        echo "${asking[$i]}: answered by what was kept" >&2
      else
        echo "${asking[$i]}: has runs to run" >&2
        case "${asking[$i]}" in
          partition\ *) working+=("${asking[$i]#partition }") ;;
          *) ids+=("${asking[$i]}") ;;
        esac
      fi
    done
    rm -rf "$work"
    printf 'partitions=%s\n' "$(jq -c --arg keep "${working[*]-}" \
      '($keep | split(" ")) as $k | { include: [.include[] | select(.shard | IN($k[]))] }' \
      <<< "$("$shards" partitions)")"
    printf 'lanes=%s\n' "$([ "${#working[@]}" -gt 0 ] && echo true || echo false)"
    printf 'corpus=%s\n' "$(jq -c --arg keep "${ids[*]-}" \
      '($keep | split(" ")) as $k | { include: [.include[] | select(.id | IN($k[]))] }' \
      <<< "$("$shards" corpus-matrix)")"
    printf 'solo=%s\n' "$([ "${#ids[@]}" -gt 0 ] && echo true || echo false)"
    ;;
  run)
    : > "$durations"
    run_one "" "${2:?a corpus entry}" "${@:3}"
    ;;
  sweep)
    : > "$durations"
    out=$(mktemp)
    status=0
    # Filters are alternatives, so one naming the whole module beside a case's would run every case.
    # A part of a module's cases may hold none of them.
    named=0
    parted=0
    for arg in "${@:2}"; do
      [[ $arg == --filter* ]] && named=1
      [[ $arg == --shard* ]] && parted=1
    done
    if ((named)); then
      tested "$cli_suite" "${@:2}" > "$out" || status=$?
    else
      tested "$cli_suite" --filter "edit_sweep." "${@:2}" > "$out" || status=$?
    fi
    listed "$out"
    selected=$(jq -s 'map(.selection.tests // [] | length) | add // 0' "$out" 2>/dev/null || echo 0)
    if [ "$status" -ne 0 ] || { [ "$selected" -eq 0 ] && ! ((named && parted)); }; then
      [ "$selected" -gt 0 ] || echo "the sweep selected no case" >&2
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
  pack)
    tar_out=${2:?a tar file}
    marked || exit 2
    work=$(mktemp -d)
    kept_answers "$work/c" || exit 1
    # A store is one file set: it travels whole, from the partition that wrote it.
    for store in "$root"/crates/*/ply/.ply-cache "$root"/crates/ply-corpus/checks/.ply-cache "$root"/crates/ply-corpus/stdlib/.ply-cache "$root"/tests/lang/.ply-cache; do
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
    for dir in "$root"/crates/*/ply "$root"/crates/ply-corpus/checks "$root"/tests/lang; do
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
    echo "usage: ci-corpus.sh partition K TIMINGS [CUT] | desks K TIMINGS CUT [ARG...] | run ID [ARG...] | select [CUT] | mark | keep DIR | pack TAR | unpack DIR C | restore DIR | compact | upstream-mark | upstream-new TAR | upstream-merge DIR" >&2
    exit 2
    ;;
esac
