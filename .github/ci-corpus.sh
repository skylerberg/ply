#!/usr/bin/env bash
# The corpus's Ply runs one partition takes (`ci-shards.sh corpus-for-partition`), one after
# another, each with the grants `benches/corpus.sh` gives the program. Each run's milliseconds are
# appended to TIMINGS as a `corpus <run> <ms>` row, and with the program's own tests go those of the
# programs under `fixtures/` it runs, each of which must test something that passes. Every run is
# taken even when one fails, and the exit status says whether any did.
#
#   ci-corpus.sh PARTITION TIMINGS
set -uo pipefail

shard=${1:?a partition}
timings=${2:?a file for the durations}
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ply="$root/target/debug/ply"
failed=0

: > "$timings"
for id in $("$root/.github/ci-shards.sh" corpus-for-partition "$shard"); do
  read -r path filter < <("$root/.github/ci-shards.sh" corpus-line "$id")
  echo "::group::corpus $id"
  started=$(date +%s%3N)
  "$ply" test "$path" ${filter:+--filter "$filter"} --host --timeout 900000 --steps 0 \
    --exec "ply=$ply" --allow machine --allow claims --fs work=. --fs "repo=$root" || failed=1
  if [ "$id" = program ]; then
    for fixture in crates/ply-corpus/fixtures/*.ply; do
      if ! "$ply" test "$fixture" --no-cache --json > /tmp/fixture.json; then
        cat /tmp/fixture.json
        failed=1
      elif ! jq -e '.summary.passed > 0' /tmp/fixture.json > /dev/null; then
        echo "$fixture tested nothing" >&2
        failed=1
      fi
    done
  fi
  printf 'corpus\t%s\t%s\n' "$id" "$(($(date +%s%3N) - started))" >> "$timings"
  echo "::endgroup::"
done
exit "$failed"
