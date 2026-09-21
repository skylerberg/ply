# Questions

Decisions that want Skyler's input. Each entry says what was assumed meanwhile; an answered
entry is deleted once the code reflects it.

## Time budgets

A self call in tail position runs as a loop, so a program can run forever where before every
computation was bounded. The guard is a wall-clock budget per entry: `ply test` gives each
test 60 s, `ply prove` each evaluation 5 s, `ply run` none, all settable with `--timeout`;
past it the entry fails with `E0503`. Say if the defaults should differ, or if you would
rather bound loops by a step count.

## Record shapes stay file-local

A record update now takes its shape from a call of a `fn` declared in the same file, and a
`let` without a type takes the written type of such a call, a local or an update. Shapes are
still never read across a module boundary: the rewrite runs before types exist, and a shape
read from another file would change this file's hashes without this file changing. Assumed:
that line stays where it is, and `{..other::make(), x: 1}` keeps needing a local annotation.
