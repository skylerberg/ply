# The front-end row, re-taken: what will be measured, and what will count as an answer

Written before any number was taken. `docs/BOOTSTRAP-PATH.md` step 3 is gated on
this row, and ADR 0030 fixed the protocol: counterbalanced arms with a null
control, minimum user CPU over blocks, timed from outside the process, load
recorded on both sides, the binary checked current before and after, no series
spanning a rebuild, and a series taken above the load gate reported as an
observation rather than a figure. `run.sh` is that protocol; `analyze.py` is the
only thing that turns its raw file into the numbers.

## Question

ADR 0030 took this row with a code generator that refused every lambda and every
callback, so the machine could enter only leaves, and every backend arm lost to
no backend. Step 2 lowered the callback family. Does entering at the roots the
narrow registry still excludes now beat no backend?

## Workload

The parser spike's six modules parsing every `examples/*.ply` file as a byte
literal — ADR 0030's workload — as one project with one `test` per example file,
labelled `row: <file>`, selected with `--filter "row:"` so the spike's own unit
tests are excluded. `--no-cache`, because a backend run never reads the result
cache anyway and the arms must not differ in what they read. The front-end cache
is warmed once before the series and left warm, so the fixed cost is small and
the same in every arm; the floor arm below measures it.

## Arms

| arm | command |
| --- | --- |
| `none` | `ply test <dir> --no-cache --filter row:` |
| `null` | the same command, under a different label — the control |
| `narrow` | `PLY_CODEGEN_REGISTER=narrow … --backend cranelift` (the scalar-signature registry ADR 0030 shipped) |
| `wide` | `… --backend cranelift` (every compiled function; the seam still admits each call by its carried types — the default since this row was taken; `observation-*.txt` were taken when `all` was the knob's spelling) |
| `floor` | `ply test <dir> --no-cache --filter row:nothing-matches` — the run's fixed cost, no test body run |

Every arm sits in every position: block *i* starts at arm *i* of the four timed
arms and proceeds in rotation. Five blocks when one run is under two seconds,
three otherwise, decided from the first run and held. Every run printed, nothing
discarded. Entries and declines are read once per backend arm from `--json`.

## Statistic

Minimum user CPU seconds per arm over the blocks; wall beside it. The null
control's distance from `none` is the resolution: a difference smaller than it
is noise.

## Load gate

`run.sh` refuses to start unless the one-minute load average is under 4, and
reports the load after the series; a series whose after-load is above 4 is an
observation, not a figure.

## Decision rule

`wideWins = true` iff `min_user(wide) < min_user(none) − |min_user(none) − min_user(null)|`
and `min_user(wide) < min_user(narrow)`. If it holds, the registry's default
becomes the wide one; if it does not, the default stays narrow and the row is
recorded either way in `docs/BOOTSTRAP-PATH.md` step 3.

## Predictions, registered

1. `narrow` stays within the control of `none`: the same leaves as ADR 0030
   measured, and few of them.
2. `wide` enters at `items.parse` and the entry count falls by orders of
   magnitude against ADR 0030's wide arm, because the root is now compilable.
3. `wide` beats `none`. If it does not, the boundary cost ADR 0030 listed per
   entry is being paid inside the callbacks — every `rt_call` re-enters through
   the same handle arena — and the next lever is that cost, not another
   construct.
