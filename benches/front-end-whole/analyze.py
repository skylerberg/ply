#!/usr/bin/env python3
"""PRE-REGISTERED.md's statistic and decision rule over `raw.txt`, and nothing else."""
import sys

raw = open(sys.argv[1]).read().splitlines()
runs = {}
floor = None
counts = {}
phases = {}
load = {}
for line in raw:
    parts = line.split()
    if not parts:
        continue
    if parts[0] == "block":
        _, _, arm, user, wall = parts
        runs.setdefault(arm, []).append((float(user), float(wall)))
    elif parts[0] == "floor":
        floor = (float(parts[1]), float(parts[2]))
    elif parts[0] == "counts":
        counts[parts[1]] = dict(kv.split("=", 1) for kv in parts[2:])
    elif parts[0] == "phases":
        phases[parts[1]] = dict(kv.split("=", 1) for kv in parts[2:] if "=" in kv)
    elif parts[0] in ("load-before", "load-after"):
        load[parts[0]] = float(parts[1])

best = {arm: min(rs) for arm, rs in runs.items()}
none, null = best["none"][0], best["null"][0]
resolution = abs(none - null)
wide = best["wide"][0]
backend_pays = wide < none - resolution

print(f"{'arm':>8} {'min user':>9} {'min wall':>9}  runs")
for arm in ("none", "null", "wide"):
    users = " ".join(f"{u:.2f}" for u, _ in runs[arm])
    print(f"{arm:>8} {best[arm][0]:>9.2f} {best[arm][1]:>9.2f}  {users}")
if floor:
    print(f"{'floor':>8} {floor[0]:>9.2f} {floor[1]:>9.2f}")
print(f"resolution (|none - null|): {resolution:.2f}s user")
order = ["parse", "expand", "tables", "resolve", "check", "hash"]
for arm, p in phases.items():
    try:
        cumulative = [float(p[name]) / 1000.0 for name in order]
    except (KeyError, ValueError):
        print(f"{arm}: phases unread ({p})")
        continue
    steps = [cumulative[0]] + [b - a for a, b in zip(cumulative, cumulative[1:])]
    print(f"{arm}: " + "  ".join(f"{name} {s:.2f}s" for name, s in zip(order, steps)) +
          f"  (whole {cumulative[-1]:.2f}s wall, one worker each)")
for arm, c in counts.items():
    print(f"{arm}: " + " ".join(f"{k}={v}" for k, v in c.items()))
gate = ""
if load.get("load-after", 0.0) > 4.0:
    gate = "  -- ABOVE THE GATE AFTER: an observation, not a figure"
print(f"load before {load.get('load-before')} after {load.get('load-after')}{gate}")
print(f"backendPays = {'true' if backend_pays else 'false'}")
