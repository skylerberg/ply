# The C floor: what a C toolchain charges per definition, written before the number

ADR 0037 refuses emitted C inside the verification loop on the ground that a
compiler invocation is a process and a program is a link, and says in its own
falsifiers that neither cost was measured on this tree. This row prices both,
on this machine, before the refusal is relied on. `run.sh` is the protocol;
`raw.txt` is what it writes and `observation-*.txt` is a run kept.

## Question

Per **changed** definition: what does one `cc` invocation over one unit shaped
like emitted Ply code cost, split into the spawn, the front end and the code
generation? Per **reached** definition: what does bringing N compiled
definitions into a process cost, when they are linked whole and when they are
loaded one library each — and does either grow faster than N?

The tree emits no C, so the unit is synthetic: a header declaring as many
helpers as `crates/ply-codegen/src/rt.rs` exports (`run.sh` counts them), and
one function per definition that tests a constructor tag, reads fields, calls
two helpers, builds a three-field record and answers. That is the shape of a
compiled Ply body over the runtime ABI, not its size; a real body may be larger
and the reading is a floor.

## Arms

| arm | command | prices |
| --- | --- | --- |
| `spawn` | `cc -c empty.c` | the process |
| `syntax-only` | `cc -fsyntax-only unit.c` | the process and the header |
| `unit-O0`, `unit-O1`, `unit-O2` | `cc -O<n> -c unit.c` | one changed definition, per optimisation level |
| `unit-dylib-O0` | `cc -O0 -shared -undefined dynamic_lookup unit.c` | one changed definition made loadable on its own |
| `link-N` | `cc -shared` over N objects | the whole-program link at N |
| `dlopen-whole-first-N`, `-warm-N` | one `dlopen` of the N-definition library, freshly linked, then again | the reached set loaded whole: a new file's first load, and a later one |
| `dlopen-bundle-first-N`, `-warm-N` | one `dlopen` per definition, each a bundle bound to the host at link time, over fresh copies, then again | the reached set loaded one image each, symbols bound two-level |
| `dlopen-flat-first-N`, `-warm-N` | one `dlopen` per definition, each a library resolving its symbols by flat lookup | the same, symbols found by scanning every loaded image |

N takes three sizes for the link and the whole library, in a ratio of four and
then four again, so a slope can be told from a constant; the per-image arms
take the two smaller sizes, and the larger of those takes its first load once.
The unit arms are taken twenty times each and the load arms three; the
objects, libraries and bundles the load arms read are built once before any
timing.

**First and warm are separate arms because the first load is the cost.**
`observation-1.txt` is the run that found it and was abandoned for it: the first
load of a freshly written image costs about a tenth of a second on this machine
and a later load of the same file a fraction of a millisecond, so a per-image
arm over fresh images is N first loads and the run did not finish. The
`-first-` arms take that deliberately, over copies made just before the load, so
a design that writes an image per changed definition is priced for what it
writes. A loader that maps relocatable objects itself, as a JIT does, writes no
image and pays none of it.

## Statistic

Minimum wall over the repeats, with user CPU beside it for the `cc` arms;
`dlopen` is timed inside the driver with a monotonic clock, since the process
around it is the thing being separated out. The `spawn` arm is the resolution
for the unit arms: a difference smaller than it is noise.

## Load gate

`run.sh` refuses to start unless the one-minute load average is under 4 and
records the load after; a run whose after-load is above 4 is an observation and
not a figure.

## Decision rule

The row reads constants and a slope, not an exponent. What it decides:

- The per-changed-definition cost is `spawn + unit-O0` at the least. It is a
  constant by construction; what the row settles is its size, which is what
  the marginal-change row would compare against Cranelift's per-definition
  cost and copy-and-patch's.
- `link-N` is the per-definition cost of a whole-program link. If it is linear
  in N, the whole-program link is O(reached set) and a loop that links only
  what the selected tests reach pays its slope times the reach, not the
  project.
- `dlopen-bundle-warm-N` against `dlopen-whole-warm-N` is the price of loading
  the reached set lazily rather than as one library. If it is linear in N and
  its per-image cost is within an order of magnitude of the per-object link
  cost, a C tier can load by reach, and a "no link" requirement on the loop's
  tier is a constant-factor argument rather than an exponent one.
- `dlopen-bundle-first-N` is what a one-image-per-definition design pays per
  changed definition on this platform, and the row reads it as a second
  per-changed-definition constant beside `unit-O0`.

## Predictions, registered

1. `spawn` is a few milliseconds; the header adds a few more; `unit-O0` is
   under a tenth of a second and `unit-O2` a small multiple of it.
2. `link-N` and every warm load are linear in N over their sizes; the flat
   warm load has a larger constant than the bundle one and is not linear.
3. A first load is a per-image cost that does not depend on the image's size,
   so the whole library's first load is about one image's, and N bundles' first
   load is about N of them.
4. The warm per-image cost is larger than the per-object link cost, and the
   whole library loads warm for a small multiple of one image.
