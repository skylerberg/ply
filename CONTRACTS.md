# Crate contracts

These public APIs are fixed. Crates are implemented concurrently against them,
so a signature here is a promise other crates have already been written to call.
Add anything you like *beyond* these; do not change what is written here. If a
contract is genuinely unworkable, implement the closest thing that compiles and
say so in your report rather than silently diverging.

`ply-span` and the pinned modules `ply-syntax::ast` / `ply-core::ty` are already
written and tested. Read them before starting — they answer most questions.

---

## ply-syntax

```rust
pub mod ast;      // written; do not modify
pub mod lexer;
pub mod parser;

pub fn parse(source: SourceId, text: &str) -> Result<ast::Module, Vec<Diagnostic>>;
```

### Grammar

```
item      := fnDef | typeDef | effectDef | testDef
fnDef     := "fn" IDENT generics? "(" params ")" ("->" type)? ("/" row)? ("=" expr | block)
generics  := "<" IDENT,* ("|" IDENT,*)? ">"        // types, then effect vars
typeDef   := "type" IDENT ("<" IDENT,* ">")? "=" (type | variants)
variants  := "|"? variant ("|" variant)*
variant   := IDENT ("(" type,* ")")?
effectDef := "nondet"? "effect" IDENT "{" opDef* "}"
opDef     := ("read"|"write") IDENT ("[" IDENT "]")? "(" type,* ")" "->" type
testDef   := "test" ("/" "nondet")? STRING block

row       := "{" atom,* ("|" IDENT)? "}" | IDENT
atom      := IDENT "." ("read"|"write") ("[" IDENT "]")?

expr      := ... standard precedence, listed below
block     := "{" stmt* expr? "}"
stmt      := "let" pattern (":" type)? "=" expr ";" | expr ";"

perform   := IDENT "." IDENT ("[" IDENT "]")? "(" expr,* ")"
handle    := "handle" expr "with" "{" hClause* retClause? "}"
hClause   := IDENT "." IDENT ("[" IDENT "]")? "(" IDENT,* ")" "->" expr ","?
retClause := "return" IDENT "->" expr ","?
withCell  := "with_cell" "[" IDENT "]" "(" expr ")" "{" IDENT "->" expr "}"
```

Precedence, loosest to tightest: `||`, `&&`, comparison (`== != < <= > >=`),
`++` (string concat), `+ -`, `* / %`, unary `- !`, application / field access /
indexing. `if`, `match`, `handle`, `with_cell`, lambda (`|x, y| expr`) are
primary expressions.

Comments are `//` to end of line. Integers are decimal, optionally `_`
separated. Strings are double-quoted with `\n \t \\ \" \r \0` escapes.

Recover from errors where cheap — report several parse errors per run rather
than stopping at the first. Every node must carry a real span.

---

## ply-core

```rust
pub mod ty;       // written; do not modify
pub mod env;
pub mod unify;
pub mod infer;

pub struct EffectInfo {
    pub name: Symbol,
    pub nondet: bool,
    pub ops: IndexMap<Symbol, OpInfo>,
}
pub struct OpInfo {
    pub name: Symbol,
    pub mode: Mode,
    pub resource_param: bool,
    pub params: Vec<Type>,
    pub ret: Type,
}

pub struct DefInfo {
    pub name: Symbol,
    pub scheme: Scheme,
    /// Closed after solving. Empty for a pure function.
    pub footprint: Footprint,
}

pub struct TestInfo {
    pub name: String,
    pub index: usize,          // position among tests in the module
    pub nondet: bool,
    pub footprint: Footprint,
}

pub struct CheckOutput {
    pub defs: IndexMap<Symbol, DefInfo>,
    pub tests: Vec<TestInfo>,
    pub effects: IndexMap<Symbol, EffectInfo>,
    pub ctors: IndexMap<Symbol, CtorInfo>,   // variant name -> arity, owning type
}

pub fn check_module(module: &Module) -> Result<CheckOutput, Vec<Diagnostic>>;
```

Requirements:

- Hindley–Milner with let-polymorphism over `Type`, extended so every expression
  also yields a `Row`.
- Row unification per DESIGN.md §1: `{A|ρ1} ~ {B|ρ2}` solves
  `ρ1 := (B\A) ∪ ρ3`, `ρ2 := (A\B) ∪ ρ3` with ρ3 fresh, plus an occurs check.
- The `handle` rule from DESIGN.md §2:
  `(row(body) \ handled) ∪ ⋃ row(clause_i)`.
- `with_cell[r](init) { c -> body }` binds `c : Cell<T>` and discharges
  `cell.read[r]` / `cell.write[r]` from the body's row. `cell_get` / `cell_set`
  are prelude builtins that perform those atoms.
- A `/ {...}` annotation is an upper bound: inference must produce a subset, and
  the annotation becomes the published signature. Violation is `EFFECT_NOT_PERMITTED`.
- After solving, a definition's row must be closed. A surviving row variable in a
  top-level signature that was not generalized is `UNBOUND_ROW_VAR`.
- A `test` that is not `nondet` whose final footprint contains an atom belonging
  to an effect declared `nondet` is `NONDET_IN_DET_TEST` (code `E0412`). The
  message must name the offending atom and point at the performing expression.

Use the codes in `ply_span::codes`. Report as many independent errors as you can
per run.

---

## ply-hash

```rust
pub mod normalize;
pub mod graph;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct DefHash(pub [u8; 32]);

impl DefHash {
    pub fn to_hex(&self) -> String;          // 64 chars
    pub fn short(&self) -> String;           // first 12 hex chars, for display
}
impl std::fmt::Display for DefHash;          // == short()

pub struct HashOutput {
    pub defs: IndexMap<Symbol, DefHash>,
    pub tests: Vec<DefHash>,                 // parallel to CheckOutput::tests
    /// Direct references, definition name -> names it mentions.
    pub deps: IndexMap<Symbol, Vec<Symbol>>,
    /// Transitive closure, including the definition itself.
    pub closure: IndexMap<Symbol, BTreeSet<Symbol>>,
}

pub fn hash_module(module: &Module, check: &CheckOutput)
    -> Result<HashOutput, Vec<Diagnostic>>;
```

Normalization, per DESIGN.md §3 — this is the crate's whole point, get it exact:

- Local binders become de Bruijn **levels**; a local's *name* never enters the hash.
- A reference to another top-level definition contributes **that definition's
  hash**, never its name.
- Names, spans, comments, and formatting are erased. Declared type and effect
  annotations are kept (they are part of the published signature).
- Serialize deterministically: postorder, every variable-length field
  length-prefixed, every node tagged with a distinct discriminant byte so that
  no two differently-shaped trees can collide.
- `hash = blake3(bytes)`.
- Mutually recursive definitions form an SCC (Tarjan). Hash the component with
  self-references replaced by component-local indices, then each member is
  `blake3(component_hash ‖ index_le_u32)`. Order members by their position in the
  source so the component hash is stable.
- A `test`'s hash covers its body, so it transitively covers everything it calls.

These properties must hold, and there must be a test for each:

1. Renaming a top-level definition changes **no** hash in the program.
2. Renaming a local parameter or `let` binding changes no hash.
3. Reformatting, recommenting, and reordering top-level items change no hash.
4. Editing a function body changes its hash and every transitive dependent's.
5. Two structurally identical definitions with different names hash identically.
6. Swapping two fields, two arguments, or two match arms changes the hash.

---

## ply-store

```rust
pub struct Store { .. }

#[derive(Clone, Serialize, Deserialize)]
pub enum Outcome {
    Pass,
    Fail { message: String, diagnostic: Option<Diagnostic> },
}

impl Store {
    /// Opens/creates `<root>/.ply-cache`.
    pub fn open(root: &Path) -> anyhow::Result<Store>;
    pub fn get(&self, hash: DefHash) -> Option<Outcome>;
    pub fn put(&mut self, hash: DefHash, outcome: Outcome);
    pub fn flush(&mut self) -> anyhow::Result<()>;
    pub fn clear(&mut self) -> anyhow::Result<()>;
    pub fn len(&self) -> usize;
}
```

The cache key is `(RUNTIME_VERSION, DefHash)`; bumping `RUNTIME_VERSION` (a
`pub const &str` in this crate) invalidates everything. JSON on disk is fine —
this is not the bottleneck and being able to read the cache by hand is worth
more than speed. Writes must be atomic (temp file + rename) so an interrupted
run cannot corrupt the cache. A corrupt or unreadable cache must degrade to an
empty cache with a warning, never a crash.

---

## ply-eval

```rust
pub enum Value {
    Int(i64), Bool(bool), Str(Arc<str>), Unit,
    List(Vector<Value>),                    // persistent or Arc<Vec<_>>; cheap clone
    Record(Arc<BTreeMap<Symbol, Value>>),
    Ctor { name: Symbol, args: Arc<Vec<Value>> },
    Closure(Arc<Closure>),
    Cell(Rc<RefCell<Value>>),
}

pub struct Interp<'a> { .. }

impl<'a> Interp<'a> {
    pub fn new(module: &'a Module, check: &'a CheckOutput) -> Self;
    pub fn eval_test(&mut self, index: usize) -> Result<(), Diagnostic>;
    pub fn eval_expr_for_test(&mut self, e: &Expr) -> Result<Value, Diagnostic>;
}
```

- Tree-walking, environment-passing. Correctness first; speed is M9's problem.
- Effect handlers are a stack of frames. `Perform` walks the stack outward to the
  first frame with a matching `(effect, op, resource)` clause, evaluates that
  clause's body in the handler's captured environment, and returns its value
  directly to the perform site — tail-resumptive, no continuation capture.
- An unhandled `Perform` at runtime is `UNHANDLED_EFFECT`. Inference should have
  ruled this out; it is a bug-catcher, not a user-facing path.
- Recursion depth must be bounded and produce a diagnostic, not a stack overflow.
- Prelude builtins: `assert`, `assert_eq`, `len`, `push`, `map`, `filter`, `fold`,
  `range`, `int_to_string`, `string_concat`, `cell_get`, `cell_set`, `panic`.
  A failing `assert`/`assert_eq` is `ASSERTION_FAILED` with a structured
  expected/actual message.
- An `Interp` must be usable from a worker thread. If `Value` holds `Rc`, keep
  each interpreter and every value it produces confined to one thread and do not
  implement `Send` for it — the scheduler hands each worker its own `Interp`.

---

## ply-test

```rust
pub struct Selection {
    pub total: usize,
    pub cached: Vec<(usize, Outcome)>,     // test index -> cached outcome
    pub to_run: Vec<usize>,
    /// Concurrency groups over `to_run`; every pair within a group has
    /// non-conflicting footprints.
    pub groups: Vec<Vec<usize>>,
}

pub fn select(check: &CheckOutput, hashes: &HashOutput, store: &Store) -> Selection;
pub fn run(selection: &Selection, module: &Module, check: &CheckOutput,
           hashes: &HashOutput, store: &mut Store) -> RunReport;

pub struct RunReport {
    pub passed: usize, pub failed: usize, pub cached: usize,
    pub failures: Vec<Failure>,
    pub duration: Duration,
}
pub struct Failure {
    pub name: String,
    pub diagnostic: Diagnostic,
    /// Definitions in this test's closure whose hash is not in the store —
    /// the suspects for this failure.
    pub suspects: Vec<Symbol>,
}
```

- Selection: a test runs iff its `DefHash` is absent from the store. `nondet`
  tests always run and are never written to the store.
- Grouping: build a conflict graph over the selected tests' footprints and colour
  it greedily, largest-footprint-first. Tests within a group run concurrently via
  `rayon`; groups run in sequence.
- A panic in one test must be caught and reported as a failure, not abort the run.
- Only write `Pass` to the store. A failure must not be cached — a red test has
  to re-run every time until it goes green.

---

## ply-cli

```
ply check [PATH]            typecheck; print inferred signatures with `--types`
ply test  [PATH]            the main event
ply run   [PATH]            evaluate `main`
ply hash  [PATH]            definition hashes, `--deps` to show the graph
ply cache clear|stats
```

`ply test` flags: `--json`, `--explain` (why each test was selected or skipped,
and how groups were formed), `--no-cache`, `--filter <substring>`, `--jobs <n>`.

Default `PATH` is `.`; a directory means every `*.ply` under it, sorted, parsed
as one module. Exit `0` on success, `1` on test failure, `2` on a compile error.
Human output goes to stdout; `--json` emits one JSON object to stdout and nothing
else.

Target output shape:

```
$ ply test
   selected 3 of 47 (44 cached)
   3 groups · 10 workers

   ✓ active_users excludes inactive          1.2ms
   ✓ orders roll up by customer              0.8ms
   ✗ balance never goes negative             2.1ms

   1 failed, 2 passed, 44 cached (0.08s)

   balance never goes negative
     assertion failed: expected 0, found -5
       at src/ledger.ply:88:5
     suspects: apply_debit, Ledger.balance
```
