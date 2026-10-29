#!/usr/bin/env bash
# The tables CI's test jobs are listed from, the check that they name what the tree holds, and the
# caches a run leaves the runs after it.
#
#   ci-shards.sh verify          every crate is a member, every test named here
#                                exists, every `probes/` directory is run by a
#                                job the `ci` aggregate requires, the jobs take
#                                every Ply run exactly once, every cache key a
#                                job writes is one a job reads and a family
#                                `supersede` prunes, every key that names the
#                                run is one a later run reads and no job needs,
#                                every action a workflow uses is one listed with
#                                the cache entries it saves, and every file the
#                                trees `ply bootstrap` reads embed is in one
#   ci-shards.sh cache-keys      just the checks of the keys a job writes and reads,
#                                of what `supersede` deletes and of the actions
#   ci-shards.sh toolchain-inputs [core]
#                                each path a bootstrap reads Ply from, one a line,
#                                which the toolchain key hashes; with `core`, of
#                                the standard library only its prelude (`PRELUDE`)
#   ci-shards.sh work ROLE       each path `ply.work` gives ROLE (`ply work`), one a
#                                line
#   ci-shards.sh store-paths     where the runs keep their stores, one pattern a
#                                line, as the cache steps save and restore them
#   ci-shards.sh ply-trees       every tree of Ply sources `ply.work` places, one
#                                a line
#   ci-shards.sh nextest-shards  the JSON matrix of the nextest shards
#   ci-shards.sh durations FILE  `binary_id test milliseconds` per test in a
#                                nextest JUnit report
#   ci-shards.sh runs-matrix     the JSON matrix of the jobs that take the Ply
#                                runs, each by its id
#   ci-shards.sh job-runs ID     the runs the job ID takes, one a line
#   ci-shards.sh run-line ID     the package a run tests, the substrings of the
#                                tests' qualified names `ply test --filter`
#                                picks its tests by, and its part `K/N`,
#                                tab-separated, a field empty where it has
#                                none: of a run, or of one module of the CLI's
#                                suite (`cli-<module>`)
#   ci-shards.sh exclude-filter  the filterset a shard leaves to the hosts and
#                                gates jobs: the host packages and the tree checks
#   ci-shards.sh gate-filter     the filterset the gates job runs: the tree checks
#   ci-shards.sh host-filter     the filterset selecting the host packages
#   ci-shards.sh tree-checks     one `package target test` line per tree check
#   ci-shards.sh formatted-sources
#                                the maintained Ply sources, one a line, which the
#                                gates job holds to `ply fmt --check`
#   ci-shards.sh nightly-modules the modules of the CLI's suite no pull request runs, on one line,
#                                which the nightly job hands `ci-ply.sh nightly`
#   ci-shards.sh supersede RUN REF
#                                delete the entries of REF nothing restores again:
#                                those this run's replaced, and those no family
#                                here names
#
# Where each package lies is `ply.work`'s to say: the commands that place one read it with the `ply`
# the job has (`target/debug`, `target/release`) or the last release (`target/stage0`, which
# `scripts/stage0.sh` fetches).

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
# Modules of the CLI's suite no pull request runs, which the `nightly` job does: minutes each, the
# reach audit checking packages cold and the copies audit listing every one, or against a postgres
# they start.
CLI_NIGHTLY=(reached copies db_in_ply db_wire pg_client)
# Minutes of a job's limit left after the deadline its runs end at (`PLY_CI_DEADLINE`), for the steps
# that keep what they wrote.
RUNS_MARGIN=4
# The packages the shards exclude, whose tests bind what a shard cannot: sockets and processes. The
# gates job runs them after its own tests.
HOST_PACKAGES=(ply-host-tests)

# Crate directories that are deliberately not workspace members, as `name:why`.
# Expanded as ${KNOWN_OUTSIDE[@]+...}: bash 3.2 treats an empty array as unset under `set -u`.
declare -a KNOWN_OUTSIDE=(
)

# Checks on the tree, as `package:target:test` (`target` is `lib` for a unit test, named by full
# module path). The gates job asserts each ran, since a check that stops running reports nothing.
TREE_CHECKS=(
  "ply-core-tests:suite:armed::every_registered_code_is_constructed_in_production"
  "ply-core-tests:suite:armed::every_variant_of_a_covered_enum_is_constructed_in_production"
  "ply-core-tests:suite:armed::every_diagnostic_constructor_call_names_its_code_literally"
  "ply-core-tests:suite:armed::every_code_declared_or_raised_has_one_row_in_the_registry"
  "ply-core-tests:suite:armed::the_registry_has_no_row_for_a_code_nothing_declares_or_raises"
  "ply-core-tests:suite:armed::no_allowlist_entry_has_outlived_its_reason"
  "ply-core-tests:suite:armed::ambiguous_enum_names_are_declared"
)

# Checks on the tree in the CLI's suite, as `module:test`. Each runs with its module's entry, so the
# table asserts it is still declared there: a check that stops being declared reports nothing.
CLI_TREE_CHECKS=(
  "copies:no two modules define the same function"
  "fixture_list:every fixture is listed"
  "tree:the harness is the only module that starts the \`ply\` binary"
)

# `probes/` directories no cargo build reaches, as `dir:job`; the job must be in `ci`'s `needs`.
declare -a PROBE_JOBS=()

# `<family>-<run id>` entries only the newest of which is ever restored. What one job hands the jobs
# of its own run is never a cache, which the repository's other runs evict while those jobs wait for
# runners: it is an artifact (`cache-keys` refuses a key that names the run and no later run reads).
SUPERSEDED=(ply-upstream- ply-stores-)

# `<family>-<digest>` entries keyed by what they hold: a run restores the newest one a `restore-keys`
# prefix matches, so an older one only holds the repository's 10 GB against what a run does read.
NEWEST=(nextest-archive- ply-runtime- ply-built- rust-passed- tree-passed- ply-c-stage-sources- ply-c-runs-)

# Families of the two above that hold an entry a job, `<family><job id>-...`: one whose job the tree
# no longer lists is restored by no run.
PER_JOB=(ply-stores- ply-c-runs-)

# A key that never moves: nothing replaces its one entry.
FIXED=(tcc-)

# The actions the workflows use: `action:family` for one that saves cache entries under keys of its
# own, of which a run restores the newest, and the bare name of one that saves none but those a step
# spells. `supersede` deletes an entry no family names, so an action left out would lose its cache
# every run: `verify` refuses a workflow that uses one.
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

# The prelude, the module of the language's own definitions, and the copy of its text the checker
# reads builtins from, as paths under the library's tree: the one part of the library a `ply` takes
# as given rather than reading a checkout's (`toolchain_inputs core`).
PRELUDE=(prelude.ply prelude_source.ply)

# What `ply work` prints of this checkout's `ply.work`, a role and a path a line, read once by the
# commands that place a package, and the tables made of it.
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
  # Packages whose own suites are runs too, as `id:path`, the id the directory's name: each failing
  # test is named in the log, where a test wrapping the run would report one failure for all of
  # them, and a pass is kept until the code its test runs moves, where a test wrapping the run reruns
  # whenever the CLI moves.
  PACKAGE_SUITES=()
  for dir in $(role compiler) $(role cli) $(role package) $(role suite); do
    PACKAGE_SUITES+=("${dir##*/}:$dir")
  done
  # Packages whose laws and contracts a `ply prove` of their own discharges, as `id:path`, the entry
  # `laws-<id>`: the library's are the `proofs` run's, and a law in any other package fails `verify`.
  PACKAGE_PROOFS=()
  for dir in $(role proved); do PACKAGE_PROOFS+=("${dir##*/}:$dir"); done
  # The Ply sources the repository maintains, which the gates job holds to `ply fmt --check`: a
  # source committed unformatted puts its reformatting into the next change that touches it. Every
  # committed `.ply` is under one of these or of the fixtures, whose spelling is their point
  # (`verify`).
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

# Where the runs keep their stores, as the cache steps save and restore them: one pattern for each
# directory packages lie in, so a package added there moves no entry's version.
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

# The path of the file a `package target test` triple names, for tests in `tests/`.
test_source_file() {
  local package=$1 target=$2 test=$3 dir modpath
  dir="$root/crates/$package/tests"
  if [[ -f "$dir/$target.rs" ]]; then
    printf '%s\n' "$dir/$target.rs"
  elif [[ $test == *::* ]]; then
    modpath=${test%::*}
    modpath=$dir/$target/${modpath//:://}
    if [[ -f $modpath/mod.rs ]]; then
      printf '%s\n' "$modpath/mod.rs"
    else
      printf '%s\n' "$modpath.rs"
    fi
  else
    printf '%s\n' "$dir/$target/main.rs"
  fi
}

# The nextest binary id of a `package target` pair: the package for `lib`, else `package::target`.
binary_id() {
  if [[ $2 == lib ]]; then printf '%s' "$1"; else printf '%s::%s' "$1" "$2"; fi
}

triples() {
  local entry rest
  for entry in "$@"; do
    rest=${entry#*:}
    printf '%s %s %s\n' "${entry%%:*}" "${rest%%:*}" "${rest#*:}"
  done
}

cmd_tree_checks() { triples "${TREE_CHECKS[@]}"; }

# `(binary_id(=..) & test(=..)) | ...` over `package target test` lines on stdin.
filter_of() {
  local package target test first=1
  while read -r package target test; do
    ((first)) || printf ' | '
    first=0
    printf '(binary_id(=%s) & test(=%s))' "$(binary_id "$package" "$target")" "$test"
  done
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

# Each `.ply` of the compiler, the CLI, its suite, a package and a proved suite that states a law or an
# `ensures`, `cost` or `space` clause, which only a `ply prove` discharges.
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

# The job that takes the run $1: `packages` for a package's own tests and a package's proofs, but
# those RUNS_ALONE names, `stdlib` for the library's tests that read fixtures, and a job of its own
# for every other run.
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

# The runs the job ID takes, one a line; a run or a module that is no job is itself.
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

# `<module>.` per module of the CLI's suite a pull request runs, on one line: the qualified names of
# a module's tests begin so.
cli_filters() {
  local module
  for module in $(modules_of "$CLI_SUITE"); do
    named_in "$module" "${CLI_NIGHTLY[@]}" || printf '%s. ' "$module"
  done
}

cmd_run_line() {
  local id=$1 path part
  case "$id" in
    stdlib | proofs) printf '%s\t\t\n' "$STD_LIBRARY" ;;
    std-tests) printf '%s\t\t\n' "$STD_TESTS" ;;
    cli-part-*)
      part=${id#cli-part-}
      if ! [[ $part =~ ^[1-9][0-9]*$ ]] || ((part > CLI_PARTS)); then
        echo "the CLI's suite runs in $CLI_PARTS parts, and '$id' is none of them" >&2
        return 1
      fi
      printf '%s\t%s\t%d/%d\n' "$CLI_SUITE" "$(cli_filters)" "$part" "$CLI_PARTS"
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

cmd_tree_check_filter() {
  cmd_tree_checks | filter_of
  printf '\n'
}

cmd_gate_filter() { cmd_tree_check_filter; }

cmd_exclude_filter() { printf '%s | %s\n' "$(cmd_host_filter)" "$(cmd_tree_checks | filter_of)"; }

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

# One job writes each cache key and another reads it. A rename that misses a side leaves a cache
# nothing restores -- a run that is quietly slow rather than red -- and a restore naming a key
# nothing writes always misses the same way. Only the literal before the first `${{ ... }}` is
# compared: it is the part a restore can match on, and the part both sides spell out. A cache is
# for what a miss only slows: the repository's other runs evict any entry while a run's jobs wait
# for runners, so a key that names the run is one a later run reads, and none is restored with
# `fail-on-cache-miss`.
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
    # `key:` inside a save step is written, inside a restore step or in a `restore-keys` list it
    # is read. An entry with no literal at all is all expression, and nothing to compare. A write
    # whose key names the run belongs to this run, and `late` holds the reads a later run makes --
    # the `restore-keys` prefixes, which do not name it.
    function note(kind, value, key) {
      key = literal(value)
      if (key == "") return
      steplit[++nlit] = key
      if (kind == "save") {
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
      }
    }
    # A step is done: what it saves is saved from its path, and what it restores is only an entry
    # saved from the same path, since a path is part of an entry'"'"'s version.
    function flush(i) {
      for (i = 1; i <= nlit; i++) {
        if (mode == "save") savepath[steplit[i]] = steppath
        else if (mode == "restore") { rlit[++nr] = steplit[i]; rpath[nr] = steppath; rwhere[nr] = stepwhere }
      }
      if (mode == "restore" && stepfail && steprun) needed[++nneeded] = stepwhere
      nlit = 0; steppath = ""; inpath = 0; stepfail = 0; steprun = 0
    }
    FNR == 1 { flush(); mode = ""; inkeys = 0; indent = 0 }
    # A new list item is a new step; the rules below read the one they are in.
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
      # A key that names the run is one only a `restore-keys` entry of a later run can match: one
      # nothing later reads carries this run'"'"'s work to this run'"'"'s jobs, which an artifact does.
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

# A path with each `..` taken with the segment before it and each `.` dropped, as text.
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

# Each path of the repository a bootstrap reads Ply from, one a line: `ply.work`, the standard
# library's, the compiler's and the program's trees it places, and every package the manifest of one
# of those names by path, and theirs. With `core`, of the library only its prelude (`PRELUDE`), and a
# prelude the tree does not hold fails, since a key that hashed nothing for it would match any.
toolchain_inputs() {
  local scope=${1:-all} all prev dir rel file
  all=$({
    printf '%s\n' "$TOOLCHAIN_WORKSPACE"
    if [[ $scope == core ]]; then
      for file in "${PRELUDE[@]}"; do
        if [[ ! -f $root/$(role std)/$file ]]; then
          echo "the library holds no $(role std)/$file, which PRELUDE names" >&2
          exit 1
        fi
        printf '%s/%s\n' "$(role std)" "$file"
      done
    else
      role std
    fi
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
      } | grep . | LC_ALL=C sort -u
    )
  done
  printf '%s\n' "$all"
}

# The paths the toolchain key hashes (`toolchain_inputs`), and those its core hashes, held to what a
# bootstrap reads: an `embed` that leaves its path is an input too, and must land in one, or a `ply`
# kept under the key would stand for a file that moved.
cmd_toolchain_embeds() {
  local bad=0
  toolchain_embeds all || bad=$((bad + 1))
  toolchain_embeds core || bad=$((bad + 1))
  return $((bad > 0))
}

# `cmd_toolchain_embeds` of the paths `toolchain_inputs SCOPE` lists. Within a tree only an `embed`
# that climbs out of its module's directory can leave it; a file of the core leaves itself with any.
toolchain_embeds() {
  local scope=$1 inputs trees path file rel target covered pattern bad=0
  inputs=$(toolchain_inputs "$scope")
  trees=$(grep -vxF -- "$TOOLCHAIN_WORKSPACE" <<< "$inputs" || true)
  if [[ -z $trees ]]; then
    echo "FAIL: $TOOLCHAIN_WORKSPACE places no part of the toolchain, so the toolchain key is held to nothing" >&2
    return 1
  fi
  while IFS=: read -r file rel; do
    [[ -n $rel ]] || continue
    target=$(lexical "$(dirname "${file#"$root"/}")/$rel")
    covered=no
    while IFS= read -r path; do
      [[ $target == "$path" || $target == "$path"/* ]] && covered=yes
    done <<< "$inputs"
    if [ "$covered" = no ]; then
      echo "FAIL: ${file#"$root"/} embeds $target, outside every path the $scope toolchain key hashes" >&2
      bad=$((bad + 1))
    fi
  done < <(while IFS= read -r path; do
    if [ -d "$root/$path" ]; then
      pattern='embed(_dir)?\("[^"]*\.\.[^"]*"'
    elif [ -f "$root/$path" ]; then
      pattern='embed(_dir)?\("[^"]*"'
    else
      continue
    fi
    # `file:path` per embed that can leave its path; a line of a `\\` text is a program a test
    # writes, not part of the tree.
    { grep -rHE --include='*.ply' "$pattern" "$root/$path" || true; } |
      { grep -vE '^[^:]*:[[:space:]]*\\\\' || true; } |
      sed -E 's/^([^:]*):.*embed(_dir)?\("([^"]*)".*/\1:\3/'
  done <<< "$inputs")
  if [ "$bad" -eq 0 ]; then
    printf 'toolchain key (%s): the %d paths a bootstrap reads Ply from hold every file they embed\n' "$scope" "$(grep -c . <<< "$inputs")"
  fi
  return $((bad > 0))
}

# Every input a fresh build's dep-info lists, held to the patterns the Rust key hashes: an input the
# key missed would let a cached runtime or archive stand for sources it was not built from. A path
# outside the repository is a registry crate's, which `Cargo.lock` pins.
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
    # Lexically, as the key's patterns spell the repository: `a/../b` is `b`.
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

# The families the table entry PREFIX names, on one line: one a job where PER_JOB lists it, else
# itself.
families_of() {
  local id
  if named_in "$1" ${PER_JOB[@]+"${PER_JOB[@]}"}; then
    jobs | while read -r id; do printf '%s%s- ' "$1" "$id"; done
  else
    printf '%s ' "$1"
  fi
}

# What `supersede` deletes of a ref's entries, read as `id created key` lines with tabs between,
# written as `id why key`. Of a family SUPERSEDED names, an entry of an earlier run where RUN saved
# one: a job that wrote nothing keeps what it had. Of a family NEWEST names, every entry but the
# newest. And an entry no family names, which nothing in this tree restores: a retired job's, or a
# family's the tables dropped.
prune_plan() {
  LC_ALL=C sort -t "$TAB" -k2,2r | awk -F'\t' -v OFS='\t' -v run="$1" -v superseded="$2" -v newest="$3" -v fixed="$4" '
    function named(key, list,   p, i, m) {
      m = split(list, p, " ")
      for (i = 1; i <= m; i++) if (index(key, p[i]) == 1) return 1
      return 0
    }
    # A key less the digests it ends in, each eight hex digits or more.
    function family(key) {
      while (match(key, /-[0-9a-f]+$/) && RLENGTH > 8) key = substr(key, 1, RSTART - 1)
      return key
    }
    # The run a key of a run-scoped family ends in, and the family: the key less that run and the
    # digests before it, so a job'"'"'s stores are one family whichever toolchain wrote them.
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

# `prune_plan` over a listing in which each of its rules decides an entry.
cmd_prune_check() {
  local a b got want
  a=$(printf 'a%.0s' {1..40})
  b=$(printf 'b%.0s' {1..40})
  want="1 superseded;4 unnamed;5 superseded;7 superseded;11 superseded;12 unnamed;13 superseded;17 unnamed;19 superseded;"
  got=$(printf '%s\t%s\t%s\n' \
    1 2026-01-01T00:00:00Z ply-stores-stdlib-100 \
    2 2026-01-02T00:00:00Z "ply-stores-stdlib-$b-200" \
    3 2026-01-01T00:00:00Z ply-stores-proofs-100 \
    4 2026-01-01T00:00:00Z ply-stores-cli-part-3-100 \
    5 2026-01-01T00:00:00Z ply-upstream-100 \
    6 2026-01-02T00:00:00Z ply-upstream-200 \
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
    prune_plan 200 "ply-upstream- ply-stores-stdlib- ply-stores-proofs-" "ply-built- ply-c-runs-stdlib- v0-rust-" "tcc-" |
    cut -f1,2 | LC_ALL=C sort -n | tr '\t\n' ' ;')
  if [[ $got != "$want" ]]; then
    echo "FAIL: of the listing it is held to, \`supersede\` would delete '$got' and should delete '$want'" >&2
    return 1
  fi
  echo "cache pruning: each rule of \`supersede\` decides the listing it is held to"
}

# Every action a workflow uses is one ACTIONS lists, with the cache entries it saves or as saving
# none, and every one it lists is used.
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

# Whether no run restores an entry of REF again: a pull request's once it is closed, a branch's
# while no pull request is open from it. A ref that cannot be asked about keeps its entries.
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

# The entries of REF nothing restores again (`prune_plan`). On main, the entries of every other ref
# no run reads again (`ref_done`), which that ref's own runs are no longer there to delete.
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

# Whether a `package target test` triple names a test that exists; prints the problem otherwise.
check_test_exists() {
  local what=$1 package=$2 target=$3 test=$4 leaf file
  leaf=${test##*::}
  if [[ $target == lib ]]; then
    if [[ ! -d "$root/crates/$package/src" ]]; then
      echo "FAIL: $what '$test' names crates/$package/src, which does not exist" >&2
      return 1
    elif ! grep -rq "fn $leaf(" "$root/crates/$package/src"; then
      echo "FAIL: no 'fn $leaf(' under crates/$package/src — $what names a test nothing defines" >&2
      return 1
    fi
  else
    file=$(test_source_file "$package" "$target" "$test")
    if [[ ! -f $file ]]; then
      echo "FAIL: $what '$test' names $file, which does not exist" >&2
      return 1
    elif ! grep -q "fn $leaf(" "$file"; then
      echo "FAIL: $file has no 'fn $leaf(' — $what names a test nothing defines" >&2
      return 1
    fi
  fi
}

cmd_verify() {
  local failures=0 member package entry candidate id target test seen dir note

  local -a known=()
  if ! members >/dev/null; then
    return 1
  fi
  local workflow_early="$root/.github/workflows/ci.yml"
  local -a all_members=()
  while read -r member; do all_members+=("$member"); done < <(members)

  # --- crates --------------------------------------------------------------
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

  for package in "${HOST_PACKAGES[@]}"; do
    if ! printf '%s\n' "${all_members[@]}" | grep -qx "$package"; then
      echo "FAIL: HOST_PACKAGES names '$package', which is not a workspace member" >&2
      failures=$((failures + 1))
    fi
  done

  for entry in ${KNOWN_OUTSIDE[@]+"${KNOWN_OUTSIDE[@]}"}; do
    known+=("${entry%%:*}")
  done
  for dir in "$root"/crates/*/; do
    member=$(basename "$dir")
    [[ -f $dir/Cargo.toml ]] || continue
    if printf '%s\n' "${all_members[@]}" | grep -qx "$member"; then
      continue
    fi
    seen=0
    for candidate in ${known[@]+"${known[@]}"}; do
      [[ $candidate == "$member" ]] && seen=1
    done
    if [[ $seen -eq 0 ]]; then
      echo "FAIL: crates/$member is a crate that no workspace member and no KNOWN_OUTSIDE entry mentions, so nothing in CI builds or tests it" >&2
      failures=$((failures + 1))
    fi
  done
  for entry in ${KNOWN_OUTSIDE[@]+"${KNOWN_OUTSIDE[@]}"}; do
    member=${entry%%:*}
    note=${entry#*:}
    if [[ ! -d "$root/crates/$member" ]]; then
      echo "FAIL: KNOWN_OUTSIDE names crates/$member, which is not in the tree — delete the entry" >&2
      failures=$((failures + 1))
    elif printf '%s\n' "${all_members[@]}" | grep -qx "$member"; then
      echo "FAIL: crates/$member is both a workspace member and KNOWN_OUTSIDE ($note)" >&2
      failures=$((failures + 1))
    fi
  done

  # --- members outside `crates/` --------------------------------------------
  local outside
  while read -r outside; do
    [[ -n $outside ]] || continue
    if [[ ! -f "$root/$outside/Cargo.toml" ]]; then
      echo "FAIL: the workspace names '$outside', which has no Cargo.toml" >&2
      failures=$((failures + 1))
      continue
    fi
    if ! grep -q -- "--workspace --all-targets" "$workflow_early"; then
      echo "FAIL: '$outside' is a member outside crates/, and no job runs '--workspace --all-targets', so nothing in CI compiles it" >&2
      failures=$((failures + 1))
    fi
  done < <(members_outside_crates)

  # --- tests named by a table ----------------------------------------------
  while read -r package target test; do
    check_test_exists "tree check" "$package" "$target" "$test" || failures=$((failures + 1))
  done < <(cmd_tree_checks)
  # --- probes ---------------------------------------------------------------
  local workflow="$root/.github/workflows/ci.yml"
  local -a probe_listed=()
  local probe job needs block
  if [[ ! -f $workflow ]]; then
    echo "FAIL: no workflow at $workflow, so no probe job can be checked" >&2
    failures=$((failures + 1))
  fi
  # The `ci` job's `needs:` list, which wraps across lines.
  needs=$(awk '/^  ci:/{f=1;next} f && /^  [a-z]/{exit} f' "$workflow" 2>/dev/null |
    tr '\n' ' ' | sed -n 's/.*needs: *\(\[[^]]*\]\).*/\1/p')
  if [[ -z $needs ]]; then
    echo "FAIL: could not read the \`ci\` job's \`needs:\` list out of $workflow -- every check below would pass vacuously" >&2
    failures=$((failures + 1))
  fi
  for entry in ${PROBE_JOBS[@]+"${PROBE_JOBS[@]}"}; do
    probe=${entry%%:*}
    job=${entry#*:}
    probe_listed+=("$probe")
    if [[ ! -d "$root/probes/$probe" ]]; then
      echo "FAIL: PROBE_JOBS names probes/$probe, which is not in the tree -- delete the entry" >&2
      failures=$((failures + 1))
      continue
    fi
    if [[ ! -x "$root/probes/$probe/run.sh" ]]; then
      echo "FAIL: probes/$probe has no executable run.sh, so job '$job' has nothing to run" >&2
      failures=$((failures + 1))
    fi
    if ! grep -q "^  $job:\$" "$workflow"; then
      echo "FAIL: PROBE_JOBS says job '$job' runs probes/$probe, and $workflow defines no such job" >&2
      failures=$((failures + 1))
    # `[[ == * ]]`, not `grep -q`: under pipefail an early-exiting grep fails the pipe.
    else
      block=$(job_block "$job")
      if [[ $block != *"probes/$probe/run.sh"* ]]; then
        echo "FAIL: job '$job' exists but its steps never run probes/$probe/run.sh, so it is required in name only" >&2
        failures=$((failures + 1))
      fi
    fi
    # Whole word, so a job name does not match inside a longer one.
    if [[ " ${needs//[][,]/ } " != *" $job "* ]]; then
      echo "FAIL: job '$job' is not in the \`ci\` job's needs list, so it is not required and a green tick can be reported over it never having run" >&2
      failures=$((failures + 1))
    fi
  done
  if [[ -d "$root/probes" ]]; then
    for dir in "$root"/probes/*/; do
      probe=$(basename "$dir")
      seen=0
      for candidate in ${probe_listed[@]+"${probe_listed[@]}"}; do
        [[ $candidate == "$probe" ]] && seen=$((seen + 1))
      done
      if [[ $seen -eq 0 ]]; then
        echo "FAIL: probes/$probe is in no CI job, so nothing in CI runs it" >&2
        failures=$((failures + 1))
      elif [[ $seen -gt 1 ]]; then
        echo "FAIL: probes/$probe is listed $seen times" >&2
        failures=$((failures + 1))
      fi
    done
  fi

  # --- the Ply tests ----------------------------------------------------------
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
  # A tree of maintained sources that declares a test is some run's package: one no run names is
  # formatted and never tested.
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
  # A module's tests are picked out by `<module>.`, which no test of another module may hold.
  local module names keys outside
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
  for entry in "${CLI_TREE_CHECKS[@]}"; do
    module=${entry%%:*}
    names=$(test_names "$root/$CLI_SUITE/$module.ply" 2>/dev/null || true)
    if ! grep -qxF -- "${entry#*:}" <<< "$names"; then
      echo "FAIL: $CLI_SUITE/$module.ply declares no test '${entry#*:}', and CLI_TREE_CHECKS names it" >&2
      failures=$((failures + 1))
    fi
  done
  for module in "${CLI_NIGHTLY[@]}"; do
    if [[ -z $(test_names "$root/$CLI_SUITE/$module.ply" 2>/dev/null || true) ]]; then
      echo "FAIL: $CLI_SUITE/$module.ply declares no test, and CLI_NIGHTLY leaves it to the nightly job" >&2
      failures=$((failures + 1))
    fi
  done
  # Each command's runs must reach a job the \`ci\` job waits on, or they run nowhere that counts.
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
  # The job that starts the runs ends them at a deadline that leaves its steps after them RUNS_MARGIN
  # minutes of its limit: a job the limit cancels keeps nothing its runs wrote.
  local limit budget
  runs_job=$(job_running "ci-ply.sh run")
  if [[ -n $runs_job ]]; then
    limit=$(job_block "$runs_job" | sed -n 's/^    timeout-minutes: *\([0-9][0-9]*\)$/\1/p')
    budget=$(deadline_of "$runs_job")
    if [[ -z $budget ]]; then
      echo "FAIL: job '$runs_job' runs \`ci-ply.sh run\` and sets no PLY_CI_DEADLINE, so its limit cancels it with nothing kept" >&2
      failures=$((failures + 1))
    elif [[ -z $limit ]] || ((budget > limit - RUNS_MARGIN)); then
      echo "FAIL: job '$runs_job' ends its runs ${budget} minutes in, which leaves less than $RUNS_MARGIN of its ${limit:-unset} for the steps that keep what they wrote" >&2
      failures=$((failures + 1))
    fi
  fi

  # --- the stage a job hands the others ---------------------------------------
  # The entries the sweep takes file by file are the ones `ci-warm.sh used` cuts file by file: one it
  # cuts whole carries every file an earlier run left to every job of this one.
  local sweep_rs="$root/crates/ply-compiled/src/c/sweep.rs" swept cut
  swept=$(sed -n 's/^const BY_FILE: \[&str; [0-9]*\] = \[\(.*\)\];$/\1/p' "$sweep_rs" | tr -d ' ' | tr ',' '\n' |
    while read -r name; do sed -n "s/^pub const $name: &str = \"\\(.*\\)\";$/\\1/p" "$sweep_rs"; done | LC_ALL=C sort)
  cut=$(sed -n 's/^ *\([a-z| ]*\)) find "\$dir" -type f ! -newer "\$mark" -delete ;;$/\1/p' "$root/.github/ci-warm.sh" |
    tr -d ' ' | tr '|' '\n' | LC_ALL=C sort)
  if [[ -z $swept || $swept != "$cut" ]]; then
    echo "FAIL: the sweep takes '$(echo $swept)' file by file and ci-warm.sh cuts '$(echo $cut)' file by file" >&2
    failures=$((failures + 1))
  fi
  # A store of kept records goes whole wherever it is cut or carried: one name in all three.
  local stores_rs stores_warm stores_ply
  stores_rs=$(sed -n 's/^pub const STORES: &str = "\(.*\)";$/\1/p' "$sweep_rs")
  stores_warm=$(sed -n 's/^stores=\$cache\/\(.*\)$/\1/p' "$root/.github/ci-warm.sh")
  stores_ply=$(sed -n 's/^stores=ply-c-cache\/\(.*\)$/\1/p' "$root/.github/ci-ply.sh")
  if [[ -z $stores_rs || $stores_rs != "$stores_warm" || $stores_rs != "$stores_ply" ]]; then
    echo "FAIL: the sweep takes the stores below '$stores_rs' whole, ci-warm.sh those below '$stores_warm' and ci-ply.sh those below '$stores_ply'" >&2
    failures=$((failures + 1))
  fi

  # --- cache keys -----------------------------------------------------------
  cmd_cache_keys || failures=$((failures + 1))
  cmd_prune_check || failures=$((failures + 1))
  cmd_actions || failures=$((failures + 1))
  cmd_toolchain_embeds || failures=$((failures + 1))

  if [[ $failures -gt 0 ]]; then
    echo "$failures problem(s) in the CI tables" >&2
    return 1
  fi
  echo "${#all_members[@]} members under crates/ (plus $(members_outside_crates | grep -c . || true) outside); ${#KNOWN_OUTSIDE[@]} crate(s) deliberately outside; ${#TREE_CHECKS[@]} tree checks and ${#CLI_TREE_CHECKS[@]} in the CLI's suite, each present in the tree; ${#PROBE_JOBS[@]} probe(s) run by a required CI job; $(grep -c . <<< "$listed") Ply runs in $(jobs | grep -c .) jobs; $NEXTEST_SHARDS nextest shards"
}

# The commands that place a package read the workspace first, outside the subshells they run.
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
  exclude-filter) cmd_exclude_filter ;;
  gate-filter) cmd_gate_filter ;;
  host-filter) cmd_host_filter ;;
  tree-checks) cmd_tree_checks ;;
  formatted-sources) printf '%s\n' "${FORMATTED_SOURCES[@]}" ;;
  tree-check-filter) cmd_tree_check_filter ;;
  rust-inputs) cmd_rust_inputs "${2:-}" ;;
  supersede) cmd_supersede "${2:?a run id}" "${3:?a ref}" ;;
  nightly-modules) echo "${CLI_NIGHTLY[*]}" ;;
  *)
    echo "usage: ci-shards.sh {verify|cache-keys|toolchain-inputs [core]|work ROLE|store-paths|ply-trees|nextest-shards|durations FILE|runs-matrix|job-runs ID|run-line ID|exclude-filter|gate-filter|host-filter|tree-checks|tree-check-filter|formatted-sources|rust-inputs DEPINFO|supersede RUN REF|nightly-modules}" >&2
    exit 2
    ;;
esac
