#!/usr/bin/env bash
# Where the integer kernel's time actually goes, decomposed. ADR 0035's gate says K1 is over the
# bar; ADR 0036 named the causes. This prices each of them separately, because a cause that is
# named and never measured is the thing a milestone spends itself on.
#
#   ./benches/value-model/k1-where.sh          # writes observation-k1-where.txt
#
# Refuses a stale binary (exit 2) and waits for the load gate, refusing if it never comes (exit 3).
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
ply="${PLY_BIN:-$root/target/release/ply}"
reps="${1:-2000}"

load1() { uptime | sed 's/.*load averages*: *//' | awk -F'[ ,]+' '{print $1}'; }

cargo build --release --manifest-path "$root/Cargo.toml" -p ply-cli
"$root/.github/binary-is-current.sh" || { echo "STALE -- rebuild before measuring" >&2; exit 2; }
(cd "$here/rust" && cargo build --release --quiet)
bars="$here/rust/target/release/kernels"

dir="$(mktemp -d)"
trap 'rm -rf "$dir"' EXIT
digest=$("$bars" | sed -n 's/.*digest=\([0-9a-f]*\).*/\1/p')
python3 - "$dir/input.ply" "$digest" <<'PY'
import sys
out, digest = sys.argv[1], sys.argv[2]
data = bytes((i % 251) for i in range(65536))
def lit(bs):
    return 'b"' + ''.join(('\\x%02x' % b) if (b < 0x20 or b >= 0x7f or b in (0x22, 0x5c)) else chr(b) for b in bs) + '"'
open(out, 'w').write('pub fn k1_input() -> Bytes = %s\n\npub fn k1_digest() -> Bytes = %s\n'
                     % (lit(data), lit(bytes.fromhex(digest))))
PY
# The shipped hash with its internals exposed, so a probe can call one phase of it.
sed 's/^fn /pub fn /; s/^type /pub type /' "$root/crates/ply-std/ply/hash.ply" > "$dir/h.ply"

cat > "$dir/probe.ply" <<PROBE
import h (blake3, block_words, compress, iv, Words16)
import input (k1_input)

fn reps() -> Int = $reps
fn mask32(x: Int) -> Int = x & 0xFFFF_FFFF

// The whole hash, which is what the gate measures.
fn whole(acc: Int, _i: Int) -> Int = acc + bytes_len(blake3(k1_input()))
test "whole" { assert_eq(fold(range(0, reps()), 0, whole), reps() * 32) }

// The same number of blocks, loading words and nothing else.
fn load_block(acc: Int, at: Int) -> Int = { let w = block_words(k1_input(), at * 64, 65536); acc + w.m0 + w.m15 }
fn load_all(acc: Int, _i: Int) -> Int = acc + fold(range(0, 1024), 0, load_block)
test "loading only" { assert_eq(fold(range(0, reps()), 0, load_all) >= 0, true) }

// The same number of compressions, on words already loaded.
fn fixed() -> Words16 = block_words(k1_input(), 0, 65536)
fn press(acc: Int, i: Int) -> Int = acc + compress(iv(), fixed(), i, 64, 0).m0
fn press_all(acc: Int, _i: Int) -> Int = acc + fold(range(0, 1024), 0, press)
test "compressing only" { assert_eq(fold(range(0, reps()), 0, press_all) >= 0, true) }

// One quarter-round's arithmetic with nothing else: no record built, none read.
fn qr(acc: Int, i: Int) -> Int = {
  let a1 = mask32(acc + i + 1);
  let d1 = rotr32(acc ^ a1, 16);
  let c1 = mask32(acc + d1);
  let b1 = rotr32(acc ^ c1, 12);
  mask32(a1 ^ b1 ^ c1 ^ d1)
}
fn wrapped(acc: Int, i: Int) -> Int = {
  let a1 = mask32(wrap_add(wrap_add(acc, i), 1));
  let d1 = rotr32(acc ^ a1, 16);
  let c1 = mask32(wrap_add(acc, d1));
  let b1 = rotr32(acc ^ c1, 12);
  mask32(a1 ^ b1 ^ c1 ^ d1)
}
fn m() -> Int = 8000000
test "arithmetic, adds checked" { assert_eq(fold(range(0, m()), 1, qr) >= 0, true) }
test "arithmetic, adds wrapping" { assert_eq(fold(range(0, m()), 1, wrapped) >= 0, true) }
PROBE

for _ in $(seq 60); do
  l=$(load1); awk -v l="$l" 'BEGIN{exit !(l < 4.0)}' && break; sleep 15
done
echo "==> load before: $(uptime)"
awk -v l="$l" 'BEGIN{exit !(l < 4.0)}' || { echo "load $l stayed above the gate of 4" >&2; exit 3; }

obs="$here/observation-k1-where.txt"
{
  echo "==> load before: $(uptime)"
  echo
  echo "K1 decomposed, $reps repetitions of one 64 KiB hash unless the row says otherwise."
  "$ply" test "$dir" --no-cache --jobs 1 --backend cranelift 2>&1 |
    grep -E "^   ok +probe\." | sed 's/^   ok  */  /'
  echo
  echo "What one \`round\` compiles to — eight quarter-rounds, against a dozen instructions each"
  echo "in the Rust bar:"
  PLY_CODEGEN_ASM="h.round" "$ply" test "$dir" --no-cache --jobs 1 --backend cranelift \
    --filter "whole" >/dev/null 2>"$dir/asm.txt" || true
  python3 - "$dir/asm.txt" <<'PY'
import sys
lines = open(sys.argv[1]).read().splitlines()
size = next((l.split(': ')[1] for l in lines if l.startswith('compiled ')), '?')
stack_ld = stack_st = heap_ld = heap_st = calls = 0
for line in lines:
    s = line.strip()
    onstack = '[sp' in s or '[fp' in s
    if s.startswith(('ldr', 'ldp')):
        stack_ld += onstack; heap_ld += not onstack
    elif s.startswith(('str', 'stp')):
        stack_st += onstack; heap_st += not onstack
    elif s.startswith('blr'):
        calls += 1
print(f"  size {size}")
print(f"  stack traffic (spills)   {stack_ld:>4} loads  {stack_st:>4} stores")
print(f"  heap traffic (fields)    {heap_ld:>4} loads  {heap_st:>4} stores")
print(f"  cold helper call sites   {calls:>4}")
PY
  echo
  echo "==> load after: $(uptime)"
} | tee "$obs"
