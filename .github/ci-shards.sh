#!/usr/bin/env bash
#
# The tables CI's test jobs are cut from, and the check that the cut is total.
#
# The suite is built once, as a nextest archive of every workspace member, and
# run by many short jobs at once: a fixed number of partitions that each take a
# slice of the tests, one job per test that has to run alone on a runner of its
# own, one for the tests that read a wall clock, one for the postgres suites,
# and one for the scripts that drive a `ply` binary. A job's wall clock is the
# archive's build plus the slowest test in it, so the tables here are cut on run
# time, and the archive is what makes "every member is tested" a property of
# `cargo nextest archive --workspace` rather than of a table.
#
# What a table can still get wrong is losing a *test* silently: a test named
# here that nothing defines selects nothing and says nothing. `verify` fails on
# that, on a crate no member reaches, on `.config/nextest.toml` disagreeing with
# the deferred table, and on a probe with no required job.
#
#   ci-shards.sh verify          every crate is a member or listed as not one,
#                                every test named here exists where the table
#                                says, `.config/nextest.toml` names exactly the
#                                deferred tests, and every directory under
#                                `probes/` is run by a named CI job that the
#                                `ci` aggregate requires
#   ci-shards.sh partitions      the JSON matrix of partition slices
#   ci-shards.sh solo-matrix     the JSON matrix of tests that run alone
#   ci-shards.sh solo-filter ID  the nextest filterset selecting one solo test
#   ci-shards.sh exclude-filter  the filterset a partition leaves to the other
#                                jobs: the solo tests, the deferred tests, the
#                                shutdown suite and the postgres packages
#   ci-shards.sh gate-filter     the filterset the gates job runs: the deferred
#                                tests, the shutdown suite and the tree checks
#   ci-shards.sh postgres-filter the filterset selecting the postgres packages
#   ci-shards.sh deferred        one `package target test` line per deferred test
#   ci-shards.sh deferred-filter the nextest filterset selecting exactly the
#                                deferred tests; `.config/nextest.toml` carries
#                                it verbatim and `verify` checks that it does
#   ci-shards.sh tree-checks     one `package target test` line per tree check

set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

# How many jobs the partitioned tests are cut across. nextest's `slice:m/n`
# deals tests round-robin after the filters, so tests of one binary spread
# across partitions rather than landing in one. Raise it when the slowest
# partition's run outlasts its slowest test by much; lower it when the
# per-job overhead -- checkout, a C compiler, the archive -- is most of a leg,
# or when the jobs after `build` outnumber what the account runs at once and
# queue: fourteen partitions queued for longer than the four fewer saved.
PARTITIONS=10

# Tests that get a runner of their own, as `id:package:target:test`. Each is
# the longest thing in its binary and, by `.config/nextest.toml`, runs with
# every test thread and nothing beside it -- so inside one job they would run
# one after another, and the job would be their sum. On separate runners each
# is its own wall clock. `verify` fails when a name here is not defined where
# the table says, and every solo job asserts that it ran exactly one test.
#
# A test of one of these binaries that is *not* named here still runs, in a
# partition, alone within it: the override in `.config/nextest.toml` is on the
# binary. Naming it here only moves it to a runner of its own.
SOLO=(
  "bootstrap:ply-codegen-tests:bootstrap:the_bootstrap_bundle_is_a_fixpoint_of_the_emitter_it_builds"
  "emit-diff-own-sources:ply-compiler-diff:emit_diff:the_port_resolves_its_own_sources_to_the_references_c"
  "emit-diff-census:ply-compiler-diff:emit_diff:the_census_of_what_keeps_the_port_out"
  "emit-diff-corpus:ply-compiler-diff:emit_diff:the_port_resolves_to_the_references_c_over_the_shipped_corpus"
  "emit-diff-agreement:ply-compiler-diff:emit_diff:the_emitter_agrees_with_ply_codegen_wherever_the_port_reaches"
)

# The packages whose tests need a postgres server and cluster binaries. They
# skip -- passing -- without them, so the partitions leave them to the job that
# has both and asserts the gates are open.
POSTGRES_PACKAGES=(ply-host-tests)

# `crates/ply-cli-tests/tests/suite/w5_shutdown.rs` is `#![cfg(unix)]`: on any
# other host it compiles to nothing and prints nothing. The gates job runs it by
# name and asserts the log names it, which a partition could not do for a module
# dealt across ten of them.
W5_FILTER='binary_id(=ply-cli-tests::suite) & test(/^w5_shutdown::/)'

# Crate directories that are deliberately not workspace members, and why. A
# crate in neither this list nor `members` is an accident: nothing builds it and
# no job tests it, which is the failure this file exists to prevent.
# Expanded as ${KNOWN_OUTSIDE[@]+...} everywhere below: bash 3.2, which is
# /bin/bash on macOS, treats "${empty[@]}" as unset under `set -u`, and this
# list is meant to stay empty.
declare -a KNOWN_OUTSIDE=(
)

# Tests whose assertion reads a wall clock, as `package:target:test`, where
# `target` is an integration test binary or the literal `lib` for a unit test.
#
# Each passes or fails on how much CPU it was given rather than on what the code
# does, so beside other tests is the wrong place for them. nextest runs them
# last and alone: `.config/nextest.toml` gives exactly these tests every test
# thread and the lowest priority, so each one starts only when the rest of the
# job has finished and nothing else runs beside it. That file carries this
# table's filterset verbatim (`deferred-filter`), and `verify` fails when the
# two disagree, because a test that drops out of the override quietly goes back
# to running under contention. What they print is shown, since a measurement
# nobody can read is not a measurement. Names are matched exactly, so a unit
# test is named by its full module path. The gates job runs them and asserts
# that each appears in its log as run; the partitions leave them out.
#
# **This list is maintained by running the suite, not by surveying the tree.**
# Two surveys have been done and each declared itself complete; each was proved
# wrong by the next run, within the hour:
#
#   * A grep of `crates/*/tests` for `Instant::now` produced seven entries. The
#     corpus shard then failed on
#     `measure::every_resumption_costs_about_what_the_first_one_did` —
#     *"the fourth resumption cost 5680.8965 us against 2196.552 us"*, against
#     `four.marginal_micros < one.micros * 2.0` — a unit test, then in `src/`,
#     which that grep could not see. Re-surveying `crates/*/src` took the list to 12.
#   * The cli-eval shard then failed on
#     `routing_a_path_of_escapes_costs_its_length_and_not_its_square`
#     (`crates/ply-cli-tests/tests/suite/w3_http_audit.rs:714`) — *"four times the escapes
#     cost 1655.9ms against 143.6ms for k, which is 11.5x"*, against
#     `four <= one * 9.0`. The second survey missed it too: the test reads no
#     Rust clock at all, it parses milliseconds out of `ply test`'s own output
#     via a `duration_of` helper, so no timing vocabulary appears in it. Run
#     alone it passes three times out of three at load 20.
#
# The list is never finished. When a job goes red on a ratio or a budget, the fix is usually
# another row here — `payload::the_map_rows_survive_subtracting_the_fold_around_them` is
# the most recent, and it is the third survey's blind spot: it subtracts a scaffold from a
# measurement and asserts the remainder is positive, so contention does not slow it down, it
# makes the answer negative.
DEFERRED=(
  "ply-eval-tests:allocation:region_arena_cost::snapshot_cost_as_a_function_of_region_size"
  "ply-eval-tests:allocation:fixture_open_cost::a_seeded_fixture_opens_per_test_in_microseconds"
  "ply-cli-tests:suite:cli::a_simulated_sleep_is_a_jump_rather_than_a_wait"
  "ply-test-tests:suite:region_fixture_cost::a_region_scoped_fixture_costs_the_fixture_and_never_the_test"
  "ply-test-tests:suite:region_fixture_cost::discarding_a_tests_own_cells_costs_nothing"
  "ply-test-tests:suite:region_fixture_cost::a_group_amortizes_the_build_up_to_a_ceiling_the_open_decides"
  "ply-test-tests:suite:region_fixture_cost::a_group_with_no_fixture_opens_and_closes_in_constant_time"
  "ply-corpus-tests:unit:measure::every_resumption_costs_about_what_the_first_one_did"
  "ply-corpus-tests:unit:measure::capture_and_resume_are_flat_in_the_frames_they_move"
  "ply-corpus-tests:unit:measure::opening_a_fixture_beats_rebuilding_it_once_the_fixture_is_real"
  "ply-store-tests:unit:store::opening_a_ten_thousand_definition_cache_is_under_the_budget"
  "ply-store-tests:unit:store::a_baseline_for_every_test_does_not_slow_the_open"
  "ply-cli-tests:suite:w3_http_audit::routing_a_path_of_escapes_costs_its_length_and_not_its_square"
  "ply-corpus-tests:unit:payload::the_map_rows_survive_subtracting_the_fold_around_them"
)

# Tests that fail on a property of the *tree* rather than of a run, as
# `package:target:test`, with the same three-field spelling as `DEFERRED`.
#
# Not "gates" in the sense §"There is CI" uses that word — those are
# dependencies that make a suite skip silently. These are checks whose subject
# is the source tree itself.
#
# They are already inside the partitions' run of `ply-span-tests`. They are
# named here as well for one reason: a check that stops running reports
# nothing, and reporting nothing is indistinguishable from passing. `verify`
# fails when a name here is not defined in the file this table says, and the
# gates job runs each by exact name and asserts it actually ran — so renaming
# one, deleting it, or filtering it away turns CI red instead of quietly
# reducing what CI checks.
#
# All seven are in `crates/ply-span-tests/tests/armed.rs`. Six of them are one defect:
# a mechanism declared and registered everywhere a reader would look for it and
# constructed nowhere. CONTRIBUTING.md s"The shape it keeps taking: declared,
# registered, raised nowhere" has the catalogue; that file's header has the rule
# and the list of what it does not cover.
#
# The seventh, `no_two_adrs_share_a_number`, is a different kind and was added
# in ad74275: its subject is the `docs/adr/` filenames rather than a mechanism
# in the source. It sits here because the file it lives in is the tree-check
# file and the reason for naming it here is identical -- a check that stops
# running reports nothing. This comment said "all six" for two commits after it
# landed, which is the staleness this table exists to make expensive.
TREE_CHECKS=(
  "ply-span-tests:armed:every_registered_code_is_constructed_in_production"
  "ply-span-tests:armed:every_variant_of_a_covered_enum_is_constructed_in_production"
  "ply-span-tests:armed:every_diagnostic_constructor_call_names_its_code_literally"
  "ply-span-tests:armed:the_code_registry_table_is_total_over_the_codes_module"
  "ply-span-tests:armed:no_allowlist_entry_has_outlived_its_reason"
  "ply-span-tests:armed:ambiguous_enum_names_are_declared"
  "ply-span-tests:armed:no_two_adrs_share_a_number"
)

# Directories that hold code no cargo build reaches, and the CI job that runs each.
#
# `KNOWN_OUTSIDE` above exists because a crate outside the workspace is a crate nothing builds,
# and it happened twice here: `crates/ply-compiler` and `crates/ply-compiler-diff` each sat
# outside the cargo workspace with their own `[workspace]`, and the first was in **no CI job at
# all** while its `README.md` predicted in writing that it would bit-rot -- and it did. Four
# language features landed, its differential went red on 28 of 763 inputs (70.2% of the corpus
# by bytes) and nothing said so for two days. Both are members now, and this list is down to
# what is genuinely not Rust.
#
# Each entry is `dir:job`, and `verify` fails unless the job exists in `.github/workflows/ci.yml`
# **and** is named in the `ci` aggregate job's `needs:` list -- because a job that nothing needs is
# not required, and the `ci` job's own comment is what says a skipped job is not a green tick.
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
    printf '%s\n' "$dir/$target/${modpath//::/\/}.rs"
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

cmd_deferred() { triples "${DEFERRED[@]}"; }
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

cmd_deferred_filter() {
  cmd_deferred | filter_of
  printf '\n'
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

# What the gates job runs: the deferred tests, the shutdown suite, and the tree
# checks by name. The tree checks run in a partition as well; here they are
# asserted to have run.
cmd_gate_filter() {
  printf '%s | %s | %s\n' "$(cmd_deferred_filter)" "$W5_FILTER" "$(cmd_tree_check_filter)"
}

# What a partition leaves to the other jobs. The solo tests are excluded by
# name, so a test that joins one of those binaries later is still run -- in a
# partition, alone within it -- rather than lost.
cmd_exclude_filter() {
  printf '%s | %s | %s | %s\n' "$(cmd_solo | cut -d' ' -f2- | filter_of)" "$(cmd_deferred_filter)" "$W5_FILTER" "$(cmd_postgres_filter)"
}

cmd_partitions() {
  local i
  printf '{"include":['
  for ((i = 1; i <= PARTITIONS; i++)); do
    ((i > 1)) && printf ','
    printf '{"slice":"%d/%d"}' "$i" "$PARTITIONS"
  done
  printf ']}\n'
}

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

# Workspace members, read out of `Cargo.toml` as text. Read rather than asked of cargo so that a
# manifest cargo refuses to parse fails here too, with the manifest named.
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
  local -a all_members=()
  while read -r member; do all_members+=("$member"); done < <(members)

  # --- crates --------------------------------------------------------------
  #
  # A `-tests` package is tests only, compiled at `opt-level = 0` by an override
  # the root manifest has to carry; and every directory under `crates/` is a
  # member or is listed here as deliberately not one.
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

  # --- tests named by a table ----------------------------------------------
  while read -r package target test; do
    check_test_exists "deferred test" "$package" "$target" "$test" || failures=$((failures + 1))
  done < <(cmd_deferred)
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
  # Cargo builds a package's binaries only for that package's own integration
  # tests, and the suite that drives `ply` is in `ply-cli-tests`.
  if ! ls "$root"/crates/ply-cli/tests/*.rs >/dev/null 2>&1; then
    echo "FAIL: crates/ply-cli/tests/ has no .rs file, so cargo builds no 'ply' for ply-cli-tests' suite to run" >&2
    failures=$((failures + 1))
  fi

  local nextest="$root/.config/nextest.toml" filter
  filter=$(cmd_deferred_filter)
  if [[ ! -f $nextest ]]; then
    echo "FAIL: no $nextest, so nothing runs the deferred tests alone" >&2
    failures=$((failures + 1))
  elif [[ $(grep -cxF "filter = '$filter'" "$nextest") -ne 1 ]]; then
    echo "FAIL: $nextest does not carry this table's deferred filter exactly once; paste the output of 'ci-shards.sh deferred-filter' into the override, single-quoted, on one line" >&2
    failures=$((failures + 1))
  fi

  # --- probes ---------------------------------------------------------------
  #
  # Is every directory accounted for, does every job this table names exist,
  # and is it actually required.
  local workflow="$root/.github/workflows/ci.yml"
  local -a probe_listed=()
  local probe job needs block
  if [[ ! -f $workflow ]]; then
    echo "FAIL: no workflow at $workflow, so no probe job can be checked" >&2
    failures=$((failures + 1))
  fi
  # The `needs:` list of the `ci` aggregate job, read by joining the whole
  # `ci:` block onto one line first, because the list wraps across lines. The
  # `exit` on the next job-level key keeps this reading `ci`'s list and not a
  # later job's, if `ci` ever stops being last.
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
    # A job that exists and is required still proves nothing unless it runs
    # *this* directory: watched to fail 2026-08-30 by making exactly that
    # substitution. The block is matched with `[[ == * ]]` rather than piped
    # into `grep -q`, which under `pipefail` reads backwards when it exits at
    # its first match.
    else
      block=$(awk -v j="  $job:" '$0 == j {f = 1; next} f && /^  [a-z]/ {exit} f' "$workflow")
      if [[ $block != *"probes/$probe/run.sh"* ]]; then
        echo "FAIL: job '$job' exists but its steps never run probes/$probe/run.sh, so it is required in name only" >&2
        failures=$((failures + 1))
      fi
    fi
    # Whole word: a substring test passes `probe` against a `needs:` holding
    # only `ucontext-probe`, which is the same false green one directory down.
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
        echo "FAIL: probes/$probe is in no CI job, so nothing in CI runs it -- which is exactly how crates/ply-compiler rotted" >&2
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
  echo "${#all_members[@]} workspace members; ${#KNOWN_OUTSIDE[@]} crate(s) deliberately outside; ${#DEFERRED[@]} deferred tests, ${#TREE_CHECKS[@]} tree checks and ${#SOLO[@]} solo tests, each present in the tree; ${#PROBE_JOBS[@]} probe(s) run by a required CI job; $PARTITIONS partitions"
}

case "${1:-}" in
  verify) cmd_verify ;;
  partitions) cmd_partitions ;;
  solo-matrix) cmd_solo_matrix ;;
  solo-filter) cmd_solo_filter "${2:?a solo id}" ;;
  exclude-filter) cmd_exclude_filter ;;
  gate-filter) cmd_gate_filter ;;
  postgres-filter) cmd_postgres_filter ;;
  deferred) cmd_deferred ;;
  deferred-filter) cmd_deferred_filter ;;
  tree-checks) cmd_tree_checks ;;
  tree-check-filter) cmd_tree_check_filter ;;
  *)
    echo "usage: ci-shards.sh {verify|partitions|solo-matrix|solo-filter ID|exclude-filter|gate-filter|postgres-filter|deferred|deferred-filter|tree-checks|tree-check-filter}" >&2
    exit 2
    ;;
esac
