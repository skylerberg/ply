#!/bin/sh
# Lexer throughput. ./bench.sh [path-to-ply-binary]
#
# Release binary only: a debug build is ~5x slower and its number means nothing.
#
# These numbers are sensitive to what else is running. On an idle machine this
# reports ~17us/token; with four cargo builds in flight (load average ~12) the
# same input reports ~35us. Quote a figure only with the load average beside it.
# The *shape* is stable either way: time is linear in input size, which is the
# claim the file is here to support.
# Reported: bytes/second and microseconds/token over a synthetic source built by
# repeating one line of real Ply, so token density is realistic and the input
# size is a knob.
set -e
PLY=${1:-../../target/release/ply}
DIR=$(mktemp -d)
trap 'rm -rf "$DIR"' EXIT
cp lexer.ply "$DIR/"
for r in 256 512 1024 2048; do
  sed "s/^fn reps() -> Int = .*/fn reps() -> Int = $r/" main.ply > "$DIR/main.ply"
  out=$( { /usr/bin/time -p "$PLY" run "$DIR" --json > "$DIR/o.json"; } 2>&1 )
  v=$(grep -o '"value": "[0-9]*"' "$DIR/o.json" | grep -o '[0-9]*')
  t=$(echo "$out" | awk '/real/{print $2}')
  echo "$((v / 1000000)) $t $((v % 1000000))" |
    awk '{printf "  %7d bytes  %6d tokens  %5ss  %7.0f B/s  %5.1f us/token\n", $1, $3, $2, $1/$2, $2*1000000/$3}'
done
