#!/usr/bin/env bash

set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

NEXTEST_SHARDS=2

TAB=$'\t'

CLI_PARTS=1
IN_PROCESS_SUITE=tests/in_process
RUNS_ALONE=(package-compiler package-in_process)
NIGHTLY=(cli-tests:repository_audit cli-tests:copies in-process:edit_sessions cli-tests:edit_matrix std-tests:db_driver std-tests:pg_client)
RUNS_MARGIN=4
HOST_PACKAGES=(ply-host-tests)

SUPERSEDED=(ply-stores-)

NEWEST=(nextest-archive- ply-runtime- ply-built- rust-passed- ply-c-stage-sources- ply-c-runs-)

PER_JOB=(ply-stores- ply-c-runs-)

FIXED=(tcc-)

ACTIONS=(
  actions/cache
  actions/checkout
  actions/download-artifact
  actions/upload-artifact
  dtolnay/rust-toolchain
  "Swatinem/rust-cache:v0-rust-"
  taiki-e/install-action
)

TOOLCHAIN_WORKSPACE=ply.work
TOOLCHAIN_BOOTSTRAP=packages/bootstrap


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
  STD_TESTS=$(role std-tests)
  STD_LIBRARY=$(role std)
  CLI_SUITE=$(role cli-tests)
  PACKAGE_SUITES=()
  for dir in $(role compiler) $(role cli) $(role package) $(role suite); do
    PACKAGE_SUITES+=("${dir##*/}:$dir")
  done
  PACKAGE_PROOFS=()
  for dir in $(role proved); do PACKAGE_PROOFS+=("${dir##*/}:$dir"); done
  FORMATTED_SOURCES=()
  for dir in $(role std) $(role std-tests) $(role compiler) $(role cli) $(role cli-tests) \
    $(role package) $(role suite); do
    FORMATTED_SOURCES+=("$dir")
  done
  FIXTURE_SOURCES=()
  for dir in $(role fixtures); do FIXTURE_SOURCES+=("$dir"); done
}

role() {
  if [[ $1 == in-process ]]; then printf '%s\n' "$IN_PROCESS_SUITE"; return; fi
  awk -v r="$1" '$1 == r { print $2 }' <<< "$WORK"
}

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

named_in() {
  local x=$1 id
  shift
  for id in "$@"; do [[ $id == "$x" ]] && return 0; done
  return 1
}

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

claiming_files() {
  local dirs=() dir
  for dir in $(role compiler) $(role cli) $(role cli-tests) $(role package) $(role proved); do
    dirs+=("$root/$dir")
  done
  grep -rlE --include='*.ply' '^law "|^[[:space:]]+(ensures|cost|space) ' "${dirs[@]}" 2>/dev/null |
    sed "s|^$root/||" | LC_ALL=C sort || true
}

test_names() {
  sed -nE 's/^test(\/[a-z]+)? "([^"]*)".*/\2/p' "$1"
}

modules_of() {
  local file
  for file in "$root/$1"/*.ply; do
    if grep -qE '^test(/[a-z]+)? "' "$file"; then basename "$file" .ply; fi
  done
}

runs() {
  local entry k
  printf 'stdlib\nstd-tests\nproofs\n'
  for ((k = 1; k <= CLI_PARTS; k++)); do printf 'cli-part-%d\n' "$k"; done
  for entry in "${PACKAGE_SUITES[@]}"; do printf 'package-%s\n' "${entry%%:*}"; done
  for entry in ${PACKAGE_PROOFS[@]+"${PACKAGE_PROOFS[@]}"}; do printf 'laws-%s\n' "${entry%%:*}"; done
}

job_of() {
  case "$1" in
    std-tests) printf 'stdlib\n' ;;
    package-* | laws-*)
      if named_in "$1" "${RUNS_ALONE[@]}"; then printf '%s\n' "$1"; else printf 'packages\n'; fi
      ;;
    *) printf '%s\n' "$1" ;;
  esac
}

jobs() {
  local id
  runs | while read -r id; do job_of "$id"; done | awk '!seen[$0]++'
}

cmd_runs_matrix() { jobs | jq -Rnc '{include: [inputs | {id: .}]}'; }

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

nightly_of() {
  local entry
  for entry in "${NIGHTLY[@]}"; do
    if [[ ${entry%%:*} == "$1" ]]; then printf '%s\n' "${entry#*:}"; fi
  done
}

suite_filters() {
  local role=$1 path=$2 module nightly
  nightly=$(nightly_of "$role")
  [[ -n $nightly ]] || return 0
  for module in $(modules_of "$path"); do
    grep -qxF -- "$module" <<< "$nightly" || printf '%s. ' "$module"
  done
}

cmd_run_line() {
  local id=$1 path part
  case "$id" in
    stdlib | proofs) printf '%s\t\t\n' "$STD_LIBRARY" ;;
    std-tests) printf '%s\t%s\t\n' "$STD_TESTS" "$(suite_filters std-tests "$STD_TESTS")" ;;
    cli-part-*)
      part=${id#cli-part-}
      if ! [[ $part =~ ^[1-9][0-9]*$ ]] || ((part > CLI_PARTS)); then
        echo "the CLI's suite runs in $CLI_PARTS parts, and '$id' is none of them" >&2
        return 1
      fi
      printf '%s\t%s\t%d/%d\n' "$CLI_SUITE" "$(suite_filters cli-tests "$CLI_SUITE")" "$part" "$CLI_PARTS"
      ;;
    package-*)
      path=$(package_path "${id#package-}") || { echo "no package suite named '${id#package-}'" >&2; return 1; }
      if [[ $path == "$IN_PROCESS_SUITE" ]]; then
        printf '%s\t%s\t\n' "$path" "$(suite_filters in-process "$path")"
      else
        printf '%s\t\t\n' "$path"
      fi
      ;;
    laws-*)
      path=$(proofs_path "${id#laws-}") || { echo "no package's proofs named '${id#laws-}'" >&2; return 1; }
      printf '%s\t\t\n' "$path"
      ;;
    cli-*)
      [[ -f "$root/$CLI_SUITE/${id#cli-}.ply" ]] || { echo "the CLI's suite has no module '${id#cli-}'" >&2; return 1; }
      printf '%s\t%s.\t\n' "$CLI_SUITE" "${id#cli-}"
      ;;
    in-process-*)
      [[ -f "$root/$IN_PROCESS_SUITE/${id#in-process-}.ply" ]] || { echo "the in-process suite has no module '${id#in-process-}'" >&2; return 1; }
      printf '%s\t%s.\t\n' "$IN_PROCESS_SUITE" "${id#in-process-}"
      ;;
    *)
      echo "no run named '$id'" >&2
      return 1
      ;;
  esac
}

job_running() {
  awk -v c="$1" '
    /^  [a-z-]+:$/ { job = $1; sub(/:$/, "", job) }
    index($0, c) { print job; exit }
  ' "$root/.github/workflows/ci.yml"
}

job_block() {
  awk -v j="  $1:" '$0 == j {f = 1; next} f && /^  [a-z]/ {exit} f' "$root/.github/workflows/ci.yml"
}

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

ci_files() {
  find "$root/.github/workflows" "$root/.github/actions" -type f \( -name '*.yml' -o -name '*.yaml' \) |
    LC_ALL=C sort
}

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
    function note(kind, value, key) {
      key = literal(value)
      if (key == "") return
      steplit[++nlit] = key
      if (kind == "save") {
        jobsaved[key] = FILENAME ":" FNR
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
        if (key in jobsaved) readback[++nreadback] = FILENAME ":" FNR " restores \"" key "\", which its job saved at " jobsaved[key]
      }
    }
    function flush(i) {
      for (i = 1; i <= nlit; i++) {
        if (mode == "save") savepath[steplit[i]] = steppath
        else if (mode == "restore") { rlit[++nr] = steplit[i]; rpath[nr] = steppath; rwhere[nr] = stepwhere }
      }
      if (mode == "restore" && stepfail && steprun) needed[++nneeded] = stepwhere
      nlit = 0; steppath = ""; inpath = 0; stepfail = 0; steprun = 0
    }
    FNR == 1 { flush(); mode = ""; inkeys = 0; indent = 0; split("", jobsaved) }
    /^  [A-Za-z0-9_-]+:[[:space:]]*$/ { flush(); mode = ""; inkeys = 0; split("", jobsaved); next }
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
      for (i = 1; i <= n; i++) {
        if (!(order[i] in run_scoped)) continue
        ok = 0
        for (r in late) if (index(order[i], r) == 1) { ok = 1; break }
        if (!ok) {
          printf "FAIL: %s writes run-scoped cache key \"%s\", which no later run reads: what a job hands the jobs of its own run goes as an artifact\n", where[order[i]], order[i] > "/dev/stderr"
          bad = 1
        }
      }
      for (i = 1; i <= nneeded; i++) {
        printf "FAIL: %s restores a key that names the run with fail-on-cache-miss: what a job needs from another job of its run goes as an artifact\n", needed[i] > "/dev/stderr"
        bad = 1
      }
      for (i = 1; i <= nreadback; i++) {
        printf "FAIL: %s: the cache does not always show a key at once after a save\n", readback[i] > "/dev/stderr"
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

cmd_c_compiler() {
  if [[ ${PLY_CI_COLD:-} == true ]]; then echo cc; else echo clang; fi
}

toolchain_inputs() {
  local scope=${1:-all} all prev dir rel file target
  all=$({
    printf '%s\n' "$TOOLCHAIN_WORKSPACE" "$TOOLCHAIN_BOOTSTRAP"
    [[ $scope == core ]] || role std
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
        if [[ $scope == core ]]; then
          embeds_from <<< "$prev" | while IFS=: read -r file target; do
            covered "$target" "$prev" || printf '%s\n' "$target"
          done
        fi
      } | grep . | LC_ALL=C sort -u
    )
  done
  printf '%s\n' "$all"
}

embeds_from() {
  local path pattern file rel
  while IFS= read -r path; do
    if [ -d "$root/$path" ]; then
      pattern='embed(_dir)?\("[^"]*\.\.[^"]*"'
    elif [ -f "$root/$path" ]; then
      pattern='embed(_dir)?\("[^"]*"'
    else
      continue
    fi
    { grep -rHE --include='*.ply' "$pattern" "$root/$path" || true; } |
      { grep -vE '^[^:]*:[[:space:]]*\\\\' || true; } |
      sed -E 's/^([^:]*):.*embed(_dir)?\("([^"]*)".*/\1:\3/' |
      while IFS=: read -r file rel; do
        [[ -n $rel ]] || continue
        file=${file#"$root"/}
        printf '%s:%s\n' "$file" "$(lexical "$(dirname "$file")/$rel")"
      done
  done
}

covered() {
  local target=$1 path
  while IFS= read -r path; do
    [[ -n $path && ($target == "$path" || $target == "$path"/*) ]] && return 0
  done <<< "$2"
  return 1
}

cmd_toolchain_embeds() {
  local bad=0
  toolchain_embeds all || bad=$((bad + 1))
  toolchain_embeds core || bad=$((bad + 1))
  return $((bad > 0))
}

toolchain_embeds() {
  local scope=$1 inputs trees file target bad=0
  inputs=$(toolchain_inputs "$scope")
  trees=$(grep -vxF -- "$TOOLCHAIN_WORKSPACE" <<< "$inputs" || true)
  if [[ -z $trees ]]; then
    echo "FAIL: $TOOLCHAIN_WORKSPACE lists no part of the toolchain, so the toolchain key would cover no files" >&2
    return 1
  fi
  while IFS=: read -r file target; do
    if ! covered "$target" "$inputs"; then
      echo "FAIL: $file embeds $target, outside every path the $scope toolchain key hashes" >&2
      bad=$((bad + 1))
    elif [ ! -e "$root/$target" ]; then
      echo "FAIL: $file embeds $target, which is not in the tree" >&2
      bad=$((bad + 1))
    fi
  done < <(embeds_from <<< "$inputs")
  if [ "$bad" -eq 0 ]; then
    printf 'toolchain key (%s): the %d paths that a bootstrap reads Ply from contain every file they embed\n' "$scope" "$(grep -c . <<< "$inputs")"
  fi
  return $((bad > 0))
}

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
      # shellcheck disable=SC2053
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

families_of() {
  local id
  if named_in "$1" ${PER_JOB[@]+"${PER_JOB[@]}"}; then
    jobs | while read -r id; do printf '%s%s- ' "$1" "$id"; done
  else
    printf '%s ' "$1"
  fi
}

prune_plan() {
  LC_ALL=C sort -t "$TAB" -k2,2r | awk -F'\t' -v OFS='\t' -v run="$1" -v superseded="$2" -v newest="$3" -v fixed="$4" '
    function named(key, list,   p, i, m) {
      m = split(list, p, " ")
      for (i = 1; i <= m; i++) if (index(key, p[i]) == 1) return 1
      return 0
    }
    function family(key) {
      while (match(key, /-[0-9a-f]+$/) && RLENGTH > 8) key = substr(key, 1, RSTART - 1)
      return key
    }
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

cmd_prune_check() {
  local a b got want
  a=$(printf 'a%.0s' {1..40})
  b=$(printf 'b%.0s' {1..40})
  want="1 superseded;4 unnamed;7 superseded;11 superseded;12 unnamed;13 superseded;17 unnamed;19 superseded;"
  got=$(printf '%s\t%s\t%s\n' \
    1 2026-01-01T00:00:00Z ply-stores-stdlib-100 \
    2 2026-01-02T00:00:00Z "ply-stores-stdlib-$b-200" \
    3 2026-01-01T00:00:00Z ply-stores-proofs-100 \
    4 2026-01-01T00:00:00Z ply-stores-cli-part-3-100 \
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
    prune_plan 200 "ply-stores-stdlib- ply-stores-proofs-" "ply-built- ply-c-runs-stdlib- v0-rust-" "tcc-" |
    cut -f1,2 | LC_ALL=C sort -n | tr '\t\n' ' ;')
  if [[ $got != "$want" ]]; then
    echo "FAIL: in its test listing, \`supersede\` would delete '$got' but should delete '$want'" >&2
    return 1
  fi
  echo "cache pruning: each rule of \`supersede\` gives the expected result on its test listing"
}

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

cmd_comments() {
  local found
  found=$(
    cd "$root"
    git grep -n -E '^[[:space:]]*(//|/\*)' -- '*.ply' '*.rs' '*.pkg' ply.work ':!packages/std' ':!tests/fixtures/lang' \
      ':!tests/programs/fmt' ':!tests/programs/doc' || true
    git grep -n -E '^[[:space:]]*(//([^/!]|$)|/\*)' -- packages/std || true
    git grep -n -E '^[[:space:]]*//' -- packages/std/internal || true
    git grep -n -E '^[[:space:]]*//' -- tests/fixtures/lang | grep -vE ':[[:space:]]*// (raises|refused|effects) .* ::' || true
    git grep -n -E '^[[:space:]]*#([^!]|$)' -- '*.sh' 'scripts/*' '.github/*.yml' '.github/*.yaml' '*.toml' \
      ':!tests' ':!examples/same-tests.sh' | grep -v '# shellcheck ' || true
    git ls-files -z 'packages/std/*.ply' ':!packages/std/internal' ':!packages/std/prelude.ply' |
      xargs -0 awk '
        FNR == 1 { doc = 0; public = 0 }
        /^\/\/\// { doc = FNR; next }
        /^[ \t]+\/\/\// { if (!public) print FILENAME ":" FNR ": a doc comment inside a private declaration"; next }
        /^[^] \t\/})]/ {
          public = ($0 ~ /^pub /)
          if (doc && !public) print FILENAME ":" doc ": a doc comment on a private definition"
          doc = 0
        }
      '
  )
  if [[ -n $found ]]; then
    echo "$found" >&2
    echo "FAIL: comments are allowed only on the public definitions of the standard library: use a clear name or a test instead" >&2
    return 1
  fi
  echo "comments: only the public definitions of the standard library have them"
}

cmd_names() {
  local retired='inst|insts|stmt|qname|ident|ctor|lhs|rhs|asked|judged|kept|keeps' found
  found=$(
    cd "$root"
    git grep -n -w -E "$retired" -- '*.ply' ':!tests/fixtures' ':!tests/programs' |
      grep -v -E '^[^:]+:[0-9]+:[[:space:]]*(\\\\|///|//!|(test(/[a-z]+)?|law) ")' |
      sed -E 's/"([^"\\]|\\.)*"//g' | grep -w -E "$retired" || true
  )
  if [[ -n $found ]]; then
    echo "$found" >&2
    echo "FAIL: these names are retired: use the full word, or a name that says what the value is" >&2
    return 1
  fi
  echo "names: no retired name is back"
}

cmd_verify() {
  local failures=0 member package entry id dir
  local workflow="$root/.github/workflows/ci.yml"

  if ! members >/dev/null; then
    return 1
  fi
  local -a all_members=()
  while read -r member; do all_members+=("$member"); done < <(members)

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
  local allow
  while read -r allow; do
    echo "FAIL: $allow silences dead-code warnings — delete what nothing calls instead" >&2
    failures=$((failures + 1))
  done < <(grep -rn -E "allow\((dead_code|unused)\)" --include=*.rs "$root/crates" | sed "s|^$root/||" || true)
  local held
  while read -r held; do
    echo "FAIL: $held is a test in a crate's sources, which never runs: move it to the crate's \`-tests\` package" >&2
    failures=$((failures + 1))
  done < <(grep -rn -E "^[[:space:]]*#\[cfg\(test\)\]" --include=*.rs "$root"/crates/*/src | sed "s|^$root/||" || true)

  for package in "${HOST_PACKAGES[@]}"; do
    if ! printf '%s\n' "${all_members[@]}" | grep -qx "$package"; then
      echo "FAIL: HOST_PACKAGES names '$package', which is not a workspace member" >&2
      failures=$((failures + 1))
    fi
  done

  for dir in "$root"/crates/*/; do
    member=$(basename "$dir")
    [[ -f $dir/Cargo.toml ]] || continue
    if ! printf '%s\n' "${all_members[@]}" | grep -qx "$member"; then
      echo "FAIL: crates/$member is a crate that is no workspace member, so nothing in CI builds or tests it" >&2
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
      echo "FAIL: '$outside' is a member outside crates/, and no job runs '--workspace --all-targets', so nothing in CI compiles it" >&2
      failures=$((failures + 1))
    fi
  done < <(members_outside_crates)

  local needs
  needs=$(awk '/^  ci:/{f=1;next} f && /^  [a-z]/{exit} f' "$workflow" 2>/dev/null |
    tr '\n' ' ' | sed -n 's/.*needs: *\(\[[^]]*\]\).*/\1/p')
  if [[ -z $needs ]]; then
    echo "FAIL: could not read the \`ci\` job's \`needs:\` list out of $workflow -- every check below would pass vacuously" >&2
    failures=$((failures + 1))
  fi

  local runs_job runs_command
  if [[ ! -f "$root/$CLI_SUITE/ply.pkg" ]]; then
    echo "FAIL: $CLI_SUITE has no ply.pkg, but a run tests it as a package" >&2
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
  if ! grep -qxF -- "$IN_PROCESS_SUITE" <<< "$(role suite)"; then
    echo "FAIL: ply.work lists no suite at $IN_PROCESS_SUITE, which IN_PROCESS_SUITE names" >&2
    failures=$((failures + 1))
  fi
  local module keys outside suite
  for suite in "$CLI_SUITE" "$IN_PROCESS_SUITE"; do
    keys=$(package_keys "$suite")
    for module in $(modules_of "$suite"); do
      outside=$(grep -F -- "$module." <<< "$keys" | grep -v "^$module\." || true)
      if [[ -n $outside ]]; then
        echo "FAIL: '$module.' is part of '${outside%%$'\n'*}', outside $suite/$module.ply, so the module's filter picks that test too" >&2
        failures=$((failures + 1))
      fi
    done
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
  for entry in "${NIGHTLY[@]}"; do
    module="$(role "${entry%%:*}")/${entry#*:}"
    if [[ -z $(test_names "$root/$module.ply" 2>/dev/null || true) ]]; then
      echo "FAIL: $module.ply declares no test, and NIGHTLY leaves it to the nightly job" >&2
      failures=$((failures + 1))
    fi
  done
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
  local limit budget
  runs_job=$(job_running "ci-ply.sh run")
  if [[ -n $runs_job ]]; then
    limit=$(job_block "$runs_job" | sed -n 's/^    timeout-minutes: *\([0-9][0-9]*\)$/\1/p')
    budget=$(deadline_of "$runs_job")
    if [[ -z $budget ]]; then
      echo "FAIL: job '$runs_job' runs \`ci-ply.sh run\` and sets no PLY_CI_DEADLINE, so its limit cancels it with nothing cached" >&2
      failures=$((failures + 1))
    elif [[ -z $limit ]] || ((budget > limit - RUNS_MARGIN)); then
      echo "FAIL: job '$runs_job' ends its runs ${budget} minutes in, which leaves less than $RUNS_MARGIN of its ${limit:-unset} for the steps that keep what they wrote" >&2
      failures=$((failures + 1))
    fi
  fi

  local sweep_rs="$root/crates/ply-compiled/src/c/sweep.rs" swept cut
  swept=$(sed -n 's/^const BY_FILE: \[&str; [0-9]*\] = \[\(.*\)\];$/\1/p' "$sweep_rs" | tr -d ' ' | tr ',' '\n' |
    while read -r name; do sed -n "s/^pub const $name: &str = \"\\(.*\\)\";$/\\1/p" "$sweep_rs"; done | LC_ALL=C sort)
  cut=$(sed -n 's/^ *\([a-z| ]*\)) find "\$dir" -type f ! -newer "\$mark" -delete ;;$/\1/p' "$root/.github/ci-warm.sh" |
    tr -d ' ' | tr '|' '\n' | LC_ALL=C sort)
  if [[ -z $swept || $swept != "$cut" ]]; then
    echo "FAIL: the sweep takes '$(echo $swept)' file by file and ci-warm.sh cuts '$(echo $cut)' file by file" >&2
    failures=$((failures + 1))
  fi
  local stores_rs stores_warm stores_ply
  stores_rs=$(sed -n 's/^pub const STORES: &str = "\(.*\)";$/\1/p' "$sweep_rs")
  stores_warm=$(sed -n 's/^stores=\$cache\/\(.*\)$/\1/p' "$root/.github/ci-warm.sh")
  stores_ply=$(sed -n 's/^stores=ply-c-cache\/\(.*\)$/\1/p' "$root/.github/ci-ply.sh")
  if [[ -z $stores_rs || $stores_rs != "$stores_warm" || $stores_rs != "$stores_ply" ]]; then
    echo "FAIL: the sweep takes the stores below '$stores_rs' whole, ci-warm.sh those below '$stores_warm' and ci-ply.sh those below '$stores_ply'" >&2
    failures=$((failures + 1))
  fi

  cmd_cache_keys || failures=$((failures + 1))
  cmd_prune_check || failures=$((failures + 1))
  cmd_actions || failures=$((failures + 1))
  cmd_toolchain_embeds || failures=$((failures + 1))
  cmd_comments || failures=$((failures + 1))
  cmd_names || failures=$((failures + 1))

  if [[ $failures -gt 0 ]]; then
    echo "$failures problem(s) in the CI tables" >&2
    return 1
  fi
  echo "${#all_members[@]} members under crates/ (plus $(members_outside_crates | grep -c . || true) outside); $(grep -c . <<< "$listed") Ply runs in $(jobs | grep -c .) jobs; $NEXTEST_SHARDS nextest shards"
}

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
  c-compiler) cmd_c_compiler ;;
  work) role "${2:?a role}" ;;
  store-paths) cmd_store_paths ;;
  ply-trees) cmd_ply_trees ;;
  nextest-shards) cmd_nextest_shards ;;
  durations) cmd_durations "${2:?a nextest JUnit report}" ;;
  runs-matrix) cmd_runs_matrix ;;
  job-runs) cmd_job_runs "${2:?a job}" ;;
  run-line) cmd_run_line "${2:?a run}" ;;
  host-filter) cmd_host_filter ;;
  formatted-sources) printf '%s\n' "${FORMATTED_SOURCES[@]}" ;;
  rust-inputs) cmd_rust_inputs "${2:-}" ;;
  supersede) cmd_supersede "${2:?a run id}" "${3:?a ref}" ;;
  nightly-modules) nightly_of "${2:?a role}" | paste -sd ' ' - ;;
  in-process-suite) printf '%s\n' "$IN_PROCESS_SUITE" ;;
  *)
    echo "usage: ci-shards.sh {verify|cache-keys|toolchain-inputs [core]|c-compiler|work ROLE|store-paths|ply-trees|nextest-shards|durations FILE|runs-matrix|job-runs ID|run-line ID|host-filter|formatted-sources|rust-inputs DEPINFO|supersede RUN REF|nightly-modules|in-process-suite}" >&2
    exit 2
    ;;
esac
