#!/usr/bin/env bash
# Runs the `ply` commands of CI jobs, and saves and restores the caches they use. The subcommands
# are `run`, `nightly`, `mark`, `keep`, `restore`, `compact`, `hand` and `handed`. The usage message
# at the end of the script lists their arguments, and the comment above each step of the script says
# what that step does.
set -uo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ply="$root/target/debug/ply"
shards="$root/.github/ci-shards.sh"
# Each path `ply.work` gives a role, as the job's `ply` reads it.
work() { "$ply" work "$root" | awk -v r="$1" '$1 == r { print $2 }'; }
TAB=$'\t'
# The caches `ply` reads, where the workflow restores them.
caches=/tmp
mark=$caches/ply-c-runs.mark
# `ms<TAB>package<TAB>key` per test this job ran, which the job uploads for the run's table of what
# each test cost.
durations=$caches/ply-test-durations.tsv

# The directory holding postgres and its tools, for the library's tests that start a cluster: the
# first on PATH, else the newest of Debian's per-version directories, which are off PATH.
postgres_bin() {
  local found
  found=$(command -v initdb 2>/dev/null || true)
  if [[ -n $found ]]; then dirname "$found"; return; fi
  find /usr/lib/postgresql -maxdepth 2 -name bin -type d 2>/dev/null | sort -V | tail -1
}
pg=$(postgres_bin)

# The options for running the CLI's test suite. The suite's tests run CLI commands inside the test
# process, so they need the `ply` binary and `sh` and `git` to start, the working directory, the
# filesystem root and the repository as filesystems, and the permissions `--allow machine`, `--allow
# shipped` and `--allow toolchain`, which let tests read the machine, the files shipped with `ply`
# and the toolchain.
cli_grants=(--timeout 900000 --json
  --exec "ply=$ply" --exec sh=/bin/sh --exec "git=$(command -v git)"
  --fs cwd=. --fs abs=/ --fs "repo=$root"
  --allow machine --allow shipped --allow toolchain)

# The epoch second every `ply` this job starts must end by; empty for no end.
deadline=${PLY_CI_DEADLINE:-}
# A run's status when the deadline stopped it, or came before it could start.
STOPPED=124

past_deadline() { [[ -n $deadline ]] && (($(date +%s) >= deadline)); }

# Milliseconds since the epoch. `date +%s%3N` is GNU's: a Mac's `date` prints the `3N`.
now_ms() { perl -MTime::HiRes=time -e 'printf "%d\n", time * 1000'; }

# Runs the command, and stops it together with every process it started when the deadline arrives.
# Returns `STOPPED` if the deadline stopped the command, or if the deadline had already passed
# before the command started.
bounded() {
  local status=0
  [[ -n $deadline ]] || { "$@"; return; }
  past_deadline && return "$STOPPED"
  timeout --kill-after=30 "$((deadline - $(date +%s)))" "$@" || status=$?
  if ((status == 124 || status == 137)) && past_deadline; then return "$STOPPED"; fi
  return "$status"
}

# Prints an error saying that run ID was stopped at the job's deadline. The error includes the
# seconds elapsed since STARTED, a time in milliseconds since the epoch.
stopped_run() {
  echo "::error::the run $2 was stopped at the job's deadline after $((($(now_ms) - $1) / 1000))s: what it finished is cached, and a re-run of this job starts from it"
}

# Whether the run left a report at $1: a `ply` that crashed or was killed wrote none.
reported() {
  jq -e 'type == "object"' "$1" > /dev/null 2>&1
}

# Prints a message saying that run ID ended with STATUS and wrote no report. The tests that finished
# before it ended are shown in the log above.
unreported() {
  if (($2 > 128)); then
    echo "the run $1 crashed: signal $(($2 - 128)) ended it before it wrote a report" >&2
  else
    echo "the run $1 exited with status $2 and wrote no report" >&2
  fi
}

# Runs `ply test` on the package at PATH (relative to the repository) with ARGS, giving it the
# permissions its tests need.
#
# The CLI's suite runs from a new temporary directory that contains an empty `.ply-scratch` file,
# because its test harness empties the working directory before each test and refuses to run without
# that file.
#
# The standard library's suites get an emptied directory as the `scratch` filesystem, also with
# `.ply-scratch`. Tests that check the real handlers, which touch the real file system and programs
# rather than test doubles, write there. These tests also get permission to run `sh`, so that they
# can run the shell on the text that `std.text` quotes for it and check the result. The path of the
# scratch directory is the same in every run, because a cached test pass is valid only for the
# permissions it ran with.
#
# Any other package runs as plain `ply test` with no permissions.
tested() {
  local path=$1 status=0 dir
  shift
  if [[ $path == "$(work cli-tests)" ]]; then
    dir=$(mktemp -d)
    touch "$dir/.ply-scratch"
    (cd "$dir" && bounded "$ply" test "$root/$path" "$@" "${cli_grants[@]}") || status=$?
    rm -rf "$dir"
  elif [[ $path == "$(work std)" || $path == "$(work std-tests)" ]]; then
    dir=$caches/ply-std-scratch
    rm -rf "$dir" && mkdir -p "$dir" && touch "$dir/.ply-scratch"
    bounded "$ply" test "$path" --json "$@" --fs "scratch=$dir" --exec sh=/bin/sh \
      ${pg:+--exec "initdb=$pg/initdb" --exec "postgres=$pg/postgres" --exec "psql=$pg/psql"} ||
      status=$?
    rm -rf "$dir"
  else
    bounded "$ply" test "$path" --json "$@" || status=$?
  fi
  return "$status"
}

# What a red run said: each failed test and diagnostic, and a rejected run's diagnostics, each with
# its notes, which name what was rejected. The report itself is one line too long for a log to show.
red() {
  jq -r '
    (.failures[]? | ("FAILED \(.key // .name): \(.diagnostic.code // "") \(.diagnostic.message // "")",
      (.diagnostic.notes[]? | "  \(.)"))),
    (.diagnostics[]? | ("\(.severity // "error") \(.code // ""): \(.message // "")",
      (.notes[]? | "  \(.)")))
  ' "$1" 2>/dev/null || cat "$1"
}

# `ms<TAB>package<TAB>key` per test the report at $1, a run of the package at $2, ran.
timed() {
  jq -r --arg p "$2" '.results[]? | "\((.duration_ms // 0) | floor)\t\($p)\t\(.key // .name)"' "$1" 2>/dev/null
}

# Prints one line for each test the report selected, giving the reason it was selected. For a
# `changed` test the line also names what changed. The marker `unfiled` means the test passed but
# its result was not saved in the cache. The function also lists each test that was taken from the
# cache. The result of each test that ran was already printed when it finished.
listed() {
  jq -r '(.results // [] | map({key: (.key // .name), value: .cached}) | from_entries) as $ran
    | .selection.tests[]? | .key as $key
    | select(.reason == "cached" or ($ran | has($key)))
    | "\(.reason)\t\($key)\(if .moved then ": " + .moved else "" end)\(if $ran[$key] == false then " (unfiled)" else "" end)"' "$1" 2>/dev/null
}

# How the tests of a run ended, counted.
summed() {
  jq -r '.summary // {} | "summary: \(.passed // 0) passed, \(.failed // 0) failed, \(.abandoned // 0) abandoned, \(.cached // 0) cached"' "$1" 2>/dev/null
}

# A jq expression that appends a summary of how the report was loaded. A report's front end loads
# results by queries, and each query either takes its result from a store or computes it. The
# summary gives three counts: results taken from a store, results computed, and results computed
# even though the report named a stored result for them, which happens when that result is missing
# from every store. The expression yields nothing for a report that has no queries, such as one
# copied from a cached result.
answers='(if .front_end.queries then "; \([.front_end.queries[].taken] | add // 0) answers taken, \([.front_end.queries[].run] | add // 0) run and \(.front_end.lost // 0) lost" else "" end)'

# Succeeds if some report reports a stored result that it needed but could not find in any store.
# This happens only when a store is damaged, so stores that are intact never cause it, whatever
# source edits were made.
lost() {
  jq -s -e 'any(.[]; (.front_end.lost // 0) > 0)' "$1" > /dev/null 2>&1
}

# Prints where a `ply test` spent its wall-clock time WALL (in milliseconds), taken from its report.
# It lists each phase the report timed, counted from the start of the process, and adds `other` for
# the time after the report was written.
spent() {
  jq -r --argjson wall "$2" '
    def s(ms): (ms / 1000 | floor | tostring) + "s";
    (.phases // {}) as $p
    | (if .front_end.reused then "spent: a kept answer, read in \(s(.front_end.phases.total // 0)); it was filed by a run that spent " else "spent: " end)
      + "before \(s($p.before // 0)), load \(s($p.load // 0)), promises \(s($p.promises // 0)), store \(s($p.store // 0)), selection \(s($p.selection // 0)), handoff \(s($p.handoff // 0)), unit \(s($p.unit // 0)) (C analysis \(s((.backend.analysis_nanos // 0) / 1e6)), codegen \(s((.backend.codegen_nanos // 0) / 1e6))), bind \(s($p.bind // 0)), tests \(s($p.tests // 0)), conclude \(s($p.conclude // 0)), other \(s([$wall - ($p.total // 0), 0] | max))" + '"$answers"'
  ' "$1" 2>/dev/null
}

# Looks up the run ID and sets the caller's variables `path` to the package it tests, `picked` to
# one `--filter` option for each substring that selects its tests, and `part` to the part of the
# suite it runs. Returns 2 if the run is unknown.
placed() {
  local line rest filter
  line=$("$shards" run-line "$1") || return 2
  path=${line%%"$TAB"*}
  rest=${line#*"$TAB"}
  part=${rest#*"$TAB"}
  picked=()
  for filter in ${rest%%"$TAB"*}; do picked+=(--filter "$filter"); done
}

# `ply test` of the run ID with ARGs added: each test's result as it finishes, then why each ran,
# where the time went, and what failed. STOPPED when the deadline ended it.
tests_of() {
  local id=$1 path part status=0 selected out started
  local -a picked=()
  shift
  placed "$id" || return 2
  out=$(mktemp)
  started=$(now_ms)
  tested "$path" ${picked[@]+"${picked[@]}"} ${part:+--shard "$part"} "$@" > "$out" || status=$?
  if ((status == STOPPED)); then
    stopped_run "$started" "$id"
    rm -f "$out"
    return "$STOPPED"
  fi
  if ! reported "$out"; then
    unreported "$id" "$status"
    rm -f "$out"
    return 1
  fi
  listed "$out"
  summed "$out"
  timed "$out" "$path" >> "$durations"
  spent "$out" "$(($(now_ms) - started))"
  selected=$(jq -s 'map(.selection.tests // [] | length) | add // 0' "$out" 2>/dev/null || echo 0)
  if [ "$selected" -eq 0 ] && [ -n "$part" ]; then
    echo "the run $id selected no test: every test of its suite is in another part"
  elif [ "$selected" -eq 0 ]; then
    echo "the run $id selected no test" >&2
    status=1
  fi
  if lost "$out"; then
    echo "the run $id ran bodies again whose results a store had cached and lost" >&2
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
# package's (`laws-<id>`): each claim as it is decided, then the tally. It fails when it proves no
# claim. STOPPED when the deadline ended it.
proofs_of() {
  local id=$1 path part status=0 out started all cached discharging
  local -a picked=()
  shift
  placed "$id" || return 2
  out=$(mktemp)
  started=$(now_ms)
  bounded "$ply" prove "$path" --json "$@" > "$out" || status=$?
  if ((status == STOPPED)); then
    stopped_run "$started" "$id"
    rm -f "$out"
    return "$STOPPED"
  fi
  if ! reported "$out"; then
    unreported "$id" "$status"
    rm -f "$out"
    return 1
  fi
  discharging=$(jq '(.duration_ms // 0) | floor' "$out" 2>/dev/null || echo 0)
  all=$(jq '.obligations // [] | length' "$out" 2>/dev/null || echo 0)
  cached=$(jq '.cached // 0' "$out" 2>/dev/null || echo 0)
  echo "proved: $all claims, $cached from the cache, the rest discharged in $((discharging / 1000))s of the $((($(now_ms) - started) / 1000))s the prove took$(jq -r "$answers" "$out" 2>/dev/null)"
  if [ "$all" -eq 0 ]; then
    echo "the run $id proved no claim" >&2
    status=1
  fi
  if lost "$out"; then
    echo "the run $id ran bodies again whose results a store had cached and lost" >&2
    status=1
  fi
  if [ "$status" -ne 0 ]; then
    disproved "$out"
    rm -f "$out"
    return 1
  fi
  rm -f "$out"
}

# Prints why a proof run failed: each claim that was not proved, with its outcome and location, and
# the diagnostics of a run that `ply` rejected.
disproved() {
  jq -r '
    (.obligations[]? | select(.outcome != "proved")
      | "FAILED \(.owner) \(.label): \(.outcome)\(if .location then " at \(.location)" else "" end)",
        "  \(del(.key, .owner, .label, .kind, .guarded, .frame, .location, .outcome) | tojson)"),
    (.diagnostics[]? | ("\(.severity // "error") \(.code // ""): \(.message // "")",
      (.notes[]? | "  \(.)")))
  ' "$1" 2>/dev/null || cat "$1"
}

marked() {
  [ -f "$mark" ] || { echo "nothing is marked: run 'ci-ply.sh mark' before the runs" >&2; return 1; }
}

# Prints the absolute path of each directory under which a run keeps a store: every path that
# `ply.work` places for the standard library, compiler, CLI and packages, but not the test fixtures.
store_dirs() {
  local dir
  for dir in $(work std) $(work std-tests) $(work compiler) $(work cli) $(work cli-tests) \
    $(work package) $(work suite); do
    printf '%s\n' "$root/$dir"
  done | LC_ALL=C sort -u
}

# The directory, relative to the cache directory, that contains the stores of cached records,
# including the stored results of the C emitter. Each store is one subdirectory. The `sweep`
# command, which deletes the stale files of a cache, takes each store directory whole and does not
# look inside it (its `STORES` option).
# shellcheck disable=SC2100 # `ci-shards.sh verify` reads this line exactly as written.
stores=ply-c-cache/bodies

# Copies into DIR every store that a run used since `mark` was run, with all its files except those
# that a running `ply` creates while it uses the store: files named `run*`, the lock file and
# `*.tmp` files. The store's manifest, the file that lists which of its data files are current, and
# its segments, the data files holding the records, are copied.
kept_answers() {
  local dir=$1 store
  rm -rf "$dir"
  mkdir -p "$dir"
  for store in "$caches/$stores"/*/; do
    [ -f "$store.used" ] && [ "$store.used" -nt "$mark" ] || continue
    (cd "$caches" && tar --exclude='run*' --exclude=lock --exclude='*.tmp' -cf - \
      "$stores/$(basename "$store")") | tar -xf - -C "$dir"
  done
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
    role=${2:?usage: ci-ply.sh nightly ROLE [MODULE...] [ARG...]}
    : > "$durations"
    out=$(mktemp)
    status=0
    # Turns each leading MODULE argument into a `--filter` option, and passes every argument from
    # the first one that starts with `-` onward to `ply test` unchanged.
    args=()
    given=0
    for arg in "${@:3}"; do
      [[ $arg == -* ]] && given=1
      if ((given)); then args+=("$arg"); else args+=(--filter "$arg."); fi
    done
    [ "${#args[@]}" -gt 0 ] || { echo "usage: ci-ply.sh nightly ROLE [MODULE...] [ARG...]" >&2; exit 2; }
    # The library's TLS tests trust a certificate for `localhost` alone, made before the run, since
    # `ply` reads what `--trust` names as it starts.
    if [[ $role == std-tests ]]; then
      certificate=$(mktemp -d)
      openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 2 -subj /CN=localhost \
        -addext subjectAltName=DNS:localhost -addext basicConstraints=critical,CA:FALSE \
        -keyout "$certificate/localhost.key" -out "$certificate/localhost.pem" 2>/dev/null || exit 1
      args+=(--fs "certificate=$certificate" --trust "$certificate/localhost.pem")
    fi
    tested "$(work "$role")" "${args[@]}" > "$out" || status=$?
    if ! reported "$out"; then
      unreported nightly "$status"
      rm -f "$out"
      exit 1
    fi
    listed "$out"
    summed "$out"
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
    # Deletes each store that DIR contains from the cache directory before copying, so a saved store
    # replaces the existing one completely. The saved store grew from the existing one, and segment
    # files that belong to different manifests cannot be mixed in one store.
    for store in "$dir/$stores"/*/; do
      [ -d "$store" ] && rm -rf "${caches:?}/$stores/$(basename "$store")"
    done
    tar -C "$dir" -cf - . | tar -C "$caches" --skip-old-files -xf -
    ;;
  compact)
    marked || exit 2
    for dir in $(store_dirs); do
      [ -d "$dir/.ply-cache" ] || continue
      # Every `ply` that opens a store creates a new directory in it. If nothing in the store is
      # newer than the file that `ci-ply.sh mark` touched, no run since the mark opened the store.
      if [ -z "$(find "$dir/.ply-cache" -newer "$mark" -print -quit)" ]; then
        rm -rf "$dir/.ply-cache"
        echo "=== ${dir#"$root"/}: opened by no run of this job, and removed"
        continue
      fi
      echo "=== ${dir#"$root"/}"
      "$ply" cache compact "$dir" || echo "the store under ${dir#"$root"/} was not compacted" >&2
    done
    ;;
  hand)
    file=${2:?a file}
    kept=()
    for dir in $(store_dirs); do
      [ -d "$dir/.ply-cache" ] && kept+=("${dir#"$root"/}/.ply-cache")
    done
    [ "${#kept[@]}" -gt 0 ] || { echo "this job has no store"; exit 0; }
    tar -cf - -C "$root" "${kept[@]}" | zstd -q -T0 -3 -o "$file" || exit 1
    echo "packed ${#kept[@]} stores, $(du -sh "$file" | cut -f1): ${kept[*]}"
    ;;
  handed)
    name=${2:?an artifact}
    run=$("$root/.github/artifact-run.sh" "$name") || run=
    [ -n "$run" ] || { echo "no run has handed over $name"; exit 0; }
    dir=$(mktemp -d)
    if gh run download "$run" -n "$name" -D "$dir" && mkdir "$dir/unpacked" &&
      zstd -d -c "$dir/stores.tar.zst" | tar -xf - -C "$dir/unpacked"; then
      laid=0
      while IFS= read -r store; do
        there=$root/${store#"$dir/unpacked/"}
        rm -rf "$there"
        mkdir -p "$(dirname "$there")"
        mv "$store" "$there" || exit 1
        laid=$((laid + 1))
      done < <(find "$dir/unpacked" -type d -name .ply-cache -prune)
      echo "$name came back from run $run: $laid stores"
      echo "found=true" >> "${GITHUB_OUTPUT:-/dev/null}"
    else
      echo "$name did not come back from run $run"
    fi
    rm -rf "$dir"
    ;;
  *)
    echo "usage: ci-ply.sh run ID [ARG...] | nightly ROLE [MODULE...] [ARG...] | mark | keep DIR | restore DIR | compact | hand FILE | handed NAME" >&2
    exit 2
    ;;
esac
