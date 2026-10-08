#!/usr/bin/env bash
# The tables CI's test jobs are cut from, the check that the cut is total, and the caches a run
# leaves the runs after it.
#
#   ci-shards.sh verify          every crate is a member, every test named here
#                                exists, every `probes/` directory is run by a
#                                job the `ci` aggregate requires, the shards run
#                                every test exactly once, no corpus run the cut
#                                places took over a third of its job's deadline
#                                in the last table, every cache key a job
#                                writes is one a job reads, every key that
#                                names the run is one a later run reads and no
#                                job needs, and every key over a crate's Ply
#                                sources names every crate's
#   ci-shards.sh cache-keys      just that last check
#   ci-shards.sh fetch-timings   the table the last run measured, from this
#                                pull request's branch or else from main, into
#                                the place the cut reads it from
#   ci-shards.sh partitions      the JSON matrix of the corpus partitions
#   ci-shards.sh nextest-shards  the JSON matrix of the nextest shards
#   ci-shards.sh shard-configs D the nextest config each shard runs under and
#                                the corpus runs each partition takes, cut from
#                                the durations CI measured; 3 when there are
#                                none and the shards fall back to slicing by
#                                test count
#   ci-shards.sh durations FILE  `binary_id test milliseconds` per test in a
#                                nextest JUnit report
#   ci-shards.sh timings BEFORE  the table the next run is cut by, from this
#                                run's rows on stdin and the table BEFORE
#   ci-shards.sh corpus-matrix   the JSON matrix of the corpus runs that get
#                                runners of their own: the desk runners, then
#                                each run alone
#   ci-shards.sh corpus-for-partition K [DIR]
#                                `lane entry` per corpus run partition K takes,
#                                from the cut in DIR, or round robin without one
#   ci-shards.sh desks-for-runner K [DIR]
#                                the desk runs runner K takes, likewise
#   ci-shards.sh corpus-line ID...
#                                the package each run tests, its filter and its
#                                part, tab-separated, a line a run
#   ci-shards.sh exclude-filter  the filterset a partition leaves to the gates
#                                job: the host packages, and the tree check it
#                                runs alone
#   ci-shards.sh gate-filter     the filterset the gates job runs: the tree checks
#   ci-shards.sh host-filter     the filterset selecting the host packages
#   ci-shards.sh tree-checks     one `package target test` line per tree check
#   ci-shards.sh rust-answered DIR OUT [SHARD]
#                                a tool config at OUT leaving out the tests whose
#                                traces in DIR still stand against this checkout,
#                                and `<profile> <count>` of it
#   ci-shards.sh rust-kept NEW JUNIT OUT
#                                the traces in NEW of the tests JUNIT says passed,
#                                copied into OUT
#   ci-shards.sh sweep-matrix EVENT  the JSON matrix of the edit sweep's jobs: under `schedule` or
#                                `workflow_dispatch`, SWEEP_SHARDS shards of the part of the cases
#                                the day names, so SWEEP_DAYS days sweep every case; none otherwise,
#                                since `edit_gate` holds a pull request to every kind of edit
#   ci-shards.sh supersede RUN REF
#                                delete the entries of REF that this run's replaced

set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

# Jobs of corpus lanes, and jobs of nextest. Apart, so a lane never shares a runner with
# nextest's threads.
PARTITIONS=8
NEXTEST_SHARDS=2

# What the last run whose test jobs all passed measured, which its `passes` job uploaded in the
# `test-durations` artifact (`fetch-timings`).
TIMINGS=/tmp/ply-test-timings/timings.tsv

TAB=$'\t'

# The Ply tests, one run an entry: the corpus program's own, each module of the corpus's checks package
# that declares a test, and each module of the CLI's suite as `cli-<module>`, selected by its name.
# Every check spawns `ply`, so a run takes its checks one after another, and the CLI's suite drives
# `ply` in one working directory, so its runs take theirs one after another too. The runs that would
# starve a partition get runners of their own; the rest go to the partitions.
CORPUS_PROGRAM=crates/ply-corpus/ply
# The programs the corpus program runs, each a `ply test` of its own whose tests must pass.
CORPUS_FIXTURES=crates/ply-corpus/fixtures
CORPUS_CHECKS=crates/ply-corpus/checks
# The fixtures the standard library's tests read, as a project: `--std` over it tests and proves every
# module of the library beside its own. Its tests and its proofs are runs of each of the library's
# top-level modules, a module beneath one its own (`std.hash.legacy` is `std.hash`'s), and of each of
# the project's modules (`stdlib_entries`).
CORPUS_STDLIB=crates/ply-corpus/stdlib
STD_LIBRARY=crates/ply-std/ply
CLI_SUITE=crates/ply-cli-tests/ply
# The checks that start desks under load and drive them over postgres: a test at a time, cut by
# duration over `DESK_RUNNERS` runners beside a postgres each, the `corpus` job's `desks-<k>`.
CORPUS_DESKS=(serving database)
DESK_RUNNERS=3
# Runs that take a runner each: the compiler's own tests, compiled, take every core, and the reach
# audit checks three whole packages cold.
CORPUS_ALONE=(cli-compiler_compiled cli-reached)
# Modules the cut may split, a lane taking a run of neighbouring tests (`corpus_cut`): whole, each
# would outlast a lane.
CORPUS_BY_TEST=(audit generated toolchain)
CLI_BY_TEST=(artifact_program bootstrap_archive corpus desk_operations incremental)
# Modules of the CLI's suite no partition runs, which the `edit-sweep` jobs do: each case edits a module
# of the tree one way and checks it warm and cold, days of runners over the tree.
CLI_NIGHTLY=(edit_sweep)
SWEEP_SHARDS=16
SWEEP_DAYS=14
# Minutes of a corpus job's limit left after the deadline its runs end at (`PLY_CI_DEADLINE`), for the
# steps that keep what they wrote: a cold partition packed and uploaded its stores and C in about one.
RUNS_MARGIN=4
# Corpus processes a partition runs side by side, each a lane of the cut: a lane's runs of one package
# go in one `ply test`, which loads the package's closure once. Two, so the program's and the packages'
# own `ply test`s, each with a front end and C of its own, are not all one lane's to take in turn.
CORPUS_LANES=2
# Packages whose own suites run as corpus entries too, as `id:path`: each failing test is named in
# the log, where a Rust test wrapping the run would report one failure for all of them.
PACKAGE_SUITES=(
  "cli:crates/ply-cli/ply"
  "prove:crates/ply-prove/ply"
  "sim:crates/ply-sim/ply"
  "store:crates/ply-store/ply"
  "suite:crates/ply-test/ply"
)
# Packages whose laws and contracts a `ply prove` of their own discharges, as `id:path`, the entry
# `laws-<id>`: the library's are its `proofs:` runs, and a law in any other package fails `verify`.
PACKAGE_PROOFS=(
  "compiler:crates/ply-compiler/ply"
)

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
  "ply-eval-tests:suite:armed::every_registered_code_is_constructed_in_production"
  "ply-eval-tests:suite:armed::every_variant_of_a_covered_enum_is_constructed_in_production"
  "ply-eval-tests:suite:armed::every_diagnostic_constructor_call_names_its_code_literally"
  "ply-eval-tests:suite:armed::every_code_declared_or_raised_has_one_row_in_the_registry"
  "ply-eval-tests:suite:armed::the_registry_has_no_row_for_a_code_nothing_declares_or_raises"
  "ply-eval-tests:suite:armed::no_allowlist_entry_has_outlived_its_reason"
  "ply-eval-tests:suite:armed::ambiguous_enum_names_are_declared"
)

# The tree check the shards leave to the gates job, as a tree check is written: it builds the `ply`
# program with this tree's own builder, and on main the gates job keeps what that build filed.
GATES_ALONE=(
  "ply-launcher-tests:suite:the_builder_these_sources_make_builds_the_program_and_it_runs"
)

# Checks on the tree in the CLI's suite, as `module:test`. Each runs with its module's entry, so the
# table asserts it is still declared there: a check that stops being declared reports nothing.
CLI_TREE_CHECKS=(
  "reached:no two modules define the same function"
  "fixture_list:every fixture is listed"
  "fmt:{path} is committed formatted"
  "fmt:the compiler is committed formatted"
  "fmt:the CLI is committed formatted"
  "fmt:the CLI's suite is committed formatted"
  "fmt:the corpus is committed formatted"
  "fmt:the packages beside the CLI are committed formatted"
  "fmt:the benches, the language tests and the examples are committed formatted"
  "tree:the harness is the only module that starts the \`ply\` binary"
)

# `probes/` directories no cargo build reaches, as `dir:job`; the job must be in `ci`'s `needs`.
declare -a PROBE_JOBS=(
  "ucontext:plan"
)

# `<family>-<run id>` entries only the newest of which is ever restored. What one job hands the jobs
# of its own run is never a cache, which the repository's other runs evict while those jobs wait for
# runners: it is an artifact (`cache-keys` refuses a key that names the run and no later run reads).
SUPERSEDED=(ply-upstream- ply-stores- ply-c-lanes- ply-c-nextest- rust-traces-)

# `<family>-<digest>` entries keyed by what they hold: a run restores the newest one a `restore-keys`
# prefix matches, so an older one only holds the repository's 10 GB against what a run does read.
NEWEST=(nextest-archive- ply-runtime- ply-c-stage-sources- ply-c-stage-own- ply-c-corpus-)

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

cmd_tree_checks() { triples "${TREE_CHECKS[@]}" "${GATES_ALONE[@]}"; }

# `(binary_id(=..) & test(=..)) | ...` over `package target test` lines on stdin.
filter_of() {
  local package target test first=1
  while read -r package target test; do
    ((first)) || printf ' | '
    first=0
    printf '(binary_id(=%s) & test(=%s))' "$(binary_id "$package" "$target")" "$test"
  done
}

# One entry id a line: `program`, the standard library's runs, `fixture-<name>` per fixture,
# `package-<id>` per package suite, `laws-<id>` per package's proofs, every checks module that declares a test, then every such module of
# the CLI's suite under `cli-`; each module as `module`, or, for one placed a test at a time,
# `module:<id>` per test, the id a hash of its label, so a duration measured for a test stays with it
# however the module's tests move.
corpus_entries() {
  local entry file
  printf 'program\n'
  stdlib_entries
  for file in "$root/$CORPUS_FIXTURES"/*.ply; do printf 'fixture-%s\n' "$(basename "$file" .ply)"; done
  for entry in "${PACKAGE_SUITES[@]}"; do printf 'package-%s\n' "${entry%%:*}"; done
  for entry in "${PACKAGE_PROOFS[@]}"; do printf 'laws-%s\n' "${entry%%:*}"; done
  module_entries "$CORPUS_CHECKS" "" "${CORPUS_BY_TEST[@]}" "${CORPUS_DESKS[@]}"
  module_entries "$CLI_SUITE" cli- "${CLI_BY_TEST[@]}" | grep -vE "^cli-($(IFS='|'; echo "${CLI_NIGHTLY[*]}"))(:|$)"
}

# The library's runs, each named by a module: `std.<module>` per top-level module of the library, those
# beneath it its own, and `<module>` per module of CORPUS_STDLIB. `stdlib:<module>` per one that
# declares a test, then `proofs:<module>` per one that declares a law or states an `ensures` or `cost`
# clause, which `ply prove` owes a proof of: apart, so the cut can give a lane the tests and another
# the proofs, each a process with a load of its own.
stdlib_entries() {
  library_modules_with '^test(/[a-z]+)? "' | sed 's/^/stdlib:/'
  library_modules_with '^law(/[a-z]+)? "|^[[:space:]]+(ensures|cost) ' | sed 's/^/proofs:/'
}

# The library's runs a `.ply` file of which holds a line the extended regular expression matches.
library_modules_with() {
  { grep -rlE --include='*.ply' "$1" "$root/$STD_LIBRARY" || true; } |
    sed "s|^$root/$STD_LIBRARY/|std.|; s|^\(std\.[^/.]*\).*|\1|" | LC_ALL=C sort -u
  { grep -lE "$1" "$root/$CORPUS_STDLIB"/*.ply || true; } | sed 's|.*/||; s|\.ply$||' | LC_ALL=C sort
}

# `run<TAB>key` per test and law of the library's runs, the name a filter is matched against:
# `std.<module>.<label>` under its top-level module, and a project module's `<module>.<label>` under
# the module.
stdlib_keys() {
  local file rel
  find "$root/$STD_LIBRARY" -name '*.ply' | LC_ALL=C sort | while IFS= read -r file; do
    rel=${file#"$root/$STD_LIBRARY/"}
    rel=${rel%.ply}
    labels_of "$file" | awk -v run="std.${rel%%/*}" -v module="std.${rel//\//.}" -v OFS="$TAB" '{ print run, module "." $0 }'
  done
  for file in "$root/$CORPUS_STDLIB"/*.ply; do
    labels_of "$file" | awk -v module="$(basename "$file" .ply)" -v OFS="$TAB" '{ print module, module "." $0 }'
  done
}

# The labels of a module's tests and laws.
labels_of() {
  sed -nE 's/^(test|law)(\/[a-z]+)? "([^"]*)".*/\3/p' "$1"
}

# `PREFIX<module>` per module of the package at DIR that declares a test, or `PREFIX<module>:<id>` per
# test of a module the rest of the arguments place a test at a time, in the order it declares them.
module_entries() {
  local dir=$1 prefix=$2 file module
  shift 2
  for file in "$root/$dir"/*.ply; do
    grep -qE '^test(/[a-z]+)? "' "$file" || continue
    module=$(basename "$file" .ply)
    if named_in "$module" "$@"; then
      corpus_test_names "$file" | label_ids | cut -f1 | sed "s/^/$prefix$module:/"
    else
      printf '%s%s\n' "$prefix" "$module"
    fi
  done
}

# `id<TAB>label` per label on stdin: a polynomial hash of the label's bytes in base 36. Arithmetic on
# numbers below 2^41 alone, which every awk holds exactly, so the runner's ids are a developer's.
label_ids() {
  LC_ALL=C awk '
    BEGIN { for (i = 1; i < 256; i++) code[sprintf("%c", i)] = i; digits = "0123456789abcdefghijklmnopqrstuvwxyz" }
    {
      h = 0
      for (i = 1; i <= length($0); i++) h = (h * 131 + code[substr($0, i, 1)]) % 4294967291
      id = ""
      do { id = substr(digits, h % 36 + 1, 1) id; h = int(h / 36) } while (h > 0)
      printf "%s\t%s\n", id, $0
    }'
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
    corpus_test_names "$file" | sed "s/^/$(basename "$file" .ply)./"
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
  for entry in "${PACKAGE_PROOFS[@]}"; do
    [[ ${entry%%:*} == "$1" ]] && { printf '%s\n' "${entry#*:}"; return 0; }
  done
  return 1
}

# Each `.ply` of a package below `crates/` other than the library's that states a law or an `ensures`
# or `cost` clause, which only a `ply prove` discharges.
claiming_files() {
  grep -rlE --include='*.ply' '^law(/[a-z]+)? "|^[[:space:]]+(ensures|cost) ' "$root"/crates/*/ply 2>/dev/null |
    grep -v "^$root/$STD_LIBRARY/" | sed "s|^$root/||" | LC_ALL=C sort || true
}

# The labels of a module's tests, in the order it declares them.
corpus_test_names() {
  sed -nE 's/^test(\/[a-z]+)? "([^"]*)".*/\2/p' "$1"
}

corpus_alone() {
  local id
  for id in "${CORPUS_ALONE[@]}"; do [[ $id == "$1" ]] && return 0; done
  return 1
}

# Whether the entry is a test of a module whose checks start desks.
corpus_desk() {
  named_in "${1%%:*}" "${CORPUS_DESKS[@]}"
}

cmd_corpus_matrix() {
  local id k first=1
  printf '{"include":['
  for ((k = 1; k <= DESK_RUNNERS; k++)); do
    ((first)) || printf ','
    first=0
    printf '{"id":"desks-%d"}' "$k"
  done
  for id in "${CORPUS_ALONE[@]}"; do
    ((first)) || printf ','
    first=0
    printf '{"id":"%s"}' "$id"
  done
  printf ']}\n'
}

# Every entry the partitions take: neither alone nor a desk run.
corpus_placed() {
  local id
  while read -r id; do
    corpus_alone "$id" || corpus_desk "$id" || printf '%s\n' "$id"
  done < <(corpus_entries)
}

# Every entry the desk runners take.
desk_placed() {
  local id
  while read -r id; do
    if corpus_desk "$id"; then printf '%s\n' "$id"; fi
  done < <(corpus_entries)
}

# The runs the partitions take, a `ply test` the rows in $1 measured over $2 milliseconds as its parts:
# `run#K/N` for K from 1 to N, N the fewest parts whose shares of the run are no longer than that, each
# the run's `ply test --shard K/N`. A proof run is a `ply prove`, which has no parts, and a single
# test's run is no shorter in any, so those stay whole, and so does every run when $2 is empty.
corpus_parted() {
  corpus_placed | awk -v rows="$1" -v limit="${2:-0}" '
    BEGIN {
      while ((getline line < rows) > 0) {
        split(line, f, "\t")
        if (f[1] == "corpus") ms[f[2]] = f[3] + 0
      }
    }
    # Of the runs named with a colon, only the library'"'"'s test runs are neither proofs nor a test.
    limit > 0 && ms[$1] > limit && ($1 !~ /:/ || $1 ~ /^stdlib:/) {
      n = int(ms[$1] / limit)
      if (n * limit < ms[$1]) n++
      for (k = 1; k <= n; k++) print $1 "#" k "/" n
      next
    }
    { print }
  '
}

# The job of the workflow whose steps run COMMAND.
job_running() {
  awk -v c="${1//./\\.}" '
    /^  [a-z-]+:$/ { job = $1; sub(/:$/, "", job) }
    $0 ~ c { print job; exit }
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

# The milliseconds a run of the job running COMMAND may take, measured: a third of the job's deadline,
# which leaves the cut room to balance the lanes and a cold run room to take longer. Nothing for a job
# with no deadline.
run_limit() {
  local minutes
  minutes=$(deadline_of "$(job_running "$1")")
  [[ -z $minutes ]] || printf '%d\n' $((minutes * 60000 / 3))
}

# `run seconds` per run the job running COMMAND takes, the partitions' as the rows in TABLE part them,
# that TABLE measured over the job's limit.
overlong() {
  local table=$1 command=$2 limit
  limit=$(run_limit "$command")
  [[ -n $limit ]] || return 0
  if [[ $command == *partition ]]; then corpus_parted "$table" "$limit"; else desk_placed; fi |
    awk -v rows="$table" -v limit="$limit" '
      BEGIN {
        while ((getline line < rows) > 0) {
          split(line, f, "\t")
          if (f[1] == "corpus") ms[f[2]] = f[3] + 0
        }
      }
      ms[$1] > limit { printf "%s %d\n", $1, ms[$1] / 1000 }
    '
}

# Why the cut leaves the run $1 longer than its limit, and what would make it shorter.
unparted() {
  case $1 in
    proofs:*) printf 'a proof run is one `ply prove`, which has no parts: give some of its claims a module of their own, or make them cheaper\n' ;;
    *#*) printf 'a part takes the tests whose keys hash to it, and one of these, or the few it took, outlast the limit: place its module a test at a time (CORPUS_BY_TEST, CLI_BY_TEST), or split the longest test\n' ;;
    *) printf 'a single test has no parts: split it into tests\n' ;;
  esac
}

# The desk runs runner K takes, one a line: the plan's cut when DIR holds one, else round robin.
cmd_desks_for_runner() {
  local k=$1 dir=${2:-} id n=0
  if [[ -n $dir ]]; then
    [[ -f $dir/desks-$k.txt ]] || { echo "the plan's cut in $dir holds no desks-$k.txt" >&2; return 1; }
    cat "$dir/desks-$k.txt"
    return 0
  fi
  while read -r id; do
    (((n % DESK_RUNNERS) + 1 == k)) && printf '%s\n' "$id"
    n=$((n + 1))
  done < <(desk_placed)
}

# `lane entry` per corpus run partition K takes: the plan's cut when DIR holds one, which it does for
# every partition once durations were measured; else round robin, a partition's lanes in turn.
cmd_corpus_for_partition() {
  local k=$1 dir=${2:-} id n=0
  if [[ -n $dir ]]; then
    [[ -f $dir/corpus-$k.txt ]] || { echo "the plan's cut in $dir holds no corpus-$k.txt" >&2; return 1; }
    cat "$dir/corpus-$k.txt"
    return 0
  fi
  while read -r id; do
    (((n % PARTITIONS) + 1 == k)) && printf '%d %s\n' $(((n / PARTITIONS) % CORPUS_LANES + 1)) "$id"
    n=$((n + 1))
  done < <(corpus_placed)
}

# `path<TAB>filter<TAB>shard` per entry named, in order, a field empty where the run has none: the
# program's entry takes every test of its package, a module's its own, and `module:<id>` the test whose
# label hashes to it, by the qualified name `ply test --filter` matches; `run#K/N` takes part K of N
# of what `run` would, by `ply test --shard K/N`. A module the cut places a test at a time is a run too,
# of every test it declares, and so are `stdlib` and `proofs`, of every test and every claim of the
# library.
cmd_corpus_line() {
  local entries id
  # Read whole before the loop can return, so the lister never writes into a closed pipe.
  entries=$(corpus_entries)
  for id in "$@"; do corpus_line_of "$id" "$entries" || return 1; done
}

# The line of the entry $1 among the entries $2.
corpus_line_of() {
  local entry line run=${1%%#*} part=
  if [[ $1 == *#* ]]; then
    part=${1#*#}
    if ! [[ $part =~ ^([1-9][0-9]*)/([1-9][0-9]*)$ ]] || ((BASH_REMATCH[1] > BASH_REMATCH[2])); then
      echo "corpus entry '$1' names no part K/N of '$run', K from 1 to N" >&2
      return 1
    fi
  fi
  while read -r entry; do
    [[ $entry == "$run" || ${entry%%:*} == "$run" ]] || continue
    line=$(entry_line "$run") || return 1
    printf '%s%s%s\n' "$line" "$TAB" "$part"
    return
  done <<< "$2"
  echo "no corpus entry named '$1'" >&2
  return 1
}

# `path<TAB>filter` of the entry $1, or of the run a module is or a run cut into parts is.
entry_line() {
  if [[ $1 == program ]]; then
    printf '%s%s\n' "$CORPUS_PROGRAM" "$TAB"
  elif [[ $1 == stdlib || $1 == proofs ]]; then
    printf '%s%s\n' "$CORPUS_STDLIB" "$TAB"
  elif [[ $1 == stdlib:* || $1 == proofs:* ]]; then
    printf '%s%s%s.\n' "$CORPUS_STDLIB" "$TAB" "${1#*:}"
  elif [[ $1 == fixture-* ]]; then
    printf '%s/%s.ply%s\n' "$CORPUS_FIXTURES" "${1#fixture-}" "$TAB"
  elif [[ $1 == package-* ]]; then
    printf '%s%s\n' "$(package_path "${1#package-}")" "$TAB"
  elif [[ $1 == laws-* ]]; then
    printf '%s%s\n' "$(proofs_path "${1#laws-}")" "$TAB"
  elif [[ $1 == cli-* ]]; then
    module_line "$CLI_SUITE" "${1#cli-}"
  else
    module_line "$CORPUS_CHECKS" "$1"
  fi
}

# `path<TAB>filter` of the run `module` or `module:<id>` of the package at DIR.
module_line() {
  local dir=$1 module=${2%%:*} name
  if [[ $2 == *:* ]]; then
    name=$(corpus_test_names "$root/$dir/$module.ply" | label_ids | awk -F"$TAB" -v id="${2##*:}" '$1 == id { print $2; exit }')
    [[ -n $name ]] || { echo "no test of $dir/$module.ply has the id '${2##*:}'" >&2; return 1; }
    printf '%s%s%s.%s\n' "$dir" "$TAB" "$module" "$name"
  else
    printf '%s%s%s.\n' "$dir" "$TAB" "$module"
  fi
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

# A Rust test is answered by its trace when nothing it read of the pack or the checkout has moved:
# the trace store is keyed by the Rust key, so a test binary that changed answers nothing. The
# answered tests are left out through a profile's `default-filter` in the tool config written to
# OUT, never on the command line, where they outgrow the length of one argument: SHARD's profile
# with its filter narrowed, or else `answered`. Prints `<profile> <tests answered>`.
cmd_rust_answered() {
  local dir=${1:?a directory of traces} out=${2:?a tool config to write} shard=${3:-}
  local pack=${PLY_PACK:-$root/target/debug/ply-pack} answered excluded count profile base
  answered=$([ -d "$dir" ] && (cd "$root" && "$pack" --answered "$dir") || true)
  if [ -z "$answered" ]; then
    excluded='none()'
    count=0
  else
    excluded=$(printf '%s\n' "$answered" | grouped_filter)
    count=$(printf '%s\n' "$answered" | grep -c .)
  fi
  if [ -n "$shard" ]; then
    profile=$(sed -n 's/^\[profile\.\(shard[0-9]*\)\]$/\1/p' "$shard" | head -n 1)
    base=$(sed -n "s/^default-filter = '''\(.*\)'''\$/\1/p" "$shard" | head -n 1)
    if [ -z "$profile" ] || [ -z "$base" ]; then
      echo "FAIL: $shard declares no shard profile with a default-filter" >&2
      return 1
    fi
    printf "[profile.%s]\ndefault-filter = '''(%s) and not (%s)'''\n" "$profile" "$base" "$excluded" > "$out"
  else
    profile=answered
    printf "[profile.%s]\ndefault-filter = '''not (%s)'''\n" "$profile" "$excluded" > "$out"
  fi
  printf '%s %s\n' "$profile" "$count"
}

# Only a pass is kept: a trace whose test failed, or that the report does not name, is dropped.
cmd_rust_kept() {
  local new=${1:?a directory of new traces} junit=${2:?a nextest JUnit report} out=${3:?a directory}
  mkdir -p "$out"
  [ -d "$new" ] && [ -f "$junit" ] || return 0
  python3 - "$new" "$junit" "$out" <<'PY'
import os, shutil, sys
import xml.etree.ElementTree as ET
new, junit, out = sys.argv[1:4]
passed = set()
for case in ET.parse(junit).getroot().iter("testcase"):
    if case.find("failure") is None and case.find("error") is None:
        passed.add((case.get("classname"), case.get("name")))
for name in os.listdir(new):
    with open(os.path.join(new, name), encoding="utf-8", errors="replace") as f:
        head = f.readline().rstrip("\n").split("\t")
    if len(head) == 3 and head[0] == "test" and (head[1], head[2]) in passed:
        shutil.copy(os.path.join(new, name), os.path.join(out, name))
PY
}

cmd_exclude_filter() {
  printf '%s | %s\n' "$(cmd_host_filter)" "$(triples "${GATES_ALONE[@]}" | filter_of)"
}

cmd_partitions() { matrix "$PARTITIONS"; }

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

# The table the next run is cut by, from this run's rows on stdin and BEFORE, the table this run was
# cut by: one duration a test, the longest any job measured. A corpus or startup row is what it cost
# when no corpus test came from the cache, so a run that took some keeps BEFORE's: cut by a warm
# run's costs, a cold run piles what the cache had saved onto one partition. A corpus or startup row
# no job reported, its job left unstarted by the selection, keeps BEFORE's. A run whose every part
# `run#K/N` reported is reported too, the sum of its parts, which the cut parts it by; a reported run
# takes along BEFORE's rows of its parts in another shape, and a warm run leaves out a part's first
# row, its tests mostly the cache's. `cached` rows only say which run this was.
cmd_timings() {
  local before=$1
  awk -F"$TAB" -v OFS="$TAB" -v before="$before" '
    function run_of(id) { sub(/#.*/, "", id); return id }
    function shape_of(id) { return match(id, /\/[0-9]+$/) ? substr(id, RSTART + 1) : "" }
    BEGIN {
      while ((getline line < before) > 0) {
        split(line, f, "\t")
        if (f[1] == "corpus" || f[1] == "startup") kept[f[1] "\t" f[2]] = f[3]
      }
    }
    $1 == "cached" { if ($3 + 0 > 0) warm = 1; next }
    NF == 3 { k = $1 OFS $2; if (!(k in ms) || $3 + 0 > ms[k]) ms[k] = $3 + 0 }
    END {
      for (k in ms) {
        split(k, f, OFS)
        if (f[1] != "corpus" || !index(f[2], "#")) continue
        run = run_of(f[2])
        shape[run] = shape_of(f[2])
        parts[run]++
        sum[run] += ms[k]
      }
      for (run in shape) if (!(("corpus" OFS run) in ms) && parts[run] == shape[run] + 0) ms["corpus" OFS run] = sum[run]
      for (k in ms) {
        split(k, f, OFS)
        if (warm && (f[1] == "corpus" || f[1] == "startup") && (k in kept)) print k, kept[k]
        else if (!warm || f[1] != "corpus" || !index(f[2], "#")) print k, ms[k]
      }
      for (k in kept) {
        if (k in ms) continue
        split(k, f, OFS)
        run = run_of(f[2])
        if (f[1] == "corpus" && index(f[2], "#") && ((run in shape) ? shape_of(f[2]) != shape[run] : (("corpus" OFS run) in ms))) continue
        print k, kept[k]
      }
    }
  ' | LC_ALL=C sort -t"$TAB" -k1,1 -k2,2
}

# `t shard binary test ms` per timed test, longest first onto the least loaded shard, then
# `load shard ms tests` per shard and `catchall shard`: the shard with the most room left, which
# is the one that runs what no other shard names.
assign() {
  LC_ALL=C sort -t"$TAB" -k3,3nr -k1,1 -k2,2 "$1" |
    awk -F"$TAB" -v n="$NEXTEST_SHARDS" '
      {
        best = 1
        for (i = 2; i <= n; i++) if (load[i] < load[best]) best = i
        load[best] += $3
        held[best]++
        printf "t\t%d\t%s\t%s\t%d\n", best, $1, $2, $3
      }
      END {
        least = 1
        for (i = 2; i <= n; i++) if (load[i] < load[least]) least = i
        for (i = 1; i <= n; i++) printf "load\t%d\t%d\t%d\n", i, load[i] + 0, held[i] + 0
        printf "catchall\t%d\n", least
      }
    '
}

# `(binary_id(=b) & (test(=x) | test(=y))) | ...` over `binary test` lines on stdin, unterminated.
grouped_filter() {
  LC_ALL=C sort -t"$TAB" -k1,1 -k2,2 | awk -F"$TAB" '
    $1 != id {
      if (open) printf ")) | "
      printf "(binary_id(=%s) & (", $1
      id = $1
      open = 1
      first = 1
    }
    { if (!first) printf " | "; printf "test(=%s)", $2; first = 0 }
    END { if (open) printf "))" }
  '
}

# The overrides that start a shard's tests longest first, spread over nextest's range so that a
# test with no measured duration keeps the default 0 and starts among the middle of them.
priority_blocks() {
  local dir=$1 priority
  LC_ALL=C sort -t"$TAB" -k3,3nr -k1,1 -k2,2 | awk -F"$TAB" -v dir="$dir" '
    { id[NR] = $1; name[NR] = $2 }
    END {
      for (r = 1; r <= NR; r++) {
        p = (NR > 1) ? 100 - 200 * (r - 1) / (NR - 1) : 100
        p = int(p / 10 + (p >= 0 ? 0.5 : -0.5)) * 10
        if (p != 0) printf("%s\t%s\n", id[r], name[r]) > (dir "/bucket." p)
      }
    }
  '
  for ((priority = 100; priority >= -100; priority -= 10)); do
    [[ -s "$dir/bucket.$priority" ]] || continue
    printf '\n[[profile.default.overrides]]\n'
    printf "filter = '''%s'''\n" "$(grouped_filter < "$dir/bucket.$priority")"
    printf 'priority = %d\n' "$priority"
  done
}

# The nextest binary id of every test binary the tree builds: the package for a library's unit
# tests, `package::target` for an integration test, `package::kind/name` for any other target.
built_binary_ids() {
  cargo metadata --no-deps --format-version 1 --manifest-path "$root/Cargo.toml" |
    jq -r '.packages[] | .name as $p | .targets[] | select(.test)
      | if .kind[0] == "test" then "\($p)::\(.name)"
        elif (.kind[0] | IN("bin", "example", "bench")) then "\($p)::\(.kind[0])/\(.name)"
        else $p end'
}

# The rows of $1 whose binary the tree still builds, on $2. Judged by binary, not by test: a filter
# naming a binary id nothing builds is a nextest error, one naming a test its binary no longer has
# matches nothing, and a test's source file cannot be read off its name.
living_durations() {
  local built dropped
  built=$(built_binary_ids) && [[ -n $built ]] || {
    echo "FAIL: cargo metadata named no test binary in $root" >&2
    return 1
  }
  # A corpus row is an entry's own or a part's of it, and lives while the entry does; a startup row is
  # a package's.
  dropped=$(printf '%s\n' "$built" | awk -F"$TAB" -v out="$2" -v placed="$({ corpus_placed; desk_placed; } | tr '\n' ' ')" '
    BEGIN {
      n = split(placed, ids, " ")
      for (i = 1; i <= n; i++) corpus[ids[i]] = 1
      n = 0
    }
    NR == FNR { live[$0] = 1; next }
    $1 == "corpus" { run = $2; sub(/#.*/, "", run); if (run in corpus) print > out; else n++; next }
    $1 == "startup" { print > out; next }
    $1 in live { print > out; next }
    { n++ }
    END { printf "%d", n }
  ' - "$1")
  [[ $dropped -eq 0 ]] || echo "$dropped measured row(s) name binaries or corpus runs the tree no longer has; left to the catch-all" >&2
}

# One `shard-<i>.toml` per nextest shard: the tests it runs as its profile's `default-filter`, and
# the order to start them in, then the corpus and desk cuts. 3 when there is nothing measured to cut,
# so the caller slices by count.
shard_configs() {
  local dir=$1 timings=$2 tmp catchall first i
  if [[ ! -s $timings ]]; then
    echo "no measured durations at $timings" >&2
    return 3
  fi
  if ! awk -F"$TAB" 'NF != 3 || $3 !~ /^[0-9]+$/ { exit 1 }' "$timings"; then
    echo "FAIL: $timings is not one 'binary_id<TAB>test<TAB>milliseconds' line per test" >&2
    return 1
  fi
  tmp=$(mktemp -d)
  : > "$tmp/living"
  living_durations "$timings" "$tmp/living" || { rm -rf "$tmp"; return 1; }
  if [[ ! -s $tmp/living ]]; then
    echo "no measured durations name a binary the tree still builds" >&2
    rm -rf "$tmp"
    return 3
  fi
  awk -F"$TAB" -v corpus="$tmp/corpus" '$1 == "corpus" || $1 == "startup" { print > corpus; next } { print }' \
    "$tmp/living" > "$tmp/nextest"
  if [[ ! -s $tmp/nextest ]]; then
    echo "no measured durations name a test nextest runs" >&2
    rm -rf "$tmp"
    return 3
  fi
  timings=$tmp/nextest
  assign "$timings" > "$tmp/assigned"
  catchall=$(awk -F"$TAB" '$1 == "catchall" { print $2 }' "$tmp/assigned")
  if awk -F"$TAB" '$1 == "load" && $4 == 0 { bare = 1 } END { exit !bare }' "$tmp/assigned"; then
    echo "$(grep -c . "$timings") measured tests do not fill $NEXTEST_SHARDS shards" >&2
    rm -rf "$tmp"
    return 3
  fi
  mkdir -p "$dir"
  for ((i = 1; i <= NEXTEST_SHARDS; i++)); do
    mkdir -p "$tmp/order.$i"
    awk -F"$TAB" -v s="$i" '$1 == "t" && $2 == s { printf "%s\t%s\t%s\n", $3, $4, $5 }' \
      "$tmp/assigned" > "$tmp/held.$i"
    cut -f1,2 "$tmp/held.$i" | grouped_filter > "$tmp/filter.$i"
  done
  {
    printf 'not ('
    first=1
    for ((i = 1; i <= NEXTEST_SHARDS; i++)); do
      [[ $i -eq $catchall ]] && continue
      ((first)) || printf ' | '
      first=0
      printf '%s' "$(cat "$tmp/filter.$i")"
    done
    printf ')'
  } > "$tmp/negation"
  mv "$tmp/negation" "$tmp/filter.$catchall"
  for ((i = 1; i <= NEXTEST_SHARDS; i++)); do
    {
      printf '[profile.shard%d]\n' "$i"
      printf "default-filter = '''%s'''\n" "$(cat "$tmp/filter.$i")"
      priority_blocks "$tmp/order.$i" < "$tmp/held.$i"
    } > "$dir/shard-$i.toml"
  done
  awk -F"$TAB" -v catchall="$catchall" '
    $1 == "load" {
      printf "shard %s: %d tests, %.1fs%s\n", $2, $4, $3 / 1000,
        ($2 == catchall ? ", and every test with no measured duration" : "")
    }
  ' "$tmp/assigned"
  touch "$tmp/corpus"
  corpus_cut "$dir" "$tmp/corpus"
  desk_cut "$dir" "$tmp/corpus"
  rm -rf "$tmp"
}

# The desk runs, longest first onto the runner that would end soonest with it, as `desks-<k>.txt` of
# an entry a line. Every runner loads the checks once, so no startup tips the choice. A run nothing
# measured counts as the median of those that were, and is placed after them.
desk_cut() {
  local dir=$1 rows=$2 k
  for ((k = 1; k <= DESK_RUNNERS; k++)); do : > "$dir/desks-$k.txt"; done
  desk_placed | awk -v rows="$rows" -v dir="$dir" -v n="$DESK_RUNNERS" '
    BEGIN {
      FS = "\t"
      while ((getline line < rows) > 0) {
        split(line, f, "\t")
        if (f[1] == "corpus") ms[f[2]] = f[3] + 0
      }
      FS = " "
    }
    { ids[++m] = $1 }
    END {
      for (i = 1; i <= m; i++) if (ids[i] in ms) { t++; order[t] = ids[i] }
      # Longest first, ties in entry order.
      for (i = 2; i <= t; i++) {
        x = order[i]
        for (j = i - 1; j >= 1 && ms[order[j]] < ms[x]; j--) order[j + 1] = order[j]
        order[j + 1] = x
      }
      median = t ? ms[order[int((t + 1) / 2)]] : 60000
      for (i = 1; i <= m; i++) if (!(ids[i] in ms)) { order[++t] = ids[i]; ms[ids[i]] = median }
      for (i = 1; i <= t; i++) {
        best = 1
        for (k = 2; k <= n; k++) if (load[k] < load[best]) best = k
        load[best] += ms[order[i]]
        held[best]++
        print order[i] >> (dir "/desks-" best ".txt")
      }
      for (k = 1; k <= n; k++) printf "desks %d: %d runs, %.1fs\n", k, held[k], load[k] / 1000
    }
  '
}

# The corpus runs the partitions take, as `corpus-<k>.txt` of `lane entry` lines. The tests of a module
# placed a test at a time, and the standard library's runs of each kind, go in as runs of neighbours,
# one run while they fit a lane and as few as they need once they do not, so a cold start they share is
# paid once a run rather than once an entry. Runs and the other entries go longest first onto the lane
# of every partition's that would end soonest with it; the lanes are taken partition by partition, so
# the first placed land on different runners. A lane pays each package's startup once. A run measured
# over a third of the partitions' deadline goes in as its parts (`corpus_parted`). A part nothing
# measured counts as its share of its run; any other entry nothing measured as the median of its
# neighbours that were measured, or else, for a module newly placed a test at a time, as its share of
# what the module measured whole, or else as the median of every entry that was.
corpus_cut() {
  local dir=$1 rows=$2 k
  for ((k = 1; k <= PARTITIONS; k++)); do : > "$dir/corpus-$k.txt"; done
  corpus_parted "$rows" "$(run_limit "ci-corpus.sh partition")" |
    awk -v rows="$rows" -v dir="$dir" -v p="$PARTITIONS" -v l="$CORPUS_LANES" \
    -v checks="$CORPUS_CHECKS" -v cli="$CLI_SUITE" -v stdlib="$CORPUS_STDLIB" '
    # What the startup a lane pays once is kept under: a part of a run is a `ply test` of its own.
    function package(id,   whole) {
      if (index(id, "#")) {
        whole = package(substr(id, 1, index(id, "#") - 1))
        return (whole in batched) ? whole "#" id : id
      }
      if (id ~ /^cli-/) return cli
      if (id ~ /^stdlib:/) return stdlib
      if (id ~ /^proofs:/) return "prove:" stdlib
      if (id == "program" || id ~ /^package-/ || id ~ /^laws-/ || id ~ /^fixture-/) return id
      return checks
    }
    # About two minutes unmeasured, a part'"'"'s measured under its run, whose every part loads the same
    # closure; a program or package suite row holds its startup whole, and so do the proof runs, each
    # a share of its prove.
    function startup(key,   run) {
      run = key
      sub(/#[0-9]+\/[0-9]+$/, "", run)
      if (run in start) return start[run]
      sub(/#.*/, "", run)
      return (run in batched) ? 120000 : 0
    }
    # A part is a run of its own, whatever its run is a neighbour of.
    function module_of(id) { return (index(id, ":") && !index(id, "#")) ? substr(id, 1, index(id, ":") - 1) : "" }
    # The share of its run a part `run#K/N` is.
    function share_of(id,   run) {
      run = substr(id, 1, index(id, "#") - 1)
      match(id, /[0-9]+$/)
      return (run in ms) ? ms[run] / substr(id, RSTART) : median
    }
    # A new unit holding nothing yet, of the package `id` is tested in.
    function unit(id) { u++; size[u] = 0; cost[u] = 0; pkg[u] = package(id) }
    function hold(id) { member[u, ++size[u]] = id; cost[u] += ms[id] }
    # The middle of the first `count` values of `v`, which it sorts.
    function middle(v, count,   i, j, x) {
      for (i = 2; i <= count; i++) {
        x = v[i]
        for (j = i - 1; j >= 1 && v[j] > x; j--) v[j + 1] = v[j]
        v[j + 1] = x
      }
      return v[int((count + 1) / 2)]
    }
    BEGIN {
      FS = "\t"
      while ((getline line < rows) > 0) {
        split(line, f, "\t")
        if (f[1] == "startup") start[f[2]] = f[3] + 0
        else {
          ms[f[2]] = f[3] + 0
          # What a module measured as one run, or as the parts of one, before it was placed a test
          # at a time.
          if (!index(f[2], ":")) { whole_of = f[2]; sub(/#.*/, "", whole_of); whole[whole_of] += f[3] + 0 }
        }
      }
      FS = " "
      batched[checks] = batched[cli] = batched[stdlib] = 1
    }
    { ids[++n] = $1 }
    END {
      for (i = 1; i <= n; i++) if (ids[i] in ms) measured[++m] = ms[ids[i]]
      median = m ? middle(measured, m) : 60000
      for (i = 1; i <= n; ) {
        mod = module_of(ids[i])
        c = 0
        for (j = i; j <= n && (j == i || (mod != "" && module_of(ids[j]) == mod)); j++)
          if (ids[j] in ms) near[++c] = ms[ids[j]]
        guess = (mod != "" && c) ? middle(near, c) : (mod != "" && (mod in whole)) ? whole[mod] / (j - i) : median
        for (; i < j; i++) {
          if (!(ids[i] in ms)) ms[ids[i]] = index(ids[i], "#") ? share_of(ids[i]) : guess
          total += ms[ids[i]]
        }
      }
      lanes = p * l
      target = total / lanes
      # A module placed a test at a time lists its tests together, in the order it declares them.
      for (i = 1; i <= n; ) {
        mod = module_of(ids[i])
        if (mod == "") { unit(ids[i]); hold(ids[i]); i++; continue }
        sum = 0
        for (j = i; j <= n && module_of(ids[j]) == mod; j++) sum += ms[ids[j]]
        runs = int(sum / target)
        if (runs * target < sum) runs++
        if (runs > j - i) runs = j - i
        if (runs < 1) runs = 1
        share = sum / runs
        unit(ids[i])
        for (x = i; x < j; x++) {
          if (size[u] > 0 && runs > 1 && cost[u] + ms[ids[x]] / 2 > share) { unit(ids[x]); runs-- }
          hold(ids[x])
        }
        i = j
      }
      # Longest first, ties in entry order.
      for (i = 1; i <= u; i++) order[i] = i
      for (i = 2; i <= u; i++) {
        x = order[i]
        for (j = i - 1; j >= 1 && cost[order[j]] < cost[x]; j--) order[j + 1] = order[j]
        order[j + 1] = x
      }
      for (i = 1; i <= u; i++) {
        x = order[i]
        best = 0
        for (j = 1; j <= lanes; j++) {
          end = load[j] + cost[x] + ((j SUBSEP pkg[x]) in loads ? 0 : startup(pkg[x]))
          if (best == 0 || end < bestend) { best = j; bestend = end }
        }
        load[best] = bestend
        loads[best, pkg[x]] = 1
        for (k = 1; k <= size[x]; k++) {
          held[best]++
          printf "%d %s\n", int((best - 1) / p) + 1, member[x, k] >> (dir "/corpus-" ((best - 1) % p + 1) ".txt")
        }
      }
      for (j = 1; j <= lanes; j++)
        printf "corpus %d.%d: %d runs, %.1fs\n", (j - 1) % p + 1, int((j - 1) / p) + 1, held[j], load[j] / 1000
    }
  '
}

cmd_shard_configs() { shard_configs "${1:?a directory to write the configs to}" "$TIMINGS"; }

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
  local files=() file
  while IFS= read -r file; do files+=("$file"); done < <(ci_files)
  awk -v families="${SUPERSEDED[*]} ${NEWEST[*]}" '
    function literal(s) {
      sub(/\$\{\{.*/, "", s)
      gsub(/^[[:space:]"]+|[[:space:]"]+$/, "", s)
      return s
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
          printf "FAIL: SUPERSEDED or NEWEST names \"%s\", which no save writes\n", fk[j] > "/dev/stderr"
          bad = 1
        }
      }
      if (bad) exit 1
      printf "cache keys: %d written and %d restored, each side matched by the other; %d run-scoped, each read by a later run and needed by no job\n", n, length(read), length(run_scoped)
    }
  ' "${files[@]}"
}

# What each key above is a key *of*. The stage, its emitted bodies and its objects are built from
# every tree that holds Ply sources, so a key that enumerates those trees by name goes stale the
# moment a package is added -- and a stale hit serves a build of sources that moved, which no test
# sees, because the tests then run against the cache rather than the sources. Any `hashFiles` that
# names a crate's own `ply/` sources -- or any crate tree at all -- must name every one of them.
cmd_cache_payloads() {
  local pattern tree pat base covered missing=0 files=() file
  while IFS= read -r file; do files+=("$file"); done < <(ci_files)
  while IFS= read -r pattern; do
    case "$pattern" in *"/ply/"*) ;; *) continue ;; esac
    for tree in "$root"/crates/*/ply; do
      [ -d "$tree" ] || continue
      tree=${tree#"$root"/}
      covered=no
      while IFS= read -r pat; do
        pat=$(printf '%s' "$pat" | tr -d "\"' ")
        [ -n "$pat" ] || continue
        # The tree itself, or its recursive form: `crates/*/ply/**` and `crates/*/ply` both
        # name the whole tree, and `crates/*/ply/*` -- one level -- names only some of it.
        base=${pat%%/\*\*}
        if [[ $tree == $pat || $tree == $base ]]; then
          covered=yes
          break
        fi
      done < <(printf '%s\n' "$pattern" | tr ',' '\n')
      if [ "$covered" = no ]; then
        printf 'FAIL: a cache key hashes crate sources and does not name %s, whose Ply sources the stage is emitted from\n' "$tree" >&2
        missing=$((missing + 1))
      fi
    done
  done < <(grep -oh 'hashFiles([^)]*)' "${files[@]}" | sed -e 's/.*hashFiles(//' -e 's/)$//')
  if [ "$missing" -eq 0 ]; then
    printf 'cache payloads: every key over crate sources names every tree holding Ply sources\n'
  fi
  return $((missing > 0))
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

# The table at TIMINGS: the `timings.tsv` of the newest `test-durations` artifact that holds one, which
# a run's `passes` job writes when its test jobs all passed, of the runs on this pull request's
# branch and else on main's. A branch cuts by what its own tree measured, and main never by a
# branch's. A fork's run names its artifacts as it likes, so only this repository's own are taken.
# The newest few of a branch are looked in; none with a table leaves none, and the cut falls back to
# slicing by count.
cmd_fetch_timings() {
  local branch run runs dir=${TIMINGS%/*} tmp
  rm -rf "$dir"
  mkdir -p "$dir"
  for branch in ${GITHUB_HEAD_REF:-} main; do
    runs=$(BRANCH=$branch gh api --paginate "repos/$GITHUB_REPOSITORY/actions/artifacts?name=test-durations&per_page=100" \
      -q '.artifacts[] | select(.expired == false and .workflow_run.head_repository_id == .workflow_run.repository_id
            and .workflow_run.head_branch == env.BRANCH) | "\(.created_at) \(.workflow_run.id)"' |
      LC_ALL=C sort -r | head -n 5 | cut -d' ' -f2) || runs=
    for run in $runs; do
      tmp=$(mktemp -d)
      if gh run download "$run" -n test-durations -D "$tmp" > /dev/null 2>&1 && [[ -s $tmp/timings.tsv ]]; then
        mv "$tmp/timings.tsv" "$TIMINGS"
        rm -rf "$tmp"
        echo "the table run $run measured on $branch: $(grep -c . "$TIMINGS") durations"
        return 0
      fi
      rm -rf "$tmp"
    done
  done
  echo "no run on ${GITHUB_HEAD_REF:+$GITHUB_HEAD_REF or }main left a table: the cut falls back to slicing by count"
}

# A family only this run's entry replaces, so a job that wrote nothing keeps what it had; and of a
# family keyed by content, the newest entry on the ref. A ref runs one run at a time. On main, every
# entry of a pull request that is closed, which no run reads again.
cmd_supersede() {
  local run=${1:?usage: ci-shards.sh supersede RUN_ID REF} ref=${2:?a ref} prefix listing current key id
  for prefix in "${SUPERSEDED[@]}"; do
    listing=$(gh api --paginate "repos/$GITHUB_REPOSITORY/actions/caches?key=$prefix&ref=$ref&per_page=100" \
      -q '.actions_caches[] | "\(.key) \(.id)"')
    current=$(awk -v suffix="-$run" '{ k = $1; n = length(suffix); if (substr(k, length(k) - n + 1) == suffix) print substr(k, 1, length(k) - n) }' <<< "$listing" | sort -u)
    while read -r key id; do
      [[ -n $id && $key != *-"$run" ]] || continue
      grep -qxF "${key%-*}" <<< "$current" || continue
      gh api -X DELETE "repos/$GITHUB_REPOSITORY/actions/caches/$id" > /dev/null && echo "superseded $key"
    done <<< "$listing"
  done
  for prefix in "${NEWEST[@]}"; do
    listing=$(gh api --paginate "repos/$GITHUB_REPOSITORY/actions/caches?key=$prefix&ref=$ref&sort=created_at&direction=desc&per_page=100" \
      -q '.actions_caches[] | "\(.key) \(.id)"')
    while read -r key id; do
      [[ -n $id ]] || continue
      gh api -X DELETE "repos/$GITHUB_REPOSITORY/actions/caches/$id" > /dev/null && echo "superseded $key"
    done < <(awk '{ family = $1; sub(/[0-9a-f]+$/, "", family); if (seen[family]++) print }' <<< "$listing")
  done
  [[ $ref == refs/heads/main ]] || return 0
  local pull last= number state=
  while read -r pull id; do
    [[ -n $id ]] || continue
    if [[ $pull != "$last" ]]; then
      last=$pull
      number=${pull#refs/pull/}
      number=${number%/merge}
      state=$(gh api "repos/$GITHUB_REPOSITORY/pulls/$number" -q .state 2> /dev/null || echo open)
    fi
    [[ $state == closed ]] || continue
    gh api -X DELETE "repos/$GITHUB_REPOSITORY/actions/caches/$id" > /dev/null &&
      echo "gave back cache $id of closed pull request #$number"
  done < <(gh api --paginate "repos/$GITHUB_REPOSITORY/actions/caches?per_page=100" \
    -q '.actions_caches[] | select(.ref | startswith("refs/pull/")) | "\(.ref) \(.id)"' | sort)
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

# The tests a shard config's `default-filter` names, as `binary_id test`. The `tr` is what keeps
# awk off a record hundreds of kilobytes long: every `|` of the filterset starts a new one.
named_by() {
  grep '^default-filter = ' "$1" | tr '|' '\n' | awk '
    {
      if (match($0, "binary_id\\(=[^)]*\\)")) {
        token = substr($0, RSTART, RLENGTH)
        id = substr(token, 12, length(token) - 12)
      }
      if (match($0, "test\\(=[^)]*\\)")) {
        token = substr($0, RSTART, RLENGTH)
        printf "%s\t%s\n", id, substr(token, 7, length(token) - 7)
      }
    }
  '
}

# The shards a table cuts run every test exactly once: no two name the same test, and one is the
# negation of all the others, so a test none of them names — a test added since the table was
# measured — runs there and nowhere else.
check_shards() {
  local what=$1 timings=$2 tmp dir catchall seen bad=0 rc=0 i
  tmp=$(mktemp -d)
  dir=$tmp/configs
  shard_configs "$dir" "$timings" > /dev/null || rc=$?
  if [[ $rc -ne 0 ]]; then
    rm -rf "$tmp"
    # A measured table too small to fill the shards is the fallback, not a failure.
    if [[ $rc -eq 3 && $what == measured ]]; then
      return 0
    fi
    echo "FAIL: the $what durations cut no shards, so nothing here is checked" >&2
    return 1
  fi
  catchall=
  seen=0
  for ((i = 1; i <= NEXTEST_SHARDS; i++)); do
    if [[ ! -f "$dir/shard-$i.toml" ]]; then
      echo "FAIL: the $what durations cut no shard $i, and a nextest job runs one" >&2
      bad=1
      continue
    fi
    if ! grep -q "^\[profile\.shard$i\]$" "$dir/shard-$i.toml"; then
      echo "FAIL: $dir/shard-$i.toml declares no [profile.shard$i], which is the profile its job runs" >&2
      bad=1
    fi
    if grep -q "^default-filter = '''not (" "$dir/shard-$i.toml"; then
      catchall=$i
      seen=$((seen + 1))
    else
      named_by "$dir/shard-$i.toml" >> "$tmp/others"
    fi
  done
  if [[ $seen -ne 1 ]]; then
    echo "FAIL: $seen of the $what shards are the negation of the rest, and exactly one has to be: a test with no measured duration runs there" >&2
    bad=1
  fi
  if [[ $bad -eq 0 ]]; then
    named_by "$dir/shard-$catchall.toml" | LC_ALL=C sort > "$tmp/caught"
    LC_ALL=C sort "$tmp/others" > "$tmp/named"
    if [[ -n $(LC_ALL=C uniq -d "$tmp/named") ]]; then
      echo "FAIL: two of the $what shards name $(LC_ALL=C uniq -d "$tmp/named" | head -1), so it would run twice" >&2
      bad=1
    fi
    if ! cmp -s "$tmp/caught" "$tmp/named"; then
      echo "FAIL: the $what catch-all shard's negation is not what the other shards name, so a test runs twice or not at all" >&2
      bad=1
    fi
  fi
  for ((i = 1; i <= PARTITIONS; i++)); do
    cmd_corpus_for_partition "$i" "$dir"
  done | cut -d' ' -f2 | LC_ALL=C sort > "$tmp/corpus-cut"
  if ! cmp -s "$tmp/corpus-cut" <(corpus_parted "$timings" "$(run_limit "ci-corpus.sh partition")" | LC_ALL=C sort); then
    echo "FAIL: the $what cut's corpus runs are not every run a partition takes, each once or as its parts" >&2
    bad=1
  fi
  for ((i = 1; i <= DESK_RUNNERS; i++)); do
    cmd_desks_for_runner "$i" "$dir"
  done | LC_ALL=C sort > "$tmp/desks-cut"
  if ! cmp -s "$tmp/desks-cut" <(desk_placed | LC_ALL=C sort); then
    echo "FAIL: the $what cut's desk runs are not every desk run, each once" >&2
    bad=1
  fi
  rm -rf "$tmp"
  return "$bad"
}

# Whether the corpus cut over the nextest rows in $1 and invented corpus costs keeps a module placed
# a test at a time on one lane while it fits one, and splits one that outgrows a lane into runs of
# neighbouring tests, each on a lane of its own.
check_runs() {
  local nextest=$1 tmp fits outgrows module most count k bad=0
  tmp=$(mktemp -d)
  most=0
  for module in "${CLI_BY_TEST[@]}"; do
    count=$(corpus_placed | grep -c "^cli-$module:" || true)
    if [[ $count -gt $most ]]; then most=$count; fits=cli-$module; fi
  done
  most=0
  for module in "${CORPUS_BY_TEST[@]}"; do
    count=$(corpus_placed | grep -c "^$module:" || true)
    if [[ $count -gt $most ]]; then most=$count; outgrows=$module; fi
  done
  if [[ -z ${fits:-} || -z ${outgrows:-} || $most -lt 3 ]]; then
    echo "FAIL: no module placed a test at a time has the tests the corpus cut's check needs" >&2
    rm -rf "$tmp"
    return 1
  fi
  cp "$nextest" "$tmp/timings.tsv"
  corpus_placed | awk -v fits="$fits:" -v outgrows="$outgrows:" -v OFS='\t' '
    { ms = index($1, fits) == 1 ? 1 : index($1, outgrows) == 1 ? 30000 : 5000; print "corpus", $1, ms }
  ' >> "$tmp/timings.tsv"
  shard_configs "$tmp/cut" "$tmp/timings.tsv" > /dev/null || {
    echo "FAIL: the corpus cut's check cut nothing" >&2
    rm -rf "$tmp"
    return 1
  }
  corpus_placed > "$tmp/order"
  # `lane position` per test of the module, its position its place in the order the module declares.
  for module in "$fits" "$outgrows"; do
    for ((k = 1; k <= PARTITIONS; k++)); do
      awk -v k="$k" -v m="$module:" 'index($2, m) == 1 { print k "." $1, $2 }' "$tmp/cut/corpus-$k.txt"
    done | awk -v m="$module:" '
      NR == FNR { if (index($1, m) == 1) at[$1] = ++n; next }
      { print $1, at[$2] }
    ' "$tmp/order" - | sort -k2,2n > "$tmp/$module.lanes"
  done
  if [[ $(cut -d' ' -f1 "$tmp/$fits.lanes" | sort -u | grep -c .) -ne 1 ]]; then
    echo "FAIL: the corpus cut split $fits over lanes though its tests fit one" >&2
    bad=1
  fi
  if [[ $(cut -d' ' -f1 "$tmp/$outgrows.lanes" | sort -u | grep -c .) -lt 2 ]]; then
    echo "FAIL: the corpus cut kept $outgrows on one lane though it outgrows one" >&2
    bad=1
  fi
  if [[ $(cut -d' ' -f1 "$tmp/$outgrows.lanes" | uniq | grep -c .) -ne $(cut -d' ' -f1 "$tmp/$outgrows.lanes" | sort -u | grep -c .) ]]; then
    echo "FAIL: the corpus cut gave a lane tests of $outgrows that are not neighbours" >&2
    bad=1
  fi
  rm -rf "$tmp"
  return "$bad"
}

# Whether the corpus cut over the nextest rows in $1 and invented corpus costs parts each `ply test`
# measured over the partitions' limit into the fewest parts no longer than it, each a `--shard` of the
# run, and leaves whole every other run, measured or not; and whether what it leaves over the limit
# is a proof run, a single test and a part measured so, which it cannot part.
check_parts() {
  local nextest=$1 tmp limit long edge library proof single over want k bad=0
  limit=$(run_limit "ci-corpus.sh partition")
  if [[ -z $limit ]]; then
    echo "FAIL: the partitions' job sets no deadline, so the corpus cut parts no run" >&2
    return 1
  fi
  tmp=$(mktemp -d)
  corpus_placed > "$tmp/placed"
  long=$(grep -m1 -E '^cli-[^:]+$' "$tmp/placed" || true)
  edge=$(grep -E '^cli-[^:]+$' "$tmp/placed" | sed -n 2p || true)
  library=$(grep -m1 '^stdlib:' "$tmp/placed" || true)
  proof=$(grep -m1 '^proofs:' "$tmp/placed" || true)
  single=$(grep -m1 -E '^cli-[^:]+:' "$tmp/placed" || true)
  if [[ -z $long || -z $edge || -z $library || -z $proof || -z $single ]]; then
    echo "FAIL: the partitions take no two runs of the CLI's modules, library tests, proofs and single tests the corpus cut's check of parts needs" >&2
    rm -rf "$tmp"
    return 1
  fi
  {
    cat "$nextest"
    printf 'corpus\t%s\t%d\n' "$long" $((limit * 5 / 2)) "$long#1/3" $((limit + 1000)) "$edge" "$limit" \
      "$library" $((limit + 1)) "$proof" $((limit * 2)) "$single" $((limit * 2))
  } > "$tmp/timings.tsv"
  awk -v long="$long" -v library="$library" '
    $0 == long { print $0 "#1/3"; print $0 "#2/3"; print $0 "#3/3"; next }
    $0 == library { print $0 "#1/2"; print $0 "#2/2"; next }
    { print }
  ' "$tmp/placed" | LC_ALL=C sort > "$tmp/expected"
  shard_configs "$tmp/cut" "$tmp/timings.tsv" > /dev/null || {
    echo "FAIL: the corpus cut's check of parts cut nothing" >&2
    rm -rf "$tmp"
    return 1
  }
  for ((k = 1; k <= PARTITIONS; k++)); do cut -d' ' -f2 "$tmp/cut/corpus-$k.txt"; done | LC_ALL=C sort > "$tmp/parted"
  if ! cmp -s "$tmp/expected" "$tmp/parted"; then
    echo "FAIL: the corpus cut did not take $long, measured at two and a half times the partitions' limit, as three parts, and $library, just over it, as two, each part once, and every other run whole" >&2
    bad=1
  fi
  over=$(overlong "$tmp/timings.tsv" "ci-corpus.sh partition" | cut -d' ' -f1 | LC_ALL=C sort | tr '\n' ' ')
  want=$(printf '%s\n' "$long#1/3" "$proof" "$single" | LC_ALL=C sort | tr '\n' ' ')
  if [[ $over != "$want" ]]; then
    echo "FAIL: the runs the corpus cut leaves over the partitions' limit are '$over', not the proof run, the single test and the part measured over it: '$want'" >&2
    bad=1
  fi
  if [[ $(cmd_corpus_line "$long#2/3") != "$(cmd_corpus_line "$long")2/3" ]]; then
    echo "FAIL: corpus-line names another run than part 2 of 3 of $long, by \`ply test --shard 2/3\`" >&2
    bad=1
  fi
  if cmd_corpus_line "$long#4/3" > /dev/null 2>&1 || cmd_corpus_line "$long#0/3" > /dev/null 2>&1; then
    echo "FAIL: corpus-line takes a part of $long that no \`ply test --shard\` names" >&2
    bad=1
  fi
  rm -rf "$tmp"
  return "$bad"
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
  done < <(grep -rn -E "allow\((dead_code|unused)\)" --include=*.rs "$root/crates" "$root/benches" | sed "s|^$root/||" || true)

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
  # The archive carries `ply` only if ply-launcher has an integration test of its own.
  if ! ls "$root"/crates/ply-launcher/tests/*.rs >/dev/null 2>&1; then
    echo "FAIL: crates/ply-launcher/tests/ has no .rs file, so cargo builds no 'ply' for the Ply runs to drive" >&2
    failures=$((failures + 1))
  fi

  # --- the shards the durations cut -----------------------------------------
  local made_up
  if [[ $NEXTEST_SHARDS -lt 2 ]]; then
    echo "FAIL: NEXTEST_SHARDS is $NEXTEST_SHARDS, and one shard is the negation of the others" >&2
    failures=$((failures + 1))
  else
    made_up=$(mktemp -d)
    # Real tests with invented costs: the cut drops durations for tests the tree no longer has,
    # so a table it can check has to name ones it has.
    made_up_count=0
    for made_up_file in "$root"/crates/ply-machine-tests/tests/suite/*.rs; do
      made_up_mod=${made_up_file##*/}; made_up_mod=${made_up_mod%.rs}
      while read -r made_up_fn; do
        made_up_count=$((made_up_count + 1))
        printf 'ply-machine-tests::suite\t%s::%s\t%d\n' \
          "$made_up_mod" "$made_up_fn" $((made_up_count * 37 + 1)) >> "$made_up/timings.tsv"
        [[ $made_up_count -ge $((NEXTEST_SHARDS + 4)) ]] && break 2
      done < <(awk '/^#\[test\]/ { t = 1; next } t && match($0, /^fn [a-z0-9_]+\(/) { print substr($0, 4, RLENGTH - 4); t = 0 }' "$made_up_file")
    done
    check_shards made-up "$made_up/timings.tsv" || failures=$((failures + 1))
    check_runs "$made_up/timings.tsv" || failures=$((failures + 1))
    check_parts "$made_up/timings.tsv" || failures=$((failures + 1))
    rm -rf "$made_up"
    if [[ -s $TIMINGS ]]; then
      check_shards measured "$TIMINGS" || failures=$((failures + 1))
    fi
  fi

  # --- the table the next run is cut by -------------------------------------
  local table warm cold
  made_up=$(mktemp -d)
  printf 'corpus\ta\t100\nstartup\tp\t90000\nx::y\tt\t5\n' > "$made_up/before.tsv"
  printf 'corpus\ta\t3\ncorpus\ta\t7\ncorpus\tb\t40\nstartup\tp\t20000\nx::y\tt\t9\n' > "$made_up/measured.tsv"
  warm="corpus a 100;corpus b 40;startup p 90000;x::y t 9;"
  cold="corpus a 7;corpus b 40;startup p 20000;x::y t 9;"
  table=$({ cat "$made_up/measured.tsv"; printf 'cached\tp\t12\n'; } | cmd_timings "$made_up/before.tsv" | tr '\t\n' ' ;')
  if [[ $table != "$warm" ]]; then
    echo "FAIL: a run that took tests from the cache wrote '$table', not the corpus rows it was cut by: '$warm'" >&2
    failures=$((failures + 1))
  fi
  table=$({ cat "$made_up/measured.tsv"; printf 'cached\tp\t0\n'; } | cmd_timings "$made_up/before.tsv" | tr '\t\n' ' ;')
  if [[ $table != "$cold" ]]; then
    echo "FAIL: a run that took nothing from the cache wrote '$table', not the longest it measured: '$cold'" >&2
    failures=$((failures + 1))
  fi
  table=$(cmd_timings "$made_up/none.tsv" < "$made_up/measured.tsv" | tr '\t\n' ' ;')
  if [[ $table != "$cold" ]]; then
    echo "FAIL: a run with no table before it wrote '$table', not the longest it measured: '$cold'" >&2
    failures=$((failures + 1))
  fi
  # `r` parted anew and reported whole, `s` reported in part, `w` reported whole again.
  printf 'corpus\t%s\t%s\n' r 900 'r#1/3' 100 'r#2/3' 200 'r#3/3' 300 s 850 's#1/2' 400 's#2/2' 450 'w#1/2' 10 > "$made_up/before.tsv"
  printf 'corpus\t%s\t%s\n' 'r#1/2' 300 'r#2/2' 500 's#1/2' 420 w 70 > "$made_up/measured.tsv"
  warm="corpus r 900;corpus s 850;corpus s#1/2 400;corpus s#2/2 450;corpus w 70;"
  cold="corpus r 800;corpus r#1/2 300;corpus r#2/2 500;corpus s 850;corpus s#1/2 420;corpus s#2/2 450;corpus w 70;"
  table=$({ cat "$made_up/measured.tsv"; printf 'cached\tp\t12\n'; } | cmd_timings "$made_up/before.tsv" | tr '\t\n' ' ;')
  if [[ $table != "$warm" ]]; then
    echo "FAIL: a run that took tests from the cache and ran parts wrote '$table', not the rows it was cut by, less its parts of another shape: '$warm'" >&2
    failures=$((failures + 1))
  fi
  table=$({ cat "$made_up/measured.tsv"; printf 'cached\tp\t0\n'; } | cmd_timings "$made_up/before.tsv" | tr '\t\n' ' ;')
  if [[ $table != "$cold" ]]; then
    echo "FAIL: a run that took nothing from the cache and ran parts wrote '$table', not each run whose every part reported as their sum: '$cold'" >&2
    failures=$((failures + 1))
  fi
  rm -rf "$made_up"

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
  local corpus_job
  for dir in "$CORPUS_PROGRAM" "$CORPUS_CHECKS" "$CLI_SUITE"; do
    if [[ ! -f "$root/$dir/ply.pkg" ]]; then
      echo "FAIL: $dir holds no ply.pkg, and a corpus run tests it as a package" >&2
      failures=$((failures + 1))
    fi
  done
  if [[ -f "$root/$CORPUS_CHECKS/program.ply" ]]; then
    echo "FAIL: $CORPUS_CHECKS/program.ply would be run under the id of the program's own tests" >&2
    failures=$((failures + 1))
  fi
  if [[ $(corpus_entries | grep -c .) -lt 2 ]]; then
    echo "FAIL: no module of $CORPUS_CHECKS declares a test, so the corpus's checks run nowhere" >&2
    failures=$((failures + 1))
  fi
  local entries
  entries=$(corpus_entries)
  for id in "${CORPUS_ALONE[@]}"; do
    if ! grep -qx "$id" <<< "$entries"; then
      echo "FAIL: '$id' runs alone and no module of $CORPUS_CHECKS by that name declares a test" >&2
      failures=$((failures + 1))
    fi
  done
  for module in "${CORPUS_DESKS[@]}"; do
    if ! grep -q "^$module:" <<< "$entries"; then
      echo "FAIL: '$module' runs on the desk runners and $CORPUS_CHECKS/$module.ply declares no test" >&2
      failures=$((failures + 1))
    fi
  done
  if [[ $DESK_RUNNERS -lt 1 ]]; then
    echo "FAIL: DESK_RUNNERS is $DESK_RUNNERS, so the desk runs run nowhere" >&2
    failures=$((failures + 1))
  fi
  # The ids a developer's awk derives are the ones the runner's cut and lanes use.
  if [[ $(printf 'ä — non-ascii\n' | label_ids | cut -f1) != ebwzok ]]; then
    echo "FAIL: this awk hashes a label to another id than every other awk does" >&2
    failures=$((failures + 1))
  fi
  # A run is picked out by a substring of its tests' qualified names: a module's by `<module>.`, which
  # no test of another module may hold, and a test placed by name by its own, which no other test of
  # its module may hold.
  local module names keys pair outside
  for pair in "$CORPUS_CHECKS:${CORPUS_BY_TEST[*]} ${CORPUS_DESKS[*]}" "$CLI_SUITE:${CLI_BY_TEST[*]}"; do
    dir=${pair%%:*}
    keys=$(package_keys "$dir")
    for module in $(sed 's/\..*//' <<< "$keys" | sort -u); do
      outside=$(grep -F -- "$module." <<< "$keys" | grep -v "^$module\." || true)
      if [[ -n $outside ]]; then
        echo "FAIL: '$module.' is part of '${outside%%$'\n'*}', outside $dir/$module.ply, so the module's run picks that test too" >&2
        failures=$((failures + 1))
      fi
    done
    for module in ${pair#*:}; do
      if [[ ! -f "$root/$dir/$module.ply" ]]; then
        echo "FAIL: $dir/$module.ply does not exist, and its tests are placed one at a time" >&2
        failures=$((failures + 1))
        continue
      fi
      names=$(corpus_test_names "$root/$dir/$module.ply")
      if [[ -n $(label_ids <<< "$names" | cut -f1 | sort | uniq -d) ]]; then
        echo "FAIL: two tests of $dir/$module.ply hash to one id, so one run would take both" >&2
        failures=$((failures + 1))
      fi
      while IFS= read -r name; do
        [[ -n $name ]] || continue
        if [[ $(grep -cF -- "$name" <<< "$names") -gt 1 ]]; then
          echo "FAIL: '$module.$name' is part of another test's name in $dir/$module.ply, so its filter picks both" >&2
          failures=$((failures + 1))
        fi
      done <<< "$names"
    done
  done
  # The library's runs likewise, by `std.<module>.` and by a project module's `<module>.`.
  keys=$(stdlib_keys)
  for module in $(stdlib_entries | sed 's/^[a-z]*://' | LC_ALL=C sort -u); do
    outside=$(awk -F"$TAB" -v m="$module" '$1 != m && index($2, m ".") { print $2; exit }' <<< "$keys")
    if [[ -n $outside ]]; then
      echo "FAIL: '$module.' is part of '$outside', a test or law of another of the library's runs, so the run of $module picks it too" >&2
      failures=$((failures + 1))
    fi
  done
  local file claimed
  while IFS= read -r file; do
    [[ -n $file ]] || continue
    claimed=0
    for entry in "${PACKAGE_PROOFS[@]}"; do [[ $file == "${entry#*:}"/* ]] && claimed=1; done
    if ((!claimed)); then
      echo "FAIL: $file states a law or contract, and no PACKAGE_PROOFS entry proves its package, so CI never discharges it" >&2
      failures=$((failures + 1))
    fi
  done < <(claiming_files)
  for entry in "${PACKAGE_PROOFS[@]}"; do
    if ! grep -q "^${entry#*:}/" < <(claiming_files); then
      echo "FAIL: PACKAGE_PROOFS names ${entry#*:}, which states no law or contract — delete the entry" >&2
      failures=$((failures + 1))
    fi
  done
  for entry in "${CLI_TREE_CHECKS[@]}"; do
    module=${entry%%:*}
    names=$(corpus_test_names "$root/$CLI_SUITE/$module.ply" 2>/dev/null || true)
    if ! grep -qxF -- "${entry#*:}" <<< "$names"; then
      echo "FAIL: $CLI_SUITE/$module.ply declares no test '${entry#*:}', and CLI_TREE_CHECKS names it" >&2
      failures=$((failures + 1))
    fi
  done
  # Every entry is a partition's, a desk runner's or alone, once, so the round robins stay total.
  local placed k
  placed=$(
    for ((k = 1; k <= PARTITIONS; k++)); do cmd_corpus_for_partition "$k" | cut -d' ' -f2; done
    for ((k = 1; k <= DESK_RUNNERS; k++)); do cmd_desks_for_runner "$k"; done
    printf '%s\n' "${CORPUS_ALONE[@]}"
  )
  if [[ $(sort <<< "$placed") != "$(sort <<< "$entries")" ]]; then
    echo "FAIL: the partitions', desk runners' and lone corpus runs are not every entry, each once" >&2
    failures=$((failures + 1))
  fi
  if ! grep -q 'ci-corpus\.sh' "$workflow"; then
    echo "FAIL: no job in $workflow runs ci-corpus.sh, so the partitions' corpus runs run nowhere" >&2
    failures=$((failures + 1))
  fi
  # Each command's runs must reach a job the \`ci\` job waits on, or they run nowhere that counts.
  for corpus_command in "ci-shards.sh corpus-matrix" "ci-corpus.sh partition" "ci-corpus.sh desks"; do
    corpus_job=$(job_running "$corpus_command")
    if [[ -z $corpus_job ]]; then
      echo "FAIL: no job in $workflow runs \`$corpus_command\`, so its corpus runs run nowhere" >&2
      failures=$((failures + 1))
    elif [[ " ${needs//[][,]/ } " != *" $corpus_job "* ]]; then
      echo "FAIL: job '$corpus_job' runs corpus tests, and is not in the \`ci\` job's needs list" >&2
      failures=$((failures + 1))
    fi
  done
  # A job that starts corpus runs ends them at a deadline that leaves its steps after them
  # RUNS_MARGIN minutes of its limit: a job the limit cancels keeps nothing its runs wrote.
  local limit budget
  for corpus_command in "ci-corpus.sh partition" "ci-corpus.sh desks"; do
    corpus_job=$(job_running "$corpus_command")
    [[ -n $corpus_job ]] || continue
    limit=$(job_block "$corpus_job" | sed -n 's/^    timeout-minutes: *\([0-9][0-9]*\)$/\1/p')
    budget=$(deadline_of "$corpus_job")
    if [[ -z $budget ]]; then
      echo "FAIL: job '$corpus_job' runs \`$corpus_command\` and sets no PLY_CI_DEADLINE, so its limit cancels it with nothing kept" >&2
      failures=$((failures + 1))
    elif [[ -z $limit ]] || ((budget > limit - RUNS_MARGIN)); then
      echo "FAIL: job '$corpus_job' ends its runs ${budget} minutes in, which leaves less than $RUNS_MARGIN of its ${limit:-unset} for the steps that keep what they wrote" >&2
      failures=$((failures + 1))
    fi
    # The cut parts a `ply test` the partitions take that outlasts the job's limit (`run_limit`); a run
    # it cannot part is too long for the cut to balance the lanes, and cold it can outlast one.
    if [[ -s $TIMINGS ]]; then
      while read -r id seconds; do
        echo "FAIL: corpus run $id took ${seconds}s in the last table, over a third of job '$corpus_job''s ${budget}-minute deadline, and the cut cannot part it: $(unparted "$id")" >&2
        failures=$((failures + 1))
      done < <(overlong "$TIMINGS" "$corpus_command")
    fi
  done

  # --- cache keys -----------------------------------------------------------
  cmd_cache_keys || failures=$((failures + 1))
  cmd_cache_payloads || failures=$((failures + 1))

  if [[ $failures -gt 0 ]]; then
    echo "$failures problem(s) in the CI tables" >&2
    return 1
  fi
  local cut="by test count, with nothing measured"
  [[ -s $TIMINGS ]] && cut="from $(grep -c . "$TIMINGS") measured durations"
  echo "${#all_members[@]} members under crates/ (plus $(members_outside_crates | grep -c . || true) outside); ${#KNOWN_OUTSIDE[@]} crate(s) deliberately outside; $((${#TREE_CHECKS[@]} + ${#GATES_ALONE[@]})) tree checks and ${#CLI_TREE_CHECKS[@]} in the CLI's suite, each present in the tree; ${#PROBE_JOBS[@]} probe(s) run by a required CI job; $(corpus_entries | grep -c .) corpus test runs in $PARTITIONS partitions; $NEXTEST_SHARDS nextest shards cut $cut"
}

# The sweep's cases for a changed path, as a `--filter`: an example, a module of the compiler's
# package, or one of the standard library; nothing for any other path.
cmd_sweep_matrix() {
  local event=$1
  if [[ $event == schedule || $event == workflow_dispatch ]]; then
    jq -cn --argjson n "$SWEEP_SHARDS" --argjson days "$SWEEP_DAYS" --argjson day "$(($(date -u +%s) / 86400))" \
      '($n * $days) as $of | {include: [range(1; $n + 1) | (($day % $days) * $n + .) as $k
        | {name: "\($k)/\($of)", shard: "\($k)/\($of)", filter: ""}]}'
  else
    echo '{"include":[]}'
  fi
}

case "${1:-}" in
  verify) cmd_verify ;;
  cache-keys) cmd_cache_keys ;;
  partitions) cmd_partitions ;;
  nextest-shards) cmd_nextest_shards ;;
  shard-configs) cmd_shard_configs "${2:-}" ;;
  durations) cmd_durations "${2:?a nextest JUnit report}" ;;
  timings) cmd_timings "${2:?the table this run was cut by, which need not exist}" ;;
  corpus-matrix) cmd_corpus_matrix ;;
  corpus-for-partition) cmd_corpus_for_partition "${2:?a partition}" "${3:-}" ;;
  desks-for-runner) cmd_desks_for_runner "${2:?a desk runner}" "${3:-}" ;;
  corpus-line) cmd_corpus_line "${2:?a corpus entry}" "${@:3}" ;;
  exclude-filter) cmd_exclude_filter ;;
  gate-filter) cmd_gate_filter ;;
  rust-answered) cmd_rust_answered "${2:-}" "${3:-}" "${4:-}" ;;
  rust-kept) cmd_rust_kept "${2:-}" "${3:-}" "${4:-}" ;;
  host-filter) cmd_host_filter ;;
  tree-checks) cmd_tree_checks ;;
  tree-check-filter) cmd_tree_check_filter ;;
  rust-inputs) cmd_rust_inputs "${2:-}" ;;
  fetch-timings) cmd_fetch_timings ;;
  supersede) cmd_supersede "${2:?a run id}" "${3:?a ref}" ;;
  sweep-matrix) cmd_sweep_matrix "${2:?an event}" ;;
  *)
    echo "usage: ci-shards.sh {verify|cache-keys|fetch-timings|partitions|nextest-shards|shard-configs DIR|durations FILE|timings BEFORE|corpus-matrix|corpus-for-partition K [DIR]|desks-for-runner K [DIR]|corpus-line ID...|exclude-filter|gate-filter|host-filter|tree-checks|tree-check-filter|rust-inputs DEPINFO|supersede RUN REF}" >&2
    exit 2
    ;;
esac
