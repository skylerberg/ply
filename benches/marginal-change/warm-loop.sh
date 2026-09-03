#!/usr/bin/env bash
# What one edit costs *inside a warm process*, across project sizes and across the one property
# that turns out to decide it: whether the project has tests that must run on every invocation.
#
#   ./benches/marginal-change/warm-loop.sh            # writes observation-warm.txt
#
# Refuses a stale binary (exit 2) and waits for the load gate, refusing if it never comes (exit 3).
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
edits="${1:-5}"

sizes=("10,25,125" "40,25,500" "160,25,2000")
load1() { uptime | sed 's/.*load averages*: *//' | awk -F'[ ,]+' '{print $1}'; }

cargo build --release --manifest-path "$root/Cargo.toml" -p ply-corpus -p ply-cli
"$root/.github/binary-is-current.sh" || { echo "STALE -- rebuild before measuring" >&2; exit 2; }
corpus="$root/target/release/ply-corpus"
ply="$root/target/release/ply"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
echo "==> generating corpora under $work"
for size in "${sizes[@]}"; do
  IFS=, read -r m d t <<<"$size"
  # Two of each size: one whose tests are all deterministic, and one with the generator's default
  # fraction of nondeterministic tests — which run on every invocation whatever the cache says.
  "$corpus" gen --out "$work/det_$m" --seed 1 --modules "$m" --defs-per-module "$d" --tests "$t" \
    --depth 6 --nondet-fraction 0.0 >/dev/null
  "$corpus" gen --out "$work/nondet_$m" --seed 1 --modules "$m" --defs-per-module "$d" --tests "$t" \
    --depth 6 >/dev/null
done

for _ in $(seq 60); do
  l=$(load1); awk -v l="$l" 'BEGIN{exit !(l < 4.0)}' && break; sleep 15
done
echo "==> load before: $(uptime)"
awk -v l="$l" 'BEGIN{exit !(l < 4.0)}' || { echo "load $l stayed above the gate of 4" >&2; exit 3; }

one() {                            # $1 corpus dir, $2 label
  local dir="$1" label="$2" out v
  out="$(mktemp)"
  v="$(find "$dir" -name '*.ply' | sort | head -1)"
  "$ply" test "$dir" >/dev/null 2>&1
  "$ply" test "$dir" --watch --json > "$out" 2>&1 &
  local pid=$!
  sleep 4
  for i in $(seq 1 "$edits"); do
    printf '\npub fn probe%d() -> Int = %d\n' "$i" "$i" >> "$v"
    sleep 1.3
  done
  kill "$pid" 2>/dev/null || true
  sleep 0.3
  python3 - "$out" "$label" <<'PY'
import json, sys
text = open(sys.argv[1]).read(); label = sys.argv[2]
dec = json.JSONDecoder(); i = 0; rows = []
while i < len(text):
    while i < len(text) and text[i] in ' \n\r\t': i += 1
    if i >= len(text): break
    v, i = dec.raw_decode(text, i); rows.append(v)
if len(rows) < 2:
    print(f"  {label:<28} no iteration after the first"); raise SystemExit
after = rows[1:]
best = min(v['front_end']['phases']['total'] for v in after)
worst = max(v['front_end']['phases']['total'] for v in after)
f = after[0]['front_end']
ran = after[0]['summary']['passed']
print(f"  {label:<28} front end {best:6.1f}-{worst:.1f} ms   parsed {f['parsed']:>4}"
      f"   reused {f.get('reused', '-'):>4}   tests run {ran:>4}")
PY
  rm -f "$out"
}

obs="$here/observation-warm.txt"
{
  echo "==> load before: $(uptime)"
  echo
  echo "One edit inside a warm process. Each row is the front end an iteration paid, over"
  echo "$edits edits, best to worst."
  echo
  echo "deterministic tests only — nothing runs unless the edit reached it:"
  for m in 10 40 160; do one "$work/det_$m" "$(( m * 25 )) definitions"; done
  echo
  echo "the generator's default fraction of nondeterministic tests, which run every time:"
  for m in 10 40 160; do one "$work/nondet_$m" "$(( m * 25 )) definitions"; done
  echo
  echo "==> load after: $(uptime)"
} | tee "$obs"
