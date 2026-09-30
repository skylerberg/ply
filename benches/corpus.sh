#!/usr/bin/env bash
# The corpus, run as the program it is: `ply run` over the artifact `benches/corpus-program.sh`
# builds from `crates/ply-corpus/ply`, with the grants it needs, and the arguments passed through.
# Paths on the line are relative to the working directory, which is the program's `work` root;
# `real` reads the toolchain's trees under it, so it runs from the repository's root, and so do
# `serve` and `w3`, whose `--repo` defaults to it.
#
#   benches/corpus.sh gen --out corpora/m20 --modules 20
#   benches/corpus.sh bench corpora/m20 --json
#   benches/corpus.sh tiers --out corpora/tiers
#   benches/corpus.sh serve --sections layers,scans,load
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
bin="${CARGO_TARGET_DIR:-$root/target}/release"

cargo build --release --quiet --manifest-path "$root/Cargo.toml" \
  -p ply-launcher --bin ply -p ply-corpus --bin ply-corpus

program=$("$root/benches/corpus-program.sh" "$bin/ply" "$bin/corpus-program")

# The program starts only what is bound here, so the floor its served tables are compared with is
# built here too.
floor="$root/benches/http-floor/floor.c"
if [ "$floor" -nt "$bin/http-floor" ]; then
  cc -O2 -o "$bin/http-floor" "$floor" -lpthread
fi

exec "$bin/ply" run "$program" --host --allow machine --allow claims \
  --exec "ply=$bin/ply" --exec "executor=$bin/ply-corpus" --exec "http_floor=$bin/http-floor" \
  --fs work=. --fs "repo=$root" -- "$@"
