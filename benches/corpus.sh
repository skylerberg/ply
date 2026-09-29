#!/usr/bin/env bash
# The corpus, run as the program it is: `ply run` over `crates/ply-corpus/ply` with the grants it
# needs, and the arguments passed through. Paths on the line are relative to the working
# directory, which is the program's `work` root; `real` reads the toolchain's trees under it, so it
# runs from the repository's root.
#
#   benches/corpus.sh gen --out corpora/m20 --modules 20
#   benches/corpus.sh bench corpora/m20 --json
#   benches/corpus.sh tiers --out corpora/tiers
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
bin="${CARGO_TARGET_DIR:-$root/target}/release"

cargo build --release --quiet --manifest-path "$root/Cargo.toml" \
  -p ply-launcher --bin ply -p ply-corpus --bin ply-corpus

exec "$bin/ply" run "$root/crates/ply-corpus/ply" --host --allow machine --allow claims \
  --exec "ply=$bin/ply" --exec "executor=$bin/ply-corpus" \
  --fs work=. --fs "repo=$root" -- "$@"
