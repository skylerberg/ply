# What Ply could not do, written down while writing a lexer in it

This is the deliverable of the spike. `lexer.ply` is the vehicle.

Each entry says what I was trying to express, what I had to write instead, and
how bad it was. Where a claim is a measurement it carries its provenance; where
it is not, it says so. Two entries (§1, §13) record a claim I made and then
refuted, because both are more useful than the claim would have been.

**Provenance for every number here.** Machine: the one in
`docs/ONBOARDING.md` §Provenance, shared with three other agent worktrees
building concurrently, so **load average was 12–24 throughout and no wall-clock
figure here is a clean one**. Where the finding is a *shape* — quadratic against
linear — that does not matter: the ratios are 4x per doubling against 2x, and
load noise is nowhere near that. Where a figure is an absolute time it is
labelled with the build and the load. The statistic is pre-registered as the
**minimum of N runs** (N given per measurement); minimum, because on a loaded
machine the minimum is the closest estimate of the unloaded time and no run is
discarded after the fact.

Two builds are used and they are not interchangeable:

- `target/debug/ply`, built 14:17 from `cargo build -j 2 --workspace`.
- `target/release/ply`, built 14:31 from `cargo build -j 2 --release -p ply-cli`.

> **Every number in this file was taken through a pre-built binary, which is a
> provenance risk this file could not have stated when it was written
> (2026-08-27).** ADR 0020 §0 is the account of the first instance —
> `crates/ply-eval/src/frame.rs` written 54 seconds before this release binary
> was built, in the mechanism §1 measures. The rule for catching that,
> `find crates -name '*.rs' -newer target/release/ply`, has a second blind spot:
> the eight `crates/ply-std/ply/*.ply` modules are `include_str!`ed into the
> binary, so an edit to one is invisible to it. `lexer.ply` imports no `std`
> module, so this file's numbers are exposed to a stale *interpreter* and not to
> a stale stdlib — but `bench.sh` in the three sibling spike directories takes
> `PLY=${1:-../../target/release/ply}` and builds nothing, so nothing checked.
> Run `.github/binary-is-current.sh` before re-taking anything here;
> `CONTRIBUTING.md` §"The binary is an instrument too" has the reproduction.

> **Read this before quoting §1's first table.** The four numbers in §1's
> "as first measured" row were taken with the **debug** binary. They are a
> factor of roughly eight slower than the release ones and must not be quoted as
> release figures. The scaling *shape* is the same in both and that is what the
> finding rests on.

---

## §1 A growing container must be built in the last sub-expression of its enclosing node, or the program is quadratic — and nothing says so

This is the finding. It is not about lexers.

A lexer threads state: a position, a token list, a diagnostic list. In Ply that
is a record passed through a fold. Written the obvious way it is quadratic:

```ply
fn emit(s: Scan, start: Int, fin: Int, t: Tok) -> Scan =
  { pos: fin,
    toks: push(s.toks, {start: start, end: fin, tok: t}),   // third of five
    ndiag: s.ndiag,
    diags: s.diags }
```

Move `toks:` to the end of the same record literal — same fields, same values,
same types, same semantics — and it is linear:

```ply
fn emit(s: Scan, start: Int, fin: Int, t: Tok) -> Scan =
  { pos: fin,
    diags: s.diags,
    toks: push(s.toks, {start: start, end: fin, tok: t}) }  // last
```

### Measured

`fold(range(0, n), <5-field record>, step)`, `toks: List<Int>`, `min` of 3 runs,
release binary, load 11–24:

| n | `toks` third of five | `toks` last of five | `toks` third, other fields `let`-bound first | no record at all |
| ---: | ---: | ---: | ---: | ---: |
| 8,000 | 0.23 s | 0.03 s | 0.31 s | 0.01 s |
| 16,000 | 0.92 s | 0.05 s | 1.11 s | 0.02 s |
| 32,000 | 13.02 s | 0.10 s | 3.68 s | 0.03 s |
| 64,000 | 20.03 s | 0.19 s | 17.61 s | 0.06 s |

Column 1 quadruples per doubling from 8k to 16k; the 32k and 64k cells are
contaminated by load (13.02 s at 32k against 20.03 s at 64k is not a shape, it
is a busy machine) and are shown rather than dropped. Column 2 doubles per
doubling, cleanly, at all four sizes.

It is **not** a record-specific effect. The same asymmetry appears in call
arguments (min of 3, release, load ~17):

| n | `sink(push(xs,i), i, i)` | `sink(i, i, push(xs,i))` |
| ---: | ---: | ---: |
| 8,000 | 0.24 s | 0.02 s |
| 16,000 | 0.96 s | 0.03 s |
| 32,000 | 3.80 s | 0.06 s |

Four in a row, exactly 4x per doubling, against exactly 2x.

### The mechanism, read off the evaluator rather than guessed

`ply_eval::rc::carry` (`crates/ply-eval/src/rc.rs:98`):

```rust
pub(crate) fn carry(env: &Env, remaining: bool) -> Env {
    if remaining { env.clone() } else { Env::empty() }
}
```

Called from eight sites — call arguments (`machine.rs:1007`, `frame.rs:107`,
`frame.rs:142`), record fields (`machine.rs:1064`, `frame.rs:263`), list items
(`machine.rs:1094`, `frame.rs:301`) and handler arguments
(`handler.rs:208`) — always with `remaining` meaning *is there another
sub-expression after this one*. When there is, the pending frame keeps a second
reference to the scope for the whole of that sub-expression's evaluation.
`Env::take_unique` (`env.rs:127`) then refuses: `let link = Rc::get_mut(node)?`,
commented *"Refuses at the first shared link"*. So the variable read clones
instead of moving, the list is at two owners, and `push` takes its copying
branch (`builtins.rs:456-473`).

When the growing sub-expression is the **last** one, `carry` hands the frame
`Env::empty()`, the scope's last reference dies, the binding goes to one owner,
and `push` rewrites in place.

`rc.rs`'s own module doc states the rule and prices it: carrying a scope past
the sub-expressions that read it left **7.4%** of updates in place; not carrying
it took the same measurement to **75.3%**. The optimization is deliberate,
documented and correct. **The gap is that its precondition is positional and
invisible.** Nothing in the type system, the syntax, a diagnostic or a warning
distinguishes the two `emit`s above, and the difference between them is
asymptotic.

> **Corrected — the first mechanism I wrote down was wrong, and so was the
> second.** My first draft of this section said *"`push(s.toks, t)` where `s` is
> a record is quadratic: the field read leaves the list aliased by the record it
> came out of"*. That is false; being in a record is not what causes the copy.
> A correction relayed to me proposed instead that the rule is *whether the
> field read is the last use of the record variable in source order*. That is
> also false, and column 3 of the table above is the refutation: the other three
> fields are bound to `let`s **before** the push, so `s.toks` is the last
> mention of `s`, and it is still quadratic. The rule is about position in the
> enclosing node, not about the variable.

### It is already being paid, in shipped code

`crates/ply-std/ply/json.ply:589-599`, `escape_runs`, the JSON string
serializer:

```ply
escape_runs(
  raw,
  stop + 1,
  push(push(acc, bytes_slice(raw, i, stop)), escaped_byte(bytes_at(raw, stop))))
```

The **outer** `push` is the last argument of `escape_runs` and is fine. The
**inner** `push(acc, ...)` is argument 0 of 2 of the outer one, so the scope is
carried and `acc` is copied — once per escape, quadratic in the number of
escapes in a string a client chose.

Measured on a standalone reproduction of exactly that function (min of 3,
release, load ~13; `k` = number of escape characters in the subject string):

| k | shipped shape | one `push` per escape, last argument | two pushes, each last argument | two pushes split by a `let` |
| ---: | ---: | ---: | ---: | ---: |
| 2,000 | 0.06 s | 0.03 s | 0.03 s | 0.07 s |
| 4,000 | 0.22 s | 0.04 s | 0.04 s | 0.21 s |
| 8,000 | 0.81 s | 0.08 s | **fails** | 0.76 s |

Three things in that table are worth more than the first column:

- **The fix that works is column 2**: push once per escape, in last-argument
  position, concatenating the run and its escape first. Linear, same recursion
  depth.
- **Column 3 is the naive fix and it breaks the module.** Splitting into two
  calls so each `push` is last doubles the recursion depth, and at k = 8,000
  the run dies with `recursion limit of 10000 nested calls exceeded`. A fix for
  the quadratic that halves the maximum string length is not a fix.
- **Column 4 shows a `let` does not rescue it.** `{ let one = push(acc, a);
  push(one, b) }` is still quadratic, because the block's continuation carries
  the scope for the rest of the block.

`json.ply` documents a *different* quadratic in its parser (§"What this costs":
escapes in one string cost one frame each and copy the text decoded so far).
This one, in its serializer, is not documented anywhere I could find.

**I have not changed `json.ply`.** This spike touches no file outside `spikes/`.

> **Since fixed — read this before quoting the table above as a live defect.**
> ADR 0020 §7 item 3 was taken: `escape_runs` now performs one `push` per
> escape in last-argument position, which is column 2 of the table above, and
> the serializer is linear **on the machine engine**. Confirmed on the shipped
> module with `ply_eval::rc::stats()` rather than a clock — whole-accumulator
> copies per encode went from exactly k to **0** at k up to 32,000 — and guarded
> by `crates/ply-eval-tests/tests/suite/stdlib_accumulator_cost.rs`. Column 3's warning was
> confirmed at the same time and on the shipped module: built as two `push`es
> that are each last, the largest string `encode_string` can encode under the
> call budget halves, from **9,993 escapes to 4,996**. The line citation
> `json.ply:589-599` above is the pre-fix one and no longer points at the
> function.
>
> > **Corrected on review, 2026-08-27, measured rather than inferred.** This
> > passage read: *"**Every `json.ply` line citation in this file below line 588
> > is now short by 16**, including §11's `json.ply:621` and
> > `json.ply:626-627` and §12's `json.ply:555` and `json.ply:564-568`:
> > documenting the positional precondition above `escape_runs` added sixteen
> > lines there."* Three things in that are wrong. The shift is **17**, not 16.
> > The citations that moved are the ones *after* the insertion, not before it,
> > so §12's `json.ply:555` and `json.ply:564-568` never moved and adding 16 to
> > them would break two citations that are still exact. And §11's
> > `json.ply:991-993` moved as well and was not listed.
>
> **Every `json.ply` citation in this file that pointed at line 588 or later is
> short by 40**, a shift confirmed on four landmarks that bracket the insertion:
> `escape_runs` 589 → 629, `hex_byte` 621 → 661, `byte_table` 626 → 666,
> `b64_alphabet` 991 → 1031. So §11's `json.ply:621`, `json.ply:626-627` and
> `json.ply:991-993` read `:661`, `:666-667` and `:1031-1033` today, while
> §12's `json.ply:555` and `json.ply:564-568` are **unchanged and correct** —
> they sit above the insertion. The functions all of them name are unmoved and
> still findable by name — `hex_byte`, `byte_table`, `b64_alphabet` — which is
> why they are left rather than renumbered into the next edit's drift.
>
> > **The shift was 17 and is now 40. Corrected 2026-08-27 by re-taking the same
> > four landmarks.** This paragraph read *"short by **17** … `escape_runs`
> > 589 → 606, `hex_byte` 621 → 638, `byte_table` 626 → 643, `b64_alphabet`
> > 991 → 1008 … read `:638`, `:643-644` and `:1008-1010` today"*. Disclosing
> > the engine-conditionality above `escape_runs` added twenty-three more lines
> > there. Which is the point the paragraph itself makes: a line citation is
> > invalidated by the next edit, and the four figures above are re-measured
> > rather than arithmetic on the last set.
>
> §1's *finding* is unaffected — the trap is still invisible, which is
> ADR 0020 §7 item 2, still open.
>
> The same counter found the shape twice more in the standard library, and both
> were fixed: `std.trace`'s `append` and `std.router`'s `numbered`. `std.db` was
> not measured.
>

### Cost to me

It decided the architecture twice. I first wrote the accumulator as a
`Map<Int, Token>` with a hand-maintained key counter, purely to route around a
quadratic I had misdiagnosed; that is two extra fields of state (`ntok`,
`ndiag`, the next key for each map), two `map_values` calls at the end, and a
whole concept in the file that did not need to be there. Once the
real rule was found the `Map` came out and a plain `List<Token>` went in, and
the file got shorter and slightly faster (desk.ply: 1.26 s list against 1.45 s
map, min of 3, release, warm cache). The cost was not the code. It was that the
code I wrote first was correct, obvious, and 100x too slow, and nothing told me
which of the two it was going to be.

---

## §2 No record update syntax

> **Closed by W4, and one sentence of it was wrong.** Record update now exists,
> spelled `{..b, f: e}` — the same `..` the record *pattern* uses, not the `...`
> this section guessed at. `docs/adr/0023-record-update.md` records the design.
> Everything below is kept as written, with the two corrections it needs marked
> where they apply.

The text as it stood:

> `{...s, toks: xs}` does not exist. `PatternKind::Record` has a `rest` flag, so
> `..` is a *pattern* wildcard; there is no spread in a record **literal**
> (`crates/ply-syntax/src/parser.rs`, `record` expression).

`..` is now both, and unambiguously: a `{` followed by `..` begins a record
update, because `..` cannot begin a statement. Expansion runs inside
`ply_syntax::parse_module`, so `{..s, toks: xs}` and the field list it stands for
are one definition with one `DefHash`
(`crates/ply-hash-tests/tests/suite/audit.rs record_update_hashes_as_its_expansion`).

Every state transition in `lexer.ply` therefore lists every field:

```ply
fn seek(s: Scan, p: Int) -> Scan = { pos: p, diags: s.diags, toks: s.toks }
```

Three functions (`emit`, `err`, `seek`) exist only to spell that out once each.
With the five-field `Map` state it was five fields written eleven times.

Mild on its own — and then §1 makes it load-bearing, because the workaround for
§1 is *the order you write those fields in*. The two gaps compound: a language
with record update would have had one obvious spelling and no positional trap,
and a language with the positional trap and record update would at least have
had one place to get it right.

`std.http` paid this worse: `chunk_trailers` at `http.ply:1016-1029` wrote out
all **thirteen** `Limits` fields in order to change one
(`max_header_bytes: state.limits.max_trailer_bytes`). It is now one line, and
the twelve fields it stops spelling are twelve it can no longer mispair.

> **This paragraph's last sentence was wrong, and it is worth being exact about
> which half.** It read:
>
> > Adding a field to `Limits` forces an edit there, and forgetting it is a
> > silently wrong limit rather than a type error.
>
> The first half stands. The second does not. `crates/ply-core/src/unify.rs`
> unifies two records by **exact key-set equality** and Ply has no width
> subtyping, so a 13-field literal handed to `fields(buf, _, _, limits: Limits)`
> where `Limits` has 14 fields cannot unify. Measured rather than argued: adding
> one `max_probe: Int` field to `type Limits` and to `default_limits()` only, and
> running `ply check crates/ply-std/ply/http.ply`, produces **four `E0201`
> type errors** — at `chunk_trailers` and at three other sites this section did
> not know about (`limits_with` at `:1666`, `limits_keeping` at `:2399`,
> `limits_streaming` at `:2844`). So the tax was **four times larger** than
> recorded and the *hazard* was smaller.
>
> The hazard that does survive is a **mispairing**, not an omission: all
> thirteen fields are `Int`, so `max_body: state.limits.max_chunk_size`
> type-checks and is a silently wrong bound in an HTTP server. That is what
> record update removes structurally, and it is asserted at
> `crates/ply-cli/tests/suite/stdlib.rs
> chunk_trailers_copies_every_limit_it_does_not_replace` — a test that goes red
> on exactly that swap while `ply check` stays green.
>
> The three sites named above were left hand-written for one round and are now
> record updates too, over a `let base: Limits = default_limits();` lift. Their
> line numbers are from `http.ply` as it stood when the experiment ran and no
> longer point at anything; `default_limits` is the one site that still spells
> `Limits` out, because it constructs from nothing and has no base to update.

**The compounding with §1 is narrowed, not removed.** Copies are pure field
reads that never grow, and the expansion emits them **first**, so a single-write
update whose value grows — `{..s, toks: push(s.toks, t)}` — lands on §1's
*linear* spelling with no thought required: there is nothing the expansion can
put after it.

The several-writes case is untouched. `{..s, toks: push(s.toks, t), pos: p}` is
**still quadratic**, because `pos: p` is emitted after the growing `toks` and so
the growing field is not last in the record node. `{..s, pos: p, toks:
push(s.toks, t)}` puts it last and so lands on the *linear* spelling this
section measured, and computes the same record value — but it is **not the
same definition**, because written fields are emitted in the order written and
field order is part of a record's hash, so choosing the linear spelling moves
the `DefHash` and re-runs what reaches it. The rule left is narrower than
before — *a growing field must be written last among the fields you write* —
and it is **syntactic**, checkable at the update site with no types, which is
what makes it lintable. The trap is not gone.

---

## §3 Ply cannot build a `Float`, so the lexer does not produce one

`TokenKind::Float(f64)` is what the Rust lexer emits. There is no
`float_of_string`, no `float_to_string`, and no `parse`. The only route in is
`float_of_decimal`, and it is not one: `Decimal` is 28 significant digits and a
bounded exponent, so `1e400` — which `lexer.rs` deliberately lets saturate to
`inf`, because that is what IEEE says decimal-to-binary conversion does — has no
path through it at all.

So `lexer.ply`'s token type carries `TFloat(Bytes)`: the literal's **normalised
text**, digit for digit the string `lexer.rs` hands to `f64::from_str`. The
harness converts it with Rust's parser before comparing
(`harness/src/lib.rs::floats_to_bits`), and that conversion is **delegated and
not checked** — it is named in the README under what the agreement does not
cover.

This is the one part of the front end that a Ply lexer provably cannot do today.
Everything else in this list is awkward; this one is absent.

---

## §4 `Int` is 64 bits, `Decimal`'s mantissa is 96, and arithmetic is checked, so both numeric bounds are decided on digit strings

`lexer.rs` decides three things with a `parse`:

- `whole.parse::<i64>()` — does this integer fit?
- `digits.parse::<i128>().ok().filter(|m| *m <= (1<<96)-1)` — does this mantissa fit?
- `text.parse::<f64>()` — §3.

Ply has **no `int_of_string`**. `decimal_of_string` exists but tops out at 28
significant digits, which is below both bounds. And Int arithmetic is *checked*
(`interp.rs:1215`, `checked_add`), so the usual trick — accumulate and look at
the sign — raises before the overflow can be observed.

What I wrote instead, for both bounds:

```ply
fn int_max() -> Bytes = b"9223372036854775807"

fn int_value(s: Scan, m: Num, fin: Int) -> Scan = {
  let d = strip_zeros(m.whole);
  let k = bytes_len(d);
  if k > 19 || (k == 19 && compare(d, int_max()) == Greater) { ... }
```

Strip leading zeros, compare lengths, and on a tie compare the digit strings
lexicographically with the generic `compare` builtin. It works and it is exact.
`dec_max()` is the same trick against `b"79228162514264337593543950335"`.

Not bad, but note what it costs: the *value* is now computed by a hand-written
`fold` over the digits (`int_of_digits`), so the lexer contains its own decimal
parser. In Rust that is one `parse` call.

---

## §5 There is no loop, there is no tail-call elimination, and the ceiling is 10,000 nested calls with no flag to raise it

> **Line citation corrected on review, 2026-08-27.** This read
> *"(`crates/ply-eval/src/limit.rs:35`)"*. The constant is at `limit.rs:50`.
> ADR 0020 §1 recorded this stale `:35` while correcting its own copy of it
> — *"`GAPS.md` §5 carries the same stale `:35`"* — but corrected it only
> there, leaving the error in the file that actually contains it. The value,
> the name and the §'s finding are unchanged.

`DEFAULT_MAX_CALLS = 10_000` (`crates/ply-eval/src/limit.rs:50`). `grep -rn
max_calls crates/ply-cli/src/` returns **one** line —

> **Corrected (2026-08-27, ADR 0022).** The constant is at `limit.rs:57`, not
> `:35`; the value is unchanged. And this section's title —
> *"there is no loop, there is no tail-call elimination, and the ceiling is
> 10,000 nested calls with no flag to raise it"* — reads two of those three as
> oversights when both were decided. **Tail-call elimination** was removed
> deliberately and the reasons are measured:
> `docs/adr/0005-control-stack-and-world.md` §7.1, which this spike does not
> cite. **The flag** is refused deliberately: results are cached as
> `(RUNTIME_VERSION, DefHash) -> Outcome` and shipping code writes only
> `Outcome::Pass`, so raising the bound is monotone and safe while *lowering* it
> would let the cache answer `Pass` for a program that would now raise — ADR
> 0022 §5. **"There is no loop" is now false**: `iterate(seed, budget, step)` is
> an early-terminating loop that is depth 1, so the
> `fold(range(0, n + 1), ..)` shape below — 140,108 of desk.ply's 159,684 steps
> being no-ops, 87% of the loop — is no longer the only way to write one.
`engine.rs:244`, `Machine::new(..).with_max_calls(DEFAULT_MAX_CALLS)` — plus
the `use` that imports the constant. **There is no CLI flag**, and `ply run
--help` offers none.

`examples/desk.ply` is 19,576 tokens and `crates/ply-std/ply/db.ply` — the file
with the most tokens in the tree, and one the compiler ships — is **29,213**.

> **Corrected (verification pass, 2026-08-24).** This read *"the largest `.ply`
> file in the tree, and one the compiler ships — is **29,212**"*. It is 29,213
> by `plydump`, and it is the largest by *tokens*, not by bytes:
> `examples/desk.ply` is 159,683 bytes against `db.ply`'s 135,285. Neither
> correction touches the conclusion — the bound is 10,000 either way. A
recursive scanner, which is the shape `std.json` uses and the shape the Rust
lexer's `loop` translates to, dies at token 10,000: a third of the way through
`db.ply`, with a diagnostic that ends the whole run. The ceiling is not a
theoretical limit a big program might one day reach. It is a third of one of
this compiler's own source files.

So the main scan is:

```ply
let done = fold(range(0, n + 1), start, |s: Scan, i: Int| one(src, s));
```

`fold` is driven by the machine's step protocol, so it nests nothing. The
awkwardness is in `range(0, n + 1)`:

- **The bound has to be guessed conservatively.** Every step that has work left
  consumes at least one byte, so `n + 1` steps is the only sound bound. For
  desk.ply that is 159,684 iterations to produce 19,576 tokens: **140,108 of
  them are no-ops**, 87% of the loop.
- **The iteration count is materialised.** `range(0, 159684)` allocates a list
  of 159,684 boxed `Int`s that the loop reads and discards. There is no lazy
  range and no `for`.
- **The loop variable is unused.** `|s: Scan, i: Int| one(src, s)` — `i` is
  there because `fold` needs an element, not because the lexer wants one.

Cheap in the end (a no-op step is two calls; the whole desk.ply run is 1.87 s)
but it is a loop written as a fold over a list of integers nobody wanted.

`std.json`'s foot says the same thing and declines to do it: *"batching the
element loop through a `fold` over a `range` would raise the bound roughly
sixty-fold and is deliberately not done here, because it trades the clearest
code in the module for a limit nothing has yet hit."* A lexer hits it.

---

## §6 The recursion that is left is bounded by the input, and I had to go and check the corpus to find out whether that mattered

Two places in `lexer.ply` still recurse once per *thing* rather than once per
token, and neither could be a fold without restructuring:

- `skip_trivia` recurses once per comment line in a run of trivia (a blank line
  is whitespace and neither adds a frame nor ends the run). Deepest run in the
  whole corpus: **135 lines**, in `examples/desk.ply`, counted over all 33 files.
  The bound is 10,000, so it is fine — but "fine" here is a fact about the
  corpus, not about the lexer.
- `string_lit`/`bytes_body` recurse once per escape in one literal, and each
  step copies the bytes decoded so far, so an escape-heavy literal is quadratic
  in its own escapes. Same shape `std.json` documents for its parser.

Neither is a defect I introduced. Both are the language: a `while` loop over a
mutable cursor has neither property.

---

## §7 No character type, no `byte_of_int`, and no `match` on bytes

`bytes_at` answers an `Int`. There is nothing that turns an `Int` back into a
`Bytes`. So:

```ply
fn all_bytes() -> Bytes = b"\x00\x01\x02 ... \xff"   // 256 escapes, 1,024 characters

fn byte_of(v: Int) -> Bytes = bytes_slice(all_bytes(), v, v + 1)
```

A 1,024-character literal in the source so that `\x41` in a byte-string literal
can decode to the byte `A`. `std.json` carries the **identical** table for the
identical reason — `json.ply:626-627`, whose comment is *"Every byte value, so
that a `\u` escape and a `\u00XX` output can name one without a builtin that
turns an `Int` into a byte"* — and uses the same slice-a-literal trick twice
more, for hex digits (`json.ply:621`) and for the base64 alphabet
(`json.ply:991-993`). Four sightings of one missing builtin in two files.

It earns its place twice, because the same table is the only way to write *the
set of all bytes above 0x7f* as a scan class:

```ply
fn bytes_stops() -> Bytes = bytes_concat(b"\"\\\n", bytes_slice(all_bytes(), 128, 256))
```

And there is no `match` on a byte value or a byte range, and no match on a
string or byte string. Every dispatch in the file is an `if`/`else if` chain on
`Int`:

```ply
fn punct(src: Bytes, s: Scan, start: Int, c: Int) -> Scan =
  if c == 40 { emit(s, start, start + 1, TPunct(b"lparen")) }
  else if c == 41 { ... }                        // 22 more
```

and the keyword table is a chain of fifteen `==` on `Bytes`, hand-bucketed by
length so an identifier pays five comparisons rather than fifteen. In Rust that
is `match s { "pub" => .., ... }` and the compiler builds the switch.

---

## §8 A Ply program cannot read a file, so the lexer's input is a source literal

No shipped effect has a file operation. The operations declared across all
eight `std` modules are config `get`/`secret`, db
`query`/`execute`/`returning`/`begin`/`commit`/`abort`/`rollback`, net
`listen`/`listen_tls`/`accept`/`recv`/`send`/`close`, trace
`event`/`enter`/`exit`/`count`/`gauge`/`time`, and signal
`stopping`/`deadline_ms`. `ply-host` does open files — for its own `--config`
and for TLS certificates — but there is no operation a *program* can perform to
read one.

So a `.ply` file reaches a Ply lexer as a `b"..."` literal or not at all, and
the harness generates one per corpus file — 173,370 characters of literal for
`examples/desk.ply`.

Cheap, as it turns out: parsing, typechecking and hashing that literal is
**0.01 s** (min of 5, release, cache cleared before each run). But it means the
lexer cannot be pointed at a file, and a self-hosted front end would need either
a file effect or a driver in Rust that hands it bytes.

---

## §9 No tuples, so every "and also" is a record declaration

`lexer.ply` declares five record types. Two of them, `Token` and `Diag`, are
real. Three exist only because a function needs to answer with more than one
thing:

- `Scan` — the fold state.
- `Num` — seven fields, threaded between `number`, `number_tail`,
  `number_value`, `float_text`, `int_value` and `decimal`, because
  `lexer.rs::number` is one function with seven locals and Ply has no way to
  hand six of them to a helper except in a record.
- `Lexed` — because `lex` answers with tokens *and* diagnostics.

`std.json` declares three near-identical ones (`Step`, `Text`, `Piece`) that
differ only in the payload field's type, and says why: *"Ply has no tuples, so a
pair is a two-field record"* (`json.ply:555`).

---

## §10 The `List` surface is `len, push, map, filter, fold, range`

No index, no concatenation, no reverse, no prepend, no sort, no `nth`. That
closed off the obvious way to recover the scanner's position from the
accumulator (read the last token's `end`) and is why `Scan` carries `pos`
separately.

`bytes_concat_all(List<Bytes>) -> Bytes` is the escape hatch that makes the
token *dump* linear — one allocation over the whole list, instead of a fold of
`++` that copies the prefix per token. `json.ply:564-568` says what bought it:
*"one allocation over the whole list, which is what W3 added it for... An array
of 4,000 numbers cost 233.8 ms to serialize."*
There is no `List<a>` equivalent, so `map` is the only way to build a list
without paying §1's positional tax.

---

## §11 `at` is three interpreter calls where one builtin would do — a second sighting

```ply
fn at(src: Bytes, i: Int) -> Int =
  if i < 0 || i >= bytes_len(src) { -1 } else { bytes_at(src, i) }
```

`bytes_at` out of range is a runtime error, and a lexer peeks past the end
constantly, so every peek goes through this. It is the most frequently executed
function in `lexer.ply`.

`std.json`'s foot already asks for `bytes_at_or(b, i, default)` and says it
should be measured before it is added. This is an independent second workload
that wants exactly the same builtin, and it is not in ADR 0012's table. I have
not measured what it would buy — noted as **not measured**.

---

## §12 Error accumulation was fine, and the reason is the lexer's design rather than the language's

`lexer.rs` never fails: it answers with tokens *and* diagnostics and keeps going.
That maps onto Ply with no friction at all — the diagnostics are just a second
list in the fold state, and there is no `Result` anywhere in `lexer.ply`.

So the thing I was told to watch for did not bite **here**. It would bite a
parser. In `std.json`, **57 of 129** `fn` definitions return a `Result<..>`, and
the module hand-writes `decode_map` and `decode_and_then` (`json.ply:99-112`).
There was no `?` when this was written; there is one now (`docs/adr/0027`), and
`json.ply` uses it at 7 sites. There is still no do-notation. A lexer is the one
front-end phase that dodges any of this.

> **Two sentences of this paragraph were wrong and are withdrawn.** It read:
>
> > In `std.json`, **58 of 129** `fn` definitions return a `Result<..>` (45%,
> > counted by regex over the `fn NAME .. -> TYPE` headers, so treat it as close
> > rather than exact); … and one number literal is split across **seven**
> > functions — `number`, `number_fraction`, `number_fraction_digits`,
> > `number_exponent`, `exponent_first`, `number_exponent_digits`, `number_of`
> > (`json.ply:450-511`) — **purely to bind an `Ok` and carry on**.
>
> **The count is 57, not 58.** Re-derived twice, by two instruments, during the
> `?` work (`docs/adr/0027`): 129 `fn` in the file, **57** returning `Result`, 2
> returning `Option`. The hedge — "close rather than exact" — is what was off;
> the number it hedged is available exactly. 57 is also what the brief for that
> work and `docs/adr/0020` §5.2 already said.
>
> **And the seven-function chain contains no `Ok` bind at all.** Not one of
> `number`, `number_fraction`, `number_fraction_digits`, `number_exponent`,
> `exponent_first`, `number_exponent_digits` or `number_of` has an `Ok`-binding
> arm or an `Err` rethrow. Every one of them ends in a **tail call inside a
> branch**, because a check that fails must answer `Err` *there* and a check
> that passes must carry on — and Ply has no early `return` with which to write
> that in one function. `number_of`'s only `match` is on an `Option` whose
> `None` maps to an `Err`, which is a mapping and not a bind.
>
> So the chain is split by the absence of an early **`return`**, not by the
> absence of `?`, and `?` collapses **none** of it: the seven functions are
> unchanged by the conversion in `docs/adr/0027`, which converted 7 sites
> elsewhere in `json.ply`. The honest statement of `json.ply`'s cost is 7
> convertible sites, 2 `Ok`-first combinators `ply-derive` depends on, 4 sites
> that map their error and which `?` cannot express, and a codec half written
> inside `decode:` lambdas where `?` is refused.

---

## §13 No dispatch mechanism, and it did not bite

README §"What is missing" is explicit that there is no typeclass, implicit or
instance resolution, and that nothing stops two codecs for one type. I expected
this to show up as the inability to write `Show` for a token type.

It did not. `dump` is a plain function, the token type has one renderer, and
there is nowhere in a lexer that wants an open dispatch. The one place the
absence shows is trivial and named in the language already: `len` is
`(List<a>) -> Int`, so a string's length is `string_len`, and `Bytes` gets a
parallel family of ten more names. Verbose, not blocking.

> **This entry is here because it is a negative result.** It was on the list to
> watch for and the honest answer is that a lexer is not the workload that finds
> it.

---

## §14 Mutable state versus regions: not needed, and the alternative is worse

I never reached for `with_cell` or `with_region`. The fold accumulator is
enough, `lex` and `dump` are pure, and that purity is worth something concrete
here — a pure `dump` is cacheable and auditable against a backend.

The cell route exists and I priced its shape while looking at §1: a
`bytes_position(src, 0, |b| ...)` loop with a cell would iterate bytes without
materialising a `range`, at the cost of an effect row on every function that
touches it and a `cell_get`/`cell_set` pair whose read has exactly the aliasing
problem §1 describes. Not taken, not measured.

---

## §15 What it costs to run

`examples/desk.ply`, 159,683 bytes, 19,576 tokens. Release binary, front-end
cache cleared before each run, **min of 5**, load 13–18:

| what | time |
| --- | ---: |
| whole run, `main` = `dump(source())` | **1.87 s** |
| same program, `main` = `bytes_len(source())` | 0.01 s |

So essentially all of it is the Ply lexer running: about **10,470 tokens per
second**, **85 KB of source per second**.

The debug binary on the same program is **9.31 s** (min of 3, cache cleared,
but at load **26** rather than 13–18, so the 5.0x ratio against the release
figure is contaminated by the load difference and should not be quoted as a
profile ratio).

For scale, measured the same way (release, cache cleared before each run, min of
3, load ~29): **`ply check examples/` is 0.43 s**. That is lex, parse, resolve,
typecheck, effect-infer and content-hash **333,595 bytes** of project source
across thirteen files — plus every `std` module they import, which this figure
does not separate out. So the Rust front end does at least 776 KB/s of the
*whole* job where this lexer does 85 KB/s of the *first phase* of it: roughly
an order of magnitude slower for a fraction of the work.

A self-hosted front end at this speed would be the slowest thing in the build.

Two caveats, both stated rather than discovered later:

- **These are all wall clock on a machine at load 13–24.** Treat them as upper
  bounds with a factor-of-two error bar. The *shape* results in §1 do not depend
  on them.
- **`bytes_concat_all` over `map` is doing the heavy lifting in `dump`.** The
  token stream is rendered with one allocation, not one per token. A `dump`
  written with `++` would be quadratic and would dominate everything above.

---

## What the language handled fine

Stated because a gap list with no negative entries is a gap list that was
looking for gaps.

- **The byte-scanning primitives are genuinely good.** `bytes_scan`,
  `bytes_scan_until` with a `Bytes` as the character class, `bytes_slice`,
  `bytes_split`, `bytes_concat_all`. A run of identifier characters of any
  length is one call. Stripping `_` out of `1_000_000` is
  `bytes_concat_all(bytes_split(b, b"_"))` — two calls, no loop. Whitespace of
  any length is one call. I did not once want a regex.
- **Sum types and pattern matching are ordinary and adequate.** The nine-variant
  `Tok` with mixed payload arities (`TDec(Bytes, Int)`) needed no thought.
- **The type checker caught the port's mistakes before it ran.** `ply check`
  passed on the first attempt and the fifteen in-language tests passed on the
  first run after that, against a lexer ported by hand from 1,069 lines of Rust.
- **`test` blocks in the language are good.** Fifteen tests asserting exact dump
  strings, named as English sentences, run by
  `ply test spikes/ply-lexer/lexer.ply --no-cache` in 0.06 s.
- **The generic `compare` builtin works on `Bytes` and is lexicographic**, which
  is what makes §4's digit-string bound check a two-line function rather than a
  loop.

## The strategic answer

Can Ply host its own compiler front end today?

**A lexer: yes, with one hole and one trap.** The hole is §3 — `Float` literals
cannot be given values in Ply, so the token type is not the Rust one. The trap
is §1, and it is not lexer-specific: it will hit a parser and a typechecker
harder, because they thread more state through more nested calls, and every one
of those is a place where the accumulator can end up in a non-final position.

**The thing that would change the answer most is not a language feature.** It is
making §1 visible — a lint, a `--explain` line, anything that says *this `push`
will copy*. Second is §3. Third is a real loop.

At 85 KB/s the front end would be the bottleneck of the build it was written to
speed up, which is worth saying plainly: the verification-loop argument for
self-hosting is about *incrementality*, not about throughput, and this
measurement does not settle whether the trade is worth it.

## The width suffix, ported, with one value it cannot reach

`lexer.ply` lexes ADR 0039's `255u8` and `0x6A09_E667u32`, and agrees with
`crates/ply-syntax` over the whole shipped standard library --- `hash.ply`
included, which is now written in `U32` and is what made the port necessary
rather than optional.

**One case is out of reach and it is this list's own kind of gap.** A `U64`
literal above the largest `Int` --- `9223372036854775808u64` and up --- is a
value the reference accepts, because it bounds a decimal spelling in `i128`.
This lexer computes in `Int`, where `two_to(63)` overflows before it can be
compared against, so the bound it applies at sixty-four bits is `int_max`'s: it
refuses about half the `U64` range that the reference accepts. `fixtures/widths.ply`
stays inside the reachable half deliberately, so the differential is green and
this paragraph is the record that it is green over less than the whole domain.
