#!/bin/sh
# The ladder behind the numbers in this directory's comments.
#
#   ./bench.sh [path-to-ply-binary]
#
# Defaults to the release binary in this worktree. A debug binary shows the
# same shape roughly five times slower, which is enough to see the cliff but
# not to quote a throughput figure from.
set -e
PLY=${1:-../../target/release/ply}
DIR=$(mktemp -d)
trap 'rm -rf "$DIR"' EXIT
cp fieldorder.ply "$DIR/"

echo "field-order cliff (same answer, one line moved):"
for which in slow fast bare; do
  printf '  %-5s' "$which"
  for n in 4000 8000 16000 32000; do
    printf 'fn main() -> Int = %s(%d)\n' "$which" "$n" > "$DIR/main.ply"
    sed -i '' '1i\
import fieldorder (slow, fast, bare)
' "$DIR/main.ply"
    t=$( { /usr/bin/time -p "$PLY" run "$DIR" >/dev/null; } 2>&1 | awk '/real/{print $2}' )
    printf ' n=%-6s %6ss' "$n" "$t"
  done
  echo
done
