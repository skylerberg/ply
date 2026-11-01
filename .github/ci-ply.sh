#!/usr/bin/env bash
set -uo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ply="$root/target/debug/ply"
shards="$root/.github/ci-shards.sh"
work() {
  if [[ $1 == in-process ]]; then "$shards" in-process-suite; return; fi
  "$ply" work "$root" | awk -v r="$1" '$1 == r { print $2 }'
}
TAB=$'\t'
caches=/tmp
mark=$caches/ply-c-runs.mark
durations=$caches/ply-test-durations.tsv

postgres_bin() {
  local found
  found=$(command -v initdb 2>/dev/null || true)
  if [[ -n $found ]]; then dirname "$found"; return; fi
  find /usr/lib/postgresql -maxdepth 2 -name bin -type d 2>/dev/null | sort -V | tail -1
}
pg=$(postgres_bin)

cli_grants=(--timeout 900000 --json
  --exec "ply=$ply" --exec sh=/bin/sh --exec "git=$(command -v git)"
  --fs cwd=. --fs abs=/ --fs "repo=$root")
in_process_grants=("${cli_grants[@]}" --allow machine --allow shipped --allow toolchain)

deadline=${PLY_CI_DEADLINE:-}
STOPPED=124

past_deadline() { [[ -n $deadline ]] && (($(date +%s) >= deadline)); }

now_ms() { perl -MTime::HiRes=time -e 'printf "%d\n", time * 1000'; }

bounded() {
  local status=0
  [[ -n $deadline ]] || { "$@"; return; }
  past_deadline && return "$STOPPED"
  timeout --kill-after=30 "$((deadline - $(date +%s)))" "$@" || status=$?
  if ((status == 124 || status == 137)) && past_deadline; then return "$STOPPED"; fi
  return "$status"
}

stopped_run() {
  echo "::error::the run $2 was stopped at the job's deadline after $((($(now_ms) - $1) / 1000))s: what it finished is cached, and a re-run of this job starts from it"
}

reported() {
  jq -e 'type == "object"' "$1" > /dev/null 2>&1
}

unreported() {
  if (($2 > 128)); then
    echo "the run $1 crashed: signal $(($2 - 128)) ended it before it wrote a report" >&2
  else
    echo "the run $1 exited with status $2 and wrote no report" >&2
  fi
}

tested() {
  local path=$1 status=0 dir
  shift
  if [[ $path == "$(work cli-tests)" || $path == "$(work in-process)" ]]; then
    local -a grants=("${cli_grants[@]}")
    [[ $path == "$(work in-process)" ]] && grants=("${in_process_grants[@]}")
    dir=$(mktemp -d)
    touch "$dir/.ply-scratch"
    (cd "$dir" && bounded "$ply" test "$root/$path" "$@" "${grants[@]}") || status=$?
    rm -rf "$dir"
  elif [[ $path == "$(work std)" || $path == "$(work std-tests)" ]]; then
    dir=$caches/ply-std-scratch
    rm -rf "$dir" && mkdir -p "$dir" && touch "$dir/.ply-scratch"
    bounded "$ply" test "$path" --json "$@" --fs "scratch=$dir" --exec sh=/bin/sh \
      ${pg:+--exec "initdb=$pg/initdb" --exec "postgres=$pg/postgres" --exec "psql=$pg/psql"} ||
      status=$?
    rm -rf "$dir"
  else
    bounded "$ply" test "$path" --json "$@" || status=$?
  fi
  return "$status"
}

red() {
  jq -r '
    (.failures[]? | ("FAILED \(.key // .name): \(.diagnostic.code // "") \(.diagnostic.message // "")",
      (.diagnostic.notes[]? | "  \(.)"))),
    (.diagnostics[]? | ("\(.severity // "error") \(.code // ""): \(.message // "")",
      (.notes[]? | "  \(.)")))
  ' "$1" 2>/dev/null || cat "$1"
}

timed() {
  jq -r --arg p "$2" '.results[]? | "\((.duration_ms // 0) | floor)\t\($p)\t\(.key // .name)"' "$1" 2>/dev/null
}

listed() {
  jq -r '(.results // [] | map({key: (.key // .name), value: .cached}) | from_entries) as $ran
    | .selection.tests[]? | .key as $key
    | select(.reason == "cached" or ($ran | has($key)))
    | "\(.reason)\t\($key)\(if .moved then ": " + .moved else "" end)\(if $ran[$key] == false then " (unfiled)" else "" end)"' "$1" 2>/dev/null
}

summed() {
  jq -r '.summary // {} | "summary: \(.passed // 0) passed, \(.failed // 0) failed, \(.abandoned // 0) abandoned, \(.cached // 0) cached"' "$1" 2>/dev/null
}

answers='(if .front_end.queries then "; \([.front_end.queries[].taken] | add // 0) query results reused, \([.front_end.queries[].run] | add // 0) computed and \(.front_end.lost // 0) lost" else "" end)'

lost() {
  jq -s -e 'any(.[]; (.front_end.lost // 0) > 0)' "$1" > /dev/null 2>&1
}

spent() {
  jq -r --argjson wall "$2" '
    def s(ms): (ms / 1000 | floor | tostring) + "s";
    (.phases // {}) as $p
    | (if .front_end.reused then "spent: read a cached result in \(s(.front_end.phases.total // 0)); the run that cached it spent " else "spent: " end)
      + "before \(s($p.before // 0)), load \(s($p.load // 0)), promises \(s($p.promises // 0)), store \(s($p.store // 0)), selection \(s($p.selection // 0)), handoff \(s($p.handoff // 0)), unit \(s($p.unit // 0)) (C analysis \(s((.backend.analysis_nanos // 0) / 1e6)), codegen \(s((.backend.codegen_nanos // 0) / 1e6))), bind \(s($p.bind // 0)), tests \(s($p.tests // 0)), conclude \(s($p.conclude // 0)), other \(s([$wall - ($p.total // 0), 0] | max))" + '"$answers"'
  ' "$1" 2>/dev/null
}

placed() {
  local line rest filter
  line=$("$shards" run-line "$1") || return 2
  path=${line%%"$TAB"*}
  rest=${line#*"$TAB"}
  part=${rest#*"$TAB"}
  picked=()
  for filter in ${rest%%"$TAB"*}; do picked+=(--filter "$filter"); done
}

tests_of() {
  local id=$1 path part status=0 selected out started
  local -a picked=()
  shift
  placed "$id" || return 2
  out=$(mktemp)
  started=$(now_ms)
  tested "$path" ${picked[@]+"${picked[@]}"} ${part:+--shard "$part"} "$@" > "$out" || status=$?
  if ((status == STOPPED)); then
    stopped_run "$started" "$id"
    rm -f "$out"
    return "$STOPPED"
  fi
  if ! reported "$out"; then
    unreported "$id" "$status"
    rm -f "$out"
    return 1
  fi
  listed "$out"
  summed "$out"
  timed "$out" "$path" >> "$durations"
  spent "$out" "$(($(now_ms) - started))"
  selected=$(jq -s 'map(.selection.tests // [] | length) | add // 0' "$out" 2>/dev/null || echo 0)
  if [ "$selected" -eq 0 ] && [ -n "$part" ]; then
    echo "the run $id selected no test: every test of its suite is in another part"
  elif [ "$selected" -eq 0 ]; then
    echo "the run $id selected no test" >&2
    status=1
  fi
  if lost "$out"; then
    echo "the run $id ran bodies again whose results a store had cached and lost" >&2
    status=1
  fi
  if [ "$status" -ne 0 ]; then
    red "$out"
    rm -f "$out"
    return 1
  fi
  rm -f "$out"
}

proofs_of() {
  local id=$1 path part status=0 out started all cached discharging
  local -a picked=()
  shift
  placed "$id" || return 2
  out=$(mktemp)
  started=$(now_ms)
  bounded "$ply" prove "$path" --json "$@" > "$out" || status=$?
  if ((status == STOPPED)); then
    stopped_run "$started" "$id"
    rm -f "$out"
    return "$STOPPED"
  fi
  if ! reported "$out"; then
    unreported "$id" "$status"
    rm -f "$out"
    return 1
  fi
  discharging=$(jq '(.duration_ms // 0) | floor' "$out" 2>/dev/null || echo 0)
  all=$(jq '.obligations // [] | length' "$out" 2>/dev/null || echo 0)
  cached=$(jq '.cached // 0' "$out" 2>/dev/null || echo 0)
  echo "proved: $all claims, $cached from the cache, the rest proved in $((discharging / 1000))s of the $((($(now_ms) - started) / 1000))s that the prove took$(jq -r "$answers" "$out" 2>/dev/null)"
  if [ "$all" -eq 0 ]; then
    echo "the run $id proved no claim" >&2
    status=1
  fi
  if lost "$out"; then
    echo "the run $id ran bodies again whose results a store had cached and lost" >&2
    status=1
  fi
  if [ "$status" -ne 0 ]; then
    disproved "$out"
    rm -f "$out"
    return 1
  fi
  rm -f "$out"
}

disproved() {
  jq -r '
    (.obligations[]? | select(.outcome != "proved")
      | "FAILED \(.owner) \(.label): \(.outcome)\(if .location then " at \(.location)" else "" end)",
        "  \(del(.key, .owner, .label, .kind, .guarded, .frame, .location, .outcome) | tojson)"),
    (.diagnostics[]? | ("\(.severity // "error") \(.code // ""): \(.message // "")",
      (.notes[]? | "  \(.)")))
  ' "$1" 2>/dev/null || cat "$1"
}

marked() {
  [ -f "$mark" ] || { echo "nothing is marked: run 'ci-ply.sh mark' before the runs" >&2; return 1; }
}

store_dirs() {
  local dir
  for dir in $(work std) $(work std-tests) $(work compiler) $(work cli) $(work cli-tests) \
    $(work package) $(work suite); do
    printf '%s\n' "$root/$dir"
  done | LC_ALL=C sort -u
}

# shellcheck disable=SC2100
stores=ply-c-cache/bodies

kept_answers() {
  local dir=$1 store
  rm -rf "$dir"
  mkdir -p "$dir"
  for store in "$caches/$stores"/*/; do
    [ -f "$store.used" ] && [ "$store.used" -nt "$mark" ] || continue
    (cd "$caches" && tar --exclude='run*' --exclude=lock --exclude='*.tmp' -cf - \
      "$stores/$(basename "$store")") | tar -xf - -C "$dir"
  done
}

case "${1:-}" in
  run)
    : > "$durations"
    runs=$("$shards" job-runs "${2:?a job, a run or a module}") || exit 2
    failed=0
    late=()
    for id in $runs; do
      if past_deadline; then
        late+=("$id")
        continue
      fi
      status=0
      echo "::group::$id"
      case "$id" in
        proofs | laws-*) proofs_of "$id" "${@:3}" || status=$? ;;
        *) tests_of "$id" "${@:3}" || status=$? ;;
      esac
      echo "::endgroup::"
      if ((status != 0)); then
        failed=1
        ((status == STOPPED)) || echo "::error::the run $id failed"
      fi
    done
    if [ "${#late[@]}" -gt 0 ]; then
      echo "::error::the job's deadline came before these runs could start: ${late[*]}"
      failed=1
    fi
    exit "$failed"
    ;;
  nightly)
    role=${2:?usage: ci-ply.sh nightly ROLE [MODULE...] [ARG...]}
    : > "$durations"
    out=$(mktemp)
    status=0
    args=()
    given=0
    for arg in "${@:3}"; do
      [[ $arg == -* ]] && given=1
      if ((given)); then args+=("$arg"); else args+=(--filter "$arg."); fi
    done
    [ "${#args[@]}" -gt 0 ] || { echo "usage: ci-ply.sh nightly ROLE [MODULE...] [ARG...]" >&2; exit 2; }
    if [[ $role == std-tests ]]; then
      certificate=$(mktemp -d)
      openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 2 -subj /CN=localhost \
        -addext subjectAltName=DNS:localhost -addext basicConstraints=critical,CA:FALSE \
        -keyout "$certificate/localhost.key" -out "$certificate/localhost.pem" 2>/dev/null || exit 1
      args+=(--fs "certificate=$certificate" --trust "$certificate/localhost.pem")
    fi
    tested "$(work "$role")" "${args[@]}" > "$out" || status=$?
    if ! reported "$out"; then
      unreported nightly "$status"
      rm -f "$out"
      exit 1
    fi
    listed "$out"
    summed "$out"
    selected=$(jq -s 'map(.selection.tests // [] | length) | add // 0' "$out" 2>/dev/null || echo 0)
    if [ "$status" -ne 0 ] || [ "$selected" -eq 0 ]; then
      [ "$selected" -gt 0 ] || echo "the nightly run selected no test" >&2
      red "$out"
      rm -f "$out"
      exit 1
    fi
    rm -f "$out"
    ;;
  mark)
    touch "$mark"
    ;;
  keep)
    dir=${2:?a directory}
    marked || exit 2
    kept_answers "$dir" || exit 1
    du -sh "$dir"
    ;;
  restore)
    dir=${2:?a directory}
    [ -d "$dir" ] || exit 0
    for store in "$dir/$stores"/*/; do
      [ -d "$store" ] && rm -rf "${caches:?}/$stores/$(basename "$store")"
    done
    tar -C "$dir" -cf - . | tar -C "$caches" --skip-old-files -xf -
    ;;
  compact)
    marked || exit 2
    for dir in $(store_dirs); do
      [ -d "$dir/.ply-cache" ] || continue
      if [ -z "$(find "$dir/.ply-cache" -newer "$mark" -print -quit)" ]; then
        rm -rf "$dir/.ply-cache"
        echo "=== ${dir#"$root"/}: opened by no run of this job, and removed"
        continue
      fi
      echo "=== ${dir#"$root"/}"
      "$ply" cache compact "$dir" || echo "the store under ${dir#"$root"/} was not compacted" >&2
    done
    ;;
  pack)
    file=${2:?a file}
    kept=()
    for dir in $(store_dirs); do
      [ -d "$dir/.ply-cache" ] && kept+=("${dir#"$root"/}/.ply-cache")
    done
    [ "${#kept[@]}" -gt 0 ] || { echo "this job has no store"; exit 0; }
    tar -cf - -C "$root" "${kept[@]}" | zstd -q -T0 -3 -o "$file" || exit 1
    echo "packed ${#kept[@]} stores, $(du -sh "$file" | cut -f1): ${kept[*]}"
    ;;
  unpack)
    name=${2:?an artifact}
    run=$("$root/.github/artifact-run.sh" "$name") || run=
    [ -n "$run" ] || { echo "no run of this workflow uploaded the artifact $name"; exit 0; }
    dir=$(mktemp -d)
    if gh run download "$run" -n "$name" -D "$dir" && mkdir "$dir/unpacked" &&
      zstd -d -c "$dir/stores.tar.zst" | tar -xf - -C "$dir/unpacked"; then
      laid=0
      while IFS= read -r store; do
        there=$root/${store#"$dir/unpacked/"}
        rm -rf "$there"
        mkdir -p "$(dirname "$there")"
        mv "$store" "$there" || exit 1
        laid=$((laid + 1))
      done < <(find "$dir/unpacked" -type d -name .ply-cache -prune)
      echo "$name came back from run $run: $laid stores"
      echo "found=true" >> "${GITHUB_OUTPUT:-/dev/null}"
    else
      echo "$name did not come back from run $run"
    fi
    rm -rf "$dir"
    ;;
  *)
    echo "usage: ci-ply.sh run ID [ARG...] | nightly ROLE [MODULE...] [ARG...] | mark | keep DIR | restore DIR | compact | pack FILE | unpack NAME" >&2
    exit 2
    ;;
esac
