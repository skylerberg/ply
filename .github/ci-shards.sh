#!/usr/bin/env bash
# The tables CI's test jobs are cut from, and the check that the cut is total.
#
#   ci-shards.sh verify          every crate is a member, every test named here
#                                exists, and every `probes/` directory is run by
#                                a job the `ci` aggregate requires
#   ci-shards.sh partitions      the JSON matrix of partition slices
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

# Tests that get a runner of their own, as `id:package:target:test`.
SOLO=(
  "bootstrap:ply-codegen-tests:bootstrap:the_bootstrap_bundle_is_a_fixpoint_of_the_emitter_it_builds"
  "compiler-on-the-tier:ply-cli-tests:suite:corpus::the_compiled_tier_runs_the_compilers_own_tests_as_the_only_engine"
  "archive-round-trip:ply-cli-tests:suite:bootstrap_archive::an_archive_is_written_and_verifies_against_the_tree_it_came_from"
  "archive-tree-moved:ply-cli-tests:suite:bootstrap_archive::an_archive_stops_describing_a_tree_that_moved"
  "corpus-session:ply-cli-tests:suite:incremental::the_example_corpus_agrees_across_a_session"
  "corpus-session-audit:ply-cli-tests:suite:incremental_audit::a_long_session_over_the_example_corpus_agrees_at_every_step"
  "parser-census:ply-codegen-tests:suite:parser_census::the_census_over_the_parser_spike"
)

# Their tests skip, passing, without a postgres server; only `test-postgres` runs them.
POSTGRES_PACKAGES=(ply-host-tests)

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
  "ply-span-tests:armed:no_two_adrs_share_a_number"
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
  # Cargo builds `ply` for ply-cli-tests only if ply-cli has an integration test of its own.
  if ! ls "$root"/crates/ply-cli/tests/*.rs >/dev/null 2>&1; then
    echo "FAIL: crates/ply-cli/tests/ has no .rs file, so cargo builds no 'ply' for ply-cli-tests' suite to run" >&2
    failures=$((failures + 1))
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
  echo "${#all_members[@]} members under crates/ (plus $(members_outside_crates | grep -c . || true) outside); ${#KNOWN_OUTSIDE[@]} crate(s) deliberately outside; ${#TREE_CHECKS[@]} tree checks and ${#SOLO[@]} solo tests, each present in the tree; ${#PROBE_JOBS[@]} probe(s) run by a required CI job; $PARTITIONS partitions"
}

case "${1:-}" in
  verify) cmd_verify ;;
  partitions) cmd_partitions ;;
  solo-matrix) cmd_solo_matrix ;;
  solo-filter) cmd_solo_filter "${2:?a solo id}" ;;
  exclude-filter) cmd_exclude_filter ;;
  gate-filter) cmd_gate_filter ;;
  postgres-filter) cmd_postgres_filter ;;
  tree-checks) cmd_tree_checks ;;
  tree-check-filter) cmd_tree_check_filter ;;
  *)
    echo "usage: ci-shards.sh {verify|partitions|solo-matrix|solo-filter ID|exclude-filter|gate-filter|postgres-filter|tree-checks|tree-check-filter}" >&2
    exit 2
    ;;
esac
