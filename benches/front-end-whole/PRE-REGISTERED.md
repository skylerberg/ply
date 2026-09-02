# The whole front end, phase by phase: what will be measured, and what will count as an answer

Written before any number was taken. `docs/BOOTSTRAP-PATH.md` step 7 waits on
this row, and ADR 0030 fixed the protocol: counterbalanced arms with a null
control, minimum user CPU over blocks, timed from outside the process, load
recorded on both sides, the binary checked current before and after, no series
spanning a rebuild, and a series taken above the load gate reported as an
observation rather than a figure. `run.sh` is that protocol; `analyze.py` is the
only thing that turns its raw file into the numbers.

## Question

Every phase of the front end now exists in Ply and agrees with the reference —
parse, the rewrites, derive expansion, resolve, check and hash. Step 3's row
priced the parser alone. What does the *whole* front end cost through the
interpreter, where in it does the time go, and how much of it does the backend
take — which is what decides whether the driver is worth porting at all.

## Workload

The parser spike's twelve modules and `examples/desk.ply` with every module of
the standard library, all as byte literals, in one project with six tests
labelled `row: <phase>`:

| test | what it runs, over the same program each time |
| --- | --- |
| `row: parse` | `parse` and the rewrites of every module |
| `row: expand` | the above and derive expansion |
| `row: tables` | the above and the resolver's tables — the index, declarations, scopes and order — without the defaults pass |
| `row: resolve` | the above and the defaults pass: `resolve_modules` whole |
| `row: check` | the above and `check_program` |
| `row: hash` | everything through `resolve`, then `hash_index` — hashing reads the resolved tree, not the checked one |

Each test re-runs the phases its own depends on, so a phase's cost is its
test's distance from the test it builds on, and the whole front end is the
`check` test plus the `hash` phase. Selected with `--filter "row:"` so the
spike's own unit tests are excluded;
`--no-cache`, because a backend run never reads the result cache and the arms
must not differ in what they read. The front-end cache is warmed once before the
series and left warm; the floor arm measures the fixed cost.

## Arms

| arm | command |
| --- | --- |
| `none` | `ply test <dir> --no-cache --filter row:` |
| `null` | the same command, under a different label — the control |
| `wide` | `… --backend cranelift` — every compiled function registered, which step 3's row made the default |
| `floor` | `ply test <dir> --no-cache --filter row:nothing-matches` — the run's fixed cost, no test body run |

`narrow` is not an arm: step 3's row retired it three times over. Every arm sits
in every position: block *i* starts at arm *i* of the three timed arms and
proceeds in rotation. Three blocks, because one run is far over two seconds.
Every run printed, nothing discarded. Per-test durations are read once per arm
from `--json`, which is the phase breakdown, and entries and declines once per
backend arm.

## Statistic

Minimum user CPU seconds per arm over the blocks; wall beside it. The null
control's distance from `none` is the resolution. The phase breakdown is each
test's own duration from the `--json` report of the block with the minimum, as
consecutive differences.

## Load gate

`run.sh` refuses to start unless the one-minute load average is under 4, and
reports the load after the series. Step 3's three series established that a
series of ten-worker test runs lifts the one-minute average past 4 by itself on
an idle machine; the workload here is four tests, so the workers are four and
the after-load says what the series did to the machine. A series whose
after-load is above 4 is an observation, not a figure, as before.

## Decision rule

`backendPays = true` iff `min_user(wide) < min_user(none) − |min_user(none) − min_user(null)|`.
Whatever it says, the phase breakdown is recorded in `docs/BOOTSTRAP-PATH.md`
step 7, and the decision that step names is taken on it: if the front end under
the backend is within a small factor of the Rust front end's known cost the
driver stays Rust and drives a Ply front end; if it is not, the next lever is
the phase the breakdown names, not the driver.

## Between the series

Recorded so each series is read against the tree it ran in. A dry run of the
probe, one worker, came first and changed two things. The probe called its
`parsed()` constant once per module inside the later rows, and the interpreter
re-parsed the standard library on every call: the memo gave up on a value past
a fixed depth, and a parsed module tree is deeper, so the tables row read at
many times the parse row. The walk is now iterative and unbounded
(`ply_eval::memo::world_independent`), and the probe binds the constant once
regardless. The resolver's name lookups were also moved from per-module lists
onto maps before the cause was found; after both changes the tables are a small
fraction of the parse.

`observation-1.txt` is the first series. It confirmed prediction 3 — the
checker barely moved under the backend — for a reason the prediction did not
give: the fragment refused a `let` binding a record pattern, two of the
checker's functions bind a tuple that way, and a refused callee refuses every
caller, so the whole checker was outside the compiled unit and ran interpreted
(`parser_census::the_census_over_the_parser_spike` names each refused function
now, which is how this was found). The fragment lowers a `let` over any
irrefutable pattern since, the checker's roots are in the unit, and
`observation-2.txt` is the series over that tree — the one step 7 reads.

## Predictions, registered

1. Checking dominates: the checker is the phase that walks every expression with
   a substitution, and its port keeps a threaded state that is copied on every
   step.
2. Hashing is second, and most of it is BLAKE3 in Ply rather than the encoding.
3. The backend enters the parser's roots as step 3's row showed, and enters
   little of the checker, because the checker's state is a record whose fields
   include maps of records of variants — carried, but the bodies that walk it
   call `map_fold`, `iterate` and lambdas over it, and every one of those
   re-enters through the seam.
