#!/usr/bin/env python3
"""PRE-REGISTERED.md's statistic and decision rule over `raw.json`, and nothing else.

    ./analyze.py raw.json
"""
import json
import sys

raw = json.load(open(sys.argv[1]))
rows = raw["rows"]
sizes, engines = [], []
for row in rows:
    if row["size"] not in sizes:
        sizes.append(row["size"])
    if row["engine"] not in engines and row["engine"] != "process":
        engines.append(row["engine"])

SCENARIOS = ["cold", "warm", "rename", "edit-leaf", "edit-hub"]


def growth(lo, hi):
    """`hi / lo`, or None where the small end is at or below the noise floor.

    A marginal cost of a millisecond or less cannot carry a ratio: the denominator is the
    measurement's own resolution, and dividing by it manufactures a slope.
    """
    if lo is None or hi is None or lo <= 1.0:
        return None
    return hi / lo


def fmt_growth(lo, hi):
    g = growth(lo, hi)
    return f"x{g:.1f}" if g is not None else "-"


def verdict_of(ratios):
    """Read over the steps that could be fitted at all, never over one endpoint pair."""
    if not ratios:
        return "flat or unfittable — the marginal cost never rises above the noise floor"
    worst = max(g / r for g, r in ratios)
    best = max(g for g, _ in ratios)
    if best < 2.0:
        return "flat"
    if 0.5 <= worst <= 2.0:
        return "proportional to the project"
    return "grows, between flat and proportional"


def step(sizes, i, j=None):
    j = i + 1 if j is None else j
    return f"{definitions(sizes[i])}->{definitions(sizes[j])}"


def report_of(size, engine):
    for row in rows:
        if row["size"] == size and row["engine"] == engine:
            return row["report"]
    return None


def scenario_of(report, name):
    for s in report["scenarios"]:
        if s["name"] == name:
            return s
    return None


def definitions(size):
    m, d, _ = (int(x) for x in size.split(","))
    return m * d


def phase(scenario, name):
    for p in scenario["phases"]:
        if p["phase"] == name:
            return p["millis"]
    return 0.0


for engine in engines:
    label = "the interpreter" if engine == "none" else f"--backend {engine}"
    print(f"\n=== {label}")
    print(f"{'scenario':<10} " + " ".join(f"{definitions(s):>9}" for s in sizes)
          + "   (total ms, minimum over repeats)")
    for name in SCENARIOS:
        cells = []
        for size in sizes:
            report = report_of(size, engine)
            s = scenario_of(report, name) if report else None
            cells.append(f"{s['total_millis']:9.1f}" if s else f"{'-':>9}")
        print(f"{name:<10} " + " ".join(cells))

    print(f"\n{'scenario':<10} " + " ".join(f"{definitions(s):>9}" for s in sizes)
          + "   (marginal ms: total minus warm)")
    marginal = {}
    for name in SCENARIOS:
        cells, values = [], []
        for size in sizes:
            report = report_of(size, engine)
            s = scenario_of(report, name) if report else None
            warm = scenario_of(report, "warm") if report else None
            if s and warm:
                v = s["total_millis"] - warm["total_millis"]
                values.append(v)
                cells.append(f"{v:9.1f}")
            else:
                values.append(None)
                cells.append(f"{'-':>9}")
        marginal[name] = values
        print(f"{name:<10} " + " ".join(cells))

    print("\nfit, each adjacent step and then end to end:")
    print(f"  {'scenario':<10} " + " ".join(f"{step(sizes, i):>14}" for i in range(len(sizes) - 1))
          + f" {step(sizes, 0, -1):>14}   verdict")
    for name in SCENARIOS:
        cells, ratios = [], []
        for i in range(len(sizes) - 1):
            cells.append(fmt_growth(marginal[name][i], marginal[name][i + 1]))
        cells.append(fmt_growth(marginal[name][0], marginal[name][-1]))
        for i in range(len(sizes) - 1):
            g = growth(marginal[name][i], marginal[name][i + 1])
            if g is not None:
                ratios.append((g, definitions(sizes[i + 1]) / definitions(sizes[i])))
        print(f"  {name:<10} " + " ".join(f"{c:>14}" for c in cells) + f"   {verdict_of(ratios)}")

    print("\nwhere a warm run's time goes:")
    for size in sizes:
        report = report_of(size, engine)
        warm = scenario_of(report, "warm") if report else None
        if not warm:
            continue
        parts = " ".join(f"{n} {phase(warm, n):.1f}" for n in
                         ("parse", "typecheck", "hash", "select", "compile", "execute"))
        print(f"  {definitions(size):>6} defs: {parts}   total {warm['total_millis']:.1f}")

process = [r for r in rows if r["engine"] == "process"]
if process:
    print("\n=== the real command, warm: nothing changed and nothing rechecked")
    for r in process:
        line = f"  {definitions(r['size']):>6} defs: warm `ply test` {r['warm_wall_seconds']}s wall"
        fe = r.get("warm_front_end")
        if fe:
            ph = fe["phases"]
            line += (f"   front end {ph['total']:.1f}ms"
                     f" (hash {ph['hash']:.1f} parse {ph['parse']:.1f}"
                     f" restore {ph['restore']:.1f} write-back {ph['write_back']:.1f})"
                     f"   rechecked {fe['rechecked']}, cached {fe['cached']}")
        print(line)
    fits = [(definitions(r["size"]), r["warm_front_end"]["phases"]["total"])
            for r in process if r.get("warm_front_end")]
    if len(fits) >= 2:
        ratios = []
        parts = []
        for (n0, t0), (n1, t1) in zip(fits, fits[1:]):
            parts.append(f"{n0}->{n1}: x{t1 / t0:.1f} for x{n1 / n0:.0f} the project")
            ratios.append((t1 / t0, n1 / n0))
        print("\n  the fixed cost, each adjacent step: " + "   ".join(parts))
        print(f"  {verdict_of(ratios)}")
        print("  Nothing was rechecked at any size, so this is the cost of establishing that,")
        print("  paid again by every invocation. It is what a warm process would not pay.")

backed = [r for r in process if r.get("backend")]
if backed:
    print("\n=== what the code generator charges, per definition it compiles")
    for r in backed:
        b = r["backend"]
        units, fragment = b.get("units", 0), b.get("fragment", 0)
        codegen_ms = b.get("codegen_nanos", 0) / 1e6
        per_build = codegen_ms / units if units else 0.0
        per_def = per_build * 1e3 / fragment if fragment else 0.0
        print(f"  {definitions(r['size']):>6} defs: fragment {fragment:>5}"
              f"   {units:>3} build(s) of it, {codegen_ms:8.1f}ms in all"
              f"   {per_build:7.1f}ms each   {per_def:6.0f} us per definition"
              f"   deciding what to compile {b.get('analysis_nanos', 0) / 1e6:.1f}ms")
    print("  The unit is built once as a pre-flight and again per worker, so `builds` is the")
    print("  worker count plus one and the total is that many whole-project compiles.")
    print("  `benches/c-floor/` holds what a C toolchain charges for the same unit of work.")

after = raw.get("load_after")
print(f"\nload before {raw.get('load_before')} after {after}"
      + ("  -- above the gate; an observation, not a figure"
         if after is not None and float(after) >= 4.0 else ""))
