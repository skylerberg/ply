#!/usr/bin/env bash
# ADR 0035's gate: PRE-REGISTERED.md's protocol over the two kernels, and nothing else.
#
#   ./benches/value-model/run.sh          # writes benches/value-model/raw.txt and analyzes it
#
# Refuses to start above the load gate (exit 3) and refuses a stale binary (exit 2).
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
ply="${PLY_BIN:-$root/target/release/ply}"
# Which tier the `ply` arm runs on. `cranelift` is the gate's own; `c` is ADR 0040's.
backend="${PLY_BACKEND:-cranelift}"
raw="$here/raw.txt"

load1() { uptime | sed 's/.*load averages*: *//' | awk -F'[ ,]+' '{print $1}'; }

echo "==> instrument check: is the binary the one this tree would produce?"
"$root/.github/binary-is-current.sh" || { echo "STALE -- rebuild before measuring" >&2; exit 2; }
echo "==> the Rust bars"
(cd "$here/rust" && cargo build --release --quiet)
bars="$here/rust/target/release/kernels"
l=$(load1)
echo "==> load before: $(uptime)"
awk -v l="$l" 'BEGIN{exit !(l < 4.0)}' || { echo "load $l is above the gate of 4; not measuring" >&2; exit 3; }

# The probe: the kernels beside the input the Rust bar hashes, and the digest it printed.
dir="$(mktemp -d)"
cp "$here/kernels.ply" "$dir/"
digest=$("$bars" | sed -n 's/.*digest=\([0-9a-f]*\).*/\1/p')
python3 - "$dir/input.ply" "$digest" <<'PY'
import sys
out, digest = sys.argv[1], sys.argv[2]
n = 65536
data = bytes((i % 251) for i in range(n))
def lit(bs):
    return 'b"' + ''.join(('\\x%02x' % b) if (b < 0x20 or b >= 0x7f or b in (0x22, 0x5c)) else chr(b) for b in bs) + '"'
open(out, 'w').write(
    'pub fn k1_input() -> Bytes = %s\n\npub fn k1_digest() -> Bytes = %s\n' % (lit(data), lit(bytes.fromhex(digest))))
PY
echo "==> probe project: $dir"

: > "$raw"
echo "binary $(shasum -a 256 "$ply" | cut -c1-16)" >> "$raw"
echo "backend $backend" >> "$raw"
echo "load-before $l" >> "$raw"

"$ply" test "$dir" --no-cache --jobs 1 --filter "kernel:nothing-matches" >/dev/null 2>&1 || true

ply_arm() {                       # prints "k1=<ms> k2=<ms>" from the report a user reads
  "$ply" test "$dir" --no-cache --jobs 1 --filter "kernel:" --backend "$backend" --json 2>/dev/null |
    python3 -c 'import json,sys
r=json.load(sys.stdin)
out={}
for t in (r.get("tests") or r.get("results") or []):
    n=t.get("name") or ""
    if not t.get("passed", True) and t.get("status") not in (None, "ok", "passed"):
        sys.exit("a kernel failed: %s" % n)
    if "k1" in n: out["k1"]=t.get("duration_ms")
    if "k2" in n: out["k2"]=t.get("duration_ms")
print("k1=%s k2=%s" % (out.get("k1"), out.get("k2")))'
}

rust_arm() { "$bars" | sed 's/ digest=.*//'; }

arms=(ply null rust)
n=3
echo "blocks $n" >> "$raw"
for ((b=1; b<=n; b++)); do
  for ((k=0; k<3; k++)); do
    arm=${arms[$(( (b-1+k) % 3 ))]}
    case "$arm" in
      ply|null) r=$(ply_arm) ;;
      rust)     r=$(rust_arm) ;;
    esac
    # seconds, both sides
    r=$(printf '%s' "$r" | python3 -c 'import sys
print(" ".join("%s=%.6f" % (kv.split("=")[0], float(kv.split("=")[1])/1000.0) for kv in sys.stdin.read().split()))')
    echo "block $b $arm $r" | tee -a "$raw"
  done
done
f=$( { /usr/bin/time -p "$ply" test "$dir" --no-cache --jobs 1 --filter "kernel:nothing-matches" >/dev/null; } 2>&1 | awk '/^user/{print $2}')
echo "floor $f" | tee -a "$raw"

echo "load-after $(load1)" >> "$raw"
echo "==> load after: $(uptime)"
"$root/.github/binary-is-current.sh" >/dev/null || { echo "the binary went STALE during the series; void" >&2; exit 2; }
python3 "$here/analyze.py" "$raw"
