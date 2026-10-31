#!/usr/bin/env bash
# Holds the tables that list CI's test jobs and the caches that each run saves for later runs, and
# checks that the tables match the repository.
#
#   ci-shards.sh verify          checks that every crate is a workspace member and no crate's
#                                sources contain a test; that the jobs take every Ply run
#                                exactly once; that a job reads every cache key a job writes, and
#                                `supersede` cleans up each key's family; that a key naming the
#                                run is read only by later runs; that every action a workflow
#                                uses is listed with the cache entries it saves; and that every
#                                file `ply bootstrap` embeds is in a tree it reads
#   ci-shards.sh cache-keys      runs only the checks of cache keys, `supersede` and actions
#   ci-shards.sh toolchain-inputs [core]
#                                prints each path a bootstrap reads Ply from, one per line; the
#                                toolchain key is a hash of these. With `core`, prints of the
#                                standard library only the files the rest embeds
#   ci-shards.sh work ROLE       prints each path that `ply.work` gives ROLE, one per line
#   ci-shards.sh store-paths     prints where the runs keep their stores, one pattern per line, as
#                                the cache steps save and restore them
#   ci-shards.sh ply-trees       prints every tree of Ply sources that `ply.work` lists, one per line
#   ci-shards.sh nextest-shards  prints the JSON matrix of the nextest shards
#   ci-shards.sh durations FILE  prints `binary_id test milliseconds` for each test in a nextest
#                                JUnit report
#   ci-shards.sh runs-matrix     prints the JSON matrix of the jobs that take the Ply runs
#   ci-shards.sh job-runs ID     prints the runs that job ID takes, one per line
#   ci-shards.sh run-line ID     prints, separated by tabs, the package that run ID tests, the
#                                `ply test --filter` substrings that select its tests, and its
#                                part `K/N`, leaving a field empty when it does not apply. ID is a
#                                run, or one module of the CLI's suite (`cli-<module>`)
#   ci-shards.sh host-filter     prints the nextest filter that selects the host packages, which
#                                the hosts job runs and the shards skip
#   ci-shards.sh formatted-sources
#                                prints the maintained Ply sources, one per line, which the
#                                sources job checks with `ply fmt --check`
#   ci-shards.sh nightly-modules ROLE
#                                prints, on one line, the modules of suite ROLE that pull requests
#                                skip and the nightly job passes to `ci-ply.sh nightly`
#   ci-shards.sh supersede RUN REF
#                                deletes the cache entries of REF that no run will restore: those
#                                this run replaced, and those whose family no table here lists
#
# The commands that need package paths read them from `ply.work`, using the job's `ply`
# (`target/debug` or `target/release`) or the last release (`target/stage0`, which
# `scripts/stage0.sh` downloads).

set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

NEXTEST_SHARDS=2

TAB=$'\t'

# The Ply runs, each one `ply test` or `ply prove` of a package, which loads the package once: the
# library's tests (`stdlib`), its tests that read fixtures (`std-tests`) and its proofs (`proofs`),
# the CLI's suite in parts, each package's suite
# (`package-<id>`) and each package's proofs (`laws-<id>`), each package where `ply.work` places it
# (`workspace`).
# The parts the CLI's suite runs in, `cli-part-<k>`, each a job's `ply test --shard K/N`: its
# `test/nondet` tests drive `ply` in one working directory, so a `ply test` takes those one after
# another. A change to the compiler runs every test again, and each part must finish all of its
# tests within its job's deadline.
CLI_PARTS=4
# A package's own tests and proofs share one job, `packages`, in turn: each loads little. These take
# a job of their own instead: the compiler's own tests take every core.
RUNS_ALONE=(package-compiler)
# Lists the test modules that pull requests skip and the `nightly` job runs, as `role:module`, where
# `role` is the suite's role in `ply.work`. Each takes minutes: `repository_audit` checks every
# package from a cold start, `copies` audits every copy, and `db_driver` and `pg_client` start a
# postgres.
NIGHTLY=(cli-tests:repository_audit cli-tests:copies std-tests:db_driver std-tests:pg_client)
# Minutes left before a job's time limit once its runs reach their deadline (`PLY_CI_DEADLINE`). The
# steps that save the runs' output, such as cache saves, use this time.
RUNS_MARGIN=4
# Packages left out of the test shards (the slices that each job runs) because their tests open
# sockets and start processes, which a shard cannot do. The `hosts` job runs them.
HOST_PACKAGES=(ply-host-tests)

# Cache families whose keys are `<family>-<run id>`. A family is the key prefix that a group of
# related cache entries shares. A run restores only the newest entry. Do not pass data between the
# jobs of one run through such a key: other runs evict cache entries while those jobs wait for
# runners, so pass the data as an artifact. The `cache-keys` command rejects a key that contains the
# run id unless a later run restores that key.
SUPERSEDED=(ply-stores-)

# Cache families whose keys are `<family>-<digest>`, where the digest is computed from the entry's
# contents. A run restores only the newest entry that matches a `restore-keys` prefix, so older
# entries are never read and only use up the repository's 10 GB cache limit.
NEWEST=(nextest-archive- ply-runtime- ply-built- rust-passed- ply-c-stage-sources- ply-c-runs-)

# The families in `SUPERSEDED` and `NEWEST` that save a separate entry for each job, with keys named
# `<family><job id>-...`. If a job is no longer in the workflow's job list, no run restores its
# entries.
PER_JOB=(ply-stores- ply-c-runs-)

# A key that never moves: nothing replaces its one entry.
FIXED=(tcc-)

# The actions that the workflows use. Write `action:family` for an action that saves cache entries
# under its own keys, where `family` is the key prefix and a run restores the newest entry. Write
# the bare name for an action that saves only the cache entries that a workflow step spells out.
#
# `supersede` deletes every cache entry that belongs to no family, so an action missing from this
# list would lose its cache on every run. `verify` rejects a workflow that uses an action not listed
# here.
ACTIONS=(
  actions/cache
  actions/checkout
  actions/download-artifact
  actions/upload-artifact
  dtolnay/rust-toolchain
  "Swatinem/rust-cache:v0-rust-"
  taiki-e/install-action
)

# The file that says where a bootstrap reads a checkout's parts (`toolchain_inputs`).
TOOLCHAIN_WORKSPACE=ply.work


# The output of `ply work` for this checkout's `ply.work`, with one line per package in the form
# `role path`. `workspace` reads it once. `role` and the arrays built below read it to find the
# packages.
WORK=
workspace() {
  [[ -z $WORK ]] || return 0
  local ply dir
  for ply in "$root/target/debug/ply" "$root/target/release/ply" "$root/target/stage0/ply" ""; do
    [[ -z $ply || -x $ply ]] && break
  done
  if [[ -z $ply ]]; then
    echo "no ply here reads $TOOLCHAIN_WORKSPACE: fetch the last release with scripts/stage0.sh" >&2
    return 1
  fi
  WORK=$("$ply" work "$root") || { echo "$ply did not read $root/$TOOLCHAIN_WORKSPACE" >&2; return 1; }
  # The standard library, a project of its own in this checkout, and its tests that read fixtures.
  STD_TESTS=$(role std-tests)
  STD_LIBRARY=$(role std)
  CLI_SUITE=$(role cli-tests)
  # Packages whose test suites each run as a separate CI run, as `id:path`, where `id` is the
  # directory's name. A separate run names each failing test in the log, while one test that ran the
  # whole suite would report a single failure for all of them. A cached passing result of a separate
  # run is reused until the code that its tests run changes, while a test that ran the whole suite
  # would rerun whenever the CLI changes.
  PACKAGE_SUITES=()
  for dir in $(role compiler) $(role cli) $(role package) $(role suite); do
    PACKAGE_SUITES+=("${dir##*/}:$dir")
  done
  # Packages whose laws and contracts are checked by their own `ply prove` run, as `id:path`, with
  # the run named `laws-<id>`. Laws and contracts are the properties that a package states and `ply
  # prove` checks. The `proofs` run checks the standard library's laws. `verify` fails when a
  # package outside this list states a law.
  PACKAGE_PROOFS=()
  for dir in $(role proved); do PACKAGE_PROOFS+=("${dir##*/}:$dir"); done
  # The Ply sources the repository maintains. The sources job checks them with `ply fmt --check`,
  # because an unformatted committed source adds its reformatting to the next change that touches
  # it. Every committed `.ply` file is under one of these directories or under a fixture directory,
  # whose exact formatting is deliberate; `verify` checks this.
  FORMATTED_SOURCES=()
  for dir in $(role std) $(role std-tests) $(role compiler) $(role cli) $(role cli-tests) \
    $(role package) $(role suite); do
    FORMATTED_SOURCES+=("$dir")
  done
  FIXTURE_SOURCES=()
  for dir in $(role fixtures); do FIXTURE_SOURCES+=("$dir"); done
}

# Each path `ply.work` gives ROLE, one a line.
role() { awk -v r="$1" '$1 == r { print $2 }' <<< "$WORK"; }

# Every tree of Ply sources `ply.work` places, one a line.
cmd_ply_trees() {
  {
    role std
    role std-tests
    role compiler
    role cli
    role cli-tests
    role package
    role suite
    role fixtures
  } | LC_ALL=C sort -u
}

# Prints the paths where the runs store their data, as the cache steps save and restore them. It
# prints one pattern for each directory that contains packages, so adding a package there does not
# change the version of any cache entry.
cmd_store_paths() {
  local dir
  for dir in $(role std) $(role std-tests) $(role compiler) $(role cli) $(role cli-tests) \
    $(role package) $(role suite); do
    case "$dir" in
      */*) printf '%s/*/.ply-cache\n' "${dir%/*}" ;;
      *) printf '%s/.ply-cache\n' "$dir" ;;
    esac
  done | LC_ALL=C sort -u
}

named_in() {
  local x=$1 id
  shift
  for id in "$@"; do [[ $id == "$x" ]] && return 0; done
  return 1
}

# The qualified name of every test the package at DIR declares, `<module>.<label>`.
package_keys() {
  local file
  for file in "$root/$1"/*.ply; do
    test_names "$file" | sed "s/^/$(basename "$file" .ply)./"
  done
}

package_path() {
  local entry
  for entry in "${PACKAGE_SUITES[@]}"; do
    [[ ${entry%%:*} == "$1" ]] && { printf '%s\n' "${entry#*:}"; return 0; }
  done
  return 1
}

proofs_path() {
  local entry
  for entry in ${PACKAGE_PROOFS[@]+"${PACKAGE_PROOFS[@]}"}; do
    [[ ${entry%%:*} == "$1" ]] && { printf '%s\n' "${entry#*:}"; return 0; }
  done
  return 1
}

# Prints each `.ply` file under the compiler, CLI, CLI tests, package and proved directories that
# states a law or an `ensures`, `cost` or `space` clause. Only `ply prove` checks these.
claiming_files() {
  local dirs=() dir
  for dir in $(role compiler) $(role cli) $(role cli-tests) $(role package) $(role proved); do
    dirs+=("$root/$dir")
  done
  grep -rlE --include='*.ply' '^law "|^[[:space:]]+(ensures|cost|space) ' "${dirs[@]}" 2>/dev/null |
    sed "s|^$root/||" | LC_ALL=C sort || true
}

# The labels of a module's tests, in the order it declares them.
test_names() {
  sed -nE 's/^test(\/[a-z]+)? "([^"]*)".*/\2/p' "$1"
}

# Each module of the package at DIR that declares a test.
modules_of() {
  local file
  for file in "$root/$1"/*.ply; do
    if grep -qE '^test(/[a-z]+)? "' "$file"; then basename "$file" .ply; fi
  done
}

# Every Ply run a pull request takes, one id a line.
runs() {
  local entry k
  printf 'stdlib\nstd-tests\nproofs\n'
  for ((k = 1; k <= CLI_PARTS; k++)); do printf 'cli-part-%d\n' "$k"; done
  for entry in "${PACKAGE_SUITES[@]}"; do printf 'package-%s\n' "${entry%%:*}"; done
  for entry in ${PACKAGE_PROOFS[@]+"${PACKAGE_PROOFS[@]}"}; do printf 'laws-%s\n' "${entry%%:*}"; done
}

# Prints the job that executes the run `$1`. Package tests and package proofs run in the shared
# `packages` job unless the run is listed in `RUNS_ALONE`. The standard library's tests that read
# fixtures run in `stdlib`. Every other run is a job of its own, with the same name.
job_of() {
  case "$1" in
    std-tests) printf 'stdlib\n' ;;
    package-* | laws-*)
      if named_in "$1" "${RUNS_ALONE[@]}"; then printf '%s\n' "$1"; else printf 'packages\n'; fi
      ;;
    *) printf '%s\n' "$1" ;;
  esac
}

# The jobs, in the order `runs` lists the first run of each.
jobs() {
  local id
  runs | while read -r id; do job_of "$id"; done | awk '!seen[$0]++'
}

cmd_runs_matrix() { jobs | jq -Rnc '{include: [inputs | {id: .}]}'; }

# Prints the runs that the job ID executes, one per line. If no run belongs to that job, prints ID
# itself.
cmd_job_runs() {
  local job=$1 id taken=0
  while read -r id; do
    if [[ $(job_of "$id") == "$job" ]]; then
      printf '%s\n' "$id"
      taken=1
    fi
  done < <(runs)
  if ((!taken)); then printf '%s\n' "$job"; fi
}

# Prints the modules that `NIGHTLY` assigns to the nightly job, one per line. It uses the entries of
# `NIGHTLY` whose suite role in `ply.work` is ROLE.
nightly_of() {
  local entry
  for entry in "${NIGHTLY[@]}"; do
    if [[ ${entry%%:*} == "$1" ]]; then printf '%s\n' "${entry#*:}"; fi
  done
}

# Prints the test filters for a pull request's run of the suite at PATH, whose role in `ply.work` is
# ROLE. The output is one line with `<module>. ` for each module that `NIGHTLY` does not assign to
# the nightly job; a test's qualified name begins with its module and a dot. Prints nothing when
# `NIGHTLY` has no modules for that role.
suite_filters() {
  local role=$1 path=$2 module nightly
  nightly=$(nightly_of "$role")
  [[ -n $nightly ]] || return 0
  for module in $(modules_of "$path"); do
    grep -qxF -- "$module" <<< "$nightly" || printf '%s. ' "$module"
  done
}

cmd_run_line() {
  local id=$1 path part
  case "$id" in
    stdlib | proofs) printf '%s\t\t\n' "$STD_LIBRARY" ;;
    std-tests) printf '%s\t%s\t\n' "$STD_TESTS" "$(suite_filters std-tests "$STD_TESTS")" ;;
    cli-part-*)
      part=${id#cli-part-}
      if ! [[ $part =~ ^[1-9][0-9]*$ ]] || ((part > CLI_PARTS)); then
        echo "the CLI's suite runs in $CLI_PARTS parts, and '$id' is none of them" >&2
        return 1
      fi
      printf '%s\t%s\t%d/%d\n' "$CLI_SUITE" "$(suite_filters cli-tests "$CLI_SUITE")" "$part" "$CLI_PARTS"
      ;;
    package-*)
      path=$(package_path "${id#package-}") || { echo "no package suite named '${id#package-}'" >&2; return 1; }
      printf '%s\t\t\n' "$path"
      ;;
    laws-*)
      path=$(proofs_path "${id#laws-}") || { echo "no package's proofs named '${id#laws-}'" >&2; return 1; }
      printf '%s\t\t\n' "$path"
      ;;
    cli-*)
      [[ -f "$root/$CLI_SUITE/${id#cli-}.ply" ]] || { echo "the CLI's suite has no module '${id#cli-}'" >&2; return 1; }
      printf '%s\t%s.\t\n' "$CLI_SUITE" "${id#cli-}"
      ;;
    *)
      echo "no run named '$id'" >&2
      return 1
      ;;
  esac
}

# The job of the workflow whose steps run COMMAND.
job_running() {
  awk -v c="$1" '
    /^  [a-z-]+:$/ { job = $1; sub(/:$/, "", job) }
    index($0, c) { print job; exit }
  ' "$root/.github/workflows/ci.yml"
}

# The lines of the workflow's job JOB.
job_block() {
  awk -v j="  $1:" '$0 == j {f = 1; next} f && /^  [a-z]/ {exit} f' "$root/.github/workflows/ci.yml"
}

# The minutes after its start by which the job JOB ends its runs (`PLY_CI_DEADLINE`); nothing for a
# job that sets no deadline.
deadline_of() {
  job_block "$1" | sed -n 's/.*PLY_CI_DEADLINE=\$((\$(date +%s) + \([0-9][0-9]*\) \* 60)).*/\1/p'
}

cmd_host_filter() {
  local package first=1
  for package in "${HOST_PACKAGES[@]}"; do
    ((first)) || printf ' | '
    first=0
    printf 'package(%s)' "$package"
  done
  printf '\n'
}

cmd_nextest_shards() { matrix "$NEXTEST_SHARDS"; }

matrix() {
  local i
  printf '{"include":['
  for ((i = 1; i <= $1; i++)); do
    ((i > 1)) && printf ','
    printf '{"shard":"%d","of":"%d"}' "$i" "$1"
  done
  printf ']}\n'
}

# `binary_id test milliseconds` per testcase in a nextest JUnit report.
cmd_durations() {
  awk '
    function attribute(line, key,   mark) {
      mark = " " key "=\""
      if (!match(line, mark)) return ""
      line = substr(line, RSTART + length(mark))
      if (!match(line, "\"")) return ""
      return substr(line, 1, RSTART - 1)
    }
    /<testcase / {
      id = attribute($0, "classname")
      name = attribute($0, "name")
      if (id != "" && name != "") printf "%s\t%s\t%d\n", id, name, attribute($0, "time") * 1000 + 0.5
    }
  ' "$@"
}

# Every file CI's steps are written in: the workflows, and the actions they use, whose steps save
# and restore caches too.
ci_files() {
  find "$root/.github/workflows" "$root/.github/actions" -type f \( -name '*.yml' -o -name '*.yaml' \) |
    LC_ALL=C sort
}

# Checks that every cache key that a job saves is restored by some job, and that every restore
# matches a saved key. A rename that updates only the save or only the restore leaves a cache that
# no run restores. A restore of a key that nothing saves never finds an entry. Either way the run is
# only slower, not failed.
#
# Only the literal text before the first `${{ ... }}` is compared. A restore can match only that
# prefix, and the save and the restore both write it out literally.
#
# A cache may hold only data whose absence slows a run without breaking it. Other runs evict entries
# while a run's jobs wait for runners, so no restore uses `fail-on-cache-miss`.
#
# A key that contains the run id can be restored only by a later run, so the check requires a later
# run to restore it.
#
# No job restores a key that it saved itself. The cache may not show a new entry immediately after
# the save, so the restore could fail even though the save worked.
cmd_cache_keys() {
  local files=() file family
  for family in ${PER_JOB[@]+"${PER_JOB[@]}"}; do
    if ! named_in "$family" "${SUPERSEDED[@]}" "${NEWEST[@]}"; then
      echo "FAIL: PER_JOB names \"$family\", which neither SUPERSEDED nor NEWEST does" >&2
      return 1
    fi
  done
  while IFS= read -r file; do files+=("$file"); done < <(ci_files)
  awk -v families="${SUPERSEDED[*]} ${NEWEST[*]} ${FIXED[*]-}" -v perjob="${PER_JOB[*]-}" '
    function literal(s) {
      sub(/\$\{\{.*/, "", s)
      gsub(/^[[:space:]"]+|[[:space:]"]+$/, "", s)
      return s
    }
    function per_job(key,   p, i, m) {
      m = split(perjob, p, " ")
      for (i = 1; i <= m; i++) if (key == p[i]) return 1
      return 0
    }
    # A `key:` inside a save step is a write. A `key:` inside a restore step, or an entry in a
    # `restore-keys` list, is a read. An entry that is only an expression has no literal text, so
    # there is nothing to compare.
    #
    # A write whose key contains the run id belongs to the current run. A later run cannot know that
    # id, so it reads through `restore-keys` prefixes, which do not contain it. `late` records those
    # prefixes as the reads that later runs make.
    function note(kind, value, key) {
      key = literal(value)
      if (key == "") return
      steplit[++nlit] = key
      if (kind == "save") {
        jobsaved[key] = FILENAME ":" FNR
        if (per_job(key) && index(value, key "${{ matrix.id }}-") == 0) jobless[++njobless] = FILENAME ":" FNR " saves \"" key "\""
        if (key in saved) return
        saved[key] = 1
        where[key] = FILENAME ":" FNR
        if (index(value, "github.run_id") > 0) run_scoped[key] = 1
        order[++n] = key
      } else if (kind == "late") {
        late[key] = 1
        read[key] = 1
      } else {
        read[key] = 1
        if (key in jobsaved) readback[++nreadback] = FILENAME ":" FNR " restores \"" key "\", which its job saved at " jobsaved[key]
      }
    }
    # Ends the current step and records its keys. For a save step, each key is mapped to the path of
    # the step. For a restore step, the key is recorded with the path of the step, because the path
    # is part of the version of a cache entry: a restore finds only entries saved with the same path.
    function flush(i) {
      for (i = 1; i <= nlit; i++) {
        if (mode == "save") savepath[steplit[i]] = steppath
        else if (mode == "restore") { rlit[++nr] = steplit[i]; rpath[nr] = steppath; rwhere[nr] = stepwhere }
      }
      if (mode == "restore" && stepfail && steprun) needed[++nneeded] = stepwhere
      nlit = 0; steppath = ""; inpath = 0; stepfail = 0; steprun = 0
    }
    FNR == 1 { flush(); mode = ""; inkeys = 0; indent = 0; split("", jobsaved) }
    # A job of a workflow is a run of steps of its own.
    /^  [A-Za-z0-9_-]+:[[:space:]]*$/ { flush(); mode = ""; inkeys = 0; split("", jobsaved); next }
    # A new list item starts a new step. The rules below apply to the step being read.
    /^[[:space:]]*-[[:space:]]/ { flush(); mode = ""; inkeys = 0 }
    /uses:[[:space:]]*actions\/cache\/save@/ { mode = "save"; inkeys = 0; stepwhere = FILENAME ":" FNR; next }
    /uses:[[:space:]]*actions\/cache\/restore@/ { mode = "restore"; inkeys = 0; stepwhere = FILENAME ":" FNR; next }
    mode == "" { next }
    /^[[:space:]]*fail-on-cache-miss:[[:space:]]*true/ { stepfail = 1; next }
    inpath {
      line = $0
      sub(/^[[:space:]]*/, "", line)
      if (line != "" && length($0) - length(line) > pindent) { steppath = steppath (steppath == "" ? "" : ",") line; next }
      inpath = 0
    }
    /^[[:space:]]*path:/ {
      rest = $0
      sub(/.*path:[[:space:]]*/, "", rest)
      pindent = match($0, /[^ ]/) - 1
      if (rest == "|") inpath = 1
      else steppath = rest
      next
    }
    inkeys && /^[[:space:]]*$/ { next }
    inkeys {
      line = $0
      sub(/^[[:space:]]*/, "", line)
      if (length($0) - length(line) <= indent) inkeys = 0
      else { note("late", line); next }
    }
    /^[[:space:]]*restore-keys:/ {
      rest = $0
      sub(/.*restore-keys:[[:space:]]*/, "", rest)
      indent = match($0, /[^ ]/) - 1
      if (rest == "|") { inkeys = 1; next }
      if (rest != "") note("late", rest)
      next
    }
    /^[[:space:]]*key:/ {
      rest = $0
      sub(/.*key:[[:space:]]*/, "", rest)
      if (mode == "restore" && index(rest, "github.run_id") > 0) steprun = 1
      note(mode, rest)
    }
    END {
      flush()
      bad = 0
      for (i = 1; i <= nr; i++)
        for (k in savepath)
          if (index(k, rlit[i]) == 1 && savepath[k] != rpath[i] && !((rwhere[i], rlit[i]) in told)) {
            told[rwhere[i], rlit[i]] = 1
            printf "FAIL: %s restores \"%s\" into %s, and \"%s\" is saved from %s: a path is part of an entry'"'"'s version, so the restore never finds it\n", rwhere[i], rlit[i], rpath[i], k, savepath[k] > "/dev/stderr"
            bad = 1
          }
      if (n == 0 || length(read) == 0) {
        printf "FAIL: read %d cache key literal(s) written and %d restored -- the check would pass vacuously\n", n, length(read) > "/dev/stderr"
        exit 1
      }
      for (i = 1; i <= n; i++) {
        ok = 0
        for (r in read) if (index(order[i], r) == 1) { ok = 1; break }
        if (!ok) {
          printf "FAIL: %s writes cache key \"%s\", which no restore matches\n", where[order[i]], order[i] > "/dev/stderr"
          bad = 1
        }
      }
      for (r in read) {
        ok = 0
        for (k in saved) if (index(k, r) == 1) { ok = 1; break }
        if (!ok) {
          printf "FAIL: a restore matches cache key \"%s\", which nothing writes\n", r > "/dev/stderr"
          bad = 1
        }
      }
      # A key that contains the run id can be matched only by a `restore-keys` entry in a later run.
      # If no later run reads the key, the key cannot pass the work of this run to the other jobs of
      # this run. An artifact does that.
      for (i = 1; i <= n; i++) {
        if (!(order[i] in run_scoped)) continue
        ok = 0
        for (r in late) if (index(order[i], r) == 1) { ok = 1; break }
        if (!ok) {
          printf "FAIL: %s writes run-scoped cache key \"%s\", which no later run reads: what a job hands the jobs of its own run goes as an artifact\n", where[order[i]], order[i] > "/dev/stderr"
          bad = 1
        }
      }
      # And a job that cannot go on without an entry of its own run fails whenever the entry was
      # evicted, and a re-run of it fails the same way.
      for (i = 1; i <= nneeded; i++) {
        printf "FAIL: %s restores a key that names the run with fail-on-cache-miss: what a job needs from another job of its run goes as an artifact\n", needed[i] > "/dev/stderr"
        bad = 1
      }
      for (i = 1; i <= nreadback; i++) {
        printf "FAIL: %s: the cache does not always show a key at once after a save\n", readback[i] > "/dev/stderr"
        bad = 1
      }
      nf = split(families, fk, " ")
      for (j = 1; j <= nf; j++) {
        if (fk[j] == "") continue
        ok = 0
        for (k in saved) if (index(k, fk[j]) == 1) { ok = 1; break }
        if (!ok) {
          printf "FAIL: SUPERSEDED, NEWEST or FIXED names \"%s\", which no save writes\n", fk[j] > "/dev/stderr"
          bad = 1
        }
      }
      # `supersede` deletes an entry no family names: a key outside them is gone by the next run.
      for (i = 1; i <= n; i++) {
        ok = 0
        for (j = 1; j <= nf; j++) if (fk[j] != "" && index(order[i], fk[j]) == 1) { ok = 1; break }
        if (!ok) {
          printf "FAIL: %s writes cache key \"%s\", which no family of SUPERSEDED, NEWEST or FIXED names, so `supersede` would delete it before a run restored it\n", where[order[i]], order[i] > "/dev/stderr"
          bad = 1
        }
      }
      for (i = 1; i <= njobless; i++) {
        printf "FAIL: %s without the job id next: PER_JOB reads an entry as its family, then its job, and deletes one of a job the tree does not list\n", jobless[i] > "/dev/stderr"
        bad = 1
      }
      if (bad) exit 1
      printf "cache keys: %d written and %d restored, each side matched by the other and each of a family `supersede` prunes; %d run-scoped, each read by a later run and needed by no job\n", n, length(read), length(run_scoped)
    }
  ' "${files[@]}"
}

# Prints PATH with each `.` segment dropped and each `..` segment removed together with the segment
# before it. It changes only the path text and does not look at the filesystem, so symbolic links
# are not resolved.
lexical() {
  awk -v p="$1" 'BEGIN {
    n = split(p, s, "/"); k = 0
    for (i = 1; i <= n; i++) {
      if (s[i] == "" || s[i] == ".") continue
      if (s[i] == "..") { if (k > 0) k--; continue }
      out[++k] = s[i]
    }
    for (i = 1; i <= k; i++) printf "%s%s", (i > 1 ? "/" : ""), out[i]
    printf "\n"
  }'
}

# Prints each path in the repository that a bootstrap reads Ply from, one per line. These are
# `ply.work`, the paths that `ply.work` assigns to the standard library, the compiler and the CLI,
# and every package that a manifest in those paths lists as a path dependency, including
# dependencies of dependencies.
#
# With scope `core`, the standard library's path is left out. The output then also includes each
# file that the remaining paths embed from outside those paths, such as the prelude, which the
# compiler embeds and from which the checker reads builtins.
toolchain_inputs() {
  local scope=${1:-all} all prev dir rel file target
  all=$({
    printf '%s\n' "$TOOLCHAIN_WORKSPACE"
    [[ $scope == core ]] || role std
    role compiler
    role cli
  } | LC_ALL=C sort -u)
  prev=
  while [[ $all != "$prev" ]]; do
    prev=$all
    all=$(
      {
        printf '%s\n' "$prev"
        while IFS= read -r dir; do
          [ -f "$root/$dir/ply.pkg" ] || continue
          sed -n 's/.*source: *Path("\([^"]*\)").*/\1/p' "$root/$dir/ply.pkg" |
            while IFS= read -r rel; do lexical "$dir/$rel"; done
        done <<< "$prev"
        if [[ $scope == core ]]; then
          embeds_from <<< "$prev" | while IFS=: read -r file target; do
            covered "$target" "$prev" || printf '%s\n' "$target"
          done
        fi
      } | grep . | LC_ALL=C sort -u
    )
  done
  printf '%s\n' "$all"
}

# Reads paths from standard input and prints `file:target` for each `embed` or `embed_dir` that can
# refer to a file outside the path it appears in. `target` is a path from the repository root. In a
# directory, only an embed whose path climbs out of its module's directory with `..` is listed. In a
# single file, every embed is listed. A line that starts with `\\` is part of a program that a test
# writes, not an embed, and is skipped.
embeds_from() {
  local path pattern file rel
  while IFS= read -r path; do
    if [ -d "$root/$path" ]; then
      pattern='embed(_dir)?\("[^"]*\.\.[^"]*"'
    elif [ -f "$root/$path" ]; then
      pattern='embed(_dir)?\("[^"]*"'
    else
      continue
    fi
    { grep -rHE --include='*.ply' "$pattern" "$root/$path" || true; } |
      { grep -vE '^[^:]*:[[:space:]]*\\\\' || true; } |
      sed -E 's/^([^:]*):.*embed(_dir)?\("([^"]*)".*/\1:\3/' |
      while IFS=: read -r file rel; do
        [[ -n $rel ]] || continue
        file=${file#"$root"/}
        printf '%s:%s\n' "$file" "$(lexical "$(dirname "$file")/$rel")"
      done
  done
}

# Whether PATH lies in one of the paths INPUTS lists, a line each.
covered() {
  local target=$1 path
  while IFS= read -r path; do
    [[ -n $path && ($target == "$path" || $target == "$path"/*) ]] && return 0
  done <<< "$2"
  return 1
}

# Checks that every file that a bootstrap embeds from outside its source trees lies inside a path
# that the toolchain key hashes, for both the full key and its `core` part (`toolchain_inputs`).
# Otherwise a cached `ply` could be reused after an embedded file changed.
cmd_toolchain_embeds() {
  local bad=0
  toolchain_embeds all || bad=$((bad + 1))
  toolchain_embeds core || bad=$((bad + 1))
  return $((bad > 0))
}

# Checks, for the scope SCOPE (`all` or `core`), that every embed found in the paths that
# `toolchain_inputs SCOPE` prints points to an existing file inside one of those paths. Fails when
# an embed points outside them or to a missing file.
toolchain_embeds() {
  local scope=$1 inputs trees file target bad=0
  inputs=$(toolchain_inputs "$scope")
  trees=$(grep -vxF -- "$TOOLCHAIN_WORKSPACE" <<< "$inputs" || true)
  if [[ -z $trees ]]; then
    echo "FAIL: $TOOLCHAIN_WORKSPACE places no part of the toolchain, so the toolchain key is held to nothing" >&2
    return 1
  fi
  while IFS=: read -r file target; do
    if ! covered "$target" "$inputs"; then
      echo "FAIL: $file embeds $target, outside every path the $scope toolchain key hashes" >&2
      bad=$((bad + 1))
    elif [ ! -e "$root/$target" ]; then
      echo "FAIL: $file embeds $target, which the tree does not hold" >&2
      bad=$((bad + 1))
    fi
  done < <(embeds_from <<< "$inputs")
  if [ "$bad" -eq 0 ]; then
    printf 'toolchain key (%s): the %d paths a bootstrap reads Ply from hold every file they embed\n' "$scope" "$(grep -c . <<< "$inputs")"
  fi
  return $((bad > 0))
}

# Checks that every input listed in the dependency-info file DEPINFO of a fresh build matches a
# pattern that the Rust key hashes. An input that the key missed would let a cached runtime or
# archive be reused although it was built from different sources. A path outside the repository
# belongs to a registry crate, which `Cargo.lock` pins, so it is skipped.
cmd_rust_inputs() {
  local depinfo=${1:?usage: ci-shards.sh rust-inputs DEPINFO} action="$root/.github/actions/rust-key/action.yml"
  local patterns=() pattern dep rel part missing=0 checked=0 covered
  while IFS= read -r pattern; do
    pattern=$(printf '%s' "$pattern" | tr -d "\"' ")
    [ -n "$pattern" ] && patterns+=("$pattern")
  done < <(grep -oh 'hashFiles([^)]*)' "$action" | sed -e 's/.*hashFiles(//' -e 's/)$//' | tr ',' '\n')
  [ "${#patterns[@]}" -gt 0 ] || { echo "FAIL: $action hashes no pattern" >&2; return 1; }
  while IFS= read -r dep; do
    [ -n "$dep" ] || continue
    case "$dep" in "$root"/*) dep=${dep#"$root"/} ;; /*) continue ;; esac
    # Normalizes the path as text, without looking at the filesystem, to match how the key's
    # patterns write repository paths: `a/../b` becomes `b`.
    local parts=()
    IFS=/ read -r -a segments <<< "$dep"
    for part in "${segments[@]}"; do
      case "$part" in
        "" | .) ;;
        ..) [ "${#parts[@]}" -gt 0 ] && unset 'parts[${#parts[@]}-1]' ;;
        *) parts+=("$part") ;;
      esac
    done
    rel=$(IFS=/; printf '%s' "${parts[*]}")
    case "$rel" in target/*) continue ;; esac
    checked=$((checked + 1))
    covered=no
    for pattern in "${patterns[@]}"; do
      # shellcheck disable=SC2053 # the pattern is a glob on purpose
      if [[ $rel == $pattern || $rel/ == $pattern ]]; then covered=yes; break; fi
    done
    if [ "$covered" = no ]; then
      echo "FAIL: the build read $rel, which no pattern of the Rust key covers" >&2
      missing=$((missing + 1))
    fi
  done < <(awk '
    { i = index($0, ":"); if (i == 0) next
      rest = substr($0, i + 1)
      gsub(/\\ /, "\001", rest)
      n = split(rest, a, / +/)
      for (j = 1; j <= n; j++) if (a[j] != "") { gsub(/\001/, " ", a[j]); print a[j] } }
  ' "$depinfo")
  [ "$checked" -gt 0 ] || { echo "FAIL: $depinfo names no input" >&2; return 1; }
  [ "$missing" -eq 0 ] && echo "rust inputs: all $checked inputs the build read are under the Rust key"
  return $((missing > 0))
}

# Prints the cache families for the prefix PREFIX, separated by spaces. If `PER_JOB` lists PREFIX,
# it prints one family per job, `<PREFIX><job id>-`. Otherwise it prints PREFIX itself.
families_of() {
  local id
  if named_in "$1" ${PER_JOB[@]+"${PER_JOB[@]}"}; then
    jobs | while read -r id; do printf '%s%s- ' "$1" "$id"; done
  else
    printf '%s ' "$1"
  fi
}

# Reads cache entries from stdin as tab-separated `id created key` lines and prints `id why key` for
# each entry that `supersede` deletes. Entries whose keys differ only by run id and trailing hash
# digests form a group. An entry is deleted when: (1) its group is in SUPERSEDED, it was saved by an
# earlier run, and run RUN has saved an entry in the same group; (2) its group is in NEWEST and it
# is not the newest entry of the group; or (3) its key belongs to no group listed in SUPERSEDED,
# NEWEST or the actions table, such as an entry of a retired job. An entry whose key matches FIXED
# is never deleted.
prune_plan() {
  LC_ALL=C sort -t "$TAB" -k2,2r | awk -F'\t' -v OFS='\t' -v run="$1" -v superseded="$2" -v newest="$3" -v fixed="$4" '
    function named(key, list,   p, i, m) {
      m = split(list, p, " ")
      for (i = 1; i <= m; i++) if (index(key, p[i]) == 1) return 1
      return 0
    }
    # Returns the key with its trailing digests removed. A digest is a hyphen followed by eight or
    # more hex digits.
    function family(key) {
      while (match(key, /-[0-9a-f]+$/) && RLENGTH > 8) key = substr(key, 1, RSTART - 1)
      return key
    }
    # `run_of` returns the run id at the end of a key. `scoped` returns the key without that run id
    # and without the hash digests before it, so all entries of one job have the same scoped key
    # whichever toolchain wrote them.
    function run_of(key,   f) {
      f = key
      sub(/-[0-9]+$/, "", f)
      return substr(key, length(f) + 2) + 0
    }
    function scoped(key) {
      sub(/-[0-9]+$/, "", key)
      return family(key)
    }
    {
      id[NR] = $1; key[NR] = $3
      if (named($3, superseded) && run_of($3) == run + 0) saved[scoped($3)] = 1
    }
    END {
      for (i = 1; i <= NR; i++) {
        k = key[i]
        if (named(k, fixed)) continue
        if (named(k, superseded)) {
          if (run_of(k) < run + 0 && scoped(k) in saved) print id[i], "superseded", k
        } else if (named(k, newest)) {
          if (seen[family(k)]++) print id[i], "superseded", k
        } else {
          print id[i], "unnamed", k
        }
      }
    }
  '
}

# Checks `prune_plan` against a fixed listing that contains an entry for each of its rules, and
# fails if it would delete different entries than expected.
cmd_prune_check() {
  local a b got want
  a=$(printf 'a%.0s' {1..40})
  b=$(printf 'b%.0s' {1..40})
  want="1 superseded;4 unnamed;7 superseded;11 superseded;12 unnamed;13 superseded;17 unnamed;19 superseded;"
  got=$(printf '%s\t%s\t%s\n' \
    1 2026-01-01T00:00:00Z ply-stores-stdlib-100 \
    2 2026-01-02T00:00:00Z "ply-stores-stdlib-$b-200" \
    3 2026-01-01T00:00:00Z ply-stores-proofs-100 \
    4 2026-01-01T00:00:00Z ply-stores-cli-part-3-100 \
    7 2026-01-01T00:00:00Z "ply-built-Linux-X64-$a" \
    8 2026-01-02T00:00:00Z "ply-built-Linux-X64-$b" \
    9 2026-01-01T00:00:00Z "ply-built-macOS-ARM64-$a" \
    10 2026-01-02T00:00:00Z "ply-c-runs-stdlib-$a" \
    11 2026-01-01T00:00:00Z "ply-c-runs-stdlib-$b" \
    12 2026-01-02T00:00:00Z "ply-c-runs-cli-part-3-$a" \
    13 2026-01-01T00:00:00Z v0-rust-build-Linux-x64-11111111-22222222 \
    14 2026-01-02T00:00:00Z v0-rust-build-Linux-x64-11111111-33333333 \
    15 2026-01-01T00:00:00Z v0-rust-clippy-Linux-x64-11111111-22222222 \
    16 2026-01-01T00:00:00Z tcc-Linux-X64 \
    17 2026-01-02T00:00:00Z sccache/a/b/c/abcdef \
    18 2026-01-03T00:00:00Z "ply-stores-stdlib-$b-300" \
    19 2026-01-01T00:00:00Z "ply-stores-stdlib-$a-150" |
    prune_plan 200 "ply-stores-stdlib- ply-stores-proofs-" "ply-built- ply-c-runs-stdlib- v0-rust-" "tcc-" |
    cut -f1,2 | LC_ALL=C sort -n | tr '\t\n' ' ;')
  if [[ $got != "$want" ]]; then
    echo "FAIL: of the listing it is held to, \`supersede\` would delete '$got' and should delete '$want'" >&2
    return 1
  fi
  echo "cache pruning: each rule of \`supersede\` decides the listing it is held to"
}

# Fails unless every action the workflows use is listed in ACTIONS and every action listed there is
# used. Each listed action names the cache entries it saves, or says it saves none.
cmd_actions() {
  local file use entry name bad=0 listed="" used=""
  for entry in "${ACTIONS[@]}"; do listed+="${entry%%:*}"$'\n'; done
  while IFS= read -r file; do
    while IFS= read -r use; do
      name=$(awk -F/ '{ print $1 "/" $2 }' <<< "${use%%@*}")
      used+="$name"$'\n'
      if ! grep -qxF -- "$name" <<< "$listed"; then
        echo "FAIL: ${file#"$root"/} uses $name, which ACTIONS does not list: \`supersede\` deletes an entry no family names, so say whether it saves cache entries under keys of its own" >&2
        bad=$((bad + 1))
      fi
    done < <(sed -n -E 's/^[[:space:]-]*uses:[[:space:]]*([^[:space:]#]+).*/\1/p' "$file" | grep -v '^\./' || true)
  done < <(ci_files)
  while IFS= read -r name; do
    [[ -n $name ]] || continue
    if ! grep -qxF -- "$name" <<< "$used"; then
      echo "FAIL: ACTIONS lists $name, which no workflow uses -- delete the entry" >&2
      bad=$((bad + 1))
    fi
  done <<< "$listed"
  [ "$bad" -eq 0 ] && echo "actions: each of the $(grep -c . <<< "$listed") the workflows use is listed with the cache entries it saves"
  return $((bad > 0))
}

# Returns success when no later run will read the cache entries of REF: for a pull request, once it
# is closed; for a branch, while no pull request is open from it. Returns failure when the state of
# REF cannot be fetched, so its entries are not deleted.
ref_done() {
  local ref=$1 number state open
  case "$ref" in
    refs/pull/*/merge)
      number=${ref#refs/pull/}
      number=${number%/merge}
      state=$(gh api "repos/$GITHUB_REPOSITORY/pulls/$number" -q .state 2> /dev/null) || return 1
      [[ $state == closed ]]
      ;;
    refs/heads/*)
      open=$(gh api --method GET "repos/$GITHUB_REPOSITORY/pulls" -f state=open -f per_page=1 \
        -f head="${GITHUB_REPOSITORY%%/*}:${ref#refs/heads/}" -q length 2> /dev/null) || return 1
      [[ $open == 0 ]]
      ;;
    *) return 1 ;;
  esac
}

# Deletes the cache entries of REF that no later run will read, as chosen by `prune_plan`. When REF
# is main, it also deletes the entries of every other ref that is finished (`ref_done`), because the
# runs of that ref are gone and cannot delete them.
cmd_supersede() {
  local run=${1:?usage: ci-shards.sh supersede RUN_ID REF} ref=${2:?a ref}
  local prefix entry id why key other last="" gone="" superseded="" newest=""
  [[ -n $(jobs) ]] || { echo "the tree lists no job, so every job's entries would go" >&2; return 1; }
  for prefix in "${SUPERSEDED[@]}"; do superseded+=$(families_of "$prefix"); done
  for prefix in "${NEWEST[@]}"; do newest+=$(families_of "$prefix"); done
  for entry in "${ACTIONS[@]}"; do [[ $entry != *:* ]] || newest+="${entry#*:} "; done
  while IFS=$TAB read -r id why key; do
    [[ -n $key ]] || continue
    gh api -X DELETE "repos/$GITHUB_REPOSITORY/actions/caches/$id" > /dev/null && echo "$why $key"
  done < <(gh api --paginate "repos/$GITHUB_REPOSITORY/actions/caches?ref=$ref&per_page=100" \
    -q '.actions_caches[] | [.id, .created_at, .key] | @tsv' |
    prune_plan "$run" "$superseded" "$newest" "${FIXED[*]-}")
  [[ $ref == refs/heads/main ]] || return 0
  while IFS=$TAB read -r other id; do
    [[ -n $id ]] || continue
    if [[ $other != "$last" ]]; then
      last=$other
      if ref_done "$other"; then gone=yes; else gone=no; fi
    fi
    [[ $gone == yes ]] || continue
    gh api -X DELETE "repos/$GITHUB_REPOSITORY/actions/caches/$id" > /dev/null &&
      echo "gave back cache $id of $other"
  done < <(gh api --paginate "repos/$GITHUB_REPOSITORY/actions/caches?per_page=100" \
    -q '.actions_caches[] | select(.ref != "refs/heads/main") | [.ref, .id] | @tsv' | LC_ALL=C sort)
}

# Workspace members under `crates/`, read out of `Cargo.toml` as text.
members() {
  local manifest="$root/Cargo.toml" found
  if [[ ! -f $manifest ]]; then
    echo "FAIL: no workspace manifest at $manifest" >&2
    return 1
  fi
  found=$(sed -n '/^members = \[/,/^]/p' "$manifest" |
    sed -n 's#.*"crates/\([a-z0-9-]*\)".*#\1#p')
  if [[ -z $found ]]; then
    echo "FAIL: read no workspace members out of $manifest — the [workspace] members list is missing or no longer one quoted \"crates/NAME\" per line" >&2
    return 1
  fi
  printf '%s\n' "$found"
}

members_outside_crates() {
  sed -n '/^members = \[/,/^]/p' "$root/Cargo.toml" |
    sed -n 's#.*"\([^"]*\)".*#\1#p' |
    grep -v '^crates/' || true
}

cmd_verify() {
  local failures=0 member package entry id dir
  local workflow="$root/.github/workflows/ci.yml"

  if ! members >/dev/null; then
    return 1
  fi
  local -a all_members=()
  while read -r member; do all_members+=("$member"); done < <(members)

  for member in "${all_members[@]}"; do
    [[ $member == *-tests ]] || continue
    if [[ -d "$root/crates/$member/src" ]]; then
      echo "FAIL: crates/$member has a src/, and a '-tests' package is tests only: it is what the opt-level override in Cargo.toml applies to" >&2
      failures=$((failures + 1))
    fi
    if ! grep -q "^\[profile.dev.package.$member\]" "$root/Cargo.toml"; then
      echo "FAIL: Cargo.toml has no [profile.dev.package.$member] override, so its suite compiles at the library's opt-level" >&2
      failures=$((failures + 1))
    fi
    if ! printf '%s\n' "${all_members[@]}" | grep -qx "${member%-tests}"; then
      echo "FAIL: '$member' is a member and '${member%-tests}' is not, so its tests run without that crate's binaries beside them" >&2
      failures=$((failures + 1))
    fi
  done
  # --- dead code the compiler is not allowed to see -------------------------
  # `pub` items in a library are never `dead_code`, and `allow(dead_code)` silences the lint
  # wherever else it would fire, so between them nothing reports a function no one calls.
  local allow
  while read -r allow; do
    echo "FAIL: $allow silences dead-code warnings — delete what nothing calls instead" >&2
    failures=$((failures + 1))
  done < <(grep -rn -E "allow\((dead_code|unused)\)" --include=*.rs "$root/crates" | sed "s|^$root/||" || true)
  # --- tests that never run --------------------------------------------------
  # Every library sets `test = false`, so a test in a crate's sources compiles under clippy and never
  # runs: Rust tests live in the `-tests` packages.
  local held
  while read -r held; do
    echo "FAIL: $held is a test in a crate's sources, which never runs: move it to the crate's \`-tests\` package" >&2
    failures=$((failures + 1))
  done < <(grep -rn -E "^[[:space:]]*#\[cfg\(test\)\]" --include=*.rs "$root"/crates/*/src | sed "s|^$root/||" || true)

  for package in "${HOST_PACKAGES[@]}"; do
    if ! printf '%s\n' "${all_members[@]}" | grep -qx "$package"; then
      echo "FAIL: HOST_PACKAGES names '$package', which is not a workspace member" >&2
      failures=$((failures + 1))
    fi
  done

  for dir in "$root"/crates/*/; do
    member=$(basename "$dir")
    [[ -f $dir/Cargo.toml ]] || continue
    if ! printf '%s\n' "${all_members[@]}" | grep -qx "$member"; then
      echo "FAIL: crates/$member is a crate that is no workspace member, so nothing in CI builds or tests it" >&2
      failures=$((failures + 1))
    fi
  done

  local outside
  while read -r outside; do
    [[ -n $outside ]] || continue
    if [[ ! -f "$root/$outside/Cargo.toml" ]]; then
      echo "FAIL: the workspace names '$outside', which has no Cargo.toml" >&2
      failures=$((failures + 1))
      continue
    fi
    if ! grep -q -- "--workspace --all-targets" "$workflow"; then
      echo "FAIL: '$outside' is a member outside crates/, and no job runs '--workspace --all-targets', so nothing in CI compiles it" >&2
      failures=$((failures + 1))
    fi
  done < <(members_outside_crates)

  local needs
  # The `ci` job's `needs:` list, which wraps across lines.
  needs=$(awk '/^  ci:/{f=1;next} f && /^  [a-z]/{exit} f' "$workflow" 2>/dev/null |
    tr '\n' ' ' | sed -n 's/.*needs: *\(\[[^]]*\]\).*/\1/p')
  if [[ -z $needs ]]; then
    echo "FAIL: could not read the \`ci\` job's \`needs:\` list out of $workflow -- every check below would pass vacuously" >&2
    failures=$((failures + 1))
  fi

  local runs_job runs_command
  if [[ ! -f "$root/$CLI_SUITE/ply.pkg" ]]; then
    echo "FAIL: $CLI_SUITE holds no ply.pkg, and a run tests it as a package" >&2
    failures=$((failures + 1))
  fi
  local listed
  listed=$(runs)
  if [[ -n $(sort <<< "$listed" | uniq -d) ]]; then
    echo "FAIL: the run '$(sort <<< "$listed" | uniq -d | head -1)' is listed twice, so two jobs would take it" >&2
    failures=$((failures + 1))
  fi
  for id in "${RUNS_ALONE[@]}"; do
    if ! grep -qx "$id" <<< "$listed" || [[ $id != package-* && $id != laws-* ]]; then
      echo "FAIL: RUNS_ALONE names '$id', which is no run the \`packages\` job would take" >&2
      failures=$((failures + 1))
    fi
  done
  # Fails when a source directory that is formatted declares tests but is not listed in any test
  # run: its tests would never run.
  local tested where
  tested=$(
    printf '%s\n' "$STD_TESTS" "$STD_LIBRARY" "$CLI_SUITE"
    for entry in "${PACKAGE_SUITES[@]}"; do printf '%s\n' "${entry#*:}"; done
  )
  for where in "${FORMATTED_SOURCES[@]}"; do
    [[ -d $root/$where ]] || continue
    grep -rqE --include='*.ply' '^test(/[a-z]+)? "' "$root/$where" || continue
    if ! grep -qxF -- "$where" <<< "$tested"; then
      echo "FAIL: $where declares tests and no run takes it, so CI never runs them: name it in PACKAGE_SUITES" >&2
      failures=$((failures + 1))
    fi
  done
  # A module's tests are selected by the filter `<module>.`, so no test in another module may have a
  # name containing `<module>.`.
  local module keys outside
  keys=$(package_keys "$CLI_SUITE")
  for module in $(modules_of "$CLI_SUITE"); do
    outside=$(grep -F -- "$module." <<< "$keys" | grep -v "^$module\." || true)
    if [[ -n $outside ]]; then
      echo "FAIL: '$module.' is part of '${outside%%$'\n'*}', outside $CLI_SUITE/$module.ply, so the module's filter picks that test too" >&2
      failures=$((failures + 1))
    fi
  done
  local file claimed
  while IFS= read -r file; do
    [[ -n $file ]] || continue
    claimed=0
    for entry in ${PACKAGE_PROOFS[@]+"${PACKAGE_PROOFS[@]}"}; do [[ $file == "${entry#*:}"/* ]] && claimed=1; done
    if ((!claimed)); then
      echo "FAIL: $file states a law or contract, and no PACKAGE_PROOFS entry proves its package, so CI never discharges it" >&2
      failures=$((failures + 1))
    fi
  done < <(claiming_files)
  for entry in ${PACKAGE_PROOFS[@]+"${PACKAGE_PROOFS[@]}"}; do
    if ! grep -q "^${entry#*:}/" < <(claiming_files); then
      echo "FAIL: PACKAGE_PROOFS names ${entry#*:}, which states no law or contract — delete the entry" >&2
      failures=$((failures + 1))
    fi
  done
  # Every committed `.ply` is a maintained source or a fixture, so none is left out of the format
  # check by being forgotten.
  local tracked under
  while IFS= read -r tracked; do
    under=0
    for where in "${FORMATTED_SOURCES[@]}" "${FIXTURE_SOURCES[@]}"; do
      [[ $tracked == "$where" || $tracked == "$where"/* ]] && under=$((under + 1))
    done
    if ((under != 1)); then
      echo "FAIL: $tracked is under $under of FORMATTED_SOURCES and FIXTURE_SOURCES, and must be under one" >&2
      failures=$((failures + 1))
    fi
  done < <(git -C "$root" ls-files '*.ply')
  for where in "${FORMATTED_SOURCES[@]}" "${FIXTURE_SOURCES[@]}"; do
    if [[ ! -e $root/$where ]]; then
      echo "FAIL: FORMATTED_SOURCES or FIXTURE_SOURCES names $where, which is not in the tree" >&2
      failures=$((failures + 1))
    fi
  done
  for entry in "${NIGHTLY[@]}"; do
    module="$(role "${entry%%:*}")/${entry#*:}"
    if [[ -z $(test_names "$root/$module.ply" 2>/dev/null || true) ]]; then
      echo "FAIL: $module.ply declares no test, and NIGHTLY leaves it to the nightly job" >&2
      failures=$((failures + 1))
    fi
  done
  # Each command that runs Ply tests must run in a job the `ci` job waits on, or a failure in those
  # tests would not fail CI.
  for runs_command in "ci-shards.sh runs-matrix" "ci-ply.sh run" "ci-ply.sh nightly"; do
    runs_job=$(job_running "$runs_command")
    if [[ -z $runs_job ]]; then
      echo "FAIL: no job in $workflow runs \`$runs_command\`, so its Ply runs run nowhere" >&2
      failures=$((failures + 1))
    elif [[ " ${needs//[][,]/ } " != *" $runs_job "* ]]; then
      echo "FAIL: job '$runs_job' runs Ply tests, and is not in the \`ci\` job's needs list" >&2
      failures=$((failures + 1))
    fi
  done
  # The job that starts the runs stops them at a deadline that leaves RUNS_MARGIN minutes of the
  # job's time limit for the steps after them. If the limit cancels the job, those steps never run
  # and nothing the runs wrote is saved.
  local limit budget
  runs_job=$(job_running "ci-ply.sh run")
  if [[ -n $runs_job ]]; then
    limit=$(job_block "$runs_job" | sed -n 's/^    timeout-minutes: *\([0-9][0-9]*\)$/\1/p')
    budget=$(deadline_of "$runs_job")
    if [[ -z $budget ]]; then
      echo "FAIL: job '$runs_job' runs \`ci-ply.sh run\` and sets no PLY_CI_DEADLINE, so its limit cancels it with nothing cached" >&2
      failures=$((failures + 1))
    elif [[ -z $limit ]] || ((budget > limit - RUNS_MARGIN)); then
      echo "FAIL: job '$runs_job' ends its runs ${budget} minutes in, which leaves less than $RUNS_MARGIN of its ${limit:-unset} for the steps that keep what they wrote" >&2
      failures=$((failures + 1))
    fi
  fi

  # The directories that the sweep in `sweep.rs` deletes file by file must be the same ones
  # `ci-warm.sh` deletes file by file. A directory `ci-warm.sh` did not delete from would give every
  # job of this run all the files an earlier run left.
  local sweep_rs="$root/crates/ply-compiled/src/c/sweep.rs" swept cut
  swept=$(sed -n 's/^const BY_FILE: \[&str; [0-9]*\] = \[\(.*\)\];$/\1/p' "$sweep_rs" | tr -d ' ' | tr ',' '\n' |
    while read -r name; do sed -n "s/^pub const $name: &str = \"\\(.*\\)\";$/\\1/p" "$sweep_rs"; done | LC_ALL=C sort)
  cut=$(sed -n 's/^ *\([a-z| ]*\)) find "\$dir" -type f ! -newer "\$mark" -delete ;;$/\1/p' "$root/.github/ci-warm.sh" |
    tr -d ' ' | tr '|' '\n' | LC_ALL=C sort)
  if [[ -z $swept || $swept != "$cut" ]]; then
    echo "FAIL: the sweep takes '$(echo $swept)' file by file and ci-warm.sh cuts '$(echo $cut)' file by file" >&2
    failures=$((failures + 1))
  fi
  # The sweep, `ci-warm.sh` and `ci-ply.sh` each delete a store of cached records as a whole
  # directory, so all three must use the same name for it.
  local stores_rs stores_warm stores_ply
  stores_rs=$(sed -n 's/^pub const STORES: &str = "\(.*\)";$/\1/p' "$sweep_rs")
  stores_warm=$(sed -n 's/^stores=\$cache\/\(.*\)$/\1/p' "$root/.github/ci-warm.sh")
  stores_ply=$(sed -n 's/^stores=ply-c-cache\/\(.*\)$/\1/p' "$root/.github/ci-ply.sh")
  if [[ -z $stores_rs || $stores_rs != "$stores_warm" || $stores_rs != "$stores_ply" ]]; then
    echo "FAIL: the sweep takes the stores below '$stores_rs' whole, ci-warm.sh those below '$stores_warm' and ci-ply.sh those below '$stores_ply'" >&2
    failures=$((failures + 1))
  fi

  cmd_cache_keys || failures=$((failures + 1))
  cmd_prune_check || failures=$((failures + 1))
  cmd_actions || failures=$((failures + 1))
  cmd_toolchain_embeds || failures=$((failures + 1))

  if [[ $failures -gt 0 ]]; then
    echo "$failures problem(s) in the CI tables" >&2
    return 1
  fi
  echo "${#all_members[@]} members under crates/ (plus $(members_outside_crates | grep -c . || true) outside); $(grep -c . <<< "$listed") Ply runs in $(jobs | grep -c .) jobs; $NEXTEST_SHARDS nextest shards"
}

# Commands that need package paths first read `ply.work` into `WORK` in the main shell, because an
# assignment made in a subshell would be lost.
case "${1:-}" in
  verify | toolchain-inputs | work | store-paths | ply-trees | runs-matrix | job-runs | run-line | \
    formatted-sources | supersede) workspace ;;
esac

case "${1:-}" in
  verify) cmd_verify ;;
  cache-keys)
    cmd_cache_keys
    cmd_prune_check
    cmd_actions
    ;;
  toolchain-inputs) toolchain_inputs "${2:-all}" ;;
  work) role "${2:?a role}" ;;
  store-paths) cmd_store_paths ;;
  ply-trees) cmd_ply_trees ;;
  nextest-shards) cmd_nextest_shards ;;
  durations) cmd_durations "${2:?a nextest JUnit report}" ;;
  runs-matrix) cmd_runs_matrix ;;
  job-runs) cmd_job_runs "${2:?a job}" ;;
  run-line) cmd_run_line "${2:?a run}" ;;
  host-filter) cmd_host_filter ;;
  formatted-sources) printf '%s\n' "${FORMATTED_SOURCES[@]}" ;;
  rust-inputs) cmd_rust_inputs "${2:-}" ;;
  supersede) cmd_supersede "${2:?a run id}" "${3:?a ref}" ;;
  nightly-modules) nightly_of "${2:?a role}" | paste -sd ' ' - ;;
  *)
    echo "usage: ci-shards.sh {verify|cache-keys|toolchain-inputs [core]|work ROLE|store-paths|ply-trees|nextest-shards|durations FILE|runs-matrix|job-runs ID|run-line ID|host-filter|formatted-sources|rust-inputs DEPINFO|supersede RUN REF|nightly-modules}" >&2
    exit 2
    ;;
esac
