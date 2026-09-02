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
# Each row re-runs the rows its phase depends on; `hash` reads the resolved tree, not the
# checked one, so its phase is its distance from `resolve` and the whole is `check` plus that.
parent = {"parse": None, "expand": "parse", "tables": "expand", "resolve": "tables",
          "check": "resolve", "hash": "resolve"}
for arm, p in phases.items():
    try:
        row = {name: float(p[name]) / 1000.0 for name in parent}
    except (KeyError, ValueError):
        print(f"{arm}: phases unread ({p})")
        continue
    steps = {name: row[name] - (row[above] if above else 0.0) for name, above in parent.items()}
    whole = row["check"] + steps["hash"]
    print(f"{arm}: " + "  ".join(f"{name} {s:.2f}s" for name, s in steps.items()) +
          f"  (whole {whole:.2f}s wall, one worker each)")
for arm, c in counts.items():
    print(f"{arm}: " + " ".join(f"{k}={v}" for k, v in c.items()))
gate = ""
if load.get("load-after", 0.0) > 4.0:
    gate = "  -- ABOVE THE GATE AFTER: an observation, not a figure"
print(f"load before {load.get('load-before')} after {load.get('load-after')}{gate}")
print(f"backendPays = {'true' if backend_pays else 'false'}")
