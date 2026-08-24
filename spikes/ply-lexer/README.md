# `spikes/ply-lexer` — a lexer for Ply, written in Ply

**This is a spike. Nothing in the workspace imports it, builds it, or runs it.**
`cargo build --workspace`, `cargo test --workspace` and `cargo clippy
--workspace` do not reach any of it; the root `Cargo.toml`'s `members` list is
untouched, and the harness under `harness/` declares its own `[workspace]` so it
resolves separately. Deleting this directory removes it completely.

It was written to answer one question — *can Ply host its own compiler front
end?* — by writing part of one rather than by reading the language reference and
guessing.

**The deliverable is [`GAPS.md`](GAPS.md), not the lexer.** Read that first.

## What is here

| | |
| --- | --- |
| `lexer.ply` | The lexer. A hand port of `crates/ply-syntax/src/lexer.rs`, function for function. 15 in-language `test` blocks. |
| `GAPS.md` | What the language could not do, with measurements. **The point of the spike.** |
| `fixtures/` | 10 hand-written inputs that reach the lexer's error paths, which no real `.ply` file in the tree does. |
| `harness/` | A separate cargo project: the reference dump, the differential test, and `plydump`. |
| `run.sh` | One command that runs everything. |

## Running it

```
./spikes/ply-lexer/run.sh
```

It builds `target/release/ply`, runs the lexer's own tests through `ply test`,
then runs `cargo fmt --check`, `cargo clippy --all-targets -D warnings` and
`cargo test` in the harness. Warm, the harness's 22 tests take **10.6 s** and
the Ply tests **0.06 s**; cold, the release build of `ply-cli` dominates and the
whole thing is minutes.

Separately:

```
./target/debug/ply test spikes/ply-lexer/lexer.ply          # 15 tests, in Ply
cd spikes/ply-lexer/harness && cargo test                   # 22 tests, the comparison
cd spikes/ply-lexer/harness && cargo run --bin plydump -- ../../../examples/clock.ply
```

## How the comparison works, and what it does not check

For every `.ply` file in `examples/`, `benches/kernel/`, `crates/ply-std/ply/`
and `fixtures/` — **33 files, 768,760 bytes, 120,490 tokens and 25
diagnostics**, counted with `plydump` — the harness:

1. runs `ply_syntax::lexer::lex` and renders the result as a text dump;
2. writes a temporary Ply project holding `lexer.ply` and a generated `probe.ply`
   whose `source()` is the file as a `b"..."` literal — there is no
   file-reading host handler, so that is the only way in;
3. runs `ply run <dir> --json` and takes the `value` field;
4. compares the two dumps record by record and reports the first difference with
   context.

The dump is `start:end:kind[:payload];` per token, then `start:end:!:code;` per
diagnostic. **Spans, payloads and diagnostics are all compared**, because each
of those is a way for two lexers to look like they agree when they do not.

### The comparison is armed

A green result over a comparison that cannot go red is the defect this
repository names first, so six tests break a dump that really did come out of
the Ply lexer and assert the comparator says so:
`the_comparison_notices_a_token_whose_kind_is_wrong`, `..._whose_payload_is_wrong`,
`..._whose_span_is_wrong`, `..._that_is_missing_from_the_end`,
`the_comparison_notices_a_diagnostic_that_was_not_raised`, and
`a_broken_ply_lexer_makes_the_agreement_tests_fail`, which mutates one string
literal inside `lexer.ply` so `++` lexes as `plus` and checks that the agreement
goes red.

### What it does not check

- **The decimal-to-binary conversion of a float literal.** Ply cannot build a
  `Float` at all (`GAPS.md` §3), so `lexer.ply` emits a float's *normalised
  literal text* and the harness converts it with Rust's `f64` parser before
  comparing (`harness/src/lib.rs::floats_to_bits`). What is compared is that the
  Ply lexer classified the literal as a float, spanned the same bytes, and
  extracted the same digits. The conversion itself is delegated and unchecked.
- **Diagnostic messages and notes.** Only the code and the primary span are
  compared. Two lexers raising `E0001` at the same span with different prose
  would pass.
- **Anything past the token stream.** No parser, no AST, no hashing.
- **The Unicode positions listed below**, which the corpus never reaches.

## Where this disagrees on purpose

`lexer.rs` asks `char::is_alphabetic` and `char::is_whitespace` at a token's
first byte. Ply has no character type and no Unicode table, so `lexer.ply` is
ASCII at that one decision point and raises a code of its own, `X0001`, rather
than guessing. Three shapes, each pinned by a test that asserts the exact dumps
on both sides:

| input | `ply_syntax` | `lexer.ply` |
| --- | --- | --- |
| `let é = 1` | `é` is an identifier | no token; `X0001` over the character |
| `a\u{a0}b` (non-breaking space) | skipped as whitespace | tokens agree; an extra `X0001` |
| `a € b`, `a — b` | no token; `E0001` over the character | no token; `X0001` over the same span |

Only the first changes the token stream. `the_two_lexers_differ_on_a_unicode_identifier`,
`..._on_a_unicode_space` and `..._only_in_the_code_on_a_unicode_symbol` are what
hold these.

Four shapes that look like they should diverge and **do not**, each pinned by
its own test because each was written down as a divergence first and then
refuted by running it: a non-ASCII character in a comment, in a string literal,
inside `b"..."`, and immediately after a backslash inside a string.

### Why the corpus agrees at all

The corpus is **not** ASCII. It holds **1,543** non-ASCII bytes — em dashes and
section signs in prose. Every one of them is either trivia or inside a string
literal (**45** of them), and those are exactly the two positions where the two
lexers agree. Not one sits at a token's first byte.
`every_non_ascii_byte_in_the_corpus_is_somewhere_both_lexers_agree` asserts
that, with the counts pinned, so a corpus that gains a file this claim does not
cover fails rather than quietly narrowing it.

> **Corrected, twice, and left here because it is the same defect this
> repository keeps finding.** I first wrote that the corpus was pure ASCII, on
> the strength of a `grep` that reported `0` for every file and was not doing
> what it looked like it was doing. The test written to confirm that refuted it
> in one run. The replacement claim — every non-ASCII byte is outside every
> token span — was refuted by the same test on the next run, by `"ééé"` inside a
> string literal in `examples/hello.ply`. Both wrong claims were about *why a
> green result was green*.

## Why the harness is a separate cargo project

`crates/ply-codegen-spike` does the same thing and `CONTRIBUTING.md` §"Things
known to be broken" item 1 records what it cost: `cargo build --workspace` does
not reach it, so it bit-rotted twice with nothing to say so. **This will rot the
same way**, and the same sentence applies: the next change to
`ply_syntax::lexer::TokenKind` will break `harness/src/lib.rs` and no workspace
command will notice.

It is taken anyway because the alternative is worse for a spike: adding a
fourteenth workspace member moves the test-target counts that `README.md`,
`CONTRIBUTING.md` and `docs/ONBOARDING.md` all state, and a spike should not
make three documents stale to buy itself a place in a suite it does not belong
in. `harness/` depends on `ply-syntax` **by path**, so it is compared against
the tree it sits in rather than against a copy, and `run.sh` is the one command.

`harness/target/` is covered by `.gitignore`'s unanchored `target/` rule, the
same rule that exists for `crates/ply-codegen-spike/target/`.

## Status

Taken 2026-08-24, in `~/.worktrees/ply/lexer`, on the machine in
`docs/ONBOARDING.md` §Provenance at load 12–26.

- `ply check spikes/ply-lexer/lexer.ply` — 1 module, 58 definitions, 15 tests.
- `ply test spikes/ply-lexer/lexer.ply --no-cache` — **15 passed, 0 failed**.
- `cd harness && cargo test` — **22 passed, 0 failed** (18 integration, 4 lib).
- Agreement holds on all **33** files.
- No file outside `spikes/ply-lexer/` was modified.
