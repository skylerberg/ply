#!/usr/bin/env bash
# The corpus, run as the program it is: `ply run` over the artifact `benches/corpus-program.sh`
# builds from `crates/ply-corpus/ply`, with the grants it needs, and the arguments passed through.
# Paths on the line are relative to the working directory, which is the program's `work` root;
# `real` reads the toolchain's trees under it, so it runs from the repository's root, and so do
# `serve`, `w3`, `w4`, `w5` and `w6-ladder`, whose `--repo` defaults to it.
#
#   benches/corpus.sh gen --out corpora/m20 --modules 20
#   benches/corpus.sh bench corpora/m20 --json
#   benches/corpus.sh tiers --out corpora/tiers
#   benches/corpus.sh serve --sections layers,scans,load
#   benches/corpus.sh w4 --db postgres://me@localhost/bench
#   benches/corpus.sh w5 --sections drain,deploy
#   benches/corpus.sh w6-ladder --db postgres://postgres@127.0.0.1/desk --out benches
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
bin="${CARGO_TARGET_DIR:-$root/target}/release"

cargo build --release --quiet --manifest-path "$root/Cargo.toml" \
  -p ply-launcher --bin ply -p ply-corpus --bin ply-corpus

program=$("$root/benches/corpus-program.sh" "$bin/ply" "$bin/corpus-program")

# The program starts only what is bound here, so the floors its tables are compared with are built
# here too; `CORPUS_BOUND` names the labels bound, since starting an unbound one ends the run. The
# libpq tool is built only where `pg_config` says where libpq is, and without it the rows that need
# it are inconclusive.
floor="$root/benches/http-floor/floor.c"
if [ "$floor" -nt "$bin/http-floor" ]; then
  cc -O2 -o "$bin/http-floor" "$floor" -lpthread
fi
execs=(--exec "ply=$bin/ply" --exec "executor=$bin/ply-corpus" --exec "http_floor=$bin/http-floor")
bound=http_floor

pg="$root/benches/pg-floor/pg.c"
if command -v pg_config >/dev/null; then
  if [ "$pg" -nt "$bin/pg-floor" ]; then
    cc -O2 -o "$bin/pg-floor" "$pg" -I"$(pg_config --includedir)" -L"$(pg_config --libdir)" -lpq
  fi
  execs+=(--exec "pg_floor=$bin/pg-floor")
  bound="$bound,pg_floor"
fi

exec "$bin/ply" run "$program" --host --allow machine --allow claims "${execs[@]}" \
  --set "CORPUS_BOUND=$bound" \
  --fs work=. --fs "repo=$root" -- "$@"
