#!/usr/bin/env bash
# The front-end row, re-taken: PRE-REGISTERED.md's protocol, and nothing else.
#
#   ./benches/front-end/run.sh                 # writes benches/front-end/raw.txt
#   ./benches/front-end/run.sh --dir <probe>   # an already-built probe project
#
# Refuses to start above the load gate (exit 3) and refuses a stale binary (exit 2).
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
ply="${PLY_BIN:-$root/target/release/ply}"
spike="$root/spikes/ply-parser"
raw="$here/raw.txt"

load1() { uptime | sed 's/.*load averages*: *//' | awk -F'[ ,]+' '{print $1}'; }

echo "==> instrument check: is the binary the one this tree would produce?"
"$root/.github/binary-is-current.sh" || { echo "STALE -- rebuild before measuring" >&2; exit 2; }
l=$(load1)
echo "==> load before: $(uptime)"
awk -v l="$l" 'BEGIN{exit !(l < 4.0)}' || { echo "load $l is above the gate of 4; not measuring" >&2; exit 3; }

dir=""
if [ "${1:-}" = "--dir" ]; then dir="$2"; shift 2; fi
if [ -z "$dir" ]; then
  dir="$(mktemp -d)"
  cp "$spike"/{lexer,spine,types,patterns,exprs,items}.ply "$dir/"
  python3 - "$root/examples" "$dir" <<'PY'
import sys, pathlib
examples, out = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
lines = ["import items (parse)", ""]
for path in sorted(examples.glob("*.ply")):
    src = path.read_bytes()
    o = []
    for ch in src:
        if ch == 0x22: o.append('\\"')
        elif ch == 0x5c: o.append('\\\\')
        elif 0x20 <= ch < 0x7f: o.append(chr(ch))
        else: o.append('\\x%02x' % ch)
    name = path.stem.replace("-", "_")
    lines.append('fn source_%s() -> Bytes = b"%s"' % (name, ''.join(o)))
    lines.append('test "row: %s" { assert(len(parse(source_%s()).node.items) >= 0) }' % (path.name, name))
    lines.append("")
(out / "probe.ply").write_text("\n".join(lines))
PY
fi
echo "==> probe project: $dir"

: > "$raw"
echo "binary $(shasum -a 256 "$ply" | cut -c1-16)" >> "$raw"
echo "load-before $l" >> "$raw"

# Warm the front-end cache once; every arm then reads the same warm cache.
"$ply" test "$dir" --no-cache --filter "row:nothing-matches" >/dev/null 2>&1 || true

run_arm() {                       # $1 label, $2... command; prints "user wall"
  local out u w
  out=$( { /usr/bin/time -p "${@:2}" >/dev/null; } 2>&1 )
  u=$(printf '%s\n' "$out" | awk '/^user/{print $2}')
  w=$(printf '%s\n' "$out" | awk '/^real/{print $2}')
  [ -n "$u" ] && [ -n "$w" ] || { echo "the timer reported nothing for $1: $out" >&2; exit 1; }
  echo "$u $w"
}

arm_cmd() {                       # $1 arm -> the command, as words on stdout
  case "$1" in
    none|null) echo "$ply test $dir --no-cache --filter row:" ;;
    narrow)    echo "env PLY_CODEGEN_REGISTER=narrow $ply test $dir --no-cache --filter row: --backend cranelift" ;;
    wide)      echo "$ply test $dir --no-cache --filter row: --backend cranelift" ;;
    floor)     echo "$ply test $dir --no-cache --filter row:nothing-matches" ;;
  esac
}

arms=(none null narrow wide)
# One run decides N, and it is kept and counted as block 1's first arm.
first=$(run_arm none $(arm_cmd none))
echo "block 1 none $first" | tee -a "$raw"
n=5
awk -v w="${first#* }" 'BEGIN{exit !(w >= 2.0)}' && n=3
echo "blocks $n" >> "$raw"

for ((b=1; b<=n; b++)); do
  for ((k=0; k<4; k++)); do
    arm=${arms[$(( (b-1+k) % 4 ))]}
    if [ "$b" -eq 1 ] && [ "$k" -eq 0 ]; then continue; fi
    r=$(run_arm "$arm" $(arm_cmd "$arm"))
    echo "block $b $arm $r" | tee -a "$raw"
  done
done
r=$(run_arm floor $(arm_cmd floor)); echo "floor $r" | tee -a "$raw"

for arm in narrow wide; do
  json=$(eval "$(arm_cmd "$arm") --json" 2>/dev/null || true)
  printf 'counts %s %s\n' "$arm" "$(printf '%s' "$json" | python3 -c 'import json,sys
r=json.load(sys.stdin); b=r.get("backend") or {}
print(" ".join(f"{k}={b.get(k)}" for k in ("fragment","offered","entered","declined")))' 2>/dev/null || echo unread)" | tee -a "$raw"
done

echo "load-after $(load1)" >> "$raw"
echo "==> load after: $(uptime)"
"$root/.github/binary-is-current.sh" >/dev/null || { echo "the binary went STALE during the series; void" >&2; exit 2; }
python3 "$here/analyze.py" "$raw"
