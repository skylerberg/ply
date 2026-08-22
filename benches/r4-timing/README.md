# R4 §1 wall-clock windows

Raw paired windows behind ADR 0019 §1's wall-clock paragraph. Each row is one
round: load average sampled at the round's *start*, then the `answer` rung in
microseconds for the seam arm and the pool arm.

Taken with `ply-corpus serve --repo . --no-load --repeats 5 --ladder-requests
8000` against two binaries differing only in `argv.rs`'s two function bodies,
with the arm order alternated per round so first-position bias cancels.

`run4.txt` is the pre-registered experiment — its filter (load < 4.5), arm
alternation, statistic and decision rule were fixed before its data existed, and
it is the row the ADR reports. `run2.txt` and `run3.txt` were collected under a
looser threshold and are the supporting cut only; re-filtering them at 4.5 clears
the 1.02 criterion, and that is exactly why it is not the reported result.

These are checked in because the ADR quotes a confidence interval, and
CONTRIBUTING says cite the file that holds a number rather than the prose that
repeats it.
