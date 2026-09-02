#!/usr/bin/env bash
# What one edit costs at three project sizes, under both engines: PRE-REGISTERED.md's
# protocol and nothing else. Writes benches/marginal-change/raw.json.
#
#   ./benches/marginal-change/run.sh
#   ./benches/marginal-change/run.sh --repeats 3
#
# Refuses a stale binary (exit 2) and refuses to start above the load gate (exit 3).
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
raw="$here/raw.json"
repeats=1
[ "${1:-}" = "--repeats" ] && repeats="$2"

sizes=("10,25,125" "40,25,500" "160,25,2000")
load1() { uptime | sed 's/.*load averages*: *//' | awk -F'[ ,]+' '{print $1}'; }

echo "==> instrument check: are the binaries the ones this tree would produce?"
cargo build --release --manifest-path "$root/Cargo.toml" -p ply-corpus -p ply-cli
"$root/.github/binary-is-current.sh" || { echo "STALE -- rebuild before measuring" >&2; exit 2; }
corpus="$root/target/release/ply-corpus"
ply="$root/target/release/ply"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
echo "==> generating corpora under $work"
for size in "${sizes[@]}"; do
  IFS=, read -r m d t <<<"$size"
  "$corpus" gen --out "$work/m${m}_d${d}_t${t}" --seed 1 \
    --modules "$m" --defs-per-module "$d" --tests "$t" \
    --depth "$(( m < 6 ? m : 6 ))" >/dev/null
done

# The build and the corpora above are this script's own load, so the gate is read after them and
# waited on rather than refused at once: a series that refuses because of its own setup tells you
# nothing about the machine it would have measured on.
for _ in $(seq 60); do
  l=$(load1)
  awk -v l="$l" 'BEGIN{exit !(l < 4.0)}' && break
  sleep 15
done
echo "==> load before: $(uptime)"
awk -v l="$l" 'BEGIN{exit !(l < 4.0)}' || { echo "load $l stayed above the gate of 4; not measuring" >&2; exit 3; }

echo "{" > "$raw"
echo "  \"load_before\": $l," >> "$raw"
echo "  \"repeats\": $repeats," >> "$raw"
echo "  \"rows\": [" >> "$raw"
first=1
for size in "${sizes[@]}"; do
  IFS=, read -r m d t <<<"$size"
  dir="$work/m${m}_d${d}_t${t}"
  for engine in none cranelift; do
    echo "==> $size · $engine"
    if [ "$engine" = none ]; then
      out=$("$corpus" bench "$dir" --repeats "$repeats" --json)
    else
      out=$("$corpus" bench "$dir" --repeats "$repeats" --backend cranelift --json)
    fi
    [ $first -eq 1 ] || echo "    ," >> "$raw"
    first=0
    printf '    {"size": "%s", "engine": "%s", "report": %s}\n' "$size" "$engine" "$out" >> "$raw"
  done

  # What the real command pays that the in-process rows do not, and the reading that matters most:
  # a warm run, nothing changed, nothing rechecked — and the front end still costs what it costs.
  "$ply" test "$dir" >/dev/null 2>&1 || true
  best=""
  for _ in 1 2 3; do
    s=$( { /usr/bin/time -p "$ply" test "$dir" >/dev/null; } 2>&1 | awk '/^real/{print $2}' )
    [ -z "$best" ] && best=$s
    awk -v a="$s" -v b="$best" 'BEGIN{exit !(a < b)}' && best=$s
  done
  front=$("$ply" test "$dir" --json 2>/dev/null | python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin)["front_end"]))')
  # What the code generator charges per definition, which is the number ADR 0037's table of loop
  # tiers compares against `benches/c-floor/`'s and which the whole-unit compile otherwise hides.
  back=$("$ply" test "$dir" --backend cranelift --json 2>/dev/null | python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin)["backend"]))')
  echo "    ," >> "$raw"
  printf '    {"size": "%s", "engine": "process", "warm_wall_seconds": %s, "warm_front_end": %s, "backend": %s}\n' \
    "$size" "$best" "$front" "$back" >> "$raw"
done
echo "  ]," >> "$raw"
echo "  \"load_after\": $(load1)" >> "$raw"
echo "}" >> "$raw"
echo "==> load after: $(uptime)"
python3 "$here/analyze.py" "$raw"
