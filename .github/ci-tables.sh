#!/usr/bin/env bash
# The tables CI's job is run from, and the check that they name what the tree holds.
#
#   ci-tables.sh verify          every crate is a member, every test named here exists, every
#                                `probes/` directory is run by the workflow, every Ply package
#                                with a law is proved, and every committed `.ply` is a maintained
#                                source or a fixture
#   ci-tables.sh runs            every Ply run, one a line, which `ci-corpus.sh all` runs
#   ci-tables.sh line ID         `path<TAB>filters` of a run, or of one module of the corpus's
#                                checks (`<module>`) or of the CLI's suite (`cli-<module>`): the
#                                package it tests and the substrings of the tests' qualified names
#                                `ply test --filter` picks its tests by, none for every test
#   ci-tables.sh host-filter     the nextest filterset selecting the host packages
#   ci-tables.sh tree-checks     one `package target test` line per tree check
#   ci-tables.sh formatted-sources
#                                the maintained Ply sources, one a line, which the job holds to
#                                `ply fmt --check`
#   ci-tables.sh sweep-parts EVENT
#                                the parts `K/N` of the edit sweep the day names under `schedule`
#                                or `workflow_dispatch`, SWEEP_PARTS of them, so SWEEP_DAYS days
#                                sweep every case; none otherwise, since `edit_gate` holds a pull
#                                request to every kind of edit
#   ci-tables.sh toolchain-key   a digest of what `ply bootstrap` reads of this checkout: the
#                                runtime's library as cargo built it, and each file of the Ply
#                                packages `ply` is built from. A `ply` built under it is the one a
#                                bootstrap would build now

set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
workflow="$root/.github/workflows/ci.yml"

TAB=$'\t'

# The Ply tests. Every check spawns `ply`, and the CLI's suite drives `ply` in one working directory,
# so a `ply test` takes either's tests one after another.
CORPUS_PROGRAM=crates/ply-corpus/ply
# The programs the corpus program runs, each a `ply test` of its own whose tests must pass.
CORPUS_FIXTURES=crates/ply-corpus/fixtures
CORPUS_CHECKS=crates/ply-corpus/checks
# The fixtures the standard library's tests read, as a project: `--std` over it tests and proves every
# module of the library beside its own.
CORPUS_STDLIB=crates/ply-corpus/stdlib
STD_LIBRARY=crates/ply-std/ply
CLI_SUITE=crates/ply-cli-tests/ply
# Modules of the CLI's suite only the nightly run takes. CLI_SWEPT edits each module of the tree one
# way and checks it warm and cold, days of a machine over the tree, so a night takes a part of its
# cases. The rest run whole each night: minutes each over the toolchain's whole trees, the reach
# audit checking three packages cold and the copies audit listing ten, or against a postgres they
# start.
CLI_NIGHTLY=(edit_sweep phases_whole reached copies db_in_ply db_wire pg_client)
CLI_SWEPT=edit_sweep
SWEEP_PARTS=16
SWEEP_DAYS=14
# Packages whose own suites are runs too, as `id:path`, the run `package-<id>`.
PACKAGE_SUITES=(
  "cli:crates/ply-cli/ply"
  "compiler:crates/ply-compiler/ply"
  "kernel:benches/kernel"
  "lang:tests/lang"
  "registry:crates/ply-registry/ply"
  "prove:crates/ply-prove/ply"
  "sim:crates/ply-sim/ply"
  "store:crates/ply-store/ply"
  "suite:crates/ply-test/ply"
)
# Packages whose laws and contracts a `ply prove` of their own discharges, as `id:path`, the run
# `laws-<id>`: the library's are the `proofs` run's, and a law in any other package fails `verify`.
PACKAGE_PROOFS=()

# The source that says what a bootstrap reads of a checkout (`toolchain_inputs`), and the runtime's
# library it links, as cargo builds it for the `ply` the job tests.
TOOLCHAIN_BOOTSTRAP=crates/ply-cli/ply/bootstrap.ply
TOOLCHAIN_LIBRARY=target/release/libply_native.a

# The packages whose tests bind sockets and processes, which run after every other Rust test.
HOST_PACKAGES=(ply-host-tests)

# Crate directories that are deliberately not workspace members, as `name:why`.
# Expanded as ${KNOWN_OUTSIDE[@]+...}: bash 3.2 treats an empty array as unset under `set -u`.
declare -a KNOWN_OUTSIDE=(
)

# Checks on the tree, as `package:target:test` (`target` is `lib` for a unit test, named by full
# module path). The job asserts each ran, since a check that stops running reports nothing.
TREE_CHECKS=(
  "ply-eval-tests:suite:armed::every_registered_code_is_constructed_in_production"
  "ply-eval-tests:suite:armed::every_variant_of_a_covered_enum_is_constructed_in_production"
  "ply-eval-tests:suite:armed::every_diagnostic_constructor_call_names_its_code_literally"
  "ply-eval-tests:suite:armed::every_code_declared_or_raised_has_one_row_in_the_registry"
  "ply-eval-tests:suite:armed::the_registry_has_no_row_for_a_code_nothing_declares_or_raises"
  "ply-eval-tests:suite:armed::no_allowlist_entry_has_outlived_its_reason"
  "ply-eval-tests:suite:armed::ambiguous_enum_names_are_declared"
)

# Checks on the tree in the CLI's suite, as `module:test`. The table asserts each is still declared
# there: a check that stops being declared reports nothing.
CLI_TREE_CHECKS=(
  "copies:no two modules define the same function"
  "fixture_list:every fixture is listed"
  "tree:the harness is the only module that starts the \`ply\` binary"
)

# The Ply sources the repository maintains, which the job holds to `ply fmt --check`: a source
# committed unformatted puts its reformatting into the next change that touches it. Every committed
# `.ply` is under one of these or of the fixtures, whose spelling is their point (`verify`).
FORMATTED_SOURCES=(
  benches
  crates/ply-cli-tests/ply
  crates/ply-cli/ply
  crates/ply-compiler/ply
  crates/ply-compiler/prelude.ply
  crates/ply-corpus/checks
  crates/ply-corpus/ply
  crates/ply-corpus/stdlib
  crates/ply-prove/ply
  crates/ply-registry/ply
  crates/ply-sim/ply
  crates/ply-std/ply
  crates/ply-store/ply
  crates/ply-test/ply
  examples
  tests/lang
)
FIXTURE_SOURCES=(
  crates/ply-cli-tests/phases
  crates/ply-codegen-tests/tests
  crates/ply-corpus/fixtures
  tests/fixtures
)

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

cmd_tree_checks() {
  local entry rest
  for entry in "${TREE_CHECKS[@]}"; do
    rest=${entry#*:}
    printf '%s %s %s\n' "${entry%%:*}" "${rest%%:*}" "${rest#*:}"
  done
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

named_in() {
  local x=$1 id
  shift
  for id in "$@"; do [[ $id == "$x" ]] && return 0; done
  return 1
}

# The labels of a module's tests, in the order it declares them.
test_names() {
  sed -nE 's/^test(\/[a-z]+)? "([^"]*)".*/\2/p' "$1"
}

# Each module of the package at DIR that declares a test.
modules_of() {
  local file
  for file in "$root/$1"/*.ply; do
    grep -qE '^test(/[a-z]+)? "' "$file" || continue
    basename "$file" .ply
  done
}

# The qualified name of every test the package at DIR declares, `<module>.<label>`.
package_keys() {
  local file
  for file in "$root/$1"/*.ply; do
    test_names "$file" | sed "s/^/$(basename "$file" .ply)./"
  done
}

# The path of the entry `id` of a table of `id:path` entries.
path_in() {
  local id=$1 entry
  shift
  for entry in "$@"; do
    [[ ${entry%%:*} == "$id" ]] && { printf '%s\n' "${entry#*:}"; return 0; }
  done
  return 1
}

# Each `.ply` of a package below `crates/` other than the library's that states a law or an `ensures`
# or `cost` clause, which only a `ply prove` discharges.
claiming_files() {
  grep -rlE --include='*.ply' '^law "|^[[:space:]]+(ensures|cost) ' "$root"/crates/*/ply 2>/dev/null |
    grep -v "^$root/$STD_LIBRARY/" | sed "s|^$root/||" | LC_ALL=C sort || true
}

# The longest first, which is the order `ci-corpus.sh all` starts them in.
cmd_runs() {
  local entry file
  printf 'checks\ncli\nstdlib\nproofs\n'
  for entry in ${PACKAGE_PROOFS[@]+"${PACKAGE_PROOFS[@]}"}; do printf 'laws-%s\n' "${entry%%:*}"; done
  for entry in "${PACKAGE_SUITES[@]}"; do printf 'package-%s\n' "${entry%%:*}"; done
  printf 'program\n'
  for file in "$root/$CORPUS_FIXTURES"/*.ply; do printf 'fixture-%s\n' "$(basename "$file" .ply)"; done
}

# `<module>.` per module of the package at DIR that declares a test and is none of the rest of the
# arguments, on one line.
filters_but() {
  local dir=$1 module
  shift
  for module in $(modules_of "$dir"); do
    named_in "$module" "$@" || printf '%s. ' "$module"
  done
}

cmd_line() {
  local id=$1 module path
  case "$id" in
    program) printf '%s\t\n' "$CORPUS_PROGRAM" ;;
    checks) printf '%s\t\n' "$CORPUS_CHECKS" ;;
    stdlib | proofs) printf '%s\t\n' "$CORPUS_STDLIB" ;;
    cli) printf '%s\t%s\n' "$CLI_SUITE" "$(filters_but "$CLI_SUITE" "${CLI_NIGHTLY[@]}")" ;;
    sweep) printf '%s\t%s.\n' "$CLI_SUITE" "$CLI_SWEPT" ;;
    nightly) printf '%s\t%s\n' "$CLI_SUITE" "$(for module in "${CLI_NIGHTLY[@]}"; do [[ $module == "$CLI_SWEPT" ]] || printf '%s. ' "$module"; done)" ;;
    fixture-*)
      [[ -f "$root/$CORPUS_FIXTURES/${id#fixture-}.ply" ]] || { echo "no fixture named '${id#fixture-}'" >&2; return 1; }
      printf '%s/%s.ply\t\n' "$CORPUS_FIXTURES" "${id#fixture-}"
      ;;
    package-*)
      path=$(path_in "${id#package-}" "${PACKAGE_SUITES[@]}") || { echo "no package suite named '${id#package-}'" >&2; return 1; }
      printf '%s\t\n' "$path"
      ;;
    laws-*)
      path=$(path_in "${id#laws-}" ${PACKAGE_PROOFS[@]+"${PACKAGE_PROOFS[@]}"}) || { echo "no package's proofs named '${id#laws-}'" >&2; return 1; }
      printf '%s\t\n' "$path"
      ;;
    cli-*)
      module=${id#cli-}
      [[ -f "$root/$CLI_SUITE/$module.ply" ]] || { echo "the CLI's suite has no module '$module'" >&2; return 1; }
      printf '%s\t%s.\n' "$CLI_SUITE" "$module"
      ;;
    *)
      [[ -f "$root/$CORPUS_CHECKS/$id.ply" ]] || { echo "no run named '$id'" >&2; return 1; }
      printf '%s\t%s.\n' "$CORPUS_CHECKS" "$id"
      ;;
  esac
}

cmd_sweep_parts() {
  local event=$1 of=$((SWEEP_PARTS * SWEEP_DAYS)) day k
  [[ $event == schedule || $event == workflow_dispatch ]] || return 0
  day=$(($(date -u +%s) / 86400 % SWEEP_DAYS))
  for ((k = 1; k <= SWEEP_PARTS; k++)); do printf '%d/%d\n' $((day * SWEEP_PARTS + k)) "$of"; done
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

# Each path of the repository a bootstrap reads Ply from, one a line: every path under `crates/` its
# source names, and every package the manifest of one of those names by path, and theirs.
toolchain_inputs() {
  local all prev dir rel
  all=$(grep -oE '"/?crates/[^"]*"' "$root/$TOOLCHAIN_BOOTSTRAP" | tr -d '"' | sed 's|^/||' | LC_ALL=C sort -u)
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

# A change that moves none of what this hashes bootstraps nothing: the files are the checkout's own,
# as git lists them, so a store or a cache a run left in a package's tree is no part of it.
cmd_toolchain_key() {
  local inputs
  inputs=$(toolchain_inputs)
  if [[ -z $inputs ]]; then
    echo "FAIL: $TOOLCHAIN_BOOTSTRAP names no path under crates/, so the toolchain key is of nothing" >&2
    return 1
  fi
  if [[ ! -f $root/$TOOLCHAIN_LIBRARY ]]; then
    echo "FAIL: no $TOOLCHAIN_LIBRARY: build the runtime's library before asking for the key" >&2
    return 1
  fi
  local -a sha=(sha256sum)
  command -v sha256sum > /dev/null || sha=(shasum -a 256)
  {
    "${sha[@]}" < "$root/$TOOLCHAIN_LIBRARY"
    # shellcheck disable=SC2086
    (cd "$root" && git ls-files -z -- $inputs | LC_ALL=C sort -z | xargs -0 "${sha[@]}")
  } | "${sha[@]}" | cut -c1-40
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
  local failures=0 member package entry candidate target test seen dir note
  local -a known=() all_members=()

  members >/dev/null || return 1
  while read -r member; do all_members+=("$member"); done < <(members)
  if [[ ! -f $workflow ]]; then
    echo "FAIL: no workflow at $workflow" >&2
    return 1
  fi

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

  local outside
  while read -r outside; do
    [[ -n $outside ]] || continue
    if [[ ! -f "$root/$outside/Cargo.toml" ]]; then
      echo "FAIL: the workspace names '$outside', which has no Cargo.toml" >&2
      failures=$((failures + 1))
      continue
    fi
    if ! grep -q -- "--workspace --all-targets" "$workflow"; then
      echo "FAIL: '$outside' is a member outside crates/, and no step runs '--workspace --all-targets', so nothing in CI compiles it" >&2
      failures=$((failures + 1))
    fi
  done < <(members_outside_crates)

  # --- tests named by a table ----------------------------------------------
  while read -r package target test; do
    check_test_exists "tree check" "$package" "$target" "$test" || failures=$((failures + 1))
  done < <(cmd_tree_checks)

  # --- probes ---------------------------------------------------------------
  local probe
  if [[ -d "$root/probes" ]]; then
    for dir in "$root"/probes/*/; do
      probe=$(basename "$dir")
      if [[ ! -x "$dir/run.sh" ]]; then
        echo "FAIL: probes/$probe has no executable run.sh" >&2
        failures=$((failures + 1))
      elif ! grep -q "probes/$probe/run\.sh" "$workflow"; then
        echo "FAIL: no step of $workflow runs probes/$probe/run.sh, so nothing in CI runs it" >&2
        failures=$((failures + 1))
      fi
    done
  fi

  # --- the Ply tests ----------------------------------------------------------
  for dir in "$CORPUS_PROGRAM" "$CORPUS_CHECKS" "$CLI_SUITE"; do
    if [[ ! -f "$root/$dir/ply.pkg" ]]; then
      echo "FAIL: $dir holds no ply.pkg, and a run tests it as a package" >&2
      failures=$((failures + 1))
    fi
  done
  if ! grep -q 'ci-corpus\.sh all' "$workflow"; then
    echo "FAIL: no step of $workflow runs \`ci-corpus.sh all\`, so the Ply runs run nowhere" >&2
    failures=$((failures + 1))
  fi
  local module keys pair
  for pair in "$CORPUS_CHECKS:" "$CLI_SUITE:${CLI_NIGHTLY[*]}"; do
    dir=${pair%%:*}
    for module in ${pair#*:}; do
      if [[ -z $(test_names "$root/$dir/$module.ply" 2>/dev/null) ]]; then
        echo "FAIL: a table names the module '$module' of $dir, which declares no test" >&2
        failures=$((failures + 1))
      fi
    done
    # A module's tests are picked out by `<module>.`, which no test of another module may hold.
    keys=$(package_keys "$dir")
    for module in $(modules_of "$dir"); do
      outside=$(grep -F -- "$module." <<< "$keys" | grep -v "^$module\." || true)
      if [[ -n $outside ]]; then
        echo "FAIL: '$module.' is part of '${outside%%$'\n'*}', outside $dir/$module.ply, so the module's run picks that test too" >&2
        failures=$((failures + 1))
      fi
    done
  done
  if ! named_in "$CLI_SWEPT" "${CLI_NIGHTLY[@]}"; then
    echo "FAIL: CLI_SWEPT names '$CLI_SWEPT', which CLI_NIGHTLY does not, so every run takes it whole" >&2
    failures=$((failures + 1))
  fi
  if [[ -z $(toolchain_inputs) ]]; then
    echo "FAIL: $TOOLCHAIN_BOOTSTRAP names no path under crates/, so the toolchain key is of nothing" >&2
    failures=$((failures + 1))
  fi
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
  local tracked under where
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
  local names
  for entry in "${CLI_TREE_CHECKS[@]}"; do
    module=${entry%%:*}
    names=$(test_names "$root/$CLI_SUITE/$module.ply" 2>/dev/null || true)
    if ! grep -qxF -- "${entry#*:}" <<< "$names"; then
      echo "FAIL: $CLI_SUITE/$module.ply declares no test '${entry#*:}', and CLI_TREE_CHECKS names it" >&2
      failures=$((failures + 1))
    fi
  done

  if [[ $failures -gt 0 ]]; then
    echo "$failures problem(s) in the CI tables" >&2
    return 1
  fi
  echo "${#all_members[@]} members under crates/ (plus $(members_outside_crates | grep -c . || true) outside); ${#KNOWN_OUTSIDE[@]} crate(s) deliberately outside; ${#TREE_CHECKS[@]} tree checks and ${#CLI_TREE_CHECKS[@]} in the CLI's suite, each present in the tree; $(cmd_runs | grep -c .) Ply runs"
}

case "${1:-}" in
  verify) cmd_verify ;;
  runs) cmd_runs ;;
  line) cmd_line "${2:?a run}" ;;
  host-filter) cmd_host_filter ;;
  tree-checks) cmd_tree_checks ;;
  formatted-sources) printf '%s\n' "${FORMATTED_SOURCES[@]}" ;;
  sweep-parts) cmd_sweep_parts "${2:?an event}" ;;
  toolchain-key) cmd_toolchain_key ;;
  *)
    echo "usage: ci-tables.sh {verify|runs|line ID|host-filter|tree-checks|formatted-sources|sweep-parts EVENT|toolchain-key}" >&2
    exit 2
    ;;
esac
