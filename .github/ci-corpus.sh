#!/usr/bin/env bash
# The Ply runs (`ci-tables.sh runs`), each a `ply test` or a `ply prove` with the grants CI gives it:
# the corpus's with the grants `benches/corpus.sh` gives the program, the CLI's suite's from a scratch
# directory of the run's own with the grants its harness drives `ply` with, and a package's own suite
# as `ply test` runs it. A run fails when a test fails or a claim does not hold, when the run refuses,
# when it selects no test or proves no claim, and when its load ran a body again because a store had
# lost the answer its record names, which is a run made cold by a defect and not by an edit.
#
#   ci-corpus.sh all [EVENT]      every run, as many side by side as the machine's cores carry, each
#                                 printed whole as it ends. The CLI's suite is loaded once, by a
#                                 `ply test` that picks no test, and then runs in parts beside
#                                 each other. Under `schedule` or `workflow_dispatch` the suite's
#                                 nightly modules run too: the day's parts of the edit sweep, and
#                                 the rest whole.
#   ci-corpus.sh run ID [ARG...]  one run, or one module of the checks (`<module>`) or of the CLI's
#                                 suite (`cli-<module>`), with ARGs added to its `ply test` or
#                                 `ply prove`
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ply="$root/target/debug/ply"
tables="$root/.github/ci-tables.sh"
cli_suite=crates/ply-cli-tests/ply
stdlib=crates/ply-corpus/stdlib
TAB=$'\t'
# The passes earlier runs earned, each filed under the runtime that earned it.
export PLY_CACHE_UPSTREAM=${PLY_CACHE_UPSTREAM:-/tmp/ply-upstream}
# `ms<TAB>package<TAB>key` per test the runs ran, which the job uploads.
durations=/tmp/test-durations/tests.tsv

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

# Where `all` keeps its runs' blocks, the lock they print through, and what the runs leave each other.
work=
lock=

# FILE's lines, all at once, then FILE gone: a run's lines are written whole once it ends, so the
# run beside it never cuts into them.
said() {
  if [[ -n $lock ]]; then flock "$lock" cat "$1"; else cat "$1"; fi
  rm -f "$1"
}

# Milliseconds since the epoch, by bash's own clock where it has one: `date +%s%3N` is not three
# digits of the second under every `date`.
now_ms() {
  local t
  if [[ -n ${EPOCHREALTIME:-} ]]; then
    t=${EPOCHREALTIME/[.,]/}
    printf '%s\n' "${t:0:13}"
  else
    printf '%s000\n' "$(date +%s)"
  fi
}

say() {
  if [[ -n $lock ]]; then flock "$lock" printf '%s\n' "$*"; else printf '%s\n' "$*"; fi
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
    (cd "$dir" && "$ply" test "$root/$path" "$@" "${cli_grants[@]}") || status=$?
    rm -rf "$dir"
  elif [[ $path == "$stdlib" ]]; then
    "$ply" test --std "$path" --json "$@" || status=$?
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

# What a report's load took of the stores as kept, what it ran anew, and how many of those it ran
# although their records stood, the answers they name being in no store. Nothing for a report no load
# made: a kept answer's, or a refusal's.
answers='(if .front_end.queries then "; \([.front_end.queries[].taken] | add // 0) answers taken, \([.front_end.queries[].run] | add // 0) run and \(.front_end.lost // 0) lost" else "" end)'

# Whether a report's load lost an answer: none is lost over stores that are whole, whatever moved.
lost() {
  jq -s -e 'any(.[]; (.front_end.lost // 0) > 0)' "$1" > /dev/null 2>&1
}

# Where a `ply test` spent its wall clock WALL (ms), from its report: the phases its clock, which
# counts from the process's start, measured, and `other` for what follows the report.
spent() {
  jq -r --argjson wall "$2" '
    def s(ms): (ms / 1000 | floor | tostring) + "s";
    (.phases // {}) as $p
    | (if .front_end.reused then "spent: a kept answer, read in \(s(.front_end.phases.total // 0)); it was filed by a run that spent " else "spent: " end)
      + "before \(s($p.before // 0)), load \(s($p.load // 0)), promises \(s($p.promises // 0)), store \(s($p.store // 0)), selection \(s($p.selection // 0)), handoff \(s($p.handoff // 0)), unit \(s($p.unit // 0)) (C analysis \(s((.backend.analysis_nanos // 0) / 1e6)), codegen \(s((.backend.codegen_nanos // 0) / 1e6))), bind \(s($p.bind // 0)), tests \(s($p.tests // 0)), conclude \(s($p.conclude // 0)), other \(s([$wall - ($p.total // 0), 0] | max))" + '"$answers"'
  ' "$1" 2>/dev/null
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

# The package the run ID tests and a `--filter` per substring it picks its tests by, into the caller's
# `path` and `picked`.
placed() {
  local line filter
  line=$("$tables" line "$1") || return 2
  path=${line%%"$TAB"*}
  picked=()
  for filter in ${line#*"$TAB"}; do picked+=(--filter "$filter"); done
}

# `ply test` of the run ID with ARGs added: each test it ran, where its time went, and what failed. A
# run that selects no test fails, but for a part of one (`--shard`), which may hold none.
tests_of() {
  local id=$1 path status=0 out started selected arg parted=0
  local -a picked=()
  shift
  placed "$id" || return 2
  for arg in "$@"; do [[ $arg == --shard ]] && parted=1; done
  out=$(mktemp)
  started=$(now_ms)
  case "$id" in
    package-* | fixture-*) "$ply" test "$path" --json "$@" > "$out" || status=$? ;;
    *) tested "$path" ${picked[@]+"${picked[@]}"} "$@" > "$out" || status=$? ;;
  esac
  listed "$out"
  timed "$out" "$path" >> "$durations"
  spent "$out" "$(($(now_ms) - started))"
  selected=$(jq -s 'map(.selection.tests // [] | length) | add // 0' "$out" 2>/dev/null || echo 0)
  if [ "$selected" -eq 0 ] && ((!parted)); then
    echo "the run $id selected no test" >&2
    status=1
  fi
  if lost "$out"; then
    echo "the run $id ran bodies again whose answers a store had kept and lost" >&2
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
# package's (`laws-<id>`); it fails when it proves no claim.
proofs_of() {
  local id=$1 path status=0 out started all cached
  local -a picked=()
  shift
  placed "$id" || return 2
  out=$(mktemp)
  started=$(now_ms)
  if [[ $id == proofs ]]; then
    "$ply" prove --std "$path" "$@" --json > "$out" || status=$?
  else
    "$ply" prove "$path" --json "$@" > "$out" || status=$?
  fi
  all=$(jq '.obligations // [] | length' "$out" 2>/dev/null || echo 0)
  cached=$(jq '.cached // 0' "$out" 2>/dev/null || echo 0)
  echo "proved: $all claims, $cached from the cache, in $((($(now_ms) - started) / 1000))s$(jq -r "$answers" "$out" 2>/dev/null)"
  if [ "$all" -eq 0 ]; then
    echo "the run $id proved no claim" >&2
    status=1
  fi
  if lost "$out"; then
    echo "the run $id ran bodies again whose answers a store had kept and lost" >&2
    status=1
  fi
  if [ "$status" -ne 0 ]; then
    disproved "$out"
    rm -f "$out"
    return 1
  fi
  rm -f "$out"
}

run() {
  case "$1" in
    proofs | laws-*) proofs_of "$@" ;;
    *) tests_of "$@" ;;
  esac
}

# Loads the package of the run ID for the runs of it that start after: a `ply test` that picks no
# test, so each of them finds the package checked rather than checking it beside the others.
loaded() {
  local path
  local -a picked=()
  placed "$1" && tested "$path" --filter "no test is named this" > /dev/null 2>&1 || true
  touch "$work/loaded-$1"
}

# The command as a block of the log under NAME, printed whole when it ends, and listed if it failed.
unit() {
  local name=$1 status=0 block
  shift
  say "started: $name"
  block=$(mktemp)
  {
    echo "::group::$name"
    "$@" || status=$?
    echo "::endgroup::"
    ((status == 0)) || echo "::error::$name failed"
  } > "$block" 2>&1
  ((status == 0)) || echo "$name" >> "$work/failed"
  said "$block"
}

# The slowest tests of the runs, slowest first in the table the job uploads, and onto the job's
# summary where there is one.
summary() {
  [ -s "$durations" ] || return 0
  LC_ALL=C sort -t"$TAB" -k1,1nr -k2,2 -k3,3 -o "$durations" "$durations"
  [[ -n ${GITHUB_STEP_SUMMARY:-} ]] || return 0
  {
    echo "### The slowest tests"
    echo
    echo "| seconds | suite | test |"
    echo "| ---: | --- | --- |"
    head -30 "$durations" | awk -F'\t' '{ gsub(/\|/, "\\|", $3); printf "| %.1f | %s | %s |\n", $1 / 1000, $2, $3 }'
    echo
    awk -F'\t' '{ t += $1 } END { printf "%d tests, %.0f s in all; every row is in the `test-durations` artifact.\n", NR, t / 1000 }' "$durations"
  } >> "$GITHUB_STEP_SUMMARY"
}

mkdir -p "$(dirname "$durations")"

case "${1:-}" in
  all)
    event=${2:-}
    lanes=$((($(nproc) + 1) / 3))
    ((lanes >= 2)) || lanes=2
    work=$(mktemp -d)
    lock=$work/lock
    : > "$lock"
    : > "$work/failed"
    : > "$durations"
    # `name|the run whose load it waits for|command`, the load first.
    queue=("the CLI's suite, loaded||loaded cli")
    for id in $("$tables" runs); do
      case "$id" in
        cli)
          # Twice the lanes, so a part holding the suite's longest tests does not outlast the rest alone.
          for ((k = 1; k <= 2 * lanes; k++)); do
            queue+=("the CLI's suite, part $k of $((2 * lanes))|cli|run cli --shard $k/$((2 * lanes))")
          done
          for part in $("$tables" sweep-parts "${event:-push}"); do
            queue+=("the edit sweep, part $part|cli|run sweep --shard $part")
          done
          if [[ $event == schedule || $event == workflow_dispatch ]]; then
            queue+=("the CLI's suite, its nightly modules|cli|run nightly")
          fi
          ;;
        *) queue+=("$id||run $id") ;;
      esac
    done
    while ((${#queue[@]} > 0)); do
      waiting=()
      full=0
      for entry in "${queue[@]}"; do
        IFS='|' read -r name after command <<< "$entry"
        if ((full)) || [[ -n $after && ! -f $work/loaded-$after ]]; then
          waiting+=("$entry")
        elif (($(jobs -rp | wc -l) >= lanes)); then
          full=1
          waiting+=("$entry")
        else
          unit "$name" $command &
        fi
      done
      queue=(${waiting[@]+"${waiting[@]}"})
      ((${#queue[@]} == 0)) || wait -n || true
    done
    wait
    summary
    if [ -s "$work/failed" ]; then
      echo "failed:" >&2
      sed 's/^/  /' "$work/failed" >&2
      rm -rf "$work"
      exit 1
    fi
    rm -rf "$work"
    ;;
  run)
    : > "$durations"
    run "${2:?a run}" "${@:3}"
    ;;
  *)
    echo "usage: ci-corpus.sh all [EVENT] | run ID [ARG...]" >&2
    exit 2
    ;;
esac
