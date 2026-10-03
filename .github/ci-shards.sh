#!/usr/bin/env bash
# The tables CI's test jobs are cut from, the check that the cut is total, and the caches a run
# parks for its own jobs, handed back when it is green.
#
#   ci-shards.sh verify          every crate is a member, every test named here
#                                exists, every `probes/` directory is run by a
#                                job the `ci` aggregate requires, the shards run
#                                every test exactly once, every cache key a job
#                                writes is one a job reads, every key that
#                                names the run is one the run gives back or a
#                                later run reads, and every key over a crate's
#                                Ply sources names every crate's
#   ci-shards.sh cache-keys      just that last check
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
#   ci-shards.sh corpus-line ID  the package one run tests and its filter
#   ci-shards.sh exclude-filter  the filterset a partition leaves to the gates
#                                job: the host packages
#   ci-shards.sh gate-filter     the filterset the gates job runs: the tree checks
#   ci-shards.sh host-filter     the filterset selecting the host packages
#   ci-shards.sh tree-checks     one `package target test` line per tree check
#   ci-shards.sh give-back RUN   delete the entries this run parked for its own
#                                jobs, once every job that reads them is done
#   ci-shards.sh supersede RUN REF
#                                delete the entries of REF that this run's replaced

set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

# Jobs of corpus lanes, and jobs of nextest. Apart, so a lane never shares a runner with
# nextest's threads.
PARTITIONS=8
NEXTEST_SHARDS=2

# What the last run whose test jobs all passed measured, restored from the cache by the `plan` job.
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
CLI_SUITE=crates/ply-cli-tests/ply
# The checks that start desks under load and drive them over postgres: a test at a time, cut by
# duration over `DESK_RUNNERS` runners beside a postgres each, the `corpus` job's `desks-<k>`.
CORPUS_DESKS=(serving database)
DESK_RUNNERS=3
# Runs that take a runner each: the compiler's own tests, compiled, take every core.
CORPUS_ALONE=(cli-compiler_compiled)
# Modules the cut may split, a lane taking a run of neighbouring tests (`corpus_cut`): whole, each
# would outlast a lane.
CORPUS_BY_TEST=(audit generated toolchain)
CLI_BY_TEST=(artifact_program bootstrap_archive corpus desk_operations incremental)
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

# Checks on the tree in the CLI's suite, as `module:test`. Each runs with its module's entry, so the
# table asserts it is still declared there: a check that stops being declared reports nothing.
CLI_TREE_CHECKS=(
  "fixture_list:every fixture is listed"
  "fmt:the maintained sources are committed formatted"
  "tree:the harness is the only module that starts the \`ply\` binary"
)

# `probes/` directories no cargo build reaches, as `dir:job`; the job must be in `ci`'s `needs`.
declare -a PROBE_JOBS=(
  "ucontext:plan"
)

# What a run parks for its own jobs, as the literal ci.yml writes before `${{ github.run_id }}`:
# the archive every partition unpacks, the emitter's stage, and the shard cut. No later run can
# name one, so a green run gives them back, and the repository's 10 GB cache stays for what does
# outlive a run: the stage under `ply-c-stage-sources-`, the kept C, the stores and the passes.
GIVE_BACK=(nextest-archive- ply-c-stage-emitter- test-shards-)

# `<family>-<run id>` entries only the newest of which is ever restored.
SUPERSEDED=(ply-upstream- ply-stores- ply-c-lanes- ply-c-nextest- test-timings-)

# `<family>-<digest>` entries keyed by what they hold: a run restores the newest one a `restore-keys`
# prefix matches, so an older one only holds the repository's 10 GB against what a run does read.
NEWEST=(ply-c-stage-sources- ply-c-corpus-)

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

# One entry id a line: `program`, `fixture-<name>` per fixture, `package-<id>` per package suite,
# every checks module that declares a test, then every such module of the CLI's suite under `cli-`;
# each module as `module`, or, for one placed a test at a time, `module:<id>` per test, the id a hash
# of its label, so a duration measured for a test stays with it however the module's tests move.
corpus_entries() {
  local entry file
  printf 'program\n'
  for file in "$root/$CORPUS_FIXTURES"/*.ply; do printf 'fixture-%s\n' "$(basename "$file" .ply)"; done
  for entry in "${PACKAGE_SUITES[@]}"; do printf 'package-%s\n' "${entry%%:*}"; done
  module_entries "$CORPUS_CHECKS" "" "${CORPUS_BY_TEST[@]}" "${CORPUS_DESKS[@]}"
  module_entries "$CLI_SUITE" cli- "${CLI_BY_TEST[@]}"
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

# `path filter`: the program's entry takes every test of its package, a module's its own, and
# `module:<id>` the test whose label hashes to it, by the qualified name `ply test --filter` matches.
# A module the cut places a test at a time is a run too, of every test it declares.
cmd_corpus_line() {
  local entry entries
  # Read whole before the loop can return, so the lister never writes into a closed pipe.
  entries=$(corpus_entries)
  while read -r entry; do
    [[ $entry == "$1" || ${entry%%:*} == "$1" ]] || continue
    if [[ $1 == program ]]; then
      printf '%s\n' "$CORPUS_PROGRAM"
    elif [[ $1 == fixture-* ]]; then
      printf '%s/%s.ply\n' "$CORPUS_FIXTURES" "${1#fixture-}"
    elif [[ $1 == package-* ]]; then
      package_path "${1#package-}"
    elif [[ $1 == cli-* ]]; then
      module_line "$CLI_SUITE" "${1#cli-}"
    else
      module_line "$CORPUS_CHECKS" "$1"
    fi
    return 0
  done <<< "$entries"
  echo "no corpus entry named '$1'" >&2
  return 1
}

# `path filter` of the run `module` or `module:<id>` of the package at DIR.
module_line() {
  local dir=$1 module=${2%%:*} name
  if [[ $2 == *:* ]]; then
    name=$(corpus_test_names "$root/$dir/$module.ply" | label_ids | awk -F"$TAB" -v id="${2##*:}" '$1 == id { print $2; exit }')
    [[ -n $name ]] || { echo "no test of $dir/$module.ply has the id '${2##*:}'" >&2; return 1; }
    printf '%s %s.%s\n' "$dir" "$module" "$name"
  else
    printf '%s %s.\n' "$dir" "$module"
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

cmd_exclude_filter() { cmd_host_filter; }

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
# run's costs, a cold run piles what the cache had saved onto one partition. `cached` rows only say
# which run this was.
cmd_timings() {
  local before=$1
  awk -F"$TAB" -v OFS="$TAB" -v before="$before" '
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
        print k, ((warm && (f[1] == "corpus" || f[1] == "startup") && (k in kept)) ? kept[k] : ms[k])
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
  # A corpus row is the entry's own, and lives while the entry does; a startup row is a package's.
  dropped=$(printf '%s\n' "$built" | awk -F"$TAB" -v out="$2" -v placed="$({ corpus_placed; desk_placed; } | tr '\n' ' ')" '
    BEGIN { n = split(placed, ids, " "); for (i = 1; i <= n; i++) corpus[ids[i]] = 1; n = 0 }
    NR == FNR { live[$0] = 1; next }
    $1 == "corpus" { if ($2 in corpus) print > out; else n++; next }
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
# placed a test at a time go in as runs of neighbours, one run while the module fits a lane and as few
# as it needs once it does not, so a cold start its tests share is paid once a run rather than once a
# test. Runs and the other entries go longest first onto the lane of every partition's that would end
# soonest with it; the lanes are taken partition by partition, so the first placed land on different
# runners. A lane pays each package's startup once. An entry nothing measured counts as the median
# of those that were.
corpus_cut() {
  local dir=$1 rows=$2 k
  for ((k = 1; k <= PARTITIONS; k++)); do : > "$dir/corpus-$k.txt"; done
  corpus_placed | awk -v rows="$rows" -v dir="$dir" -v p="$PARTITIONS" -v l="$CORPUS_LANES" \
    -v checks="$CORPUS_CHECKS" -v cli="$CLI_SUITE" '
    function package(id) {
      if (id ~ /^cli-/) return cli
      if (id == "program" || id ~ /^package-/ || id ~ /^fixture-/) return id
      return checks
    }
    function module_of(id) { return index(id, ":") ? substr(id, 1, index(id, ":") - 1) : "" }
    # A new unit holding nothing yet, of the package `id` is tested in.
    function unit(id) { u++; size[u] = 0; cost[u] = 0; pkg[u] = package(id) }
    function hold(id) { member[u, ++size[u]] = id; cost[u] += ms[id] }
    BEGIN {
      FS = "\t"
      while ((getline line < rows) > 0) {
        split(line, f, "\t")
        if (f[1] == "startup") start[f[2]] = f[3] + 0
        else ms[f[2]] = f[3] + 0
      }
      FS = " "
    }
    { ids[++n] = $1 }
    END {
      for (i = 1; i <= n; i++) if (ids[i] in ms) measured[++m] = ms[ids[i]]
      for (i = 2; i <= m; i++) {
        x = measured[i]
        for (j = i - 1; j >= 1 && measured[j] > x; j--) measured[j + 1] = measured[j]
        measured[j + 1] = x
      }
      median = m ? measured[int((m + 1) / 2)] : 60000
      for (i = 1; i <= n; i++) {
        if (!(ids[i] in ms)) ms[ids[i]] = median
        total += ms[ids[i]]
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
      # About two minutes unmeasured; a program or package suite row holds its startup whole.
      start[checks] = (checks in start) ? start[checks] : 120000
      start[cli] = (cli in start) ? start[cli] : 120000
      for (i = 1; i <= u; i++) {
        x = order[i]
        best = 0
        for (j = 1; j <= lanes; j++) {
          end = load[j] + cost[x] + ((j SUBSEP pkg[x]) in loads ? 0 : start[pkg[x]])
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
# compared: it is the part a restore can match on, and the part both sides spell out.
cmd_cache_keys() {
  local files=() file
  while IFS= read -r file; do files+=("$file"); done < <(ci_files)
  awk -v give_back="${GIVE_BACK[*]}" -v families="${SUPERSEDED[*]} ${NEWEST[*]}" '
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
      nlit = 0; steppath = ""; inpath = 0
    }
    FNR == 1 { flush(); mode = ""; inkeys = 0; indent = 0 }
    # A new list item is a new step; the rules below read the one they are in.
    /^[[:space:]]*-[[:space:]]/ { flush(); mode = ""; inkeys = 0 }
    /uses:[[:space:]]*actions\/cache\/save@/ { mode = "save"; inkeys = 0; stepwhere = FILENAME ":" FNR; next }
    /uses:[[:space:]]*actions\/cache\/restore@/ { mode = "restore"; inkeys = 0; stepwhere = FILENAME ":" FNR; next }
    mode == "" { next }
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
      # A key that names the run carries the work of this run to the jobs of this run, and no later
      # run can name it: the run has to give it back, unless a `restore-keys` entry matches it.
      ng = split(give_back, gk, " ")
      for (i = 1; i <= n; i++) {
        if (!(order[i] in run_scoped)) continue
        ok = 0
        for (j = 1; j <= ng; j++) if (gk[j] != "" && index(order[i], gk[j]) == 1) { ok = 1; break }
        if (!ok) for (r in late) if (index(order[i], r) == 1) { ok = 1; break }
        if (!ok) {
          printf "FAIL: %s writes run-scoped cache key \"%s\", which no later run reads and GIVE_BACK does not name\n", where[order[i]], order[i] > "/dev/stderr"
          bad = 1
        }
      }
      # And the other way: an entry that names no key is a delete that quietly stops matching.
      for (j = 1; j <= ng; j++) {
        if (gk[j] == "") continue
        ok = 0
        for (k in saved) if (index(k, gk[j]) == 1) { ok = 1; break }
        if (!ok) {
          printf "FAIL: GIVE_BACK names \"%s\", which no save writes\n", gk[j] > "/dev/stderr"
          bad = 1
        }
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
      printf "cache keys: %d written and %d restored, each side matched by the other; %d run-scoped, each read later or given back\n", n, length(read), length(run_scoped)
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

# Deletes this run's entries under the keys above. A GitHub key is immutable, so an entry no later
# run reads holds the repository's cache budget against the caches that do outlive a run.
cmd_give_back() {
  local run=${1:?usage: ci-shards.sh give-back RUN_ID} key listing size id
  for key in ${GIVE_BACK[@]+"${GIVE_BACK[@]}"}; do
    listing=$(gh api "repos/$GITHUB_REPOSITORY/actions/caches?key=$key$run" \
      -q '.actions_caches[] | "\(.size_in_bytes) \(.id)"')
    while read -r size id; do
      [[ -n $id ]] || continue
      gh api -X DELETE "repos/$GITHUB_REPOSITORY/actions/caches/$id"
      echo "gave back $key$run, $((size / 1000000)) MB"
    done <<< "$listing"
  done
}

# A family only this run's entry replaces, so a job that wrote nothing keeps what it had; of a family
# keyed by content, the newest entry on the ref; and what an earlier run on the ref parked for its own
# jobs, which a cancelled run never gave back. A ref runs one run at a time. On main, every entry of
# a pull request that is closed, which no run reads again.
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
  for prefix in "${GIVE_BACK[@]}"; do
    listing=$(gh api --paginate "repos/$GITHUB_REPOSITORY/actions/caches?key=$prefix&ref=$ref&per_page=100" \
      -q '.actions_caches[] | "\(.key) \(.id)"')
    while read -r key id; do
      [[ -n $id && $key != *-"$run" ]] || continue
      gh api -X DELETE "repos/$GITHUB_REPOSITORY/actions/caches/$id" > /dev/null && echo "gave back $key"
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
  if ! cmp -s "$tmp/corpus-cut" <(corpus_placed | LC_ALL=C sort); then
    echo "FAIL: the $what cut's corpus runs are not every run a partition takes, each once" >&2
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
      block=$(awk -v j="  $job:" '$0 == j {f = 1; next} f && /^  [a-z]/ {exit} f' "$workflow")
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
    corpus_job=$(awk -v c="${corpus_command//./\\.}" '
      /^  [a-z-]+:$/ { job = $1; sub(/:$/, "", job) }
      $0 ~ c { print job; exit }
    ' "$workflow")
    if [[ -z $corpus_job ]]; then
      echo "FAIL: no job in $workflow runs \`$corpus_command\`, so its corpus runs run nowhere" >&2
      failures=$((failures + 1))
    elif [[ " ${needs//[][,]/ } " != *" $corpus_job "* ]]; then
      echo "FAIL: job '$corpus_job' runs corpus tests, and is not in the \`ci\` job's needs list" >&2
      failures=$((failures + 1))
    fi
  done

  # --- cache keys -----------------------------------------------------------
  cmd_cache_keys || failures=$((failures + 1))
  cmd_cache_payloads || failures=$((failures + 1))
  # GIVE_BACK is a table until a job runs it, and a job that is not required can stop running with
  # nothing red about it.
  local give_back_job
  give_back_job=$(awk '
    /^  [a-z-]+:$/ { job = $1; sub(/:$/, "", job) }
    /ci-shards\.sh give-back/ { print job; exit }
  ' "$workflow")
  if [[ -z $give_back_job ]]; then
    echo "FAIL: GIVE_BACK names the caches a run gives back, and no job in $workflow runs \`ci-shards.sh give-back\`" >&2
    failures=$((failures + 1))
  elif [[ $give_back_job != ci && " ${needs//[][,]/ } " != *" $give_back_job "* ]]; then
    echo "FAIL: job '$give_back_job' gives this run's own caches back, and is neither \`ci\` nor in its needs list" >&2
    failures=$((failures + 1))
  fi

  if [[ $failures -gt 0 ]]; then
    echo "$failures problem(s) in the CI tables" >&2
    return 1
  fi
  local cut="by test count, with nothing measured"
  [[ -s $TIMINGS ]] && cut="from $(grep -c . "$TIMINGS") measured durations"
  echo "${#all_members[@]} members under crates/ (plus $(members_outside_crates | grep -c . || true) outside); ${#KNOWN_OUTSIDE[@]} crate(s) deliberately outside; ${#TREE_CHECKS[@]} tree checks and ${#CLI_TREE_CHECKS[@]} in the CLI's suite, each present in the tree; ${#PROBE_JOBS[@]} probe(s) run by a required CI job; $(corpus_entries | grep -c .) corpus test runs in $PARTITIONS partitions; $NEXTEST_SHARDS nextest shards cut $cut"
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
  corpus-line) cmd_corpus_line "${2:?a corpus entry}" ;;
  exclude-filter) cmd_exclude_filter ;;
  gate-filter) cmd_gate_filter ;;
  host-filter) cmd_host_filter ;;
  tree-checks) cmd_tree_checks ;;
  tree-check-filter) cmd_tree_check_filter ;;
  give-back) cmd_give_back "${2:?a run id}" ;;
  supersede) cmd_supersede "${2:?a run id}" "${3:?a ref}" ;;
  *)
    echo "usage: ci-shards.sh {verify|cache-keys|partitions|nextest-shards|shard-configs DIR|durations FILE|timings BEFORE|corpus-matrix|corpus-for-partition K [DIR]|desks-for-runner K [DIR]|corpus-line ID|exclude-filter|gate-filter|host-filter|tree-checks|tree-check-filter|give-back RUN|supersede RUN REF}" >&2
    exit 2
    ;;
esac
