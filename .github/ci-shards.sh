#!/usr/bin/env bash
#
# The table CI's test jobs are cut from, and the check that the cut is total.
#
# `cargo test --workspace` is 915.8s of in-target time — the `test result:`
# lines summed, excluding compilation — so CI runs it as several jobs rather
# than one. A partition is a chance to lose a package silently,
# which is this repository's most expensive defect class, so the partition lives
# here once and `verify` reads the workspace members out of `Cargo.toml` and
# fails if a member is in no shard, in two shards, or named here and absent from
# the tree.
#
#   ci-shards.sh verify        every member is in exactly one shard, and every
#                              deferred test exists where this table says
#   ci-shards.sh matrix        the JSON matrix for the parallel test job
#   ci-shards.sh packages ID   `-p` arguments for one shard
#   ci-shards.sh skips         `--skip` arguments the parallel shards need
#   ci-shards.sh deferred      one `package target test` line per deferred test

set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

# Shard id, then its packages.
#
# Balanced against measured `cargo test -p <package>` wall clock — warm target,
# `-j 2 -- --test-threads=2` to approximate a two-core hosted runner, on the
# machine in docs/ONBOARDING.md §Provenance (Apple M4, 10 cores, rustc 1.93.1),
# 2026-08-24:
#
#   ply-corpus 289s   ply-cli 149s   ply-eval 137s   ply-store  32s
#   ply-hash    15s   ply-test 13s   ply-core   8s   ply-span    5s
#   ply-syntax   3s   ply-prove 4s   ply-std    2s   ply-derive  1s
#
# 658s summed, and `ply-corpus` alone is 44% of it, so three shards is the
# useful number: a fourth cannot finish sooner than that one package takes, and
# every extra shard pays the dependency build again. Splitting `ply-corpus`
# further would mean partitioning by test target, which would give up the
# property `verify` checks — that every *package* is somewhere.
#
# Your figures will differ; the ordering is what this table depends on. Re-take
# with `cargo test -p <package>` if a package grows a slow suite, and move it.
#
# `ply-host` is a shard of its own because it is the only package that needs a
# database. The postgres job runs it and no other job does.
SHARDS=(
  "corpus:ply-corpus"
  "cli-eval:ply-cli ply-eval"
  "core:ply-span ply-syntax ply-derive ply-core ply-hash ply-store ply-test ply-prove ply-std"
  "postgres:ply-host"
)

# The shard the postgres job owns. It is not in the parallel matrix.
POSTGRES_SHARD=postgres

# Crate directories that are deliberately not workspace members, and why. A
# crate in neither this list nor `members` is an accident: nothing builds it and
# no job tests it, which is the failure this file exists to prevent.
# Expanded as ${KNOWN_OUTSIDE[@]+...} everywhere below: bash 3.2, which is
# /bin/bash on macOS, treats "${empty[@]}" as unset under `set -u`, and ADR 0016
# 3.5 says this list is meant to become empty.
declare -a KNOWN_OUTSIDE=(
  "ply-codegen-spike:its own workspace on purpose, per ADR 0016 3.5, so that deferring M9 deletes it with rm -r"
)

# Tests whose assertion reads a wall clock, as `package:target:test`, where
# `target` is an integration test binary or the literal `lib` for a unit test.
#
# Each passes or fails on how much CPU it was given rather than on what the code
# does, so several test binaries at once on a two-core hosted runner is the
# wrong place for them. The parallel shards `--skip` them by name; the timing
# job runs them alone, one at a time, single-threaded, with `--nocapture` so the
# figures they print reach the log. Names are matched with `--exact`, so a unit
# test is named by its full module path.
#
# **This list is maintained by running the shards, not by surveying the tree.**
# Two surveys have been done and each declared itself complete; each was proved
# wrong by the next shard run, within the hour:
#
#   * A grep of `crates/*/tests` for `Instant::now` produced seven entries. The
#     corpus shard then failed on
#     `measure::tests::every_resumption_costs_about_what_the_first_one_did` —
#     *"the fourth resumption cost 5680.8965 us against 2196.552 us"*, against
#     `four.marginal_micros < one.micros * 2.0` — a unit test in `src/`, which
#     that grep could not see. Re-surveying `crates/*/src` took the list to 12.
#   * The cli-eval shard then failed on
#     `routing_a_path_of_escapes_costs_its_length_and_not_its_square`
#     (`crates/ply-cli/tests/w3_http_audit.rs:714`) — *"four times the escapes
#     cost 1655.9ms against 143.6ms for k, which is 11.5x"*, against
#     `four <= one * 9.0`. The second survey missed it too: the test reads no
#     Rust clock at all, it parses milliseconds out of `ply test`'s own output
#     via a `duration_of` helper, so no timing vocabulary appears in it. Run
#     alone it passes three times out of three at load 20.
#
# Treat 13 as the current count and not as the answer. When a shard goes red on
# a ratio or a budget, the fix is usually another row here.
DEFERRED=(
  "ply-eval:region_arena_cost:snapshot_cost_as_a_function_of_region_size"
  "ply-eval:fixture_open_cost:a_seeded_fixture_opens_per_test_in_microseconds"
  "ply-eval:simulation:a_long_sleep_is_a_jump"
  "ply-test:region_fixture_cost:a_region_scoped_fixture_costs_the_fixture_and_never_the_test"
  "ply-test:region_fixture_cost:discarding_a_tests_own_cells_costs_nothing"
  "ply-test:region_fixture_cost:a_group_amortizes_the_build_up_to_a_ceiling_the_open_decides"
  "ply-test:region_fixture_cost:a_group_with_no_fixture_opens_and_closes_in_constant_time"
  "ply-corpus:lib:measure::tests::every_resumption_costs_about_what_the_first_one_did"
  "ply-corpus:lib:measure::tests::capture_and_resume_are_flat_in_the_frames_they_move"
  "ply-corpus:lib:measure::tests::opening_a_fixture_beats_rebuilding_it_once_the_fixture_is_real"
  "ply-store:lib:tests::opening_a_ten_thousand_definition_cache_is_under_the_budget"
  "ply-store:lib:tests::a_baseline_for_every_test_does_not_slow_the_open"
  "ply-cli:w3_http_audit:routing_a_path_of_escapes_costs_its_length_and_not_its_square"
)

shard_packages() {
  local id=$1 entry
  for entry in "${SHARDS[@]}"; do
    if [[ ${entry%%:*} == "$id" ]]; then
      printf '%s\n' "${entry#*:}"
      return 0
    fi
  done
  echo "no shard named '$id'" >&2
  return 1
}

cmd_packages() {
  local list package
  # Captured before the loop on purpose: `for x in $(f)` discards f's exit
  # status, so an unknown shard id printed its error and still exited 0 — and
  # the caller then ran `cargo test` with no `-p` at all.
  list=$(shard_packages "$1") || return 1
  for package in $list; do
    printf -- '-p %s ' "$package"
  done
  printf '\n'
}

cmd_skips() {
  local package target test
  while read -r package target test; do
    printf -- '--skip %s ' "$test"
  done < <(cmd_deferred)
  printf '\n'
}

# One `package target test` line per entry. Split on the first two colons only:
# a unit test's name contains `::`, so `${entry##*:}` would take the last
# segment of the module path and skip the wrong thing — or nothing.
cmd_deferred() {
  local entry rest
  for entry in "${DEFERRED[@]}"; do
    rest=${entry#*:}
    printf '%s %s %s\n' "${entry%%:*}" "${rest%%:*}" "${rest#*:}"
  done
}

cmd_matrix() {
  local entry id first=1
  printf '{"include":['
  for entry in "${SHARDS[@]}"; do
    id=${entry%%:*}
    [[ $id == "$POSTGRES_SHARD" ]] && continue
    ((first)) || printf ','
    first=0
    printf '{"shard":"%s"}' "$id"
  done
  printf ']}\n'
}

# Workspace members as package names, read from the root manifest rather than
# from `cargo metadata`: this runs before anything is compiled.
members() {
  local manifest="$root/Cargo.toml" found
  if [[ ! -f $manifest ]]; then
    echo "FAIL: no workspace manifest at $manifest" >&2
    return 1
  fi
  found=$(sed -n '/^members = \[/,/^]/p' "$manifest" |
    sed -n 's#.*"crates/\([a-z0-9-]*\)".*#\1#p')
  if [[ -z $found ]]; then
    echo "FAIL: read no workspace members out of $manifest — the [workspace] members list is missing or no longer one quoted \"crates/NAME\" per line, and every check below would pass vacuously" >&2
    return 1
  fi
  printf '%s\n' "$found"
}

cmd_verify() {
  local failures=0 member package entry candidate id target test leaf file seen dir note

  local -a known=()
  if ! members >/dev/null; then
    return 1
  fi
  local -a all_members=()
  while read -r member; do all_members+=("$member"); done < <(members)

  local -a listed=()
  for entry in "${SHARDS[@]}"; do
    for package in ${entry#*:}; do
      listed+=("$package")
    done
  done

  for member in "${all_members[@]}"; do
    seen=0
    for package in "${listed[@]}"; do
      [[ $package == "$member" ]] && seen=$((seen + 1))
    done
    if [[ $seen -eq 0 ]]; then
      echo "FAIL: workspace member '$member' is in no shard, so CI never tests it" >&2
      failures=$((failures + 1))
    elif [[ $seen -gt 1 ]]; then
      echo "FAIL: workspace member '$member' is in $seen shards" >&2
      failures=$((failures + 1))
    fi
  done

  for package in "${listed[@]}"; do
    if ! printf '%s\n' "${all_members[@]}" | grep -qx "$package"; then
      echo "FAIL: a shard names '$package', which is not a workspace member" >&2
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

  while read -r package target test; do
    if [[ $target == lib ]]; then
      # A unit test: the name is a module path, and what has to exist is the
      # leaf `fn` somewhere under the package's `src/`.
      leaf=${test##*::}
      if [[ ! -d "$root/crates/$package/src" ]]; then
        echo "FAIL: deferred test '$test' names crates/$package/src, which does not exist" >&2
        failures=$((failures + 1))
      elif ! grep -rq "fn $leaf(" "$root/crates/$package/src"; then
        echo "FAIL: no 'fn $leaf(' under crates/$package/src — the shards skip a name nothing defines" >&2
        failures=$((failures + 1))
      fi
    else
      file="$root/crates/$package/tests/$target.rs"
      if [[ ! -f $file ]]; then
        echo "FAIL: deferred test '$test' names $file, which does not exist" >&2
        failures=$((failures + 1))
      elif ! grep -q "fn $test(" "$file"; then
        echo "FAIL: $file has no 'fn $test(' — the shards skip a name nothing defines" >&2
        failures=$((failures + 1))
      fi
    fi
    id=""
    for entry in "${SHARDS[@]}"; do
      for candidate in ${entry#*:}; do
        [[ $candidate == "$package" ]] && id=${entry%%:*}
      done
    done
    if [[ -z $id ]]; then
      echo "FAIL: deferred test '$test' is in '$package', which is in no shard" >&2
      failures=$((failures + 1))
    fi
  done < <(cmd_deferred)

  if [[ $failures -gt 0 ]]; then
    echo "$failures problem(s) in the shard table" >&2
    return 1
  fi
  echo "${#all_members[@]} workspace members, each in exactly one shard; ${#KNOWN_OUTSIDE[@]} crate(s) deliberately outside; ${#DEFERRED[@]} deferred tests, each present in the tree"
}

case "${1:-}" in
  verify) cmd_verify ;;
  matrix) cmd_matrix ;;
  packages) cmd_packages "${2:?a shard id}" ;;
  skips) cmd_skips ;;
  deferred) cmd_deferred ;;
  *)
    echo "usage: ci-shards.sh {verify|matrix|packages ID|skips|deferred}" >&2
    exit 2
    ;;
esac
