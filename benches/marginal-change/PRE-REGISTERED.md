# What one edit costs, and whether it grows with the project

ADR 0021's claim is an exponent: Ply's verification loop is O(the change) and
every toolchain it competes with is O(the project). ADR 0037 registered the row
that reads it as a slope, for both of Ply's engines. This is that row's
protocol; `run.sh` takes it and `analyze.py` is the only thing that turns the
raw file into the numbers.

## Question

What does one edit cost, end to end, and does that cost grow with the size of
the project — under the interpreter, and under the compiled backend?

## Workload

A generated corpus at three sizes from `benches/run.sh`'s ladder, in a ratio of
four and then four again, so a slope can be told from a constant:

| label | `modules,defs_per_module,tests` | definitions |
| --- | --- | --- |
| small | `10,25,125` | 250 |
| mid | `40,25,500` | 1,000 |
| large | `160,25,2000` | 4,000 |

Two points fit any line, so three is the minimum that can refute one.

## Arms

`ply-corpus bench`'s five scenarios, each taken twice over — once with no
backend and once under `--backend cranelift`:

| scenario | the edit |
| --- | --- |
| `cold` | the cache cleared: the O(project) bound on this project |
| `warm` | nothing changed: the run's fixed cost |
| `rename` | a definition renamed corpus-wide, which moves no hash |
| `edit-leaf` | one definition's body, few dependents |
| `edit-hub` | one definition's body, most of the corpus downstream |

The corpus and its cache are restored after each scenario, so the order they
run in cannot leak into the numbers.

## Statistic

`bench`'s own: minimum total milliseconds over repeats per scenario, with the
ten phases beside it. The minimum, because a slower run only ever means the
machine did something else too.

**The marginal cost of a scenario is its total minus `warm`'s**, since `warm` is
what the run costs having changed nothing. That subtraction is the quantity
ADR 0021's claim is about, and it is what the fit below is taken over.

**The process start is not in this row.** `bench` runs the phases in one
process, so what it prices is the work and not the invocation. A separate arm
times `ply test` from outside at each size, which is the fixed cost a real loop
also pays.

## Load gate

`run.sh` refuses to start above a one-minute load average of 4 and records the
load after; a series whose after-load is above 4 is an observation and not a
figure. The binary is checked current before the series.

## Decision rule

Per scenario and per engine, the fit is the ratio of marginal cost between the
smallest and largest sizes against the ratio of the sizes:

- **Flat** if the cost ratio is below 2 for a size ratio of 16 — the marginal
  cost does not follow the project.
- **Proportional** if the cost ratio is within a factor of two of 16.

Then:

- Under the interpreter, `rename` must be flat and `cold` must be proportional.
  `rename` moves no hash, so a run after one has provably nothing to do; if it
  is not flat, the claim this project rests on is wrong at its centre.
- `edit-leaf` is read for where it falls between the two, not against a bar.
- Under the backend, the same test, and the reading that matters is **which
  phase carries any growth**. Growth in `compile` means the per-definition code
  cache is what pays first; a fixed cost that dominates the marginal one means
  the warm process does.

**This rule was sharpened after a smoke test at one size and before any fit
existed.** The smoke test showed the instrument works — a backed `warm` run
selects 2 of 125 rather than 125, and `compile` is the same tens of
milliseconds in every scenario — and it cannot have set a bar about slopes,
because it has one size and a slope needs two. ADR 0037 states the same rule in
the form it was first registered in; the operational difference is that the
subtraction of `warm` is written here and was implied there.
