# ADR 0005 — The control stack and the world

Status: accepted — **the machine landed and is the default engine**; §2's
persistent forkable world is **superseded by ADR 0017**. §3's resumption
semantics stand unchanged and are what ADR 0017 §3 was rewritten to preserve.
Supersedes: DESIGN.md §2's "v0 restriction: tail-resumptive only", and the
ST-monad reading of `with_cell`'s region.
Superseded in part by: `docs/adr/0017-regions.md` (§2 only).

The machine is not merely landed — `Engine::Machine` is the default, so it is the
engine every run uses unless told otherwise, and `Engine::Treewalk` is the
fallback that refuses a `resume` binder with `E0504`.

**`ply_eval::world` no longer exists**, and what each of §2's operations became
is stated inline there. **This matters because §2 is the section most likely to
be read for a cost model, and every cost in its table is now the cost of
something that does not exist.**

## Context

ROADMAP.md lists two M6 items: multi-shot continuations, which move the
evaluator to an explicit control stack, and copy-on-write world state, which
lets a fixture be built once and forked per test. They are one milestone
because one design question decides both, and answering it for either answers
it for the other.

The question is what `with_cell` guarantees. Today:

```ply
with_cell[users](initial) { cell -> handle body() with { ... } }
```

The cell's `cell.read[users]` / `cell.write[users]` atoms are discharged at the
region boundary, and a `Cell` in the region's result type is a compile error, so
a cell cannot outlive the region that made it. That is the ST-monad trick and it
is why a handler backed by a test-local cell contributes nothing to the test's
footprint — which is in turn why the test is provably isolated, which is the
whole selection-and-scheduling story.

Multi-shot continuations break the argument as stated. Capture a continuation
inside the region, resume it after the region has returned, and code holding the
cell runs outside the region whose type-level check was supposed to make that
impossible. The check inspects the region's result *type*; a continuation
carries the cell in its captured environment, where no type mentions it.

Three answers were available.

**(a) Brand the region and forbid escape.** Rank-2 polymorphism over a region
variable, ST-style, so a continuation that mentions the cell cannot be typed
outside. Principled, and hostile to the point: the handler patterns multi-shot
exists to enable — backtracking, generators, a scheduler that parks and resumes
— are exactly the ones that move a continuation across a boundary. It also puts
rank-2 types into a language whose type system is otherwise Hindley–Milner with
ground-atom rows, for one construct.

**(b) Make the world a persistent value.** State stops being a location that a
value points into and becomes an entry in a map that the machine threads. A cell
is a key. Escape stops being a memory-safety question, because there is no
memory to be unsafe about.

**(c) Deep-copy cell state at capture.** (b) with worse performance, no
structural sharing, and no fork story.

This ADR takes **(b)**, and (b) *is* the forkable-world milestone: once the world
is a value, forking it is holding it.

## Decision

### 0. The rule everything else follows from

> **State is a value the machine threads. Control is a value the machine
> splices. A continuation captures control only.**

Everything below is a consequence. The world is never snapshotted at a capture
and never restored at a resumption; there is exactly one current world at every
point of an execution, and it moves forward. That is what makes a state handler
writable (§3), and the fact that the world is nevertheless a *value* is what
makes forking a fixture free (§2) and test isolation structural rather than
conventional (§5).

### 1. The machine

The tree-walking evaluator becomes a CEK-style abstract machine with an explicit
control stack. Continuation frames are persistent and shared by pointer, so
capturing a continuation copies nothing that is proportional to the work already
done.

#### 1.1 The stack

The stack is a list of **segments**. A segment is a persistent list of frames
sitting on top of the `handle` that delimits it; the outermost segment has no
delimiter and is called the base.

```
Stack   ::= Segment*                   -- head is innermost; last is the base
Segment ::= (frames: Frame*, prompt: Prompt?)
Prompt  ::= (clauses, effects, return-clause, env, module, span)
```

Capturing a delimited continuation is taking the segments from the innermost
down to and including the one whose prompt matched. That is **one entry per
enclosing handler crossed** — one, in every ordinary program — and never one
entry per pending frame. The frames inside a captured segment are not copied,
compared, or walked: the segment is two pointers.

```
Continuation ::= Segment*              -- innermost first
capture(K, n) = (K[0..n], K[n..])
resume(K, k)  = k ++ K
```

`resume` pushes the captured segments back onto **whatever stack is current**,
which may be a different stack from the one they were cut out of and may already
carry a previous resumption's leftovers. Because a captured segment carries its
own prompt, the handler is reinstalled by the act of resuming: handlers are
**deep**. And a clause body runs on the stack *below* its own handler, so a
clause that performs the operation it handles reaches the next handler out
instead of catching itself forever — which is the rule today's tree-walker
already implements by truncating the handler stack.

This is landed in `ply_eval::cont`.

#### 1.2 Frames

Twenty kinds, landed as `ply_eval::cont::Frame`. Every one names the value it is
waiting for and what it does with it. Together they cover `NodeKind`
exhaustively, plus the three prelude builtins that call back into user code.

| frame | waiting for | then |
| --- | --- | --- |
| `Unary` | the operand | apply the operator |
| `BinaryRhs` | the left operand | evaluate the right |
| `BinaryApply` | the right operand | apply the operator |
| `ShortCircuit` | the left operand of `&&` / `\|\|` | evaluate the right |
| `AppCallee` | the callee | evaluate argument 0, or apply if there are none |
| `AppArgs` | `args[next-1]` | evaluate `args[next]`, or apply |
| `Call` | the body's value | nothing; it bounds recursion and feeds the tracer |
| `If` | the condition | evaluate a branch |
| `MatchArms` | the scrutinee | try arms from `next` |
| `MatchGuard` | a guard | take the arm, or resume trying from `at+1` |
| `BlockStep` | `stmts[next-1]` | bind or discard, then `stmts[next]` or the tail |
| `RecordField` | a field's value | evaluate the next field, or build the record |
| `FieldAccess` | the base | project |
| `ListItem` | an element | evaluate the next, or build the list |
| `PerformArgs` | `args[next-1]` | evaluate `args[next]`, or enter `Perform` |
| `WithCellBody` | the region's initial value | allocate the cell, evaluate the body |
| `MapStep` | `f(items[next-1])` | evaluate `f(items[next])`, or build the list |
| `FilterStep` | the predicate's answer | keep or drop, then the next element |
| `FoldStep` | the accumulator | fold the next element, or return it |
| `Resume` | a value | splice a captured continuation and hand it the value |

`map`, `filter` and `fold` are frames rather than host recursion because a
continuation captured inside the function passed to `map` would otherwise be
captured across a native frame that cannot be re-entered — the second resumption
would have nowhere to return to.

`Call` transforms nothing. Every frame that holds pending code carries its own
`env` and `module`, so returning from a call restores the caller's scope with no
help; `Call` exists to bound recursion and to give the causal slice of ADR 0004
its enter and exit events at the one place that already holds the qualified name.

#### 1.3 States and transitions

A configuration is `⟨S, K, W⟩` — state, stack, world.

```
S ::= Eval(code, env, module)      -- evaluating an expression
    | Return(value)                -- handing a value to the stack
    | Perform(effect, op, resource, args, span)
    | Halt(value)
```

**Evaluating** decomposes and pushes:

```
⟨Eval(lit, ρ, m), K, W⟩              → ⟨Return(lit), K, W⟩
⟨Eval(x, ρ, m), K, W⟩                → ⟨Return(lookup(x, ρ, m)), K, W⟩
⟨Eval(a ⊕ b, ρ, m), K, W⟩            → ⟨Eval(a, ρ, m), K·BinaryRhs(⊕, b, ρ, m), W⟩
⟨Eval(f(ā), ρ, m), K, W⟩             → ⟨Eval(f, ρ, m), K·AppCallee(ā, ρ, m), W⟩
⟨Eval(e.op[r](ā), ρ, m), K, W⟩       → ⟨Eval(a₀, ρ, m), K·PerformArgs(…, 1), W⟩
                                        or ⟨Perform(e, op, r, [], σ), K, W⟩ if ā is empty
⟨Eval(with_cell[r](i){x→b}, ρ, m), K, W⟩
                                     → ⟨Eval(i, ρ, m), K·WithCellBody(r, x, b, ρ, m), W⟩
⟨Eval(handle b with H, ρ, m), K, W⟩  → ⟨Eval(b, ρ, m), K ◁ Prompt(H, ρ, m), W⟩
```

`◁` opens a new segment; `·` pushes a frame into the innermost one.

**Returning** consumes:

```
⟨Return(v), K, W⟩  where K.next() = Frame(F, K′)    → dispatch on F
⟨Return(v), K, W⟩  where K.next() = Leave(P, K′)
      and P has  return x → r                       → ⟨Eval(r, P.env[x↦v], P.module), K′, W⟩
      and P has no return clause                    → ⟨Return(v), K′, W⟩
⟨Return(v), K, W⟩  where K.next() = Done            → ⟨Halt(v), K, W⟩
```

The three dispatches that are not mechanical:

```
Frame::WithCellBody(r, x, b, ρ, m):
      (id, W′) = W.alloc(v)
      → ⟨Eval(b, ρ[x ↦ Cell id], m), K′, W′⟩

Frame::AppArgs, last argument, callee is a closure:
      → ⟨Eval(body, ρ_c[params ↦ v̄], m_c), K′·Call(name, σ), W⟩

Frame::AppArgs, last argument, callee is a continuation k:
      → ⟨Return(v₀), K′.resume(k), W⟩

Frame::Resume(k):
      → ⟨Return(v), K.resume(k), W⟩
```

Applying a continuation is the only rule that changes the stack's *shape* on a
return, and it is one line. A continuation takes exactly one argument — the value
the `perform` it was captured at should have produced — and any other count is
`ARITY_MISMATCH`.

**Performing** searches, splits, and dispatches:

```
⟨Perform(e, op, r, v̄, σ), K, W⟩
  K.find_handler(e, op, r) = None
      → error UNHANDLED_EFFECT at σ

  K.find_handler(e, op, r) = (n, P, i),  (k, K′) = K.capture(n)

    clause i is tail-resumptive  (op(x̄) → body)
      → ⟨Eval(body, P.env[x̄ ↦ v̄], P.module), K′·Resume(k), W⟩

    clause i is general          (op(x̄) resume κ → body)
      → ⟨Eval(body, P.env[x̄ ↦ v̄][κ ↦ Continuation(k)], P.module), K′, W⟩
```

`W` is threaded through capture and through resumption **unchanged**. That is the
whole of §3, stated as a rule.

The two clause forms differ in exactly one thing: the tail-resumptive one has the
`Resume` frame pushed for it. `op(x̄) → e` is `op(x̄) resume κ → κ(e)`, which is
why every existing handler keeps its meaning and its typing (§4) unchanged.

#### 1.4 Surface syntax for the continuation

A clause opts in to a reified continuation with a binder in its head:

```
hClause := IDENT "." IDENT ("[" IDENT "]")? "(" IDENT,* ")" ("resume" IDENT)? "->" expr ","?
```

```ply
amb.flip[coin]() resume k -> k(true) + k(false)
```

`resume` is **contextual**: it is a keyword only between a clause's `)` and its
`->`, `lexer::is_ident("resume")` stays true, and a program that already binds
`resume` as an ordinary name is unaffected. The binder itself is an ordinary
value binder in the clause's scope, with the ordinary resolution order.

Bare `-> e` stays tail-resumptive and keeps its current typing rule. That is not
backwards compatibility for its own sake: a tail-resumptive clause is the
overwhelming majority, its body's type is the operation's return type rather than
the whole `handle`'s, and making every clause general would retype every handler
in every existing program to no benefit.

#### 1.5 The code representation

Frames hold subexpressions. A frame cannot hold `&'a Expr` without a lifetime on
`Value`, which would spread to `World`, `Env` and every crate that holds an
evaluated value, and it cannot hold an owned `Expr`, because a frame is pushed
per node and cloning a subtree per push is quadratic.

So the AST is lowered once per machine into `ply_eval::code`: the same shape with
`Rc` on every node. `lower` costs one traversal — the same order as the
per-function `Arc<Expr>` clone `Interp::new` already pays — and it retires the
per-lambda deep clone the tree-walker does on every closure creation. Patterns,
names, literals and operators are reused from the AST rather than mirrored,
because the machine never suspends inside one.

### 2. The world

> **Superseded by ADR 0017.** Everything in this section landed and then was
> removed. `ply_eval::world` does not exist in the tree; `World`, `World::fork`,
> `World::high_water` and `set_base_world` are gone, and `Value::Cell` is a
> `Slot` in a `TaskRegions` region stack
> (`crates/ply-eval/src/task_regions.rs`). ADR 0017 §1 explains why the two are
> mutually exclusive: Perceus-style in-place update fires only on a uniquely
> owned value, and a design that forks worlds keeps reference counts high by
> construction.
>
> The section is kept rather than deleted because it is the argument the
> replacement had to preserve, and ADR 0017 §3 turns on getting that argument
> right. The mapping, so a reader is not left to infer it:
>
> | this section | what replaced it |
> | --- | --- |
> | `World::fork` at an entry point, O(1) | `Fixture::open`, and `TaskRegions::reset` between entry points — whose doc comment names itself "the replacement for `World::fork` at an entry point" (`task_regions.rs`) |
> | "a cell is a key, not a pointer" | still true, and now the key is an arena `Slot` rather than a `CellId` into a persistent map |
> | "the world is monotone; an entry is never removed" | **false now.** A region's slots are reclaimed at its lexical close unless a continuation can still reach them, which is ADR 0017 §3's `unique` / `shared` split |
> | `Isolation::World` | `Isolation::Region` (`crates/ply-test/src/schedule.rs`), and the cost of the change is ADR 0017 §6 |
>
> What did **not** move is §3. Read it as current.

```rust
pub struct CellId(pub u32);
pub struct World { /* RedBlackTreeMap<CellId, Value>, next: u32 */ }
```

As originally landed in `ply_eval::world`, with these operations:

| operation | cost | note |
| --- | --- | --- |
| `fork` | O(1) | it is `clone`; the whole of "fork a fixture per test" |
| `alloc(Value) -> CellId` | O(log n) | the id comes from the world's own counter |
| `get(CellId)` | O(log n) | |
| `set(CellId, Value) -> bool` | O(log n) | `false` when the id is not in this world |
| `with(CellId, Value) -> World` | O(log n) | the persistent form, for a caller keeping the old world |
| `cells()` | O(n) | ascending by id, so two runs iterate identically |

`Value::Cell` changes from `Rc<RefCell<Value>>` to `CellId`. **A cell is a key,
not a pointer.** That is the sentence the whole ADR rests on: a key cannot
dangle into freed memory, two keys cannot alias one location across a fork, and
the escape question stops being about safety.

Three properties are load-bearing.

**The world is monotone.** An entry is never removed. `with_cell` allocates on
entry and does nothing on exit. This is what lets a continuation captured inside
a region be resumed outside it and read the cell successfully, rather than being
forbidden (a) or being a dangling read. It is also semantics-preserving against
today: an `Rc<RefCell>` cell survives its region whenever anything still
references it, which a store into an enclosing cell already achieves without any
continuation at all.

The cost is stated rather than hidden: a `with_cell` inside a loop retains one
entry per iteration for the rest of the run, where today the `Rc` is freed.
Reclamation is not in M6 (see "Not in M6").

**A fork is a value, and nothing crosses between siblings.** Two worlds forked
from one ancestor share every id below the ancestor's high-water mark and may
hand out the *same* id for different cells above it. That is sound only because
the machine holds exactly one world at a time and no operation carries a value
from one fork into a sibling. `World::high_water` exists to make the boundary
inspectable, and the landed unit test asserts the collision so that nobody
"fixes" it into a global counter and concludes the invariant is unnecessary.

**Handler-backed resources are cells.** There is no separate map for them. A
handler is a closure over an environment; the state it carries is a cell; the
cell is in the world. A "fixture" is therefore a `(World, Value)` pair — the
world after running the setup, and the value through which a test reaches it —
and forking a fixture is `World::fork` plus a `Value` clone. `Interp` and
`Machine` both take one via `set_base_world`, and every entry point forks from it.

### 3. Resumption semantics

This is the part implementers will get wrong if it is stated loosely, so it is
stated twice.

> **A resumption observes the world as of the handler's call to `resume`, not as
> of the capture.** The world is threaded; there is one current world; capture
> and resumption do not touch it.

#### 3.1 Why not the world as of capture

Snapshot-at-capture is the reading the phrase "each resumption gets its own
world" invites, and it makes a state handler unwritable. Consider the canonical
one:

```ply
with_cell[s](0) { c ->
  handle body() with {
    state.get[s]()  resume k -> k(cell_get(c)),
    state.put[s](v) resume k -> { cell_set(c, v); k(()) },
    return x -> x
  }
}
```

The `put` clause writes the cell and *then* resumes. If the resumption restored
the world as of the capture, the write would be discarded before the computation
that asked for it ever ran, and `put(5); get()` would answer `0`. The only state
handler you could then write would pass the state through the resumption value —
the state-passing encoding that region-scoped cells exist to avoid.

Threading is also what makes the tail-resumptive identity `op(x) → e` ≡
`op(x) resume k → k(e)` hold. Every handler in the language today writes state
through a cell and relies on the write being visible to the rest of the
computation. Snapshot semantics would silently change all of them.

#### 3.2 Worked examples

Take a handler that also touches a cell of its own, so the difference is
observable:

```ply
effect amb { read flip[coin]() -> Bool }
```

**Resumes zero times — an abort handler.**

```ply
with_cell[log](0) { c ->
  handle {
    cell_set(c, 1);
    let b = amb.flip[coin]();
    cell_set(c, 2);
    if b { 10 } else { 20 }
  } with {
    amb.flip[coin]() resume k -> cell_get(c),
    return x -> x
  }
}
```

The clause observes `c = 1`: the write before the `perform` happened, the write
after it never ran. `k` is dropped. The result of the whole `handle` is the
clause's value, `1`. Nothing runs at a discarded continuation — Ply has no
finalizers, so dropping one is dropping a pointer. Any cell the abandoned
computation had allocated stays in the world, unreachable.

**Resumes once — the state handler above.**

`body()` performing `state.put[s](5)` then `state.get[s]()` observes `5`. The
`put` clause's write to `c` happened before `k(())`, and the resumed computation
runs in the world that write produced. This is the case that decides the design.

**Resumes twice — nondeterministic choice.**

```ply
with_cell[trace](0) { c ->
  handle {
    let b = amb.flip[coin]();
    cell_set(c, cell_get(c) + 1);
    if b { 10 } else { 20 }
  } with {
    amb.flip[coin]() resume k -> k(true) + k(false),
    return x -> x
  }
}
```

The `handle` evaluates to `30`. And afterwards `cell_get(c)` is **2**, because
both resumptions ran against one threaded world and each incremented it once.
Under snapshot-at-capture it would be `1`. That number is the observable that
pins the semantics, and it is a required test.

| | what the clause sees | what the resumption sees | what survives |
| --- | --- | --- | --- |
| zero resumptions | the world at the `perform` | — | the clause's writes |
| one resumption | the world at the `perform` | the clause's writes | everything, in order |
| two resumptions | the world at the `perform` | resumption *n* sees resumption *n−1*'s writes | the last branch's writes |

#### 3.3 How a handler threads its own state across resumptions

Explicitly, with the cell it already has. A handler that wants each branch to
start from the same state saves and restores around each resumption:

```ply
amb.flip[coin]() resume k -> {
  let before = cell_get(c);
  let a = k(true);
  cell_set(c, before);
  let b = k(false);
  a + b
}
```

Now each branch starts from `before` and the cell ends holding whatever the
second branch left. Per-branch state is four lines and no new construct, and —
importantly — it is *visible in the handler*, where a reader can see that the
handler is the thing deciding.

This is the classic `State ∘ Nondet` versus `Nondet ∘ State` ordering. Ply's
machine fixes one order (threaded) and lets the handler build the other. The
reverse choice — snapshot by default, thread by explicit encoding — cannot be
undone by a handler at all, which is the asymmetry that settles it.

### 4. Footprint typing under multi-shot

#### 4.1 The rule

A row is a **set**. Resuming twice performs the same atoms twice and the set is
the same set. The handle rule is therefore unchanged:

```
footprint(handle body with H) = (row(body) \ handled) ∪ ⋃ row(clause_i) ∪ row(return)
```

`resume` performs nothing of its own. Its atoms are the residual atoms of the
handled computation, which `row(body)` already carries.

The typing of a general clause:

```
Γ ⊢ body : T_b / ρ_b                          handled = { atom(C_i) }

  C_i tail-resumptive  op(x̄) → e_i :
      Γ, x̄ : params_i  ⊢  e_i : ret_i / ρ_i

  C_i general          op(x̄) resume κ → e_i :
      Γ, x̄ : params_i, κ : (ret_i) → R / ρ_κ  ⊢  e_i : R / ρ_i

  return clause        return x → e_r :
      Γ, x : T_b  ⊢  e_r : R / ρ_r        (absent ⟹ R = T_b, ρ_r = {})

  ρ_h = (ρ_b \ handled) ∪ ⋃ ρ_i ∪ ρ_r
  ρ_κ := ρ_h
  ───────────────────────────────────────────────────────────────────────
  Γ ⊢ handle body with { C̄ ; return } : R / ρ_h
```

Two things about `ρ_κ := ρ_h` that an implementer must get exactly right:

- **One `ρ_κ` per `handle`, not per clause.** Every clause's continuation is the
  same residual computation, so they share the variable. Allocate it fresh before
  inferring any clause and bind it into every general clause's environment.
- **Solving it drops a self-occurrence in the tail.** `ρ_h` is built from the
  clause rows, which may carry `ρ_κ` as their tail, so `ρ_κ := ρ_h` is
  self-referential. Set union is idempotent, so the least fixed point is reached
  in one step: solve `ρ_κ` to `ρ_h` with `ρ_κ` removed from the tail, and
  substituting back gives `A ∪ A = A`. This is the **only** row variable for
  which a self-occurrence is permitted; it must be created and solved inside
  `infer_handle` and general unification's occurs check must stay exactly as it is.

`nondet` is unaffected. A `nondet` operation performed once and resumed twice
produces its value once, at the `perform`; both resumptions receive the same
value. Multi-shot introduces no nondeterminism, and E0412 needs no change.

#### 4.2 What that means for scheduling

Footprints exist to decide which tests may run concurrently. Two consequences,
one reassuring and one worth saying out loud.

**The conflict graph is invariant under multi-shot.** Adding or removing a
resumption changes no row, so it changes no footprint, so it changes no edge, so
it changes no colouring and no group count. This is not a coincidence to be
grateful for; it is the reason rows are sets. There is no scheduling risk to
manage here and no analysis to add, and a required test pins it.

**A footprint has never been a count, and multi-shot is where that starts to
show.** A test whose footprint is `{db.write[orders]}` and whose handler resumes
twice writes twice. The scheduler was already correct about this — one write and
a thousand writes conflict with the same things — but a *reader* of the artifact
must not read `observed_footprint` as evidence about how often. ADR 0004's
`entered[].calls` is the field that answers frequency, and it counts resumptions
correctly because it counts entries.

The cost that does move is wall clock: a test can be exponential in resumptions
while its footprint stays a singleton. Groups are coloured by conflict, never by
cost, and a group's duration is its slowest member — so `--explain` must report
per-test duration, which it already does.

### 5. What forking buys the scheduler

The conflict graph exists because tests share real resources. If a test's
resources are entirely world-backed, and every test gets its own forked world,
then no two such tests can interfere — whatever their footprints say.

**Definitions.**

- An effect is **world-backed** iff its atoms name state that lives in a
  `World`. Exactly one effect is world-backed: the builtin `cell`.
- A footprint is **world-isolated** iff every atom in it is world-backed. The
  empty footprint is world-isolated.

**Rule.** A world-backed atom conflicts with nothing. `EffectAtom::conflicts_with`
returns `false` when either side is world-backed, and `Footprint::conflicts_with`
inherits it. Two tests that both retain `cell.write[users]` do not conflict:
each test's `users` cell is an entry in its own forked world and no reference
crosses.

**Measurable properties**, because a slogan that is not a number is not a claim:

1. Every world-isolated test is in group 0, for any number of them. Adding *N*
   world-isolated tests to a corpus changes the group count by **zero**.
2. The group count equals the colouring of the *shared* tests alone.
3. `ply test` reports `isolated: 43 of 47` in its summary; `--explain` reports
   `isolation: world` or `isolation: shared` per test with the atoms that made it
   shared; `--json` carries both.

That third one is the "unit/integration distinction stops mattering" claim in
the only form worth having: a number printed on every run that a project can
watch go up. A project at `47 of 47` has no integration tests in the sense that
matters — not because it wrote none, but because every one of them is provably
unable to disturb another.

**Being honest about how much of this is new.** A test whose resources are
entirely handler-backed already has an *empty* footprint under the M2 handle
rule, is already world-isolated, and already colours into group 0. Most of §5 is
therefore an invariant to preserve, not a capability to add. Two things are
genuinely new:

- A `cell` atom surviving into a test's footprint is reachable for the first
  time. It needs a `Cell` used outside its `with_cell` region, which the region
  check forbids by result type and which multi-shot achieves through a captured
  continuation. Without the rule above, two such tests would be serialized
  against each other for no reason.
- The property becomes reported and tested rather than accidental. Nothing
  today would notice if a future change made a handler-backed test conflict with
  its neighbour; the group-count test above notices.

### 6. Validating the rewrite

The rewrite touches everything, so the old evaluator stays available and both
run on every test until the new one has earned the default.

**`--engine <treewalk|machine|both>`** on `ply test`, `ply run` and `ply check`,
with `ply_eval::Engine` as the shared vocabulary. The default is `treewalk` when
M6 opens and `machine` when it closes. **It is now `machine`**, with
`RUNTIME_VERSION` at 0.4.0; the deletion below is what remains.

**What `both` compares**, per test, exactly:

- the `Result<(), Diagnostic>`, by its full JSON serialization — code, severity,
  message, every label with its span, every note. Not "both failed"; byte
  equality, which is the standard the incremental front end already holds itself
  to and for the same reason: a weaker comparison hides the defect the check
  exists to catch.
- the observed footprint from the tracer, and the number of atoms performed. A
  row is a set, so the count is a separate claim: one atom performed three times
  and performed once are the same footprint and not the same execution. The
  tracer records a `perform` — a `cell_get` is a builtin over a `CellId` that
  carries no resource label, and the world comparison below is the stronger
  statement about those effects anyway.
- the final world, as the sequence of `(CellId, rendered Value)` from
  `World::cells`, which is ordered and therefore comparable.

**A count of what was compared is part of the answer.** `Report` carries
`footprints_compared`, and a corpus where it is short of `compared` fails the
audit. The footprint axis was dead for a milestone — no engine overrode
`observed_footprint`, so it returned early on every call — and nothing failed,
because a zero was only ever reported.

A mismatch fails the run with a diagnostic naming the test and both outcomes. It
is never a warning: a divergence between two evaluators of one language is the
most expensive class of defect this project can have, because the cache makes it
sticky.

**Programs the old engine cannot run.** A clause with a `resume` binder is
machine-only. The tree-walker must **refuse** it with a diagnostic naming the
clause and saying which engine runs it — never approximate it as
tail-resumptive, which would produce a plausible wrong answer. Under `both` such
a test runs once, on the machine, and `--explain` records it as `machine-only`.

**Caching.** `--engine` other than the default implies `--no-cache` in both
directions. A `Pass` in the store is a claim about what the authoritative engine
did, and a non-authoritative engine may neither read one nor write one. Flipping
the default is a `RUNTIME_VERSION` bump for the same reason.

**Where the comparison runs.** `ply-eval`'s own unit tests are parameterized over
the engine, so all of them run twice. Beyond that: `examples/`, `tests/fixtures/`,
and `ply-corpus`'s generated programs, which is the corpus that will find the
disagreements the hand-written tests do not.

**When the tree-walker is deleted.** All four of:

1. `--engine both` is green on every corpus and every fixture, and every
   `ply-eval` unit test runs on both engines.
2. The machine passes the multi-shot suite in "Required tests" below.
3. `ply test examples/` on the machine is within 1.3× the tree-walker's wall
   clock, on the existing bench harness.
4. `RUNTIME_VERSION` is bumped in the same change that makes `Engine::Machine`
   the default.

All four now hold. On the 10,000-definition / 5,000-test corpus, cold and
uncached, the machine's execute phase is 0.38s against the tree-walker's 2.32s —
0.16×, where the gate is 1.3×. It was 1.7× *slower* until the machine stopped
lowering the whole program per worker per concurrency group: a body is lowered
when it is first called, which is what the lowered representation buys and the
tree-walker's per-worker deep clone of every body cannot.

Then, **before M6 closes**, a change deletes `interp.rs`, `Engine`, the
`--engine` flag, `stacker`, and `grow`. The flag's lifetime is one milestone.
Two evaluators is two semantics, and the second one starts drifting the moment
nobody is comparing them — so deletion is an M6 exit item, not a follow-up
somebody schedules later. It is deliberately not the same change as the flip:
the audit is what would catch a bad flip, and deleting it in the same breath
removes the evidence for the decision being made.

### 7. What the explicit stack lets us delete

The recursion depth guard is a workaround for a native stack limit. Its own
comment says so: *"a semantic limit on runaway recursion, not a stack-safety
one: `grow` keeps the native stack from being the binding constraint."* Once the
stack is a heap value, none of that is true and the workaround goes.

| deleted | why |
| --- | --- |
| `grow()`, `stacker::maybe_grow`, the `stacker` dependency | a Ply call costs one `Frame::Call` on the heap, not several native frames |
| `Interp`'s private depth counter | replaced by `Stack::calls()` — see the correction below |
| the `#[inline(never)]` on every `eval_*` arm | they exist to keep the recursive `eval` frame small |
| `Value::Cell(Rc<RefCell<Value>>)` and the "cell is already borrowed" error | a `RefCell` reentrancy failure cannot happen to a map entry — a user-facing failure path that stops existing (**done in this change**) |
| `Interp::reset`'s "a previous failure can leave frames installed" | a stack that is a value cannot leak from one entry point to the next |
| `tests::recursion_to_the_depth_limit_survives_a_one_mebibyte_thread_stack` | it asserts that 10,000 nested calls do not overflow a 1 MiB worker stack; once no Ply call touches the native stack the property is vacuous |

The *semantic* limit stays: a runaway recursion must be a diagnostic and not an
out-of-memory kill. It is our own bookkeeping, queryable in O(1), and adjusted in
O(1) when a continuation is spliced. The message keeps the phrase "recursion
limit" so that ADR 0004's `AssertionKind::RecursionLimit` still classifies it,
<!-- Corrected 2026-08-24: it does not. That variant is declared in
`ply_test::slice` and constructed nowhere, so nothing classifies on it; what
reads the phrase is four tests matching the string. See `ply-eval/src/limit.rs`
and CONTRIBUTING §"Things known to be broken". -->
and the diagnostic can now name the innermost `Call` frames — the actual
recursion path, which the old guard could not produce.

That last row is the one existing test M6 removes. It is removed because the
behaviour it guards stops existing, not because it fails.

#### 7.1 Correction: pending frames are not the semantic bound

This section originally offered `DEFAULT_MAX_FRAMES` on `Stack::frames()` as the
*exact* replacement for the depth guard. It is not exact, in two ways that an
audit found and that shipped two evaluators disagreeing:

- **It does not cover tail position.** The machine reused the pending `Call`
  frame of a call in tail position, so a tail call cost zero frames and no frame
  budget could ever fire for one. `fn spin(n) = spin(n + 1)` ran past a
  45-second wall clock with no diagnostic, where the tree-walker answered in
  3.8ms — and under `--engine both` that hangs the runner after the
  authoritative engine has already answered.
- **It counts a different thing at a different scale.** 10,000 nested calls
  against 1,000,000 pending frames means a program between the two budgets is a
  diagnostic on one engine and an answer on the other, and a program past both
  still produces two different messages.

The bound is therefore on **nested calls**, shared:
`ply_eval::limit::DEFAULT_MAX_CALLS`, 10,000, counted by both engines — the
tree-walker as its own nesting, the machine as `Stack::calls()`, the
`Frame::Call`s pending on its stack, maintained in O(1) through push, pop,
capture and splice. One diagnostic builder serves both, so the message and the
`innermost calls:` note are one string. A clause body runs below its own
handler on both engines, so the calls the body made since the handler was
installed are not pending while the clause runs — the tree-walker holds them
aside exactly as `capture` does.

**Tail-call elision is gone with it.** A tail call costs a `Frame::Call` like
any other. Charging it is what bounds a tail-recursive runaway, and eliding it
is what made the two engines disagree; the constant-space property it bought is
unobservable under a 10,000-call budget anyway. Restoring it belongs to the
change that deletes the tree-walker, together with the fuel budget a tail loop
would then need — with only one engine left, "a tail-recursive loop is a loop"
becomes an available answer.

**Still the decision, and cited at last — ADR 0022.** This paragraph settled the
tail-call question, **and three later documents each re-derived it as an open
problem without citing it.** ADR 0022 takes the **second half** of the sentence
above — the fuel budget — without the first: `iterate(seed, budget, step)` elides
no call, costs exactly one frame per step on the machine and one host-loop
iteration on the tree-walker, and **takes its bound as an argument so a runaway
is a diagnostic naming a number the program wrote. Tail-call elision stays out.**

**A separate frame bound is gone, and the reason is worth keeping.** This section
argued that a call costs at least one frame, so the *call* bound is reached first
and a frame bound catches only a pathological program. **That is wrong, and it is
the same error §7.1 was written to correct, made once more: a *call* costs one
frame, a *body* costs as many as it pends**, so a body pending enough frames per
call reaches the frame bound first.

**The ceiling had to go rather than be copied into the tree-walker, because it
was a function of *spelling*, not of behaviour.** Measured on two definitions of
the same function making the same nested calls: written with a folded constant it
answered, and written as a long chain of additions it raised. **Giving the
tree-walker the same ceiling would have made both engines refuse a program over
how its additions were written**, and would still have left a backend answering
where both raised. What is left is `Machine::with_max_frames`, an opt-in ceiling
that is **not part of what a program means**, and a machine carrying one enters
no compiled body — **a native body pends no frames and cannot honour a limit
counted in them.**

This also retires §7.1's third bullet: with no ceiling, `Stack::frames()` is an
observation and `Stack::calls()` is the bound.

## Consequences

- **No headline invariant moves.** Renaming a function still selects zero tests;
  moving a definition still changes no hash; incremental and `--no-incremental`
  still agree; a `nondet` atom in a `det` test is still E0412; bisection still
  names the culprit. The machine changes how a program runs, not what a program
  *is*.
- **The `resume` binder enters normalization.** A clause with a binder is a
  different definition from one without, so it must be part of the hashed body —
  and renaming the binder must change no hash, because it is a local and becomes
  a de Bruijn level like every other. This is a `BODY_ENCODING` and
  `FRONTEND_VERSION` bump. Getting it wrong the other way — omitting the binder
  from the hash — makes two programs with different semantics share a cache
  entry, which is the worst defect available in this system.
- **`RUNTIME_VERSION` bumps once,** when the default engine flips.
- **`rpds` joins the workspace** at 1.2.1. It was chosen over `im` 15.1.0 because
  it is parameterized over the shared-pointer kind, so `List` and
  `RedBlackTreeMap` can use `Rc` and non-atomic refcounts, matching the existing
  decision that an `Interp` is confined to one thread; because `im` has not
  shipped since 2021 and splits `Rc` support into a second crate; and because
  `RedBlackTreeMap` iterates in key order, which the byte-identical-artifact rule
  needs and a HAMT does not give.
- **The world never shrinks within a run.** A `with_cell` in a hot loop retains
  one entry per iteration. Each test runs in a fresh fork, so growth is bounded
  by one test, but a single pathological test that today runs in constant space
  will not.
- **`ply-test` gains a fork point.** Each test's worker forks the base world
  rather than starting from an empty one, which is also what ADR 0004's "forking
  a fixture per hybrid (M6)" was waiting for.

## Required tests

The machine, against the tree-walker:

1. Every existing `ply-eval` unit test passes on both engines, compared by full
   diagnostic equality.
2. `--engine both` over `examples/`, `tests/fixtures/` and the generated corpus
   reports zero divergences.
3. A tree-walker asked to run a clause with a `resume` binder refuses with a
   diagnostic naming the clause, and does not evaluate it.

The world:

4. Forking a world and writing to the fork leaves the original unchanged, and
   vice versa. *(landed)*
5. Two tests that both write `cell.write[users]` run in one group and neither
   observes the other's writes.
6. A cell allocated inside a `with_cell` region is still readable through a
   continuation resumed after the region returned — the escape case, which is a
   *success* and not an error.
7. A base world seeded once and forked per test gives every test the seeded
   state and no test another's writes.

Resumption:

8. Zero resumptions: the clause observes the writes made before the `perform`
   and none made after it; the `handle` evaluates to the clause's value.
9. One resumption: `put(5); get()` under the cell-backed state handler answers
   `5` — the resumption sees the clause's write.
10. Two resumptions: the `amb` example evaluates to `30` **and** leaves the trace
    cell at `2`. Both halves are the test; the second is what distinguishes
    threading from snapshotting.
11. A handler that saves and restores the cell around each resumption gives each
    branch the same starting state.
12. A continuation resumed after its handler's `handle` expression has returned
    splices onto the then-current stack and runs.
13. `op(x) -> e` and `op(x) resume k -> k(e)` produce identical results,
    identical worlds, and identical footprints on every fixture that has a
    handler.
14. A handler that performs the operation it handles reaches the next handler
    out, not itself, under both clause forms.

Machine mechanics:

15. Capturing a continuation across *n* enclosing handlers copies *n* segments,
    independent of how many frames are pending. Asserted through
    `Continuation::segments` and `Continuation::frames`. *(landed)*
16. A continuation captured inside `map`'s callback and resumed twice produces
    two complete lists.
17. Exceeding a frame ceiling that was asked for (`Machine::with_max_frames`)
    is a diagnostic whose notes name the innermost `Call` frames, and whose
    message deliberately does **not** contain "recursion limit" — it is one
    engine's heap running out, not a statement about the program.
    *(Revised 2026-08-24. It read: "Exceeding `DEFAULT_MAX_FRAMES` is a
    diagnostic whose message contains 'recursion limit' and whose notes name the
    innermost `Call` frames." There is no `DEFAULT_MAX_FRAMES`, and phrasing a
    resource ceiling as a recursion limit is what let it read as a program
    answer.)*
18. A continuation applied to the wrong number of arguments is `ARITY_MISMATCH`.

Typing and scheduling:

19. A row is unchanged by the number of resumptions: the same program with a
    clause resuming zero, one and two times has one footprint.
20. `ρ_κ` solving does not trip the occurs check, and a handler whose clause
    calls its own continuation infers a closed row.
21. Adding *N* world-isolated tests to a corpus leaves the group count unchanged,
    for N = 0, 1 and 100.
22. `ply test --json` reports `isolation` per test and `isolated: n of m` in the
    summary, and the numbers agree with the footprints.
23. A `nondet` operation performed once and resumed twice delivers the same value
    to both resumptions.

Content addressing:

24. Adding a `resume` binder to a clause changes that definition's hash.
25. Renaming the `resume` binder changes no hash.

## Alternatives considered

**(a) Brand the region, rank-2, and forbid escape.** The principled answer, and
it forbids the programs multi-shot exists for. It also introduces rank-2
polymorphism into an otherwise Hindley–Milner type system to serve one
construct, and it moves the failure from "impossible" to "a type error a user
has to understand", which for a construct as ordinary as a state handler is a
bad trade. Kept in weakened form: the existing result-type region check stays,
because it is a good error message for the ordinary mistake. It is no longer
load-bearing for soundness, and that demotion is deliberate.

**(c) Deep-copy cell state at capture.** Gives per-resumption worlds — which §3
argues is the wrong semantics anyway — at O(state) per capture, with no
structural sharing and no fork story. It is (b) with everything good about (b)
removed.

**Snapshot the world at capture and restore it at each resumption.** The reading
"each resumption gets its own world" invites this, and §3.1 is the argument
against: it makes a cell-backed state handler unwritable, because a clause's own
write is discarded before the computation that asked for it runs. It is also
one-way — a handler can build snapshot semantics out of threaded semantics with
four lines, and cannot build threaded semantics out of snapshot semantics at all.

**Shallow handlers** (the handler is *not* reinstalled on resumption). Cheaper to
implement and it makes every stateful handler recursive by hand. Deep is what
the existing tail-resumptive semantics already is, so shallow would also break
every existing program.

**`resume` as an implicitly-bound name in every clause.** No grammar change, and
it makes every clause general, which retypes every existing handler (a clause
body's type becomes the `handle`'s result rather than the operation's) and
forces a capture at every `perform` whether or not anyone wanted one. The opt-in
binder keeps the common case free and the intent visible.

**Frames holding `&'a Expr`, with a lifetime on `Value`.** Simpler inside
`ply-eval` and it removes the lowered code representation entirely. Rejected
because `Value<'a>` spreads to `World`, `Env`, `Closure`, `Continuation` and
every crate that holds an evaluated value, and because `Value`'s shape is a
pinned contract several crates are written against.

**Trampoline the existing tree-walker instead of rewriting it.** Turns the
recursion into a loop without ever reifying a continuation, so it buys the stack
depth and none of the milestone. The frames have to exist for capture to be O(1);
once they exist, the tree-walker's recursive `eval` has nothing left to do.

**A global cell counter shared by every world.** Would make ids unique across
forks and remove the sibling-collision hazard. Rejected because it makes a
`World` depend on a mutable counter it does not own, which is precisely the
property the design is trying to delete — and because the hazard it removes is
already unreachable: nothing carries a value from one fork into a sibling.

## Not in M6

- **A source-level `fixture` construct.** M6 lands the mechanism —
  `(World, Value)`, `World::fork`, `set_base_world` — and no syntax to declare
  one. A fixture is a definition, so it needs a hash story, a determinism story
  and a place in the namespace, and inventing those under a machine rewrite is
  two designs in one change. Until then a test builds its own state and `ply-test`
  forks an empty base, which is the mechanism running with a trivial input.
  **This is the largest gap in the milestone**: "build a fixture once, fork per
  test in microseconds" is demonstrable through the API and not writable in Ply.
- **A world snapshot/restore builtin.** It would let a handler get per-resumption
  state without the save/restore idiom, and it is a capability with no type-level
  account: restoring the world un-does writes that the row still reports. M7's
  deterministic simulation is where a principled version belongs.
- **Reclaiming world entries.** Every cheap rule is unsound. Removing on region
  exit breaks a cell stored into an enclosing cell, which is reachable today
  without any continuation. Removing when no capture happened during the region
  breaks the same case. Correct reclamation needs reachability from the live
  environment graph — a tracing collector — and that is its own change.
- **One-shot / linearity annotations on continuations.** A `resume`-once clause
  could skip the capture and the type system could enforce it. Worth doing when
  there is a measurement saying the capture costs something; there is not.
- **Effect-typed control operators** beyond `resume`: no `shift`/`reset`, no
  first-class prompts. `handle` is the only delimiter.
