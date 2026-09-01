# CLAUDE.md

Instructions for agents working in this repository. `CONTRIBUTING.md` is the
full contract and this file does not replace it — read §"The one rule" and
§"Writing a claim down" before writing anything down.

## Keep `docs/GUIDE.md` current

`docs/GUIDE.md` is the user-facing manual: the one document somebody reads to
learn how to *write* Ply, as opposed to `DESIGN.md` (why each mechanism exists)
or `docs/ONBOARDING.md` (clone to first change). It transcribes **surface** —
signatures, keywords, flags, codes, counts — so it is exactly the document that
goes stale silently, and staleness in it is worse than in the others: a reader
consulting it has by definition not read the source.

**If your change moves a surface below, move the guide in the same change.** Not
in a follow-up, and not "the guide is prose, someone will notice" — the seven
claims §"The one rule" enumerates were all noticed years late.

| if you change | update |
| --- | --- |
| `crates/ply-syntax/src/lexer.rs` — `Kw`, `TokenKind`, literal forms, escapes | §3.3 keywords, §3.4 literals, §3.5 the token set |
| `crates/ply-syntax/src/parser.rs` — `bin_op`, item forms, contextual keywords | §3.3, §3.5 precedence, §4 items, §6 expressions, §7 effects |
| `crates/ply-syntax/src/ast.rs` — an `Item`, `ExprKind` or `PatternKind` variant | §4.1, §6.3 patterns, §6 expression forms |
| `crates/ply-core/src/infer.rs` — `require_written_signature`, `settle_numerics`, `infer_stmt`'s `let` | §5.9, which states what is written and what is inferred, and §5.2's numeric rule |
| `crates/ply-core/src/infer.rs` — `install_prelude` | §13, the builtin tables, **with the signature as inference publishes it** |
| `crates/ply-core/src/infer.rs` — `BUILTIN_TYPE_CONS` | §5.1 scalars, §5.7 |
| `crates/ply-core/src/prelude.rs` — `ADTS`, the prelude effects | §5.7, §10.2 |
| `crates/ply-derive/src/rules.rs` — `Shape`, `Refusal`, `Deriver` | §12.1, §12.4 |
| `crates/ply-eval/src/limit.rs` — `DEFAULT_MAX_CALLS`, `MAX_VALUE_DEPTH` | §6.9, §19.1 |
| `crates/ply-span/src/lib.rs` — `codes` | §18, which is **total over `codes`** |
| `crates/ply-cli/src/cli.rs` — a command, a flag, a default | §17, and the prose in §9, §10.4, §11.5, §15 that names the flag |
| `crates/ply-cli/src/lib.rs` — `EXIT_*` | §17 exit codes |
| `crates/ply-std/ply/*.ply` — any byte | §14's `ply std` block (counts **and** digest both move) and the module's own subsection |

### Check it, do not recall it

§"The one rule" applies to the guide with one extra trap in front of it.

**The binary is an instrument, and a stale one answers questions about the
language wrongly** — not just slowly. `.github/binary-is-current.sh` is the
check; run it *before* probing, not after a confusing result:

```sh
.github/binary-is-current.sh && cargo build --release   # if it says STALE
```

A stale `target/release/ply` reported `unknown name 'iterate'` during the pass
that wrote this guide: the binary was built 2026-08-24 and `iterate` landed in
`84b0049` on 2026-08-27, so it predated the feature by three days and answered
a question about the language wrongly. `docs/ONBOARDING.md` §1 frames staleness
as a timing concern; it is a semantics concern too.

Then: write samples into a scratch directory and actually run them
(`ply check`, `ply test`, `ply run`). Doing that on the first pass corrected six
claims a draft had wrong, including two — `range`/`assert` arity, and `Cell`'s
type-argument count — where the evaluator and the checker disagree and the
evaluator is the one you would naturally read.

### This rule is not armed

Nothing fails if you ignore it. There is no test asserting that §18 is total
over `codes`, that §17 matches `cli.rs`, or that §14's counts match `ply std`.
Per §"Do not state a guarantee you have not armed": **not enforced.**

Arming it is worth doing and has not been done. The idiom is already in the tree
— `w6_report_integrity::the_shipped_ladder_still_describes_the_tree_it_ships_in`
fails when `README.md` stops describing the tree — and the three cheapest
candidates are a test that §18's code table is total over `ply_span::codes`, one
that §17's flag names are a superset of `Cli::command()`'s, and one that §14's
`ply std` block matches the command's output. Until those exist this section is
an instruction, which is the weaker instrument, and it is recorded as such
rather than described as a guarantee.

### Related checklists

`CONTRIBUTING.md` §"Adding things" carries the per-surface checklists for a
diagnostic code, a host handler and an ADR. **None of them mentions the guide**,
so following one of those and stopping there leaves §18 or §15.2 behind.
