#!/usr/bin/env python3
"""PRE-REGISTERED.md's statistic and decision rule over a raw file, and nothing else.

    ./analyze.py observation-2.txt
"""
import re
import sys

runs, inner, load = {}, {}, {}
for line in open(sys.argv[1]).read().splitlines():
    parts = line.split()
    if len(parts) == 5 and parts[1] == "user" and parts[3] == "wall":
        runs.setdefault(parts[0], []).append((float(parts[2]), float(parts[4])))
    elif len(parts) == 4 and parts[1] == "inside":
        inner.setdefault(parts[0], []).append(float(parts[3]))
    elif parts[:1] and parts[0] in ("load-before", "load-after"):
        load[parts[0]] = float(parts[1])

if not runs:
    sys.exit("no timed arms in that file")


def best(arm):
    """Minimum over repeats: the loader's own clock where it took one, else wall."""
    if arm in inner:
        return min(inner[arm])
    return min(w for _, w in runs[arm])


def sized(prefix):
    """Every `<prefix>-<N>` arm, by N."""
    out = {}
    for arm in list(runs) + list(inner):
        m = re.fullmatch(rf"{prefix}-(\d+)", arm)
        if m:
            out[int(m.group(1))] = best(arm)
    return dict(sorted(out.items()))


print("per changed definition (ms, minimum over repeats)")
for arm in ("spawn", "syntax-only", "unit-O0", "unit-O1", "unit-O2",
            "unit-dylib-O0", "unit-link-only"):
    if arm in runs:
        user = min(u for u, _ in runs[arm])
        print(f"  {arm:<16} wall {best(arm):8.1f}   user {user:8.1f}")
if "spawn" in runs and "unit-O0" in runs:
    print(f"  resolution (spawn): {best('spawn'):.1f} ms;"
          f" a changed definition costs at least {best('unit-O0'):.1f} ms")

print("\nper reached definition (ms, minimum over repeats; per definition beside it)")
for prefix in ("link", "dlopen-whole-first", "dlopen-whole-warm",
               "dlopen-bundle-first", "dlopen-bundle-warm",
               "dlopen-flat-first", "dlopen-flat-warm"):
    points = sized(prefix)
    if not points:
        continue
    row = "  ".join(f"{n}: {ms:8.1f} ({ms / n * 1e3:6.1f} us/def)"
                    for n, ms in points.items())
    print(f"  {prefix:<20} {row}")
    if len(points) >= 2:
        ns = list(points)
        lo, hi = ns[0], ns[-1]
        growth = (points[hi] / points[lo]) / (hi / lo)
        verdict = "linear" if 0.5 <= growth <= 2.0 else (
            "superlinear" if growth > 2.0 else "sublinear")
        print(f"  {'':<20} {lo} -> {hi}: cost x{points[hi] / points[lo]:.1f}"
              f" for size x{hi / lo:.0f} -- {verdict}")

if load:
    print(f"\nload before {load.get('load-before')} after {load.get('load-after')}"
          + ("  -- above the gate; an observation, not a figure"
             if max(load.values(), default=0) >= 4.0 else ""))
