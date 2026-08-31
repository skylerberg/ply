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
#   ci-shards.sh verify        every member is in exactly one shard, every
#                              deferred test and every tree check exists where
#                              this table says, and every directory under
#                              `spikes/` is either run by a named CI job or
#                              listed as deliberately outside with a reason
#   ci-shards.sh matrix        the JSON matrix for the parallel test job
#   ci-shards.sh packages ID   `-p` arguments for one shard
#   ci-shards.sh skips         `--skip` arguments the parallel shards need
#   ci-shards.sh deferred      one `package target test` line per deferred test
#   ci-shards.sh tree-checks   one `package target test` line per tree check

set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

# Shard id, then its packages.
#
# Balanced against what the shards actually cost **in CI**, which is the machine
# the balance is for. Taken from run 33338854134 (ubuntu-24.04, 2026-08-30) by
# differencing the timestamps of consecutive `Running <binary>` lines in each
# job log, so each figure is test execution only — the ~39s dependency build
# every shard pays is excluded, and per-binary startup is counted in:
#
#   ply-corpus 426s   ply-eval 404s   ply-cli 200s   ply-host ~60s
#   the other nine packages, summed: 16s
#
# **The previous table's basis had drifted and the split it produced was the
# slowest thing in CI.** It read, from `cargo test -p <package>` on the machine
# in docs/ONBOARDING.md §Provenance, warm target, `-j 2 -- --test-threads=2`,
# 2026-08-24:
#
#   ply-corpus 289s   ply-cli 149s   ply-eval 137s   ply-store  32s
#   ply-hash    15s   ply-test 13s   ply-core   8s   ply-span    5s
#   ply-syntax   3s   ply-prove 4s   ply-std    2s   ply-derive  1s
#
#   "658s summed, and `ply-corpus` alone is 44% of it, so three shards is the
#   useful number: a fourth cannot finish sooner than that one package takes,
#   and every extra shard pays the dependency build again."
#
# The floor argument is still sound; the arrangement had stopped sitting on the
# floor. `ply-eval` roughly tripled (137s -> 404s), so `cli-eval` became 604s
# against `ply-corpus`'s 426s floor, while `core` finished its 1,604 tests in
# **16s** — one runner idle for ten minutes while another held the whole run up.
# Splitting `ply-eval` off and folding the nine fast packages in with `ply-cli`
# puts the three parallel shards at 426s / 404s / 216s, which is the floor, and
# keeps the shard count at three so no extra dependency build is paid.
#
# The figures above were taken **before** `[profile.dev] opt-level = 2` landed
# in the root manifest -- the root `Cargo.toml`'s profile block holds that
# measurement, its provenance and its caveats. Expect the jobs to become
# compile-bound rather than test-bound.
#
# Re-taken after the profile change, but **on the wrong machine and above the
# load gate**: the three shards run locally (Apple M4, 10 cores, so not the
# two-core runner these are balanced for) at 1-minute load between 6 and 24
# against CONTRIBUTING.md s"Gate on an idle machine"'s threshold of 4 --
# `corpus` 120s / 197 tests, `eval` 69s / 1,033, `cli` 41s / 2,319. Treat those
# as a shape and not as figures. The shape is that `ply-corpus` stops being
# level with `ply-eval` and becomes the clear long pole, roughly 1.7x it, which
# is the ordering this table already assumes. **A re-take on a two-core runner
# at load < 4 has not been done**, and it is what would justify moving anything.
#
# Your figures will differ; re-take with `cargo test -p <package>` if a package
# grows a slow suite, and move it. Splitting `ply-corpus` further would mean
# partitioning by test target, which would give up the property `verify` checks
# — that every *package* is somewhere.
#
# `ply-host` is a shard of its own because it is the only package that needs a
# database. The postgres job runs it and no other job does.
#
# `ply-codegen` joined the `cli` shard on 2026-08-31 and it costs that shard
# almost nothing to run and something real to build: its own suite is 9 tests in
# ~1.2s, and the ~31 cranelift packages behind it are a dependency build every
# shard that builds `ply-cli` now pays anyway, because `ply-cli` depends on it.
# It is in the same shard as `ply-cli` on purpose rather than by balance: the
# tests that decide whether a code generator is policeable are
# `crates/ply-cli/tests/backend.rs`, and a partition that could run one without
# the other would let half of ADR 0026 §4.5's condition go green alone.
#
# **This table's own gate caught the omission.** Adding the crate without
# adding it here failed `ci-shards.sh verify` with *"workspace member
# 'ply-codegen' is in no shard, so CI never tests it"* — which is the failure
# this file exists to produce, observed rather than assumed.
SHARDS=(
  "corpus:ply-corpus"
  "eval:ply-eval"
  "cli:ply-cli ply-span ply-syntax ply-derive ply-core ply-hash ply-store ply-test ply-prove ply-std ply-codegen"
  "postgres:ply-host"
)

# The shard the postgres job owns. It is not in the parallel matrix.
POSTGRES_SHARD=postgres

# Crate directories that are deliberately not workspace members, and why. A
# crate in neither this list nor `members` is an accident: nothing builds it and
# no job tests it, which is the failure this file exists to prevent.
# Expanded as ${KNOWN_OUTSIDE[@]+...} everywhere below: bash 3.2, which is
# /bin/bash on macOS, treats "${empty[@]}" as unset under `set -u`, and this
# list is meant to become empty.
#
# The reason attached to the one entry below was rewritten on 2026-08-28,
# because the old one had stopped being true. It read: "its own workspace on
# purpose, per ADR 0016 3.5, so that deferring M9 deletes it with rm -r". R5
# falsified that: `rm -r crates/ply-codegen-spike` leaves the whole compiled
# seam standing in `crates/ply-eval/src/compiled.rs`, so the deletion no longer
# buys what ADR 0016 3.5 said it buys, and performing it today would remove the
# only implementation of `Compiled` in existence and leave the declaration
# behind. ADR 0026 4.7 amends 3.5 accordingly and makes the deletion
# conditional on something checkable, which is what the entry now records.
#
# The reproduction was done on 2026-08-28 and the condition came back NOT
# satisfied, so the entry is narrowed rather than deleted. Seven of the eight
# configurations moved: five into crates/ply-eval/tests/differential_corpus.rs
# over ply_eval::backend::Reference at corpus scale, exceeds-budget=4 through
# `ply test --backend` in crates/ply-cli/tests/backend.rs, and answers= on the
# offer count of zero that is its whole point. The eighth does not move, for a
# structural reason: the spike's backend is native code on a fixed stack, so
# ignoring the budget entirely CRASHES and run_guarded reports it from outside;
# Reference is a tree-walker whose frames grow on the heap, so the same
# corruption HANGS -- measured at no output and no exit in 45 seconds against
# 0.03s for the run that reports. Nothing in the workspace can report a run that
# never comes back, so the spike is still the only place that demonstration
# lives.
# **The condition is MET as of 2026-08-31 and the entry is still here.** The
# note below read, until then: "seven of eight is where 2026-08-28 left it --
# the unbounded exceeds-budget runaway crashes under the spike's native frames
# and only hangs under a tree-walker, and a run that never comes back cannot be
# reported from inside it". The workspace now has a backend with native frames:
# `ply test --backend cranelift:wrong:exceeds-budget` over a recursion with no
# base case aborts, exit 134, in 0.02s, and `ply-cli/tests/backend.rs` has
# always run `ply` as a child so the reporter was never the problem. Eight of
# eight, under `cargo test --workspace`.
#
# What holds the entry is no longer the condition. It is that deleting the crate
# deletes the only instrument for two open things: CONTRIBUTING.md item 18's 42
# unexplained agreement disagreements, and ADR 0018 0.5's 6.199x, which nothing
# else produces. ADR 0026 4.7 records both and says item 18 should carry the
# deletion.
declare -a KNOWN_OUTSIDE=(
  "ply-codegen-spike:its own workspace on purpose; ADR 0026 4.7's deletion condition -- ALL EIGHT wrong backends reproduced in the workspace -- was MET on 2026-08-31 by crates/ply-codegen, so what keeps this crate is no longer the condition but the two open findings only it can measure: CONTRIBUTING.md item 18's 42 agreement disagreements and ADR 0018 0.5's 6.199x kernel figure. Closing item 18 is what should carry the rm -r"
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

# Tests that fail on a property of the *tree* rather than of a run, as
# `package:target:test`, with the same three-field spelling as `DEFERRED`.
#
# Not "gates" in the sense §"There is CI" uses that word — those are
# dependencies that make a suite skip silently. These are checks whose subject
# is the source tree itself.
#
# They are already inside `cargo test -p ply-span`, so the parallel shards run
# them. They are named here as well for one reason: a check that stops running
# reports nothing, and reporting nothing is indistinguishable from passing.
# `verify` fails when a name here is not defined in the file this table says,
# and the `test` job runs each by `--exact` name and asserts it actually ran —
# so renaming one, deleting it, or filtering it away turns CI red instead of
# quietly reducing what CI checks.
#
# All seven are in `crates/ply-span/tests/armed.rs`. Six of them are one defect:
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
  "ply-span:armed:every_registered_code_is_constructed_in_production"
  "ply-span:armed:every_variant_of_a_covered_enum_is_constructed_in_production"
  "ply-span:armed:every_diagnostic_constructor_call_names_its_code_literally"
  "ply-span:armed:the_code_registry_table_is_total_over_the_codes_module"
  "ply-span:armed:no_allowlist_entry_has_outlived_its_reason"
  "ply-span:armed:ambiguous_enum_names_are_declared"
  "ply-span:armed:no_two_adrs_share_a_number"
)

# Directories under `spikes/`, and the CI job that runs each.
#
# `KNOWN_OUTSIDE` above exists because a crate in no shard is a crate nothing
# builds. A spike is the same failure with a worse blast radius, and it has
# already happened twice here: `spikes/ply-parser` sat outside the cargo
# workspace with its own `[workspace]`, in **no CI job at all**, while its
# `README.md` predicted in writing that it would bit-rot -- and it did. Four
# language features landed, its differential went red on 28 of 763 inputs
# (70.2% of the corpus by bytes) and nothing said so for two days.
#
# So this is the same check one directory up. Each entry is `dir:job`, and
# `verify` fails unless the job exists in `.github/workflows/ci.yml` **and** is
# named in the `ci` aggregate job's `needs:` list -- because a job that nothing
# needs is not required, and the `ci` job's own comment is what says a skipped
# job is not a green tick.
declare -a SPIKE_JOBS=(
  "ply-parser:parser-spike"
)

# Spikes deliberately in no CI job, with the reason, in `KNOWN_OUTSIDE`'s shape.
#
# **This list is a finding, not a decision that closes anything.** Three of the
# four are `.ply` files and a `bench.sh` with no harness and no differential:
# there is nothing for a job to assert, and a benchmark whose output is a number
# is not a check. `spikes/ply-lexer` is different and its entry says so.
declare -a SPIKES_OUTSIDE_CI=(
  "ply-lexer:it HAS a differential and a run.sh and it is BROKEN -- as of 2026-08-30 \`spikes/ply-lexer/run.sh\` does not reach a single test because its harness does not compile: \`non-exhaustive patterns: &ply_syntax::lexer::TokenKind::Question not covered\` at src/lib.rs:66, the identical bit-rot the parser spike one directory over was just repaired for. It is cited by ADR 0020 6.1, ADR 0021 and ADR 0022 for throughput figures that cannot currently be re-taken. Adding a job here would only report a red that is already known; the entry exists so that the red is written down where CI is configured rather than only in a session transcript. Fix the spike, then move it to SPIKE_JOBS"
  "ply-lexer-nesting:three files -- main.ply, nesting.ply, bench.sh -- and no harness, no fixtures and no differential. It measures how deep a fold nests; its output is a number for ADR 0022, not a pass or a fail"
  "ply-lexer-rc:same shape -- main.ply, fieldorder.ply, bench.sh. It measures what building a container anywhere but last in a record literal costs, which is spikes/ply-lexer/GAPS.md 1's measurement. A number, not a check"
  "ply-lexer-throughput:same shape -- main.ply, lexer.ply, bench.sh. It measures tokens per second for ADR 0020 6.1. A number, not a check"
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

# Same three-field split as `cmd_deferred`, for the same reason.
cmd_tree_checks() {
  local entry rest
  for entry in "${TREE_CHECKS[@]}"; do
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

  # The same existence check as the deferred table, and it matters more here:
  # a deferred test that vanishes only stops being skipped, while a tree check
  # that vanishes stops being checked and says nothing.
  while read -r package target test; do
    file="$root/crates/$package/tests/$target.rs"
    if [[ ! -f $file ]]; then
      echo "FAIL: tree check '$test' names $file, which does not exist" >&2
      failures=$((failures + 1))
    elif ! grep -q "fn $test(" "$file"; then
      echo "FAIL: $file has no 'fn $test(' — CI asserts a check nothing defines" >&2
      failures=$((failures + 1))
    fi
    id=""
    for entry in "${SHARDS[@]}"; do
      for candidate in ${entry#*:}; do
        [[ $candidate == "$package" ]] && id=${entry%%:*}
      done
    done
    if [[ -z $id ]]; then
      echo "FAIL: tree check '$test' is in '$package', which is in no shard" >&2
      failures=$((failures + 1))
    fi
  done < <(cmd_tree_checks)

  # --- spikes ---------------------------------------------------------------
  #
  # Same three questions as the crate half: is every directory accounted for,
  # does every job this table names exist, and is it actually required.
  local workflow="$root/.github/workflows/ci.yml"
  local -a spike_listed=()
  local spike job needs block
  if [[ ! -f $workflow ]]; then
    echo "FAIL: no workflow at $workflow, so no spike job can be checked" >&2
    failures=$((failures + 1))
  fi
  # The `needs:` list of the `ci` aggregate job. Every SPIKE_JOBS entry has to
  # appear in it or the job is not required and gates nothing.
  #
  # Read by joining the whole `ci:` block onto one line first, because the list
  # is wrapped across two lines whenever `cargo fmt`-style line length would be
  # exceeded -- a `sed -n 's/^ *needs: *//p'` read the first line of it and the
  # check below then failed on a job that was in fact listed. The `exit` on the
  # next job-level key is what keeps this reading `ci`'s list and not a later
  # job's, if `ci` ever stops being last.
  needs=$(awk '/^  ci:/{f=1;next} f && /^  [a-z]/{exit} f' "$workflow" 2>/dev/null |
    tr '\n' ' ' | sed -n 's/.*needs: *\(\[[^]]*\]\).*/\1/p')
  if [[ -z $needs ]]; then
    echo "FAIL: could not read the \`ci\` job's \`needs:\` list out of $workflow -- every check below would pass vacuously" >&2
    failures=$((failures + 1))
  fi
  for entry in ${SPIKE_JOBS[@]+"${SPIKE_JOBS[@]}"}; do
    spike=${entry%%:*}
    job=${entry#*:}
    spike_listed+=("$spike")
    if [[ ! -d "$root/spikes/$spike" ]]; then
      echo "FAIL: SPIKE_JOBS names spikes/$spike, which is not in the tree -- delete the entry" >&2
      failures=$((failures + 1))
      continue
    fi
    if [[ ! -x "$root/spikes/$spike/run.sh" ]]; then
      echo "FAIL: spikes/$spike has no executable run.sh, so job '$job' has nothing to run" >&2
      failures=$((failures + 1))
    fi
    if ! grep -q "^  $job:\$" "$workflow"; then
      echo "FAIL: SPIKE_JOBS says job '$job' runs spikes/$spike, and $workflow defines no such job" >&2
      failures=$((failures + 1))
    # A job that exists and is required still proves nothing unless it runs
    # *this* spike. Without this arm, `ply-parser:spike` -- the codegen spike's
    # job, which exists and is in `needs:` -- passed every check above while
    # `spikes/ply-parser/run.sh` was executed by nothing. Watched to fail
    # 2026-08-30 by making exactly that substitution.
    #
    # The job's block is read into a variable and matched with `[[ == * ]]`
    # rather than piped into `grep -q`: this file runs under `pipefail`, and a
    # `grep -q` that exits at its first match closes the pipe, so the producer's
    # SIGPIPE becomes the pipeline's status and the test reads backwards --
    # more matching output making failure more likely. That is not
    # hypothetical here: `spikes/ply-parser/arm-harness.sh`'s header records the
    # same construction scoring its three loudest results wrong.
    else
      block=$(awk -v j="  $job:" '$0 == j {f = 1; next} f && /^  [a-z]/ {exit} f' "$workflow")
      if [[ $block != *"spikes/$spike/run.sh"* ]]; then
        echo "FAIL: job '$job' exists but its steps never run spikes/$spike/run.sh, so the spike is required in name only" >&2
        failures=$((failures + 1))
      fi
    fi
    # Whole word: a substring test passes `spike` against a `needs:` holding
    # only `parser-spike`, which is the same false green one directory down.
    if [[ " ${needs//[][,]/ } " != *" $job "* ]]; then
      echo "FAIL: job '$job' is not in the \`ci\` job's needs list, so it is not required and a green tick can be reported over it never having run" >&2
      failures=$((failures + 1))
    fi
  done
  for entry in ${SPIKES_OUTSIDE_CI[@]+"${SPIKES_OUTSIDE_CI[@]}"}; do
    spike=${entry%%:*}
    note=${entry#*:}
    spike_listed+=("$spike")
    if [[ ! -d "$root/spikes/$spike" ]]; then
      echo "FAIL: SPIKES_OUTSIDE_CI names spikes/$spike, which is not in the tree -- delete the entry ($note)" >&2
      failures=$((failures + 1))
    fi
  done
  if [[ -d "$root/spikes" ]]; then
    for dir in "$root"/spikes/*/; do
      spike=$(basename "$dir")
      seen=0
      for candidate in ${spike_listed[@]+"${spike_listed[@]}"}; do
        [[ $candidate == "$spike" ]] && seen=$((seen + 1))
      done
      if [[ $seen -eq 0 ]]; then
        echo "FAIL: spikes/$spike is in no CI job and in no SPIKES_OUTSIDE_CI entry, so nothing in CI runs it and nothing says why -- which is exactly how spikes/ply-parser rotted" >&2
        failures=$((failures + 1))
      elif [[ $seen -gt 1 ]]; then
        echo "FAIL: spikes/$spike is listed $seen times" >&2
        failures=$((failures + 1))
      fi
    done
  fi

  if [[ $failures -gt 0 ]]; then
    echo "$failures problem(s) in the shard table" >&2
    return 1
  fi
  echo "${#all_members[@]} workspace members, each in exactly one shard; ${#KNOWN_OUTSIDE[@]} crate(s) deliberately outside; ${#DEFERRED[@]} deferred tests and ${#TREE_CHECKS[@]} tree checks, each present in the tree; ${#SPIKE_JOBS[@]} spike(s) run by a required CI job and ${#SPIKES_OUTSIDE_CI[@]} deliberately outside"
}

case "${1:-}" in
  verify) cmd_verify ;;
  matrix) cmd_matrix ;;
  packages) cmd_packages "${2:?a shard id}" ;;
  skips) cmd_skips ;;
  deferred) cmd_deferred ;;
  tree-checks) cmd_tree_checks ;;
  *)
    echo "usage: ci-shards.sh {verify|matrix|packages ID|skips|deferred|tree-checks}" >&2
    exit 2
    ;;
esac
