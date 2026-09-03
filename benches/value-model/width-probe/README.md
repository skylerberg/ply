# What does the number type cost, with the compiler held constant?

The control ADR 0038 rests its "the width is a code-generator lever, not a
representation cost" reading on. Four transliterations of the same BLAKE3 over
the same input, differing only in how a word is typed and what an add means, all
four compiled by LLVM:

| arm | what it models |
| --- | --- |
| `w32` | `benches/value-model/rust`'s bar — `u32`, `wrapping_add`, `rotate_right` |
| `i64c` | `i64` masked to 32 bits after every add, each add checked — `crates/ply-std/ply/hash.ply` today |
| `i64w` | the same with the check removed and the mask kept, separating width from checking |
| `i64t` | the same again with every word in the record tagged `(v << 1) \| 1` — Ply's compiled value model exactly |

Everything else is held constant: the same tree walk, the same sixteen-field
state record living in memory across the call, the same input. So a difference
is the type.

```
cd benches/value-model/width-probe && cargo run --release
```

Counterbalanced across four blocks; each arm's figure is its minimum, and the
digest is asserted identical across the three untagged arms so a transliteration
that drifts fails rather than skews. Read the instruction mix with
`objdump -d --disassemble-symbols=<sym>` over `i64c::round` and `w32::round` —
that is where the masks and the tag round-trips are visible, or not.

**What it is not.** It is not a measurement of Ply, and no ratio here belongs
beside `analyze.py`'s. It says what a strong optimiser does with each
representation, which is the question ADR 0038 needed and the gate does not
answer.
