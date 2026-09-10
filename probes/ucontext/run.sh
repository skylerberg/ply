#!/usr/bin/env bash
# The two probes ADR 0044 ran to pick its primitive, as a check: the C library's context
# functions switch stacks here, and a stack snapshot restored in place resumes the same frame
# twice. The runtime's own switcher is tested in `crates/ply-codegen/src/stack.rs`; this keeps the
# record's commands true on the platforms CI builds on.
#
# It sat under `spikes/` while that word still applied to it. The decision it informed is landed
# and the switcher it chose is shipping, so what is left is a platform assumption worth checking
# on every runner -- which is a probe, not a spike.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
out="$(mktemp -d)"
trap 'rm -rf "$out"' EXIT

cc -w -o "$out/uctx" "$here/uctx.c"
"$out/uctx" | tee "$out/uctx.log"
grep -q "multi-shot by in-place restore: ok" "$out/uctx.log"

cc -O2 -w -o "$out/uctx_bench" "$here/uctx_bench.c"
"$out/uctx_bench"
