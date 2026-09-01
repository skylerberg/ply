#!/bin/sh
# The ladder behind the self-hosting spike's nesting numbers.
#
#   ./bench.sh [path-to-ply-binary]
#
# Pre-registered before any data was taken: statistic is the MINIMUM of 3 runs
# per cell (minimum, because on a shared machine the minimum is the closest
# estimate of the unloaded time and no run is discarded after the fact); sizes
# are n = 4,000 / 8,000 / 16,000; a cell is called quadratic iff BOTH successive
# doubling ratios are >= 3.0 and linear iff both are <= 2.5. The load average at
# the start of each cell is printed rather than filtered on, so a reader can see
# which numbers to distrust.
set -e
PLY=${1:-../../target/release/ply}
DIR=$(mktemp -d)
trap 'rm -rf "$DIR"' EXIT
cp nesting.ply "$DIR/"

for which in tail last first firstc grouped_one grouped_ten; do
  printf '  %-7s' "$which"
  for n in 4000 8000 16000; do
    best=
    for _ in 1 2 3; do
      printf 'import nesting (tail, first, firstc, last, grouped_one, grouped_ten)\nfn main() -> Int = %s(%d)\n' "$which" "$n" > "$DIR/main.ply"
      rm -rf "$DIR/.ply-cache"
      t=$( { /usr/bin/time -p "$PLY" run "$DIR" >/dev/null; } 2>&1 | awk '/real/{print $2}' )
      best=$(printf '%s\n%s\n' "$best" "$t" | awk 'NF' | sort -g | head -1)
    done
    printf ' n=%-6s %7ss' "$n" "$best"
  done
  printf '   load %s\n' "$(uptime | sed 's/.*averages: //' | awk '{print $1}')"
done
