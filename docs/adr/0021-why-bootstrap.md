# ADR 0021 — Why a bootstrapped compiler, and what would unblock one

**Status:** accepted as a statement of intent. It decides no implementation.

ADR 0020 answers *can Ply host its own front end today?* — no, and it measures
why. **It does not say why anyone wanted one. That rationale existed only in a
conversation, which meant the next reader would find a rejection with no goal
behind it.** This is the goal.

## §1 The claim

**Ply's verification loop is O(the change). Every other toolchain this project
competes with is O(the project.)**

That is not a performance difference. **It is a difference in exponent, and it is
the whole thesis.** `cargo test --workspace` costs in-target time proportional to
the size of the repository, excluding all compilation. A warm `ply test` over the
same examples selects out all but a handful of tests and finishes in
milliseconds.

**The second is not "faster." It is *proportional to what changed***, because a
test runs iff its `DefHash` is absent from the cache, and a rename changes no
hash. The first is proportional to the size of the repository, **and no amount of
CI, sharding, caching or hardware alters that. Those are constant factors on an
exponent.**

So the argument for a bootstrapped compiler is not that Ply would be faster than
Rust — ADR 0020 measures it as slower by more than an order of magnitude, **and
that is a floor.** It is that compiler work done *in Ply* would verify in time
proportional to the edit, **and compiler work done in Rust never will.**

## §2 Why today's measurement is the wrong instrument

The obvious way to price this is to measure what fraction of an agent's wall
clock is currently spent waiting on tooling versus on inference, and to act if
tooling is large. **That instrument answers a question about a regime that is
ending.**

If tooling is a tenth of the loop today and inference gets a hundred times
faster, tooling becomes almost all of it. **The measurement would have been
accurate and useless — the same error as reporting an in-fragment kernel ratio
for an end-to-end decision (ADR 0018 §0). Right number, wrong denominator.**

The correct framing is that **Ply *only matters* in the world where fast
inference arrives.** In every other world its thesis is a curiosity. So it should
be developed for that world rather than hedged against it.

**A second payoff path makes the bet less contingent than that sounds.**
O(change) versus O(project) also pays off from the project simply growing, with
inference speed held fixed. **At today's test count the constant factors still
mask it. They will not at ten times that.**

## §3 What ADR 0020 established, and what it did not

Established, by writing a Ply lexer in Ply and measuring it:

- **Expressiveness is not the blocker.** Ply lexed the whole corpus and lexes
  itself.
- **Throughput is**, by more than an order of magnitude against the Rust front
  end, **and that is a floor.**
- **The bootstrap method does not survive past the lexer.** Lexing is flat, so a
  `fold` over a range carries it. A parser must not outrun the nested-call
  ceiling, and if it cannot be written by recursive descent it must be an
  explicit-stack automaton — **a different program, which cannot be
  differentially compared function-for-function, and that comparison is the
  entire reason the lexer's correctness was trustworthy. The verification method
  is lost, not just the porting strategy.**

  > **Withdrawn by ADR 0022.** The premise — that a parser's recursion *is* the
  > grammar, recursing per definition and per argument — is inherited from ADR
  > 0020 §5.1 and **is not true of `crates/ply-syntax/src/parser.rs`, the
  > reference implementation.** It drives every sequence with a loop, with one
  > shared `comma_list` called from a dozen-plus sites, and reserves recursion
  > for grammar nesting, **which it bounds itself far below the call ceiling and
  > far above the corpus maximum.** And "there is no flag" is now a **decision**
  > rather than an omission: a bare `--max-calls` is refused, because lowering
  > the bound would let the result cache answer `Pass` for a program that would
  > raise (ADR 0022 §5). What replaces it is `iterate`, an early-terminating loop
  > that is depth 1 on both engines. **§5's second falsifier has fired on this
  > ADR's own terms. The throughput objection is untouched and remains this
  > ADR's live one.**

Not established, and **it is the number that would most change the estimate**:
ADR 0020 §6.2's lexer-to-front-end multiplier was **assumed**. The only thing
that would settle it is writing the parser, which that ADR recommends against —
**so the figure that could refute its central estimate is one it declines to
take.** *(The parser has since been written and the multiplier measured for two
phases of six; ADR 0020's own correction block carries it, and the conclusion
strengthened.)*

## §4 The critical path

**None of these are self-hosting work.** All of them are defects that hurt every
Ply program, and each is a precondition.

1. ~~**A lint for the field-order rule.**~~ **Superseded by
   [ADR 0024](0024-ownership-as-a-checked-property.md): the lint was built and
   refuted by measurement, and the precondition is answered by *checking*
   ownership rather than by warning about it.**

   > **Withdrawn: "there is no local property an author can check, which makes a
   > lint the only fix rather than the convenient one."** The first clause is
   > right and **the conclusion does not follow.** A lint over this property is a
   > partial oracle, and measurement showed what that means: it fired on a shape
   > that copies nothing and stayed silent on one that is fully quadratic — **a
   > false negative on the exact shape it existed for.** "No local property an
   > author can check" is an argument for putting the property in the *type*,
   > **which is what this record already did for effects.**

   The rest of the item stands and is the reason ADR 0024 exists: a growing
   container must be built in the last sub-expression of its enclosing node or
   the program is quadratic; **the rule is non-local, so a correct callee goes
   quadratic when its caller places the call in a non-final position**; and it is
   already being paid in shipped code, in `json.ply`'s serializer.
2. **The nested-call ceiling.** No flag. A parser needs it raised or needs the
   bound to stop being reachable by ordinary sequence recursion.
3. **The fragment, entered at token granularity.** ADR 0020 §6.3's profile shows
   dispatch dominating builtin bodies by roughly twenty to one, **so compilation
   removes the right half.** What is unmeasured is the cost at one entry per
   token rather than one per file.
4. **The `Map`, record and list machinery.** ADR 0018 §0 records it as a fifth of
   executed work, **outside the fragment however many functions compile.**
   Widening moves which functions compile; **it does not make a `Map` insert
   cheaper.**

## §5 What would make this ADR wrong

- **If the inference speedup does not arrive.** Then tooling stays a minority of
  the loop, O(project) remains affordable, and this is over-engineering. §2's
  second payoff path is the hedge, **and it is slower.**
- **If the parser turns out to be expressible as recursive descent after all** —
  by raising the ceiling, by trampolining, or by a form nobody has tried. Then
  the differential method survives and §3's central objection dissolves.

  > **This has fired, by the third route — "a form nobody has tried".** Not by
  > raising the ceiling, which ADR 0022 §5 refuses, and not by trampolining. The
  > reference parser already reserves recursion for grammar nesting and drives
  > sequences with loops, and `iterate` gives Ply the same shape at depth 1. **So
  > §3's ceiling objection dissolves and the differential method survives it.
  > §3's throughput objection does not, and it is the one this ADR's decision
  > actually rests on.** Recorded because **a falsifier that fires and is not
  > written down is a falsifier nobody wrote.**
- **If a Rust-side tool could make `cargo`'s loop O(change).** Nothing in this
  project has tried, **and it would remove the motive entirely.** The reason to
  doubt it is that Ply had to be designed around content-addressed definitions
  from the start to get the property; **retrofitting it onto a language whose
  compilation unit is a crate is a different and much larger problem.**

## §6 What this ADR is not

**It is not a decision to self-host.** ADR 0020 decides against it on today's
interpreter and that decision stands. This records why the goal exists, **so that
the next person to read a rejection knows what was being rejected and what would
change the answer.**
