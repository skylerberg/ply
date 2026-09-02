# The bootstrap path — what still stands between Ply and a compiler written in Ply

A plan, not a decision. ADR 0020 decided against self-hosting the front end on
today's interpreter and that decision stands; ADR 0021 records why the goal
exists anyway. This file is the entry for whoever continues toward it: what is
no longer a blocker, what is, the order to take the rest in, and the measurement
each step is gated on. Every claim below names the record that holds it. Figures
stay in those records and in the commands that produce them; none is restated
here, per `CONTRIBUTING.md` §"Writing a claim down".

The goal it serves is ADR 0021's: a front end whose cost is O(change) rather than
O(project), fast enough to sit inside the sub-second verification loop the
project exists to make fast. "Reasonably close to Rust" means that loop, not a
benchmark.

## Where it stands

**No longer a blocker, and each is checked by something in the tree:**

- **Expressiveness.** A lexer and a recursive-descent parser written in Ply agree
  with `crates/ply-syntax` on the reference corpus, tree and diagnostics, byte
  for byte; the inputs they disagree on are the ones written in syntax that
  postdates the port (`spikes/ply-parser/GAPS.md` §11R). CI runs the differential
  on every push (`.github/workflows/ci.yml`, job `spikes/ply-parser`).
- **The call ceiling.** `iterate` gives a parser the reference's own shape —
  loops for sequences, recursion only for grammar nesting — at depth one (ADR
  0022). A raisable ceiling is refused there, with the reason.
- **The positional cost rule.** ADR 0034 is landed entire: a last use moves its
  value out of its slot (`position_invariance_g1`), a copy is bounded whatever a
  list's length and a `[x, ..rest]` pattern shares rather than copies
  (`accumulator_shape`, `list_pattern_rest`), a record update writes into a
  dying base (`record_update_reuse`), and `reuse fn` turns the cost report into
  an obligation `ply check` enforces
  (`check::tests::a_reuse_fn_is_refused_only_for_a_copy_its_own_body_causes`).
  Position decides nothing; a copy means a genuine second owner.
- **Four of the parser spike's ranked gaps.** A list index (ADR 0027), record
  update (ADR 0023), `?` (ADR 0028), and bit operators with a filesystem effect
  and a hash written in Ply (ADR 0033).

**The blocker is throughput, and it is the only one.** Lexing plus parsing in
Ply costs more than an order of magnitude what the whole six-phase Rust front
end costs on identical input in one sitting, and four of the six phases are
unwritten. `spikes/ply-parser/GAPS.md` §13 and §13R hold the series and
`spikes/ply-parser/measure-multiplier.sh` re-takes it. ADR 0021 §"The critical
path" locates the cost: interpreter dispatch dominates builtin bodies by roughly
twenty to one, so compilation removes the right half — and a fifth of executed
work is map, record and list machinery that no amount of compiling reaches.

## Why the interpreter path cannot close it

ADR 0030 measured compiled code on the front end itself and the finding is the
shape of the whole problem. Entering compiled code at the leaves pays the
boundary on every entry, and at a front end's entry rate **an infinitely fast
backend at that granularity cannot win**. The entry has to move to the root of a
subtree, and what keeps it out is the code generator: it refuses a lambda because
there is no closure representation at all, refuses the builtins that call user
code, and refuses an *enclosing* function when a callee is uncompiled instead of
emitting a trampoline — so one lambda under a root refuses the root. ADR 0018
found the earlier form of the same constraint: the fragment covered most of a
kernel's work and bought nothing until the interpreter could call compiled code
(R5).

The seam is wider than it was, and what it still refuses is specific.
`crates/ply-eval/src/compiled.rs` carries a value whose declared type is built
from `Int`, `Bool` and `Bytes` — lists, maps, records and declared types of
those included, since ADR 0030's widening — and a backend answers one value
with no arena, handler stack or route back in. What cannot cross: a function, a
type variable, `String`, `Unit`, `Float`, `Decimal`, and the cell, task and
secret kinds (`CarriedTypes::blocker`). Separately, the *registry* of what the
machine may enter is narrowed to scalar signatures by default
(`backend::scalar_signature`), because ADR 0030 measured that registering every
carried signature adds leaf islands and loses; `PLY_CODEGEN_REGISTER=all` is
the measurement knob.

## The path, in order

Each step names its gate. Take them in this order because each later step's
measurement is confounded until the earlier one has moved.

1. **The front-end row exists, and it is the ordering.** ADR 0026 named the
   bootstrap front end as a workload class with no row; ADR 0030 then took it —
   the parser spike's modules parsing the example files as byte literals,
   through `ply test --backend`, against a null control, arms rotated. Every
   backend arm was slower than no backend, by nearly ten times the control,
   because the entries are leaf islands and a boundary crossing costs more than
   the machine's own dispatch. So the first step is not a measurement but the
   two that follow, and the row is re-taken after each root lands, with ADR
   0030's protocol and its bar: beat the unbacked arm by more than the null
   control, on an idle machine, pre-registered (`CONTRIBUTING.md` §"Gate on an
   idle machine").
2. **The code generator's roots — landed, except the gate.** String
   concatenation and nested patterns were already lowered when this was
   re-censused; the callback family was the whole of the rest, and it is
   lowered as one piece: a lambda is a compiled function taking its captures as
   leading arguments, built into a `ClosureKind::Native` value that lives only
   inside the entry that made it (the seam carries no function); a named
   function, a constructor and a builtin used as values are closures over
   nothing; a call through a local or through an expression is `rt_call`;
   `map`, `filter`, `fold` and `iterate` are loops in the runtime that call the
   callback back; and the bitwise operators are lowered with the interpreter's
   shift-count refusal. A trampoline for an uncompiled callee was not needed:
   the cascade was the callbacks' blast radius, and the parser spike's census
   now refuses only effects, `Decimal` and `Float` literals, a `handle` and
   `secret_of_string` (`parser_census::the_census_over_the_parser_spike` pins
   that no lambda, callback or value-call is refused). With every carried
   signature registered, the examples and the spike's own tests agree with the
   backend attached under `--audit-backend`. What remains is the gate, and it
   cannot move until step 3: the narrow registry enters the same leaves it
   entered before.
3. **The registry is wide — landed; the seam's remaining kinds are not.**
   Every function the fragment compiles is registered and the seam admits each
   call by its carried types; `PLY_CODEGEN_REGISTER=narrow` keeps ADR 0030's
   scalar-signature arm. Decided on ADR 0030's row re-taken under
   `benches/front-end/PRE-REGISTERED.md`: with the wide registry the whole
   parse of each example file runs inside one native entry and beats no backend
   by ten times the null control's resolution, while the narrow registry loses
   as before. Both series (`observation-*.txt`) are recorded as observations
   rather than figures, because the load rose past the gate during each — a
   clean re-take is owed and `run.sh` takes it. The backend now refuses an
   answer holding a closure, cell, task or secret itself, so no registry width
   leaks one. Still to admit: what `CarriedTypes::blocker` refuses that a front
   end needs — type variables (the spike's generic `comma_list`), `String`,
   `Unit` — each with the wrong-backend mutations that police it. What the row
   also says: the compiled parse is a fifth faster than the interpreted one,
   not five times, so the next lever is what compiled code does with values
   (step 4), not what it lowers.
4. **What compiled code does with values — landed, at parity.**
   `front_end_alloc_sites` (in `ply-codegen-tests`) attributes one parse's
   allocations by site under both engines, the way `w6_alloc_sites` attributes
   a request's. It first read the compiled parse at several times the
   interpreted one, because the arena handed every value to a helper as a
   shared handle and none of the last-use ownership ADR 0034 gave the machine
   reached compiled code. Now it does: a borrowed local is duplicated only
   where it is consumed, a helper takes every argument, a field read at its
   base's last use or its own moves out in place, an exact-shape update writes
   in place, and a pattern binds by moving out of a value nothing else holds.
   Two more things the interpreter did and compiled code did not were found
   by the same census: pure nullary definitions are memoised (`rt_constant`,
   with `ply_eval::memo`'s rule), and constructor payloads no longer drain the
   argument pool. The census pins the compiled arm within one percent of the
   interpreter's allocations; what is left is mostly the arena's teardown.
   Two traps, since each would otherwise be re-found. The counts are conserved
   whichever way the pool is fed — a buffer that becomes a payload was
   allocated by some earlier miss — so a warm-up parse hides nothing and a
   pool change never moves the total. And a caller that passes a record and
   then reads a field of it in a later argument, as the spike's state setters
   do, hands the callee a shared record whose update copies under both
   engines; argument order cannot be changed under effects, so that is the
   spike's to hoist into a `let`. What the row must say next is time, and the
   census no longer says where it goes.
5. **The language tax the spike priced.** In `spikes/ply-parser/GAPS.md`'s
   order: tuples (§3 — **landed** as sugar over positional records, `(a, b)` is
   `{_0: a, _1: b}` in a type, a value and a pattern), `const` (§5 — the value of a nullary pure definition is
   already memoised at run time by `ply-eval::memo`, so what remains is the
   spelling), `?` inside lambdas (§2, `E0118` — **landed**: a lambda may write
   `-> T` before a block body and `?` reads it; an `iterate` step answers `Iter`
   and can never carry one), `?` as a `let`'s value inside a branch (`E0119` —
   **stays**: lifting the conditional whole with its success wrapped would keep
   what runs unchanged, but it is a second expansion shape, and ADR 0028 §4
   chose refusal over hoisting for exactly that reason; re-take that decision
   before building it, and the spike converted every guard without it),
   keywords reserved in the field namespace (§6 — **landed** on both sides: a
   keyword names a field wherever a field is named, and only the punned forms,
   which bind a variable too, are refused), an expression-position `unreachable` (§8 — the
   expression exists: `panic` is typed `String -> a`, and what the spike wanted
   beyond it was a placeholder *visible in the differential*, which is the
   spike's choice rather than a gap), and §9's small pieces (`min` and `max`
   **landed** as integer builtins; `saturating_sub` is `max(a - b, 0)`; the rest
   of §9 is spelling the language does not owe). Float construction is ADR 0020's
   one absolute hole. Each is an ordinary language
   change under ADR 0001's rule that no existing hash may move, and each moves
   `docs/GUIDE.md` in the same change.
6. **The other four phases, behind the differential.** Resolve, inference,
   effect inference and hashing, each ported the way the parser was: a reference
   dumper on the Rust side, a corpus, and mutations that prove the comparison
   can go red (`spikes/ply-parser/arm-*.sh`). **Resolve is ported**
   (`spikes/ply-parser/resolve.ply`, `GAPS.md` §15): the tables, the load
   order, the diagnostics and the whole defaults pass agree with the reference
   over the standard library, every example with it, every program the
   reference's own tests build and a hand-written bundle of the error paths;
   `arm-resolve.sh` arms it. The one tax it met is worth knowing before the
   next port: record update needs the base's field list declared in the same
   file (ADR 0029's parse-time expansion), so a rewrite of an imported AST node
   spells every field. Inference with rows is the hard
   one. `std.hash` exists (ADR 0033) and its throughput is not measured. The
   syntax the parser spike predated is ported first, so its own differential is
   green before anything is built on it: the bit operators (with the shift join
   and the lambda-parameter pipe guard) and keyword fields are in, the corpus
   is re-mined from the reference's tests, and the agreement tests pass over
   all of it. Re-mine whenever the reference grows syntax; the harness's
   diagnostics pin moves with the corpus and says so.
7. **The driver.** Incremental caching, the content-addressed store and the
   gates are Rust, and ADR 0020 notes a self-hosted front end would be cached by
   machinery it does not own. Whether the front end lives behind the Rust driver
   or the driver is ported too is a decision for when phases exist to drive; it
   is listed so it is not discovered late.
8. **Repair the oracles as they are needed.** The lexer spike's harness did not
   compile past the tokens ADR 0028 and ADR 0033 added, and its lexer knew
   neither them nor hex literals; both are repaired, the differential is green
   over the corpus and the standard library, and CI runs it (`lexer-spike`).
   `CONTRIBUTING.md` §"Things known to be broken" item 18 remains: the codegen
   spike's agreement corpus is red while its own tests stay green. A bootstrap
   is verified with exactly these instruments, and a green result over an
   instrument that runs nothing is the defect class this project names as its
   most expensive.

## What would make this plan wrong

- **If root entry, once reachable, still loses to no backend.** ADR 0030's
  per-entry cost — a registry lookup, a context, a clone and an arena push per
  argument, two post-conditions, a clone out — is then the ceiling, and the seam
  has to change shape before any construct is worth lowering.
- **If the container share stays the ceiling after steps 2 and 3.** Then step 4
  is the milestone and the generator's constructs were the cheap half.
- **If the inference speedup ADR 0021 bets on does not arrive.** Then tooling
  stays a minority of the loop, O(project) stays affordable, and the bootstrap is
  over-engineering; ADR 0021 says so in its own falsifiers and the second payoff
  path — the project simply growing — is slower.
- **If a Rust-side tool makes the conventional loop O(change).** Nothing has
  tried, and it would remove the motive entirely.

## What this is not

Not a decision to self-host. ADR 0020's decision stands until a re-take of
ADR 0030's row clears its bar, and this file should be corrected in place when
it does: replace the sentence, do not add a block saying the sentence moved.
