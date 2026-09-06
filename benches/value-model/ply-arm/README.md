# The Ply arm

The Ply side of ADR 0035's gate, measured the way the Rust side is: one process, the kernel called
directly, the minimum over repeats.

```sh
cargo build --release -p ply-arm          # from the repository root
./target/release/ply-arm <project-dir> <c|cranelift>
# k1=0.176 k2=29.703 digest=68d647e6...
```

`run.sh` builds and calls this; there is no reason to run it by hand except to read a single arm.

## Why it exists rather than `ply test --json`

`ply test` runs a kernel once per process. k1 takes a fifth of a millisecond, and a process costs
about the same, so the arm was mostly timing the harness — the ratio moved by 1.5 points between
invocations at one load, which is half the bar. It is 0.20 now.
`../observation-in-process.txt` has the numbers.

The seam was never the problem: `--audit-backend` reports k1 as `entered 1, declined 0`, one call
into compiled code and never back out. This calls that same entry, without the machine around it.

## Why it is a workspace member

`benches/value-model/rust` is a workspace of its own because having no dependencies is the point
of it. This one depends on `ply-codegen`'s public surface — `closure`, `Unit`, `c::build`,
`heap`, `rt` — so it is a member instead, and `cargo clippy --workspace --all-targets` and
`cargo fmt --all` keep it building when that surface moves. Outside the workspace it would rot
until somebody ran the gate by hand and found it did not compile.

## What it is not

It is not a standalone C program. The emitted C calls the runtime in `ply-codegen/src/rt.rs`, which
is Rust, and the tables it reads against are built in Rust from the unit's constants, shapes and
constructors. A genuinely standalone arm would need the runtime as a linkable library and a C-side
way to construct those — for a hot path that is already pure compiled C.
`../c-tier/probe.c` is the standalone shape, and pays for it by hand-writing the value model
instead of using the emitter's output, which is a thing that can drift.
