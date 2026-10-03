#!/usr/bin/env bash
#
# What a CI run cost, and where its minutes went.
#
#   .github/ci-timings.sh runs [N]           the last N runs of `CI`: how much of each was queue
#   .github/ci-timings.sh jobs [RUN] [NAME]  the jobs of RUN longest first, the chain of them that
#                                            decided when it ended, and, with NAME, their steps
#   .github/ci-timings.sh why [RUN] [NAME]   one job: its steps, what the actions inside it did,
#                                            the cache entries it found and wrote, its pauses
#   .github/ci-timings.sh tests [RUN] [N]    the N slowest tests of RUN (default 40), from the
#                                            `test-durations` artifact its `passes` job uploads
#   .github/ci-timings.sh --self-test        the timestamp arithmetic, against fixtures
#
# RUN is a run id, a run URL, or a branch with a run; with none, the newest run of `CI`. NAME is a
# case-insensitive substring of a job's name; with none, `why` explains the run's longest job.
#
# `jobs` is the API's view, and it reports a composite action as the one step that invokes it.
# `why` also reads the job's log, which is where the steps inside `./.github/actions/suite` and
# every action beside it are timed, and the only place a cache entry is visible: two runs that
# differ by minutes differ in `Cache restored from key:`. Neither shows the time inside a step
# that emits nothing while it works, so `why` reports the pauses between lines as well: gaps of
# 5s or more, or of `PAUSE` seconds where a longer one is asked for.
#
# Needs `gh` and `jq`.
set -euo pipefail

# The helpers every awk below starts with: `awk "$TIME_AWK"'program'`, the
# two adjacent so that they are the one argument awk wants.
TIME_AWK='
# Seconds since the epoch, fractional seconds included, from an ISO-8601 UTC stamp: days from the
# civil date by the usual era/doy/doe arithmetic, so no `date`, whose flags are not the same on
# the runner as on any laptop.
function epoch(s,   raw, y, mo, dy, hh, mi, ss, era, yoe, doy, doe) {
  raw = s
  y = substr(s, 1, 4) + 0; mo = substr(s, 6, 2) + 0; dy = substr(s, 9, 2) + 0
  hh = substr(s, 12, 2) + 0; mi = substr(s, 15, 2) + 0; ss = substr(s, 18, 2) + 0
  y = y - (mo <= 2)
  era = int(y / 400); yoe = y - era * 400
  doy = int((153 * (mo + (mo > 2 ? -3 : 9)) + 2) / 5) + dy - 1
  doe = yoe * 365 + int(yoe / 4) - int(yoe / 100) + doy
  s = (era * 146097 + doe - 719468) * 86400 + hh * 3600 + mi * 60 + ss
  if (length(raw) > 19) s = s + ("0." substr(raw, 21))
  return s + 0
}
# A duration a reader can hold in their head: one unit, and a decimal below a minute, where 0.9s
# and 9s are both worth telling apart.
function spell(s) {
  if (s >= 3600) return sprintf("%dh%02dm", int(s / 3600), int((s % 3600) / 60))
  if (s >= 60) return sprintf("%dm%02ds", int(s / 60), int(s % 60))
  return sprintf("%.1fs", s)
}
function stamp(s) { return sprintf("+%d:%02d", int(s / 60), int(s % 60)) }
# The value of `key=` up to the `;` or `]` that ends it.
function field(s, key,   v, i) {
  if (!match(s, key "=")) return ""
  v = substr(s, RSTART + length(key) + 1)
  i = index(v, ";"); if (i > 0) v = substr(v, 1, i - 1)
  i = index(v, "]"); if (i > 0) v = substr(v, 1, i - 1)
  return v
}
'

awk_time() { awk "$TIME_AWK$1" "${@:2}"; }
spell() { awk_time "BEGIN { printf \"%s\", spell($1) }"; }
die() { echo "ci-timings.sh: $*" >&2; exit 2; }

# A run id, from a run id, a run URL, or a branch that has a run; with nothing, the newest run.
resolve_run() {
  local want=${1:-}
  case "$want" in
    *"/actions/runs/"*) want=${want##*"/actions/runs/"}; want=${want%%[/?]*} ;;
  esac
  case "$want" in
    # The newest run whose jobs have started: one that is still queued has nothing to show, and
    # is usually the one just pushed.
    "") gh run list --workflow=ci.yml --limit 10 --json databaseId,status \
      --jq '[.[] | select(.status != "queued" and .status != "pending" and .status != "requested")]
            | (.[0].databaseId // empty)' ;;
    *[!0-9]*) gh run list --workflow=ci.yml --branch "$want" --limit 1 --json databaseId \
      --jq '.[0].databaseId // empty' ;;
    *) echo "$want" ;;
  esac
}

# The run and its jobs, read once into $tmp however many questions a mode asks of them.
fetch() {
  gh api "repos/{owner}/{repo}/actions/runs/$1" > "$tmp/run.json" ||
    die "no run $1 in this repository"
  gh api --paginate "repos/{owner}/{repo}/actions/runs/$1/jobs?per_page=100" |
    jq -s '{jobs: [.[].jobs[] | {id, name, status, conclusion, started_at, completed_at, steps}]}' \
      > "$tmp/jobs.json"
  local started
  started=$(jq '[.jobs[] | select(.started_at != null)] | length' "$tmp/jobs.json")
  if [ "$started" = 0 ]; then
    die "no job of run $1 has started yet ($(jq -r .status "$tmp/run.json"))"
  fi
}

# `start<TAB>end<TAB>name` per job, in epoch seconds, 0 where the API has no timestamp yet.
jobs_tsv() {
  jq -r '.jobs[] | [.started_at, .completed_at, .name] | @tsv' "$tmp/jobs.json" |
    awk_time '
      BEGIN { FS = OFS = "\t" }
      { s = ($1 == "" || $1 == "null") ? 0 : epoch($1)
        e = ($2 == "" || $2 == "null") ? 0 : epoch($2)
        print s, e, $3 }'
}

# The first step of the run and the completion of its last job, in epoch seconds.
span_tsv() {
  jobs_tsv | awk -F'\t' '
    $1 > 0 && (first == 0 || $1 < first) { first = $1 }
    $2 > last { last = $2 }
    END { printf "%d\t%d\n", first, last }'
}

# The steps of one job, longest first, from the API: 1s resolution, and every action folded into
# the step that invokes it. `-1` for a step that has not finished.
steps_tsv() {
  jq -r --argjson id "$1" '
    .jobs[] | select(.id == $id) | .steps[]
    | [.started_at, .completed_at, .conclusion, .name] | @tsv' "$tmp/jobs.json" |
    awk_time '
      BEGIN { FS = OFS = "\t" }
      { s = ($1 == "" || $1 == "null") ? 0 : epoch($1)
        e = ($2 == "" || $2 == "null") ? 0 : epoch($2)
        print (s > 0 && e >= s) ? e - s : -1, $3, $4 }' |
    sort -t"$(printf '\t')" -k1,1nr
}

# A job's steps, one per line.
print_steps() {
  steps_tsv "$1" | awk_time '
    BEGIN { FS = OFS = "\t" }
    { printf "  %9s  %-8s %s\n", ($1 < 0 ? "running" : sprintf("%.0fs", $1)),
        ($2 == "skipped" ? "skipped" : ""), $3 }'
}

# What a job's log says that the API cannot, one `kind<TAB>...` line per finding:
#   step  <seconds> <depth> <name>   a step inside an action, from the runner's own duration
#   cache <offset> <line>            an entry restored, saved, or missed
#   pause <seconds> <offset> <line>  a gap between lines, and what was printed before it
# A pause is only reported where the line before it is not one of the runner's markers, which
# already account for the time of the step they open.
log_sections() {                                   # $1 the job's start; the log on stdin
  awk -v pause="${PAUSE:-5}" -v start="$1" "$TIME_AWK"'
    BEGIN { OFS = "\t"; ts = "[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9:.]+Z " }
    # A line with no timestamp of its own is a continuation of the one before, and is not read
    # again as if it were that line.
    { got = ($0 ~ "^" ts) }
    got {
      now = epoch(substr($0, 1, index($0, "Z")))
      text = substr($0, index($0, "Z") + 2)
      # nextest colours its lines, and a report is not a terminal.
      gsub(/\033\[[0-9;]*m/, "", text)
      # Only where the line before the gap is not a marker: a marker already accounts for the time
      # of the step it opens, and the marker of a *previous* step would account for it twice.
      if (seen && before_text !~ /^##\[/ && now - before >= pause)
        printf "pause\t%.1f\t%.1f\t%s\n", now - before, before - start, before_text
      seen = 1; before = now; before_text = text
    }
    got && text ~ /##\[start-action / && text !~ /##\[end-action / {
      id = field(text, "id")
      display[id] = field(text, "display")
      next
    }
    got && text ~ /##\[end-action / {
      id = field(text, "id")
      ms = field(text, "duration_ms")
      if (display[id] != "") printf "step\t%.1f\t%d\t%s\n", ms / 1000, gsub(/\./, ".", id), display[id]
      next
    }
    got && text ~ /Cache restored from key:|Cache saved with key:|Cache not found|Failed to save|Cannot save/ {
      printf "cache\t%.1f\t%s\n", now - start, text
    }
  '
}

# The log findings, as three sections.
log_report() {
  awk_time '
    BEGIN { FS = "\t" }
    $1 == "step"  { step[++ns] = sprintf("%s%7.1fs  %s", substr("        ", 1, 2 * $3), $2, $4) }
    $1 == "cache" { cache[++nc] = sprintf("  %7s  %s", stamp($2), $3) }
    $1 == "pause" {
      np++
      pause[np] = sprintf("  %7s  %8.1fs  after: %.84s", stamp($3), $2, $4)
      big[np] = $2 + 0
    }
    END {
      print "  steps inside the actions (the log times what the API folds into one step):"
      if (ns == 0) print "    (none)"
      for (i = 1; i <= ns; i++) print "  " step[i]
      print "  cache entries, in order (a restore is the entry this job found):"
      if (nc == 0) print "    (none)"
      for (i = 1; i <= nc; i++) print cache[i]
      # Longest first, and only the longest few: a run of tests prints a line per test, and the
      # gap between two of them is the test, not a pause a reader would act on.
      print "  the longest pauses of 5s or more between log lines:"
      if (np == 0) print "    (none)"
      for (slot = 1; slot <= 5 && slot <= np; slot++) {
        best = 0
        for (i = 1; i <= np; i++) if (!taken[i] && big[i] > big[best]) best = i
        taken[best] = 1
        print pause[best]
      }
      if (np > 5) printf "    (%d shorter)\n", np - 5
    }'
}

cmd_runs() {
  local limit=${1:-5} id branch event conclusion created jobs first last opened queue
  gh run list --workflow=ci.yml --limit "$limit" \
    --json databaseId,headBranch,event,conclusion,status,createdAt \
    --jq '.[] | [.databaseId, .headBranch, .event,
          (if (.conclusion // "") == "" then .status else .conclusion end), .createdAt] | @tsv' |
  while IFS=$'\t' read -r id branch event conclusion created; do
    jobs=$(gh api --paginate "repos/{owner}/{repo}/actions/runs/$id/jobs?per_page=100" \
      --jq '.jobs[] | [.started_at, .completed_at] | @tsv' 2>/dev/null || true)
    read -r first last <<< "$(printf '%s\n' "$jobs" | awk_time '
      { if ($1 == "" || $1 == "null") next
        s = epoch($1)
        e = ($2 == "" || $2 == "null") ? 0 : epoch($2)
        if (first == 0 || s < first) first = s
        if (e > last) last = e }
      END { printf "%.0f %.0f", first + 0, last + 0 }')"
    if [ "$first" = 0 ]; then
      printf '%12s  %-30.30s %-13s %-11s %9s  %9s\n' \
        "$id" "$branch" "$event" "$conclusion" queued running
    else
      opened=$(awk_time "BEGIN { printf \"%.0f\", epoch(\"$created\") }")
      queue=$((first > opened ? first - opened : 0))
      printf '%12s  %-30.30s %-13s %-11s %9s  %9s\n' \
        "$id" "$branch" "$event" "$conclusion" "$(spell "$queue")" "$(spell "$((last - first))")"
    fi
  done
}

cmd_jobs() {
  local run=$1 pattern=${2:-} branch event conclusion first last span
  fetch "$run"
  # `-` for a run that has not ended: an empty field would collapse and shift the rest.
  IFS=$'\t' read -r branch event conclusion < <(jq -r '
    [.head_branch, .event, (if (.conclusion // "") == "" then .status else .conclusion end)] | @tsv' \
    "$tmp/run.json")
  read -r first last < <(span_tsv)
  # A job still running has no completion, so a run younger than its first job has no span yet.
  span=$((last > first ? last - first : 0))
  echo "run $run · $branch · $event · $conclusion · $(spell "$span") from the first step to the last job"
  echo
  echo "jobs, longest first"
  jobs_tsv | awk -F'\t' -v OFS='\t' -v first="$first" '
    { print ($2 > 0 && $1 > 0) ? $2 - $1 : -1, ($1 > 0) ? $1 - first : -1, $3 }' |
    sort -t"$(printf '\t')" -k1,1nr |
    awk -F'\t' '
      { printf "  %9s  %8s  %s\n", ($1 < 0 ? "running" : sprintf("%ds", $1)),
          ($2 < 0 ? "-" : sprintf("+%d:%02d", $2 / 60, $2 % 60)), $3 }'
  echo
  echo "the chain that decided when the run ended: each link was still running when the next began"
  jobs_tsv | awk -F'\t' -v first="$first" -v last="$last" '
    { n++; s[n] = $1 + 0; e[n] = $2 + 0; name[n] = $3 }
    END {
      cur = 0
      # `>=`: of jobs that ended in the same second, the one that ended the run is the last one
      # the API lists, which is the `ci` gate.
      for (i = 1; i <= n; i++) if (e[i] >= e[cur]) cur = i
      while (cur != 0) {
        k++; link[k] = name[cur]; dur[k] = e[cur] - s[cur]
        prev = 0
        for (i = 1; i <= n; i++) if (e[i] > 0 && e[i] <= s[cur] && (prev == 0 || e[i] > e[prev])) prev = i
        cur = prev
      }
      total = 0
      for (i = k; i >= 1; i--) { total += dur[i]; printf "  %8ds  %s\n", dur[i], link[i] }
      printf "  %8ds  not on the chain: queue, gaps between jobs, and the jobs it overlaps\n", last - first - total
    }'
  if [ -n "$pattern" ]; then
    echo
    steps_for "$pattern"
  fi
}

# The steps of every job whose name holds PATTERN.
steps_for() {
  local id name
  while IFS=$'\t' read -r id name; do
    echo "$name"
    print_steps "$id"
    echo
  done < <(jq -r --arg p "$1" \
    '.jobs[] | select(.name | ascii_downcase | contains($p | ascii_downcase)) | [.id, .name] | @tsv' \
    "$tmp/jobs.json")
}

cmd_tests() {
  local run=$1 n=$2
  [ -n "$run" ] || die "no run of CI to read"
  gh run download "$run" -n test-durations -D "$tmp/durations" >/dev/null ||
    die "run $run has no test-durations artifact: an older run, or one whose passes job did not finish"
  awk -F'\t' -v n="$n" '
    NR <= n { printf "%8.1fs  %-32s %s\n", $1 / 1000, $2, $3 }
    { t += $1 }
    END { printf "%d tests, %.0f s in all\n", NR, t / 1000 }
  ' "$tmp/durations/tests.tsv"
}

cmd_why() {
  local run=$1 pattern=${2:-} id name started completed status rows count
  fetch "$run"
  if [ -n "$pattern" ]; then
    rows=$(jq -r --arg p "$pattern" \
      '.jobs[] | select(.name | ascii_downcase | contains($p | ascii_downcase)) | [.id, .name] | @tsv' \
      "$tmp/jobs.json")
  else
    # Nothing named: the job the run spent the most on, which is the one worth explaining.
    name=$(jobs_tsv | awk -F'\t' '$1 > 0 && $2 > $1 && $2 - $1 > d { d = $2 - $1; n = $3 } END { print n }')
    rows=$(jq -r --arg n "$name" '.jobs[] | select(.name == $n) | [.id, .name] | @tsv' "$tmp/jobs.json")
    echo "(no job named: the longest, \"$name\")"
  fi
  count=$(printf '%s\n' "$rows" | grep -c .)
  if [ "$count" -eq 0 ]; then
    die "no job of run $run matches \"$pattern\""
  fi
  if [ "$count" -gt 1 ]; then
    echo "ci-timings.sh: \"$pattern\" names $count jobs:" >&2
    printf '%s\n' "$rows" | cut -f2 | sed 's/^/  /' >&2
    die "name one job"
  fi
  IFS=$'\t' read -r id name <<< "$rows"

  local s e
  IFS=$'\t' read -r s e started completed status < <(jq -r --argjson id "$id" \
    '.jobs[] | select(.id == $id) | [.started_at, (.completed_at // "-"), .status] | @tsv' \
    "$tmp/jobs.json" | awk_time '
      BEGIN { FS = OFS = "\t" }
      { print ($1 == "" ? 0 : epoch($1)), ($2 == "-" ? 0 : epoch($2)), $1, $2, $3 }')
  if [ "$completed" = "-" ]; then completed=; fi

  echo "job \"$name\" ($id) · $([ "$e" -gt 0 ] && spell "$((e - s))" || echo "$status") · $started${completed:+ → $completed}"
  echo
  echo "steps (from the API: 1s resolution, and a composite action is one step here)"
  print_steps "$id"
  echo
  gh api "repos/{owner}/{repo}/actions/jobs/$id/logs" > "$tmp/log"
  log_sections "$s" < "$tmp/log" | log_report
}

# The arithmetic the parsers rest on, against fixtures: a wrong number here is a report that lies,
# and nothing else would notice.
self_test() {
  local fail=0 got want
  check() {
    got=$1; want=$2
    [ "$got" = "$want" ] || { echo "FAIL: $3: got '$got', want '$want'" >&2; fail=1; }
  }
  check "$(awk_time 'BEGIN { printf "%.0f", epoch("2026-09-27T00:00:00Z") }')" \
    1790467200 "epoch of a whole second"
  check "$(awk_time 'BEGIN { printf "%.1f", epoch("2026-09-27T00:00:00.5000000Z") }')" \
    1790467200.5 "epoch of a fractional second"
  check "$(awk_time 'BEGIN { printf "%.0f", epoch("2026-09-27T00:00:00.0000000Z") }')" \
    1790467200 "epoch of a zero fraction"
  check "$(awk_time 'BEGIN { printf "%.0f", epoch("2025-12-31T23:59:59Z") }')" \
    1767225599 "epoch either side of a year"
  check "$(awk_time 'BEGIN { printf "%s %s %s", spell(9.4), spell(90), spell(3661) }')" \
    "9.4s 1m30s 1h01m" "a duration in each of its units"
  check "$(awk_time 'BEGIN { printf "%s", stamp(229) }')" "+3:49" "an offset from a job's start"

  local fixture out
  fixture=$(mktemp)
  cat > "$fixture" <<'LOG'
2026-09-27T00:00:00.0000000Z ##[group]Run something that prints twice
2026-09-27T00:00:00.1000000Z hello
2026-09-27T00:00:06.6000000Z done
2026-09-27T00:00:06.7000000Z ##[start-action display=Warm the thing;id=__self.__run]
2026-09-27T00:00:06.8000000Z ##[end-action id=__self.__run;outcome=success;conclusion=success;duration_ms=1234]
2026-09-27T00:00:06.9000000Z ##[start-action display=Nested;id=__self.__gone.__run]
2026-09-27T00:00:07.0000000Z ##[end-action id=__self.__gone.__run;outcome=success;conclusion=success;duration_ms=2500]
   a continuation line with no timestamp of its own
2026-09-27T00:00:12.0000000Z Cache restored from key: fixture-key
LOG
  out=$(log_sections 1790467200 < "$fixture")
  check "$(printf '%s\n' "$out" | grep -c '^step')" 2 "steps inside an action"
  check "$(printf '%s\n' "$out" | awk -F'\t' '$1 == "step" && $3 == 2 { printf "%s %s", $2, $4 }')" \
    "2.5 Nested" "a step nested a level deeper, and the depth it is printed at"
  check "$(printf '%s\n' "$out" | awk -F'\t' '$1 == "step" && $3 == 1 { printf "%.1f %s", $2, $4 }')" \
    "1.2 Warm the thing" "an action's own duration"
  check "$(printf '%s\n' "$out" | awk -F'\t' '$1 == "pause" { printf "%.1f %s", $2, $4 }')" \
    "6.5 hello" "a pause, and the line printed before it"
  check "$(printf '%s\n' "$out" | awk -F'\t' '$1 == "cache" { printf "%.1f %s", $2, $3 }')" \
    "12.0 Cache restored from key: fixture-key" "a cache line and its offset"
  rm -f "$fixture"

  [ "$fail" -eq 0 ] || exit 1
  echo "self-test: the epochs, a duration, an action's own time, a pause and a cache line all hold"
}

[ $# -gt 0 ] || die "usage: ci-timings.sh {runs [N]|jobs [RUN] [NAME]|why [RUN] [NAME]|tests [RUN] [N]|--self-test}"
if [ "$1" = "--self-test" ]; then self_test; exit 0; fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

case "$1" in
  runs) cmd_runs "${2:-5}" ;;
  jobs) cmd_jobs "$(resolve_run "${2:-}")" "${3:-}" ;;
  why) cmd_why "$(resolve_run "${2:-}")" "${3:-}" ;;
  tests) cmd_tests "$(resolve_run "${2:-}")" "${3:-40}" ;;
  *) die "unknown mode: $1 (runs, jobs, why, tests, --self-test)" ;;
esac
