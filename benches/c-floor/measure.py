"""The timed half of run.sh: every arm, every repeat printed, then the minimum per arm.

Wall is a monotonic clock around the spawn; user is the children's rusage delta,
so a clang driver's `-cc1` child is counted. Both in milliseconds.
"""
import resource
import shutil
import subprocess
import sys
import time
from collections import defaultdict

work, cc, repeats_unit, repeats_load, *sizes = sys.argv[1:]
repeats_unit, repeats_load = int(repeats_unit), int(repeats_load)
sizes = [int(s) for s in sizes]
runs = defaultdict(list)


def timed(label, argv):
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    t0 = time.perf_counter()
    out = subprocess.run(argv, cwd=work, capture_output=True, text=True)
    wall = (time.perf_counter() - t0) * 1e3
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    if out.returncode != 0:
        sys.exit(f"{label}: exit {out.returncode}\n{out.stderr}")
    user = (after.ru_utime - before.ru_utime) * 1e3
    runs[label].append((user, wall))
    print(f"{label} user {user:.2f} wall {wall:.2f}", flush=True)
    return out.stdout


for _ in range(repeats_unit):
    timed("spawn", [cc, "-c", "empty.c", "-o", "empty.o"])
    timed("syntax-only", [cc, "-fsyntax-only", "unit.c"])
    timed("unit-O0", [cc, "-O0", "-c", "unit.c", "-o", "unit0.o"])
    timed("unit-O1", [cc, "-O1", "-c", "unit.c", "-o", "unit1.o"])
    timed("unit-O2", [cc, "-O2", "-c", "unit.c", "-o", "unit2.o"])
    timed("unit-dylib-O0", [cc, "-O0", "-shared", "-undefined", "dynamic_lookup",
                            "-o", "unit0.dylib", "unit.c"])
    timed("unit-link-only", [cc, "-shared", "-undefined", "dynamic_lookup",
                             "-o", "unit0b.dylib", "unit0.o"])

def fresh(n):
    """Copies of the bundles: a fresh file is validated again on its first load."""
    names = []
    for i in range(n):
        shutil.copyfile(f"{work}/u{i}.bundle", f"{work}/f{i}.bundle")
        names.append(f"./f{i}.bundle")
    return names


def load(label, images):
    inner = timed(label, ["./dl", *images])
    print(f"{label} inside {inner.strip()}", flush=True)


per_image_sizes = [n for n in sizes if n <= 1000]
for n in sizes:
    objs = [f"u{i}.o" for i in range(n)]
    for _ in range(repeats_load):
        timed(f"link-{n}", [cc, "-shared", "-undefined", "dynamic_lookup",
                            "-o", f"all{n}.dylib", *objs])
        load(f"dlopen-whole-first-{n}", [f"./all{n}.dylib"])
        load(f"dlopen-whole-warm-{n}", [f"./all{n}.dylib"])
for n in per_image_sizes:
    # A first load costs a tenth of a second per image here, so the largest per-image
    # size takes its first load once.
    for r in range(repeats_load if n < max(per_image_sizes) else 1):
        bundles = fresh(n)
        load(f"dlopen-bundle-first-{n}", bundles)
        load(f"dlopen-bundle-warm-{n}", bundles)
    libs = [f"./u{i}.dylib" for i in range(n)]
    load(f"dlopen-flat-first-{n}", libs)
    for _ in range(repeats_load):
        load(f"dlopen-flat-warm-{n}", libs)

print("summary (minimum over repeats, ms)")
for label, rows in runs.items():
    user = min(u for u, _ in rows)
    wall = min(w for _, w in rows)
    print(f"  {label:18} user {user:8.2f} wall {wall:8.2f}")
