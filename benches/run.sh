#!/usr/bin/env bash
# Generate a corpus at each size and print the phase split for every scenario.
#
# Sizes are `modules,defs_per_module,tests`. Generated corpora are not tracked:
# a corpus is a function of its seed, so the seed is the artifact worth keeping
# rather than the megabytes it expands to.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(dirname "$here")"
out="${PLY_BENCH_OUT:-$here/corpora}"
seed="${PLY_BENCH_SEED:-1}"
repeats="${PLY_BENCH_REPEATS:-1}"

sizes=("${@}")
if [ ${#sizes[@]} -eq 0 ]; then
  sizes=(10,25,125 20,25,250 40,25,500 80,25,1000 160,25,2000 200,50,5000)
fi

cargo build --release --manifest-path "$root/Cargo.toml" -p ply-corpus
bin="$root/target/release/ply-corpus"

for size in "${sizes[@]}"; do
  IFS=, read -r modules defs tests <<<"$size"
  dir="$out/m${modules}_d${defs}_t${tests}"
  echo "=== $size"
  "$bin" gen --out "$dir" --seed "$seed" \
    --modules "$modules" --defs-per-module "$defs" --tests "$tests" \
    --depth "$(( modules < 6 ? modules : 6 ))"
  "$bin" bench "$dir" --repeats "$repeats"
done
