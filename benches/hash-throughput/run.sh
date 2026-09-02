#!/usr/bin/env bash
# ADR 0033's throughput question, taken under the load gate: the Ply BLAKE3 over a fixed
# input, interpreted and under the backend, beside the lexer's rate the record set as its bar.
#
#   ./benches/hash-throughput/run.sh      # prints the rows; keep them as observation-N.txt
#
# Refuses to start above the load gate (exit 3) and refuses a stale binary (exit 2).
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
ply="${PLY_BIN:-$root/target/release/ply}"

load1() { uptime | sed 's/.*load averages*: *//' | awk -F'[ ,]+' '{print $1}'; }

"$root/.github/binary-is-current.sh" || { echo "STALE -- rebuild before measuring" >&2; exit 2; }
l=$(load1)
awk -v l="$l" 'BEGIN{exit !(l < 4.0)}' || { echo "load $l is above the gate of 4; not measuring" >&2; exit 3; }
echo "binary $(shasum -a 256 "$ply" | cut -c1-16)"
echo "load-before $l"

dir="$(mktemp -d)"
python3 - "$dir/probe.ply" <<'PY'
import sys
n = 65536
data = bytes((i % 251) for i in range(n))
lit = 'b"' + ''.join(('\\x%02x' % b) if (b < 0x20 or b >= 0x7f or b in (0x22, 0x5c)) else chr(b) for b in data) + '"'
open(sys.argv[1], 'w').write(
    'import std.hash (blake3)\n\nfn input() -> Bytes = %s\n\n'
    'test "row: hash 65536 bytes" { assert(bytes_len(blake3(input())) == 32) }\n' % lit)
PY

for arm in none wide; do
  flag=""; [ "$arm" = wide ] && flag="--backend cranelift"
  ms=$("$ply" test "$dir" --no-cache --jobs 1 --filter "row:" $flag --json 2>/dev/null |
    python3 -c 'import json,sys
r=json.load(sys.stdin)
for t in (r.get("tests") or r.get("results") or []):
    if (t.get("name") or "").startswith("row: "): print(t.get("duration_ms"))')
  echo "hash $arm 65536 bytes ${ms} ms"
done

echo "lexer none (spikes/ply-lexer-throughput/bench.sh, bytes/s is the rate the record's bar names)"
(cd "$root/spikes/ply-lexer-throughput" && ./bench.sh "$ply")
echo "load-after $(load1)"
"$root/.github/binary-is-current.sh" >/dev/null || { echo "the binary went STALE during the series; void" >&2; exit 2; }
