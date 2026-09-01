#!/usr/bin/env bash
#
# The table CI's test jobs are cut from, and the check that the cut is total.
#
# CI runs the suite as several jobs rather than one, each compiling only the
# packages it tests, because a job's wall clock is compile plus run and the
# compile is the larger half. Every job pays the dependency graph under its
# packages, so a shard is cut on that graph first and on run time second.
#
# A partition is a chance to lose a package silently, which is this
# repository's most expensive defect class, so the partition lives here once and
# `verify` reads the workspace members out of `Cargo.toml` and fails if a member
# is in no shard, in two shards, or named here and absent from the tree.
#
#   ci-shards.sh verify        every member is in exactly one shard, every
#                              deferred test and every tree check exists where
#                              this table says, `.config/nextest.toml` names
#                              exactly the deferred tests, and every directory
#                              under `spikes/` is either run by a named CI job
#                              or listed as deliberately outside with a reason
#   ci-shards.sh matrix        the JSON matrix for the parallel test job
#   ci-shards.sh packages ID   `-p` arguments for one shard
#   ci-shards.sh deferred      one `package target test` line per deferred test
#   ci-shards.sh deferred-filter
#                              the nextest filterset selecting exactly the
#                              deferred tests; `.config/nextest.toml` carries it
#                              verbatim and `verify` checks that it does
#   ci-shards.sh tree-checks   one `package target test` line per tree check

set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

# Shard id, then its packages.
#
# **Split on dependency weight, not on measured seconds.** Four packages need
# the heavy half of the graph -- `ply-codegen` and `ply-cli` pull cranelift,
# `ply-cli` and `ply-corpus` also pull tokio, rustls and a postgres client,
# `ply-host` pulls the latter three. Everything else builds against about
# eighty crates instead of about two hundred. That boundary is a property of
# the manifests rather than a reading off one runner, so it does not go stale
# between commits the way the figures below do.
#
# It decides the table twice over. A test binary links its whole graph, so the
# *same* test target costs materially more in a shard that carries cranelift
# and a TLS stack than in one that does not -- which makes "put the light
# packages where the light graph is" a win on total work even before it is a
# win on balance. The previous arrangement did the opposite: nine light
# packages sat in the `cli` shard, linking twelve-odd test binaries against the
# heaviest graph in the tree.
#
# **The imbalance is compile and linking, not tests.** Balancing on test
# seconds, which is what earlier revisions of this table did, was measuring the
# smaller half. So: `cli` keeps only what needs cranelift, `corpus` and
# `postgres` are their own graphs, and the light packages sit on the light one.
#
# `ply-eval` is a shard of its own because it is the one light package whose
# suite is long: on the light graph the other nine run in seconds between
# them, while `ply-eval`'s integration suite sweeps every corpus on disk under
# a dozen backends. Splitting it off puts that suite on a runner that compiles
# only its own graph, and leaves `light` compiling `ply-eval`'s library once
# without its tests. Cache pressure was the reason this was not done earlier,
# and it is no longer a reason: a shard's dependency cache is a fraction of a
# gigabyte now that dependencies carry no debuginfo, the budget is 10GB per
# repository and eviction is LRU, so one more shard is one more entry rather
# than a cold build for everyone. Re-take the shard wall clocks from the first
# warm `main` run after a rebalance and move a package if one shard is the pole
# while another idles.
#
# Splitting `ply-corpus` further would mean partitioning by test target, which
# would give up the property `verify` checks -- that every *package* is
# somewhere.
#
# `ply-host` is a shard of its own because it is the only package that needs a
# database. The postgres job runs it and no other job does.
#
# `ply-codegen` sits with `ply-cli` on purpose rather than by balance: the
# tests that decide whether a code generator is policeable are
# `crates/ply-cli/tests/suite/backend.rs`, and a partition that could run one
# without the other would let half of the backend decision s4.5's condition go green alone.
# It also costs that shard almost nothing to run -- its own suite is 12 tests
# in ~4.4s -- and the cranelift packages behind it are a build `ply-cli` pays
# anyway.
#
# **This table's own gate caught an omission once.** Adding `ply-codegen`
# without adding it here failed `ci-shards.sh verify` with *"workspace member
# 'ply-codegen' is in no shard, so CI never tests it"* -- which is the failure
# this file exists to produce, observed rather than assumed.
SHARDS=(
  "corpus:ply-corpus"
  "cli:ply-cli ply-codegen"
  "eval:ply-eval"
  "light:ply-span ply-syntax ply-derive ply-core ply-hash ply-store ply-test ply-prove ply-std"
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
# purpose, per the codegen spike, so that deferring M9 deletes it with rm -r". R5
# falsified that: `rm -r crates/ply-codegen-spike` leaves the whole compiled
# seam standing in `crates/ply-eval/src/compiled.rs`, so the deletion no longer
# buys what the codegen spike said it buys, and performing it today would remove the
# only implementation of `Compiled` in existence and leave the declaration
# behind. the backend authorisation amends 3.5 accordingly and makes the deletion
# conditional on something checkable, which is what the entry now records.
#
# The reproduction was done on 2026-08-28 and the condition came back NOT
# satisfied, so the entry is narrowed rather than deleted. Seven of the eight
# configurations moved: five into crates/ply-eval/tests/suite/differential_corpus.rs
# over ply_eval::backend::Reference at corpus scale, exceeds-budget=4 through
# `ply test --backend` in crates/ply-cli/tests/suite/backend.rs, and answers= on the
# offer count of zero that is its whole point. The eighth does not move, for a
# structural reason: the spike's backend is native code on a fixed stack, so
# ignoring the budget entirely CRASHES and run_guarded reports it from outside;
# Reference evaluates on a nested machine whose frames grow on the heap, so the
# same corruption HANGS -- measured at no output and no exit in 45 seconds against
# 0.03s for the run that reports. Nothing in the workspace can report a run that
# never comes back, so the spike is still the only place that demonstration
# lives.
# **The condition is MET as of 2026-08-31 and the entry is still here.** The
# note below read, until then: "seven of eight is where 2026-08-28 left it --
# the unbounded exceeds-budget runaway crashes under the spike's native frames
# and only hangs under an interpreting backend, and a run that never comes back
# cannot be reported from inside it". The workspace now has a backend with native frames:
# `ply test --backend cranelift:wrong:exceeds-budget` over a recursion with no
# base case aborts, exit 134, in 0.02s, and `ply-cli/tests/suite/backend.rs` has
# always run `ply` as a child so the reporter was never the problem. Eight of
# eight, under `cargo test --workspace`.
#
# What holds the entry is no longer the condition. It is that deleting the crate
# deletes the only instrument for two open things: CONTRIBUTING.md item 18's 42
# unexplained agreement disagreements, and the compute-kernel record's 6.199x, which nothing
# else produces. the backend authorisation records both and says item 18 should carry the
# deletion.
declare -a KNOWN_OUTSIDE=(
  "ply-codegen-spike:its own workspace on purpose; the backend authorisation's deletion condition -- ALL EIGHT wrong backends reproduced in the workspace -- was MET on 2026-08-31 by crates/ply-codegen, so what keeps this crate is no longer the condition but the two open findings only it can measure: CONTRIBUTING.md item 18's 42 agreement disagreements and the compute-kernel record's 6.199x kernel figure. Closing item 18 is what should carry the rm -r"
)

# Tests whose assertion reads a wall clock, as `package:target:test`, where
# `target` is an integration test binary or the literal `lib` for a unit test.
#
# Each passes or fails on how much CPU it was given rather than on what the code
# does, so several test binaries at once is the wrong place for them. nextest
# runs them last and alone: `.config/nextest.toml` gives exactly these tests
# every test thread and the lowest priority, so each one starts only when the
# rest of the shard has finished and nothing else runs beside it. That file
# carries this table's filterset verbatim (`deferred-filter`), and `verify`
# fails when the two disagree, because a test that drops out of the override
# quietly goes back to running under contention. What they print is shown,
# since a measurement nobody can read is not a measurement. Names are matched
# exactly, so a unit test is named by its full module path.
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
#     (`crates/ply-cli/tests/suite/w3_http_audit.rs:714`) — *"four times the escapes
#     cost 1655.9ms against 143.6ms for k, which is 11.5x"*, against
#     `four <= one * 9.0`. The second survey missed it too: the test reads no
#     Rust clock at all, it parses milliseconds out of `ply test`'s own output
#     via a `duration_of` helper, so no timing vocabulary appears in it. Run
#     alone it passes three times out of three at load 20.
#
# The list is never finished. When a shard goes red on a ratio or a budget, the fix is usually
# another row here — `payload::tests::the_map_rows_survive_subtracting_the_fold_around_them` is
# the most recent, and it is the third survey's blind spot: it subtracts a scaffold from a
# measurement and asserts the remainder is positive, so contention does not slow it down, it
# makes the answer negative.
DEFERRED=(
  "ply-eval:allocation:region_arena_cost::snapshot_cost_as_a_function_of_region_size"
  "ply-eval:allocation:fixture_open_cost::a_seeded_fixture_opens_per_test_in_microseconds"
  "ply-eval:suite:simulation::a_long_sleep_is_a_jump"
  "ply-test:suite:region_fixture_cost::a_region_scoped_fixture_costs_the_fixture_and_never_the_test"
  "ply-test:suite:region_fixture_cost::discarding_a_tests_own_cells_costs_nothing"
  "ply-test:suite:region_fixture_cost::a_group_amortizes_the_build_up_to_a_ceiling_the_open_decides"
  "ply-test:suite:region_fixture_cost::a_group_with_no_fixture_opens_and_closes_in_constant_time"
  "ply-corpus:lib:measure::tests::every_resumption_costs_about_what_the_first_one_did"
  "ply-corpus:lib:measure::tests::capture_and_resume_are_flat_in_the_frames_they_move"
  "ply-corpus:lib:measure::tests::opening_a_fixture_beats_rebuilding_it_once_the_fixture_is_real"
  "ply-store:lib:tests::opening_a_ten_thousand_definition_cache_is_under_the_budget"
  "ply-store:lib:tests::a_baseline_for_every_test_does_not_slow_the_open"
  "ply-cli:suite:w3_http_audit::routing_a_path_of_escapes_costs_its_length_and_not_its_square"
  "ply-corpus:lib:payload::tests::the_map_rows_survive_subtracting_the_fold_around_them"
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
  "ply-lexer:it HAS a differential and a run.sh and it is BROKEN -- as of 2026-08-30 \`spikes/ply-lexer/run.sh\` does not reach a single test because its harness does not compile: \`non-exhaustive patterns: &ply_syntax::lexer::TokenKind::Question not covered\` at src/lib.rs:66, the identical bit-rot the parser spike one directory over was just repaired for. It is cited by the self-hosting spike, the bootstrap goal and the call-ceiling decision for throughput figures that cannot currently be re-taken. Adding a job here would only report a red that is already known; the entry exists so that the red is written down where CI is configured rather than only in a session transcript. Fix the spike, then move it to SPIKE_JOBS"
  "ply-lexer-nesting:three files -- main.ply, nesting.ply, bench.sh -- and no harness, no fixtures and no differential. It measures how deep a fold nests; its output is a number for the call-ceiling decision, not a pass or a fail"
  "ply-lexer-rc:same shape -- main.ply, fieldorder.ply, bench.sh. It measures what building a container anywhere but last in a record literal costs, which is spikes/ply-lexer/GAPS.md 1's measurement. A number, not a check"
  "ply-lexer-throughput:same shape -- main.ply, lexer.ply, bench.sh. It measures tokens per second for the self-hosting spike. A number, not a check"
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

# The `-p` arguments for one build that covers every deferred test. The
# timing job used to run `cargo test -p <package>` once per entry; cargo
# resolves features over *the selected packages*, so each entry got its own
# resolution and the tree recompiled between them. One selection is one
# resolution, so this is what keeps the build to one.
# One `(binary_id(=…) & test(=…))` term per deferred test, joined with `|`.
# Exact matchers on both sides: `binary_id(ply-corpus)` would also match
# `ply-corpus::suite`, and a substring on the test name would widen the set the
# moment somebody names a test after another.
cmd_deferred_filter() {
  local package target test id first=1
  while read -r package target test; do
    if [[ $target == lib ]]; then
      id=$package
    else
      id="$package::$target"
    fi
    ((first)) || printf ' | '
    first=0
    printf '(binary_id(=%s) & test(=%s))' "$id" "$test"
  done < <(cmd_deferred)
  printf '\n'
}
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
      file=$(test_source_file "$package" "$target" "$test")
      leaf=${test##*::}
      if [[ ! -f $file ]]; then
        echo "FAIL: deferred test '$test' names $file, which does not exist" >&2
        failures=$((failures + 1))
      elif ! grep -q "fn $leaf(" "$file"; then
        echo "FAIL: $file has no 'fn $leaf(' — the shards skip a name nothing defines" >&2
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
    file=$(test_source_file "$package" "$target" "$test")
    leaf=${test##*::}
    if [[ ! -f $file ]]; then
      echo "FAIL: tree check '$test' names $file, which does not exist" >&2
      failures=$((failures + 1))
    elif ! grep -q "fn $leaf(" "$file"; then
      echo "FAIL: $file has no 'fn $leaf(' — CI asserts a check nothing defines" >&2
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
  local nextest="$root/.config/nextest.toml" filter
  filter=$(cmd_deferred_filter)
  if [[ ! -f $nextest ]]; then
    echo "FAIL: no $nextest, so nothing runs the deferred tests alone" >&2
    failures=$((failures + 1))
  elif [[ $(grep -cxF "filter = '$filter'" "$nextest") -ne 1 ]]; then
    echo "FAIL: $nextest does not carry this table's deferred filter exactly once; paste the output of 'ci-shards.sh deferred-filter' into the override, single-quoted, on one line" >&2
    failures=$((failures + 1))
  fi
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
  deferred) cmd_deferred ;;
  deferred-filter) cmd_deferred_filter ;;
  tree-checks) cmd_tree_checks ;;
  *)
    echo "usage: ci-shards.sh {verify|matrix|packages ID|deferred|deferred-filter|tree-checks}" >&2
    exit 2
    ;;
esac
