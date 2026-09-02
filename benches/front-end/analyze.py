#!/usr/bin/env python3
"""PRE-REGISTERED.md's statistic and decision rule over `raw.txt`, and nothing else."""
import json
import sys

raw = open(sys.argv[1]).read().splitlines()
runs = {}
floor = None
counts = {}
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
    elif parts[0] in ("load-before", "load-after"):
        load[parts[0]] = float(parts[1])

best = {arm: min(rs) for arm, rs in runs.items()}
none, null = best["none"][0], best["null"][0]
resolution = abs(none - null)
wide, narrow = best["wide"][0], best["narrow"][0]
wide_wins = wide < none - resolution and wide < narrow

print(f"{'arm':>8} {'min user':>9} {'min wall':>9}  runs")
for arm in ("none", "null", "narrow", "wide"):
    users = " ".join(f"{u:.2f}" for u, _ in runs[arm])
    print(f"{arm:>8} {best[arm][0]:>9.2f} {best[arm][1]:>9.2f}  {users}")
if floor:
    print(f"{'floor':>8} {floor[0]:>9.2f} {floor[1]:>9.2f}")
print(f"resolution (|none - null|): {resolution:.2f}s user")
for arm, c in counts.items():
    print(f"{arm}: " + " ".join(f"{k}={v}" for k, v in c.items()))
print(f"load before {load.get('load-before')} after {load.get('load-after')}"
      + ("  -- ABOVE THE GATE AFTER: an observation, not a figure" if load.get("load-after", 0) >= 4 else ""))
print(f"wideWins = {str(wide_wins).lower()}")

json.dump(
    {
        "best": {arm: {"user": u, "wall": w} for arm, (u, w) in best.items()},
        "runs": {arm: [{"user": u, "wall": w} for u, w in rs] for arm, rs in runs.items()},
        "floor": floor and {"user": floor[0], "wall": floor[1]},
        "resolution": resolution,
        "counts": counts,
        "load": load,
        "wideWins": wide_wins,
    },
    open(sys.argv[1].replace("raw.txt", "row.json"), "w"),
    indent=2,
)
