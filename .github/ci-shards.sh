#!/usr/bin/env bash
# The tables CI's test jobs are cut from, and the check that the cut is total.
#
#   ci-shards.sh verify          every crate is a member, every test named here
#                                exists, every `probes/` directory is run by a
#                                job the `ci` aggregate requires, and the shards
#                                run every test exactly once
#   ci-shards.sh partitions      the JSON matrix of partitions
#   ci-shards.sh shard-configs D the nextest config each partition runs under,
#                                cut from the durations CI measured; 3 when
#                                there are none and the partitions fall back to
#                                slicing by test count
#   ci-shards.sh durations FILE  `binary_id test milliseconds` per test in a
#                                nextest JUnit report
#   ci-shards.sh solo-matrix     the JSON matrix of tests that run alone
#   ci-shards.sh solo-filter ID  the nextest filterset selecting one solo test
#   ci-shards.sh exclude-filter  the filterset a partition leaves to the other
#                                jobs: the solo tests, the shutdown suite and
#                                the postgres packages
#   ci-shards.sh gate-filter     the filterset the gates job runs: the shutdown
#                                suite and the tree checks
#   ci-shards.sh postgres-filter the filterset selecting the postgres packages
#   ci-shards.sh tree-checks     one `package target test` line per tree check

set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

PARTITIONS=8

# What the partitions of the last run measured, restored from the cache by the `plan` job.
TIMINGS=/tmp/ply-test-timings/timings.tsv

TAB=$'\t'

# Tests that get a runner of their own, as `id:package:target:test`.
SOLO=(
  "bootstrap:ply-codegen-tests:bootstrap:the_bootstrap_bundle_is_a_fixpoint_of_the_emitter_it_builds"
  "cli-program:ply-cli-tests:suite:artifact_program::the_committed_program_is_what_these_sources_build"
  "compiler-on-the-tier:ply-cli-tests:suite:corpus::the_compiled_tier_runs_the_compilers_own_tests_as_the_only_engine"
  "archive-round-trip:ply-cli-tests:suite:bootstrap_archive::an_archive_is_written_and_verifies_against_the_tree_it_came_from"
  "archive-tree-moved:ply-cli-tests:suite:bootstrap_archive::an_archive_stops_describing_a_tree_that_moved"
  "corpus-session:ply-cli-tests:suite:incremental::the_example_corpus_agrees_across_a_session"
  "corpus-session-audit:ply-cli-tests:suite:incremental_audit::a_long_session_over_the_example_corpus_agrees_at_every_step"
)

# Their tests skip, passing, without a postgres server; only `test-postgres` runs them.
POSTGRES_PACKAGES=(ply-host-tests)

# `-tests` packages with no same-named crate: the CLI's suite drives ply-launcher's binary, and
# ply-cli is the program's sources, not a crate.
UNPAIRED_TESTS=(ply-cli-tests)

# `#![cfg(unix)]`, so the gates job asserts it ran.
W5_FILTER='binary_id(=ply-cli-tests::suite) & test(/^w5_shutdown::/)'

# Crate directories that are deliberately not workspace members, as `name:why`.
# Expanded as ${KNOWN_OUTSIDE[@]+...}: bash 3.2 treats an empty array as unset under `set -u`.
declare -a KNOWN_OUTSIDE=(
)

# Checks on the tree, as `package:target:test` (`target` is `lib` for a unit test, named by full
# module path). The gates job asserts each ran, since a check that stops running reports nothing.
TREE_CHECKS=(
  "ply-span-tests:armed:every_registered_code_is_constructed_in_production"
  "ply-span-tests:armed:every_variant_of_a_covered_enum_is_constructed_in_production"
  "ply-span-tests:armed:every_diagnostic_constructor_call_names_its_code_literally"
  "ply-span-tests:armed:the_code_registry_table_is_total_over_the_codes_module"
  "ply-span-tests:armed:no_allowlist_entry_has_outlived_its_reason"
  "ply-span-tests:armed:ambiguous_enum_names_are_declared"
  "ply-cli-tests:suite:fmt::the_maintained_sources_are_committed_formatted"
)

# `probes/` directories no cargo build reaches, as `dir:job`; the job must be in `ci`'s `needs`.
declare -a PROBE_JOBS=(
  "ucontext:ucontext-probe"
)

# The path of the file a `package target test` triple names, for tests in `tests/`.
test_source_file() {
  local package=$1 target=$2 test=$3 dir modpath
  dir="$root/crates/$package/tests"
  if [[ -f "$dir/$target.rs" ]]; then
    printf '%s\n' "$dir/$target.rs"
  elif [[ $test == *::* ]]; then
    modpath=${test%::*}
    printf '%s\n' "$dir/$target/${modpath//:://}.rs"
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

# `id package target test` per solo test.
cmd_solo() {
  local entry rest
  for entry in "${SOLO[@]}"; do
    rest=${entry#*:}
    printf '%s ' "${entry%%:*}"
    triples "$rest"
  done
}

# `(binary_id(=..) & test(=..)) | ...` over `package target test` lines on stdin.
filter_of() {
  local package target test first=1
  while read -r package target test; do
    ((first)) || printf ' | '
    first=0
    printf '(binary_id(=%s) & test(=%s))' "$(binary_id "$package" "$target")" "$test"
  done
}

cmd_solo_filter() {
  local id package target test
  while read -r id package target test; do
    if [[ $id == "$1" ]]; then
      printf '%s\n' "$(printf '%s %s %s\n' "$package" "$target" "$test" | filter_of)"
      return 0
    fi
  done < <(cmd_solo)
  echo "no solo test named '$1'" >&2
  return 1
}

cmd_postgres_filter() {
  local package first=1
  for package in "${POSTGRES_PACKAGES[@]}"; do
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

cmd_gate_filter() {
  printf '%s | %s\n' "$W5_FILTER" "$(cmd_tree_check_filter)"
}

# Solo tests are excluded by name, so a new test in one of their binaries still runs in a partition.
cmd_exclude_filter() {
  printf '%s | %s | %s\n' "$(cmd_solo | cut -d' ' -f2- | filter_of)" "$W5_FILTER" "$(cmd_postgres_filter)"
}

cmd_partitions() {
  local i
  printf '{"include":['
  for ((i = 1; i <= PARTITIONS; i++)); do
    ((i > 1)) && printf ','
    printf '{"shard":"%d","of":"%d"}' "$i" "$PARTITIONS"
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

# `t shard binary test ms` per timed test, longest first onto the least loaded shard, then
# `load shard ms tests` per shard and `catchall shard`: the shard with the most room left, which
# is the one that runs what no other shard names.
assign() {
  LC_ALL=C sort -t"$TAB" -k3,3nr -k1,1 -k2,2 "$1" |
    awk -F"$TAB" -v n="$PARTITIONS" '
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

# One `shard-<i>.toml` per partition: the tests it runs as its profile's `default-filter`, and the
# order to start them in. 3 when there is nothing measured to cut, so the caller slices by count.
# The rows of $1 whose binary and test the tree still has, on $2. A rename or a deletion leaves
# stale rows in the cached table, and a filter naming a binary id nothing builds is a nextest
# error; what the table does not name, the catch-all shard runs.
living_durations() {
  local id test ms package target file dropped=0
  : > "$2"
  while IFS="$TAB" read -r id test ms; do
    case "$id" in
      *::*) package=${id%%::*}; target=${id#*::} ;;
      *) package=$id; target=lib ;;
    esac
    if [[ $target == lib ]]; then
      [[ -d $root/crates/$package/src ]] || { dropped=$((dropped + 1)); continue; }
    elif [[ $target == bin/* ]]; then
      name=${target#bin/}
      [[ -f $root/crates/$package/src/main.rs || -f $root/crates/$package/src/bin/$name.rs || -f $root/crates/$package/src/bin/$name/main.rs ]] \
        || grep -q "name = \"$name\"" "$root/crates/$package/Cargo.toml" 2>/dev/null \
        || { dropped=$((dropped + 1)); continue; }
    else
      file=$(test_source_file "$package" "$target" "$test")
      [[ -f $file ]] || { dropped=$((dropped + 1)); continue; }
    fi
    printf '%s\t%s\t%s\n' "$id" "$test" "$ms" >> "$2"
  done < "$1"
  [[ $dropped -eq 0 ]]     || echo "$dropped measured row(s) name tests the tree no longer has; left to the catch-all" >&2
}

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
  living_durations "$timings" "$tmp/living"
  if [[ ! -s $tmp/living ]]; then
    echo "no measured durations name a test the tree still has" >&2
    rm -rf "$tmp"
    return 3
  fi
  timings=$tmp/living
  assign "$timings" > "$tmp/assigned"
  catchall=$(awk -F"$TAB" '$1 == "catchall" { print $2 }' "$tmp/assigned")
  if awk -F"$TAB" '$1 == "load" && $4 == 0 { bare = 1 } END { exit !bare }' "$tmp/assigned"; then
    echo "$(grep -c . "$timings") measured tests do not fill $PARTITIONS partitions" >&2
    rm -rf "$tmp"
    return 3
  fi
  mkdir -p "$dir"
  for ((i = 1; i <= PARTITIONS; i++)); do
    mkdir -p "$tmp/order.$i"
    awk -F"$TAB" -v s="$i" '$1 == "t" && $2 == s { printf "%s\t%s\t%s\n", $3, $4, $5 }' \
      "$tmp/assigned" > "$tmp/held.$i"
    cut -f1,2 "$tmp/held.$i" | grouped_filter > "$tmp/filter.$i"
  done
  {
    printf 'not ('
    first=1
    for ((i = 1; i <= PARTITIONS; i++)); do
      [[ $i -eq $catchall ]] && continue
      ((first)) || printf ' | '
      first=0
      printf '%s' "$(cat "$tmp/filter.$i")"
    done
    printf ')'
  } > "$tmp/negation"
  mv "$tmp/negation" "$tmp/filter.$catchall"
  for ((i = 1; i <= PARTITIONS; i++)); do
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
  rm -rf "$tmp"
}

cmd_shard_configs() { shard_configs "${1:?a directory to write the configs to}" "$TIMINGS"; }

cmd_solo_matrix() {
  local id package target test first=1
  printf '{"include":['
  while read -r id package target test; do
    ((first)) || printf ','
    first=0
    printf '{"id":"%s"}' "$id"
  done < <(cmd_solo)
  printf ']}\n'
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
    # A measured table too small to fill the partitions is the fallback, not a failure.
    if [[ $rc -eq 3 && $what == measured ]]; then
      return 0
    fi
    echo "FAIL: the $what durations cut no shards, so nothing here is checked" >&2
    return 1
  fi
  catchall=
  seen=0
  for ((i = 1; i <= PARTITIONS; i++)); do
    if [[ ! -f "$dir/shard-$i.toml" ]]; then
      echo "FAIL: the $what durations cut no shard $i, and a partition job runs one" >&2
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
    if ! printf '%s\n' "${all_members[@]}" | grep -qx "${member%-tests}" \
      && ! printf '%s\n' "${UNPAIRED_TESTS[@]}" | grep -qx "$member"; then
      echo "FAIL: '$member' is a member and '${member%-tests}' is not, so its tests run without that crate's binaries beside them" >&2
      failures=$((failures + 1))
    fi
  done
  for package in "${POSTGRES_PACKAGES[@]}"; do
    if ! printf '%s\n' "${all_members[@]}" | grep -qx "$package"; then
      echo "FAIL: POSTGRES_PACKAGES names '$package', which is not a workspace member" >&2
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
  while read -r id package target test; do
    check_test_exists "solo test '$id'" "$package" "$target" "$test" || failures=$((failures + 1))
    seen=0
    for entry in "${SOLO[@]}"; do
      [[ ${entry%%:*} == "$id" ]] && seen=$((seen + 1))
    done
    if [[ $seen -gt 1 ]]; then
      echo "FAIL: SOLO names '$id' $seen times" >&2
      failures=$((failures + 1))
    fi
  done < <(cmd_solo)
  if [[ ! -f $(test_source_file ply-cli-tests suite w5_shutdown::x) ]]; then
    echo "FAIL: W5_FILTER names crates/ply-cli-tests/tests/suite/w5_shutdown.rs, which does not exist" >&2
    failures=$((failures + 1))
  fi
  # Cargo builds `ply` for ply-cli-tests only if ply-launcher has an integration test of its own.
  if ! ls "$root"/crates/ply-launcher/tests/*.rs >/dev/null 2>&1; then
    echo "FAIL: crates/ply-launcher/tests/ has no .rs file, so cargo builds no 'ply' for ply-cli-tests' suite to run" >&2
    failures=$((failures + 1))
  fi

  # --- the shards the durations cut -----------------------------------------
  local made_up made_up_test
  if [[ $PARTITIONS -lt 2 ]]; then
    echo "FAIL: PARTITIONS is $PARTITIONS, and one shard is the negation of the others" >&2
    failures=$((failures + 1))
  else
    made_up=$(mktemp -d)
    # Real tests with invented costs: the cut drops durations for tests the tree no longer has,
    # so a table it can check has to name ones it has.
    made_up_count=0
    for made_up_file in "$root"/crates/ply-cli-tests/tests/suite/*.rs; do
      made_up_mod=${made_up_file##*/}; made_up_mod=${made_up_mod%.rs}
      while read -r made_up_fn; do
        made_up_count=$((made_up_count + 1))
        printf 'ply-cli-tests::suite\t%s::%s\t%d\n' \
          "$made_up_mod" "$made_up_fn" $((made_up_count * 37 + 1)) >> "$made_up/timings.tsv"
        [[ $made_up_count -ge $((PARTITIONS + 4)) ]] && break 2
      done < <(sed -n 's/^fn \([a-z0-9_]*\)(.*/\1/p' "$made_up_file")
    done
    check_shards made-up "$made_up/timings.tsv" || failures=$((failures + 1))
    rm -rf "$made_up"
    if [[ -s $TIMINGS ]]; then
      check_shards measured "$TIMINGS" || failures=$((failures + 1))
    fi
  fi

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
    # Whole word, so `probe` does not match `ucontext-probe`.
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

  if [[ $failures -gt 0 ]]; then
    echo "$failures problem(s) in the CI tables" >&2
    return 1
  fi
  local cut="by test count, with nothing measured"
  [[ -s $TIMINGS ]] && cut="from $(grep -c . "$TIMINGS") measured durations"
  echo "${#all_members[@]} members under crates/ (plus $(members_outside_crates | grep -c . || true) outside); ${#KNOWN_OUTSIDE[@]} crate(s) deliberately outside; ${#TREE_CHECKS[@]} tree checks and ${#SOLO[@]} solo tests, each present in the tree; ${#PROBE_JOBS[@]} probe(s) run by a required CI job; $PARTITIONS partitions cut $cut"
}

case "${1:-}" in
  verify) cmd_verify ;;
  partitions) cmd_partitions ;;
  shard-configs) cmd_shard_configs "${2:-}" ;;
  durations) cmd_durations "${2:?a nextest JUnit report}" ;;
  solo-matrix) cmd_solo_matrix ;;
  solo-filter) cmd_solo_filter "${2:?a solo id}" ;;
  exclude-filter) cmd_exclude_filter ;;
  gate-filter) cmd_gate_filter ;;
  postgres-filter) cmd_postgres_filter ;;
  tree-checks) cmd_tree_checks ;;
  tree-check-filter) cmd_tree_check_filter ;;
  *)
    echo "usage: ci-shards.sh {verify|partitions|shard-configs DIR|durations FILE|solo-matrix|solo-filter ID|exclude-filter|gate-filter|postgres-filter|tree-checks|tree-check-filter}" >&2
    exit 2
    ;;
esac
