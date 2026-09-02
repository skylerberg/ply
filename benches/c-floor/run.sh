#!/usr/bin/env bash
# What a C toolchain charges per changed definition and per reached definition,
# on units shaped like emitted Ply code: PRE-REGISTERED.md's protocol and nothing
# else. Writes benches/c-floor/raw.txt.
#
#   ./benches/c-floor/run.sh
#
# Builds its fixtures, waits for the load gate, and refuses if it is never met (exit 3).
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
raw="$here/raw.txt"
cc="${CC:-cc}"
sizes=(250 1000 4000)
repeats_unit=20
repeats_load=3

load1() { uptime | sed 's/.*load averages*: *//' | awk -F'[ ,]+' '{print $1}'; }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
helpers=$(grep -c 'extern "C" fn' "$root/crates/ply-codegen/src/rt.rs")

# --- the fixtures ------------------------------------------------------------
# A header the size of the runtime's helper table, and one unit per definition:
# a function that tests a constructor, reads fields, calls helpers, builds a
# record and answers. Distinct names and constants per unit.
{
  echo '#include <stdint.h>'
  echo 'typedef struct { uint64_t w; } Value;'
  for ((i = 0; i < helpers; i++)); do echo "Value rt_h$i(Value, Value);"; done
  echo 'int64_t rt_tag(Value); Value rt_field(Value, int64_t); Value rt_alloc(int64_t);'
  echo 'void rt_set(Value, int64_t, Value); _Noreturn void rt_fail(const char *);'
} > "$work/rt.h"
unit() {                          # $1 index -> the unit's source on stdout
  local i=$1
  echo '#include "rt.h"'
  echo "Value ply_def_$i(Value a, Value b) {"
  echo "  switch (rt_tag(a)) {"
  for k in 0 1 2; do
    echo "  case $k: {"
    echo "    Value x = rt_field(a, $k), y = rt_field(b, $((k + 1)));"
    echo "    Value r = rt_alloc(3);"
    echo "    rt_set(r, 0, rt_h$(( (i + k) % helpers ))(x, y));"
    echo "    rt_set(r, 1, rt_h$(( (i * 7 + k) % helpers ))(y, x));"
    echo "    rt_set(r, 2, (Value){ x.w + $i + $k });"
    echo "    return r; }"
  done
  echo "  default: rt_fail(\"ply_def_$i\"); }"
  echo "}"
}
: > "$work/empty.c"
unit 0 > "$work/unit.c"
top=${sizes[$((${#sizes[@]} - 1))]}
for ((i = 0; i < top; i++)); do unit "$i" > "$work/u$i.c"; done

cat > "$work/dl.c" <<'C'
#include <dlfcn.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>
static double now(void) { struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t); return t.tv_sec + t.tv_nsec / 1e9; }
int main(int argc, char **argv) {
  double t0 = now();
  for (int i = 1; i < argc; i++) {
    void *h = dlopen(argv[i], RTLD_NOW | RTLD_LOCAL);
    if (!h) { fprintf(stderr, "%s: %s\n", argv[i], dlerror()); return 1; }
    char name[64]; snprintf(name, sizeof name, "ply_def_%d", i - 1);
    if (!dlsym(h, name) && !dlsym(h, "ply_def_0")) { fprintf(stderr, "no symbol in %s\n", argv[i]); return 1; }
  }
  printf("dlopen-ms %.3f\n", (now() - t0) * 1e3);
  return 0;
}
C
{
  echo '#include "rt.h"'
  echo '#include <stdlib.h>'
  for ((i = 0; i < helpers; i++)); do echo "Value rt_h$i(Value a, Value b) { return (Value){ a.w ^ b.w ^ $i }; }"; done
  echo 'int64_t rt_tag(Value v) { return (int64_t)(v.w & 3); }'
  echo 'Value rt_field(Value v, int64_t i) { return (Value){ v.w + (uint64_t)i }; }'
  echo 'Value rt_alloc(int64_t n) { return (Value){ (uint64_t)n }; }'
  echo 'void rt_set(Value r, int64_t i, Value v) { (void)r; (void)i; (void)v; }'
  echo '_Noreturn void rt_fail(const char *s) { (void)s; abort(); }'
} > "$work/rt_stubs.c"
"$cc" -O2 -Wl,-export_dynamic -o "$work/dl" "$work/dl.c" "$work/rt_stubs.c"

# The objects, per-definition libraries and host-bound bundles the load arms read: built once, not timed.
(cd "$work" && seq 0 $((top - 1)) | xargs -P 4 -I{} sh -c "$cc -O0 -c u{}.c -o u{}.o && $cc -shared -undefined dynamic_lookup -o u{}.dylib u{}.o && $cc -bundle -bundle_loader ./dl -o u{}.bundle u{}.o")

# The fixtures are built above the gate; the timing is not. Spin until quiet, then refuse if it never came.
for ((i = 0; i < 40; i++)); do
  l=$(load1)
  awk -v l="$l" 'BEGIN{exit !(l < 4.0)}' && break
  sleep 15
done
echo "==> load before: $(uptime)"
awk -v l="$l" 'BEGIN{exit !(l < 4.0)}' || { echo "load $l is above the gate of 4; not measuring" >&2; exit 3; }

: > "$raw"
echo "cc $("$cc" --version | head -1)" >> "$raw"
echo "arch $(uname -m)" >> "$raw"
echo "helpers $helpers" >> "$raw"
echo "load-before $l" >> "$raw"

python3 "$here/measure.py" "$work" "$cc" "$repeats_unit" "$repeats_load" "${sizes[@]}" | tee -a "$raw"

echo "load-after $(load1)" | tee -a "$raw"
echo "==> load after: $(uptime)"
