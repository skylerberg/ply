#!/usr/bin/env python3
"""Applies benches/r5-timing/PRE-REGISTERED.md to an mcts report. Nothing here
chooses anything: the filter, the statistic and the thresholds were fixed
before the data existed."""
import json, sys

LOAD_MAX = 4.5      # pre-registered
STALL_FACTOR = 1.5  # pre-registered
MIN_WINDOWS = 11    # pre-registered
BAR = 1.10          # pre-registered

def pct(xs, q):
    xs = sorted(xs)
    return xs[round((len(xs)-1)*q)]

def filter_windows(w):
    a, r, l = w['a_micros'], w['ratios'], w['loads']
    amin = min(a)
    keep, drop_stall, drop_load = [], 0, 0
    for i in range(len(r)):
        if a[i] > STALL_FACTOR*amin: drop_stall += 1; continue
        if l[i] > LOAD_MAX:          drop_load  += 1; continue
        keep.append(r[i])
    return keep, drop_stall, drop_load

d = json.load(open(sys.argv[1]))
print("== load average at each window of the reported rung ==")
top = [r for r in d['rungs'] if r['name'] == 'everything the fragment accepts'][0]
print("   ", [round(x,2) for x in top['windows']['loads']])
print("    GATE (load at the first window of the ladder): %.2f  threshold 4.50  -> %s"
      % (d['rungs'][0]['windows']['loads'][0],
         "PASS" if d['rungs'][0]['windows']['loads'][0] <= LOAD_MAX else "REFUSE"))

print("\n== controls (must land in 0.95-1.05 or the run is void) ==")
ctl = [r for r in d['rungs'] if r['name'] == 'control: nothing enterable'][0]
for label, w, extra in (("control: nothing enterable", ctl['windows'],
                         "entries %.0f, declines %.0f" % (ctl['entries_per_call'], ctl['declines_per_call'])),
                        ("harness_floor", d['harness_floor'], "")):
    k, ds, dl = filter_windows(w)
    med = pct(k, 0.5) if k else float('nan')
    ok = 0.95 <= med <= 1.05
    print("   %-28s median %.4fx over %d/%d windows  %s   %s"
          % (label, med, len(k), len(w['ratios']), "IN RANGE" if ok else "OUT OF RANGE", extra))

print("\n== the ladder ==")
for r in d['rungs']:
    k, ds, dl = filter_windows(r['windows'])
    if len(k) < MIN_WINDOWS:
        print("   %-32s VOID: only %d of %d windows survived" % (r['name'], len(k), len(r['windows']['ratios'])))
        continue
    print("   %-32s %7.3fx  [%.3f, %.3f]  %2d/%2d windows (%d stalled, %d loaded)  entries/call %.0f  declines/call %.0f"
          % (r['name'], pct(k,0.5), pct(k,0.10), pct(k,0.90), len(k), len(r['windows']['ratios']),
             ds, dl, r['entries_per_call'], r['declines_per_call']))

k, ds, dl = filter_windows(top['windows'])
med, lo, hi = pct(k,0.5), pct(k,0.10), pct(k,0.90)
print("\n== THE REPORTED NUMBER ==")
print("   top rung 'everything the fragment accepts': %.3fx  10th-90th [%.3f, %.3f]" % (med, lo, hi))
print("   entries during the timed run: %.0f" % top['entries_per_call'])
print("   surviving windows: %d of %d" % (len(k), len(top['windows']['ratios'])))
verdict = ("entry-paid-off" if (med >= BAR and lo >= 1.00 and top['entries_per_call'] > 0)
           else "inconclusive" if top['entries_per_call'] == 0
           else "entry-did-not-pay-off")
print("   PRE-REGISTERED VERDICT: %s" % verdict)

print("\n== worst per-function (pre-registered: the worst, not the mean) ==")
rows = []
for f in d['per_function']:
    k, ds, dl = filter_windows(f['windows'])
    if not k: continue
    rows.append((pct(k,0.5), pct(k,0.10), pct(k,0.90), f['function'],
                 f['micros_per_interpreted_call'], f['entries_in_timed_run'], len(k), len(f['windows']['ratios'])))
rows.sort()
for med_, lo_, hi_, name, us, ent, nk, nt in rows:
    flag = "  <-- REGRESSED" if med_ < 1.0 else ""
    print("   %-26s %7.3fx  [%.3f, %.3f]  %9.3f us/call  %7d entries  %2d/%2d win%s"
          % (name, med_, lo_, hi_, us, ent, nk, nt, flag))
if d['per_function_not_timed']:
    print("   NOT TIMED:", d['per_function_not_timed'])
print("\n   WORST: %s at %.3fx" % (rows[0][3], rows[0][0]) if rows else "   no per-function rows survived")
