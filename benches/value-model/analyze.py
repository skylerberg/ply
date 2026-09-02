#!/usr/bin/env python3
"""PRE-REGISTERED.md's statistic and decision rule over `raw.txt`, and nothing else."""
import sys

# ADR 0035's bar: the compiled model within this factor of Rust on both kernels.
BAR = 3.0
KERNELS = ("k1", "k2")

raw = open(sys.argv[1]).read().splitlines()
runs = {}   # (arm, kernel) -> [seconds]
load = {}
for line in raw:
    parts = line.split()
    if not parts:
        continue
    if parts[0] == "block":
        # block <n> <arm> <kernel>=<seconds> ...
        _, _, arm, *pairs = parts
        for kv in pairs:
            kernel, seconds = kv.split("=", 1)
            runs.setdefault((arm, kernel), []).append(float(seconds))
    elif parts[0] in ("load-before", "load-after"):
        load[parts[0]] = float(parts[1])

def best(arm, kernel):
    return min(runs[(arm, kernel)])

print(f"{'kernel':>6} {'ply':>9} {'null':>9} {'rust':>9} {'ratio':>7}  verdict")
stands = True
for k in KERNELS:
    ply, null, rust = best("ply", k), best("null", k), best("rust", k)
    resolution = abs(ply - null) / rust
    ratio = ply / rust
    if abs(ratio - BAR) <= resolution:
        verdict = "undecided (within the resolution of the bar)"
        stands = False
    elif ratio <= BAR:
        verdict = "within the bar"
    else:
        verdict = "OVER the bar"
        stands = False
    print(f"{k:>6} {ply:>9.3f} {null:>9.3f} {rust:>9.3f} {ratio:>7.2f}  {verdict}")
gate = ""
if load.get("load-after", 0.0) > 4.0:
    gate = "  -- ABOVE THE GATE AFTER: an observation, not a figure"
print(f"load before {load.get('load-before')} after {load.get('load-after')}{gate}")
print(f"modelStands = {'true' if stands else 'false'}  (bar {BAR})")
