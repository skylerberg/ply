//! What a **hostile** host handler can do, and what — if anything — notices.

use ply_core::ty::{EffectAtom, Footprint, Resource};
use ply_core::{CheckOutput, check_program};
use ply_eval::host::{
    Determinism, HostAnswer, HostBinding, HostHandler, HostOp, HostRegistry, HostRequest,
    HostResource, HostRuntime, Linearity,
};
use ply_eval::{Machine, TaskId, Value};
use ply_span::{Diagnostic, SourceId, Span, Symbol, codes};
use ply_syntax::ast::{Mode, ModuleName, Program};
use ply_syntax::resolve::{Resolved, resolve};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

struct Compiled {
    program: Program,
    resolved: Resolved,
    check: CheckOutput,
}

fn compile(source: &str) -> Compiled {
    let mut program =
        ply_syntax::parse_program(vec![(SourceId(0), ModuleName::from_dotted("t"), source)])
            .expect("the fixture parses");
    let resolved = resolve(&mut program).expect("the fixture resolves");
    let check = check_program(&program, &resolved).expect("the fixture typechecks");
    Compiled {
        program,
        resolved,
        check,
    }
}

fn compile_error(source: &str) -> Vec<Diagnostic> {
    let mut program =
        ply_syntax::parse_program(vec![(SourceId(0), ModuleName::from_dotted("t"), source)])
            .expect("the fixture parses");
    let resolved = resolve(&mut program).expect("the fixture resolves");
    check_program(&program, &resolved).expect_err("the fixture was expected not to typecheck")
}

impl Compiled {
    fn machine(&self) -> Machine<'_> {
        Machine::new(&self.program, &self.resolved, &self.check)
    }

    fn bound(&self, entries: Vec<(HostOp, Arc<dyn HostHandler>)>) -> Machine<'_> {
        let mut registry = HostRegistry::new();
        for (op, handler) in entries {
            registry.register(op, handler);
        }
        let binding = registry.bind(&self.check).expect("the fixture binds");
        let mut machine = self.machine();
        machine.set_host_binding(Arc::new(binding));
        machine
    }
}

fn op(effect: &str, name: &str, resource: HostResource, linearity: Linearity) -> HostOp {
    HostOp {
        effect: Symbol::new(effect),
        op: Symbol::new(name),
        resource,
        determinism: Determinism::Nondeterministic,
        linearity,
        blocking: false,
        secrets: false,
        path: "audit::hostile",
    }
}

fn any(effect: &str, name: &str) -> HostOp {
    op(effect, name, HostResource::Any, Linearity::Repeatable)
}

fn atom(effect: &str, resource: &str, mode: Mode) -> EffectAtom {
    EffectAtom::new(effect, Resource::Named(Symbol::new(resource)), mode)
}

#[track_caller]
fn diagnostic(outcome: Result<(), Diagnostic>) -> Diagnostic {
    outcome.expect_err("the program was expected to fail")
}

/// A handler registered against a **read** operation on one resource, which mutates state of its
/// own on every call and answers the new value.
#[derive(Default)]
struct Mutates {
    writes: AtomicU64,
}

impl HostHandler for Mutates {
    fn call(&self, _: &dyn HostRuntime, _: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        let n = self.writes.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(HostAnswer::Value(Value::Int(n as i64)))
    }
}

/// One `read` operation over one resource, and a program that only ever reads.
const READS: &str = r#"
nondet effect db {
  read get[r](key: Int) -> Int
}

fn lookup(k: Int) -> Int / {db.read[users]} = db.get[users](k)

test/nondet "first" { assert_eq(lookup(1), 1) }

test/nondet "second" { assert_eq(lookup(1), 2) }
"#;

/// **The headline.**
#[test]
fn documents_a_read_declared_handler_that_writes_is_recorded_as_a_read() {
    let compiled = compile(READS);
    let handler = Arc::new(Mutates::default());
    let mut machine = compiled.bound(vec![(any("db", "get"), handler.clone())]);
    machine.set_declared_footprint(Footprint::from_atoms([atom("t.db", "users", Mode::Read)]));

    machine.eval_test(0).expect("the run is green");

    let used = machine.host_use().expect("the run reached the host");
    assert_eq!(
        used.atoms.atoms().cloned().collect::<Vec<_>>(),
        [atom("t.db", "users", Mode::Read)],
        "the recorded footprint is the registration's claim, not the handler's behaviour"
    );
    assert_eq!(
        handler.writes.load(Ordering::SeqCst),
        1,
        "the handler mutated once, and nothing above the boundary can tell"
    );
    assert!(
        !used.atoms.atoms().any(|a| a.mode == Mode::Write),
        "a write happened and no write is reported"
    );
}

/// The same fact stated where it is dangerous: two entry points, one machine, and a handler whose
/// Rust-side state carries between them.
#[test]
fn documents_a_lying_handler_couples_two_entry_points_that_share_nothing() {
    let compiled = compile(READS);
    let handler = Arc::new(Mutates::default());
    let mut machine = compiled.bound(vec![(any("db", "get"), handler.clone())]);

    machine.eval_test(0).expect("the first test is green");
    machine
        .eval_test(1)
        .expect("the second test is green *because* it saw the first one's write");

    assert_eq!(handler.writes.load(Ordering::SeqCst), 2);
    // And the machine's own reset is honest about everything it owns: the per-entry-point counters
    // really did restart.
    assert_eq!(machine.host_ops(), 0, "`get` is registered `Repeatable`");
    assert_eq!(
        machine.host_use().expect("reached the host").operations,
        1,
        "the second entry point reports only its own operation"
    );
}

/// The narrow-resource lie, which is the same hole one level down.
#[test]
fn documents_a_narrow_registration_may_touch_a_resource_it_never_named() {
    /// Answers from a counter that another registration also owns.
    struct Shared(Arc<AtomicU64>);

    impl HostHandler for Shared {
        fn call(&self, _: &dyn HostRuntime, _: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
            Ok(HostAnswer::Value(Value::Int(
                self.0.fetch_add(1, Ordering::SeqCst) as i64 + 1,
            )))
        }
    }

    let compiled = compile(
        r#"
nondet effect db {
  read  get[r](key: Int) -> Int
  write put[r](key: Int, value: Int) -> Int
}

fn readers() -> Int / {db.read[users]} = db.get[users](1)

fn writers() -> Int / {db.write[orders]} = db.put[orders](1, 1)

test/nondet "reads users" { assert_eq(readers(), 1) }

test/nondet "writes orders" { assert_eq(writers(), 2) }
"#,
    );
    let cell = Arc::new(AtomicU64::new(0));
    let mut machine = compiled.bound(vec![
        (
            op(
                "db",
                "get",
                HostResource::Only(Resource::Named(Symbol::new("users"))),
                Linearity::Repeatable,
            ),
            Arc::new(Shared(Arc::clone(&cell))) as Arc<dyn HostHandler>,
        ),
        (
            op(
                "db",
                "put",
                HostResource::Only(Resource::Named(Symbol::new("orders"))),
                Linearity::Repeatable,
            ),
            Arc::new(Shared(Arc::clone(&cell))),
        ),
    ]);

    let users = compiled.check.tests[0].footprint.clone();
    let orders = compiled.check.tests[1].footprint.clone();
    assert!(
        !users.conflicts_with(&orders),
        "the declared footprints do not conflict, so these two may run concurrently"
    );

    machine.eval_test(0).expect("green");
    machine
        .eval_test(1)
        .expect("green, and only because it saw the other resource's handler move");
    assert_eq!(cell.load(Ordering::SeqCst), 2);
}

/// `blocking: true` means "the work leaves this thread and a token comes back", and the boundary
/// now holds a handler to the half of that which is checkable.
#[test]
fn a_blocking_handler_that_answers_a_value_inline_is_refused() {
    struct Inline;

    impl HostHandler for Inline {
        fn call(&self, _: &dyn HostRuntime, _: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
            Ok(HostAnswer::Value(Value::Int(1)))
        }
    }

    let compiled = compile(
        r#"
nondet effect net {
  write send[s](payload: Int) -> Int
}

test/nondet "blocking, allegedly" { assert_eq(net.send[socket](1), 1) }
"#,
    );
    let mut declared = any("net", "send");
    declared.blocking = true;
    let mut machine = compiled.bound(vec![(declared, Arc::new(Inline))]);
    let d = diagnostic(machine.eval_test(0));
    assert_eq!(d.code, codes::HOST_BLOCKING_ANSWER, "{}", d.message);
    assert!(
        d.message.contains("audit::hostile"),
        "the refusal must name the registration that made the claim: {}",
        d.message
    );

    // The same handler, declared honestly, is green: the check is about the declaration disagreeing
    // with the answer, not about answering a value.
    let mut machine = compiled.bound(vec![(any("net", "send"), Arc::new(Inline))]);
    machine.eval_test(0).expect("green");
}

/// And the residual, stated so nobody reads `E0428` as more than it is: the machine still calls
/// **every** handler's `call` on its own thread, `blocking` or not.
#[test]
fn a_blocking_handler_is_still_entered_on_the_machines_thread() {
    struct Reports(AtomicU64);

    impl HostHandler for Reports {
        fn call(&self, _: &dyn HostRuntime, _: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
            // A `ThreadId` is opaque, so identity is established by hashing it into a `u64` the
            // test can compare against its own.
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            std::hash::Hash::hash(&std::thread::current().id(), &mut hasher);
            self.0
                .store(std::hash::Hasher::finish(&hasher), Ordering::SeqCst);
            Ok(HostAnswer::Pending(ply_eval::host::Pending {
                token: 7,
                label: "send",
            }))
        }
    }

    /// Resolves whatever it is handed, which is what a dispatched job that has already finished
    /// looks like.
    struct Resolves;

    impl HostRuntime for Resolves {
        fn poll(&self, _: &ply_eval::host::Pending) -> Result<Option<Value>, Diagnostic> {
            Ok(Some(Value::Int(1)))
        }

        fn park(&self) -> Result<(), Diagnostic> {
            Ok(())
        }

        fn block_on(&self, _: ply_eval::host::Pending) -> Result<Value, Diagnostic> {
            Ok(Value::Int(1))
        }
    }

    let compiled = compile(
        r#"
nondet effect net {
  write send[s](payload: Int) -> Int
}

test/nondet "blocking, honestly" { assert_eq(net.send[socket](1), 1) }
"#,
    );
    let mut declared = any("net", "send");
    declared.blocking = true;
    let handler = Arc::new(Reports(AtomicU64::new(0)));
    let mut machine = compiled.bound(vec![(declared, handler.clone())]);
    machine.set_host_runtime(std::rc::Rc::new(Resolves));
    machine.eval_test(0).expect("green");

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(&std::thread::current().id(), &mut hasher);
    assert_eq!(
        handler.0.load(Ordering::SeqCst),
        std::hash::Hasher::finish(&hasher),
        "`call` itself runs on the machine's thread; only the work it dispatches leaves"
    );
}

/// A handler that unwinds takes the machine with it.
#[test]
fn documents_a_panicking_handler_unwinds_out_of_the_machine() {
    struct Panics;

    impl HostHandler for Panics {
        fn call(&self, _: &dyn HostRuntime, _: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
            panic!("the trusted computing base panicked");
        }
    }

    let compiled = compile(
        r#"
nondet effect net {
  write send[s](payload: Int) -> Int
}

test/nondet "panics" { assert_eq(net.send[socket](1), 1) }
"#,
    );
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut machine = compiled.bound(vec![(any("net", "send"), Arc::new(Panics))]);
        machine.eval_test(0)
    }));
    std::panic::set_hook(previous);
    assert!(
        outcome.is_err(),
        "the machine turned a handler panic into a value or a diagnostic, which would be better \
         than this and means the note above is out of date"
    );
}

/// A host answer is not type-checked against the operation it answers, so a handler can inject a
/// value the program's own types say is impossible.
#[test]
fn documents_a_wrongly_typed_host_answer_is_a_diagnostic_that_blames_the_program() {
    struct Wrong;

    impl HostHandler for Wrong {
        fn call(&self, _: &dyn HostRuntime, _: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
            Ok(HostAnswer::Value(Value::Str("not an Int".into())))
        }
    }

    let compiled = compile(
        r#"
nondet effect net {
  write send[s](payload: Int) -> Int
}

test/nondet "arithmetic on a host answer" { assert_eq(net.send[socket](1) + 1, 2) }
"#,
    );
    let mut machine = compiled.bound(vec![(any("net", "send"), Arc::new(Wrong))]);
    let d = diagnostic(machine.eval_test(0));

    assert_eq!(d.code, codes::RUNTIME_ERROR, "{}", d.message);
    assert!(
        !d.message.contains("audit::hostile") && !d.notes.join(" ").contains("audit::hostile"),
        "the handler that produced the impossible value is not named: {d:?}"
    );
}

/// A fabricated task handle is refused rather than used as an index.
#[test]
fn a_fabricated_task_handle_from_a_host_answer_is_refused() {
    struct Fake;

    impl HostHandler for Fake {
        fn call(&self, _: &dyn HostRuntime, _: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
            Ok(HostAnswer::Value(Value::Task(TaskId(9999))))
        }
    }

    let compiled = compile(
        r#"
nondet effect net {
  write send[s](payload: Int) -> Task<Int>
}

test/nondet "a handle from nowhere" {
  let real = task.spawn(|| 1);
  assert_eq(task.join(net.send[socket](1)) + task.join(real), 2)
}
"#,
    );
    let mut entries: Vec<(HostOp, Arc<dyn HostHandler>)> = ["spawn", "join", "yield"]
        .into_iter()
        .map(|name| (any("task", name), Arc::new(Fake) as Arc<dyn HostHandler>))
        .collect();
    entries.push((any("net", "send"), Arc::new(Fake)));

    let mut machine = compiled.bound(entries);
    let d = diagnostic(machine.eval_test(0));
    assert_eq!(d.code, codes::REGION_ESCAPE_AT_BOUNDARY, "{}", d.message);
    assert!(d.message.contains("`Task`"), "{}", d.message);
    assert!(
        d.notes.iter().any(|n| n.contains("audit::hostile")),
        "the handler that minted it is named: {:#?}",
        d.notes
    );
}

/// The defence that does hold, and the one worth keeping: a cell cannot be written into a host
/// operation's declared signature at all.
#[test]
fn a_cell_cannot_cross_a_host_operations_signature() {
    let cell = compile_error(
        r#"
nondet effect net {
  write send[s](c: Cell<Int>) -> Cell<Int>
}

test/nondet "smuggle a cell" {
  with_cell[users](5) { c -> assert_eq(cell_get(net.send[socket](c)), 5) }
}
"#,
    );
    assert!(
        cell.iter().any(|d| d.code == codes::RESOURCE_REQUIRED),
        "{:?}",
        cell.iter().map(|d| d.code).collect::<Vec<_>>()
    );

    let closure = compile_error(
        r#"
nondet effect net {
  write send[s](f: () -> Int) -> () -> Int
}

test/nondet "smuggle a closure over a cell" {
  with_cell[users](5) { c -> {
    let g = net.send[socket](|| cell_get(c));
    assert_eq(g(), 5)
  } }
}
"#,
    );
    assert!(
        closure
            .iter()
            .any(|d| d.code == codes::EFFECT_NOT_PERMITTED),
        "{:?}",
        closure.iter().map(|d| d.code).collect::<Vec<_>>()
    );
}

/// Every route into the boundary that is not a bare `perform` in a test body.
#[test]
fn no_indirection_reaches_the_host_in_a_hermetic_run() {
    const CASES: [(&str, &str); 6] = [
        (
            "a helper the test calls",
            "test/nondet \"x\" { assert_eq(helper(1), 1) }",
        ),
        (
            "a closure the test builds and applies",
            "test/nondet \"x\" { let f = || net.send[socket](1); assert_eq(f(), 1) }",
        ),
        (
            "a builtin's callback",
            "test/nondet \"x\" { assert_eq(len(map(range(0, 1), |i| net.send[socket](i))), 1) }",
        ),
        (
            "the body of a `handle` clause for an unrelated effect",
            "test/nondet \"x\" { assert_eq(handle { other.ask() } with { other.ask() -> net.send[socket](1) }, 1) }",
        ),
        (
            "a `with_cell` region body",
            "test/nondet \"x\" { with_cell[users](0) { c -> assert_eq(net.send[socket](cell_get(c)), 1) } }",
        ),
        (
            "a partially applied helper",
            "test/nondet \"x\" { let f = helper; assert_eq(f(1), 1) }",
        ),
    ];

    for (what, body) in CASES {
        let source = format!(
            r#"
nondet effect net {{
  write send[s](payload: Int) -> Int
}}

effect other {{
  read ask() -> Int
}}

fn helper(k: Int) -> Int / {{net.write[socket]}} = net.send[socket](k)

{body}
"#
        );
        let compiled = compile(&source);
        let handler = Arc::new(Mutates::default());
        let mut registry = HostRegistry::new();
        registry.register(any("net", "send"), handler.clone());

        let mut machine = compiled.machine();
        machine.set_host_binding(Arc::new(HostBinding::hermetic_with(registry)));
        let d = diagnostic(machine.eval_test(0));
        assert_eq!(
            d.code,
            codes::HERMETIC_BOUNDARY,
            "{what}: reached the boundary as {} instead",
            d.code
        );
        assert_eq!(
            handler.writes.load(Ordering::SeqCst),
            0,
            "{what}: the handler ran in a hermetic run"
        );
    }
}

/// A `simulate` region is the one route that must be refused even *with* `--host`, and it must be
/// refused before the handler is called: DPOR re-runs the region once per interleaving, so a socket
/// inside one sends a packet per schedule explored and then calls the total a proof.
#[test]
fn a_region_never_reaches_the_host_bound_or_not() {
    let compiled = compile(
        r#"
nondet effect net {
  write send[s](payload: Int) -> Int
}

fn helper(k: Int) -> Int / {net.write[socket]} = net.send[socket](k)

test/nondet "a socket inside a region" { simulate { assert_eq(helper(1), 1) } }
"#,
    );

    for bound in [false, true] {
        let handler = Arc::new(Mutates::default());
        let mut registry = HostRegistry::new();
        registry.register(any("net", "send"), handler.clone());
        let mut machine = compiled.machine();
        machine.set_host_binding(Arc::new(if bound {
            registry.bind(&compiled.check).expect("binds")
        } else {
            HostBinding::hermetic_with(registry)
        }));
        let d = diagnostic(machine.eval_test(0));
        assert_eq!(d.code, codes::HOST_IN_SIMULATION, "bound: {bound}");
        assert_eq!(
            handler.writes.load(Ordering::SeqCst),
            0,
            "bound: {bound}: the refusal must precede the packet"
        );
    }
}

/// A registration for an effect the program never declares is silently idle when it is `Any` — by
/// design, so that one registry can be compiled into every program — and the thing worth checking
/// is that idle really means unreachable.
#[test]
fn an_idle_any_registration_contributes_no_atom_and_serves_nothing() {
    let compiled = compile(
        r#"
nondet effect net {
  write send[s](payload: Int) -> Int
}

test/nondet "only net" { assert_eq(net.send[socket](1), 1) }
"#,
    );
    let mut registry = HostRegistry::new();
    registry.register(any("net", "send"), Arc::new(Mutates::default()));
    // An effect this program has never heard of, registered `Any`.
    registry.register(any("postgres", "query"), Arc::new(Mutates::default()));
    let binding = registry
        .bind(&compiled.check)
        .expect("an idle driver binds");

    assert_eq!(
        binding
            .listing()
            .rows
            .iter()
            .map(|r| r.to_string())
            .collect::<Vec<_>>(),
        ["t.net.send[socket]"],
        "an idle registration must contribute no line to the trusted computing base"
    );
    assert!(
        binding
            .resolve(
                &Symbol::new("t.postgres"),
                &Symbol::new("query"),
                Some(&Symbol::new("rows"))
            )
            .is_none()
    );
    assert!(!binding.serves(&atom("t.postgres", "rows", Mode::Read)));
}

/// The linearity counter is what stands between a multi-shot handler and a packet sent twice, and
/// `Linearity::Repeatable` is a handler author's unverifiable claim that replay costs nothing.
#[test]
fn documents_a_false_repeatable_claim_buys_a_replay_and_nothing_notices() {
    let compiled = compile(
        r#"
nondet effect net {
  write send[s](payload: Int) -> Int
}

effect retry {
  read ask() -> Int
}

test/nondet "resumed three times over a send" {
  handle {
    let n = retry.ask();
    net.send[socket](n)
  } with {
    retry.ask() resume k -> k(1) + k(2) + k(3)
  }
}
"#,
    );
    let handler = Arc::new(Mutates::default());
    let mut machine = compiled.bound(vec![(any("net", "send"), handler.clone())]);
    machine
        .eval_test(0)
        .expect("a repeatable operation replays");

    assert_eq!(
        handler.writes.load(Ordering::SeqCst),
        3,
        "one `perform`, three packets"
    );
    assert_eq!(
        machine.host_ops(),
        0,
        "and the linearity rule was never engaged, because the claim is trusted"
    );
    assert_eq!(
        machine.host_use().expect("reached the host").operations,
        3,
        "`host_use` counts them, which is the only place the replay is visible at all"
    );
}

/// The claim a caller states once per entry point is never cleared, and the test runner never
/// states it at all.
#[test]
fn documents_a_declared_footprint_outlives_the_entry_point_that_stated_it() {
    let compiled = compile(READS);
    let handler = Arc::new(Mutates::default());
    let mut machine = compiled.bound(vec![(any("db", "get"), handler.clone())]);

    machine.set_declared_footprint(Footprint::from_atoms([atom("t.db", "users", Mode::Read)]));
    machine.eval_test(0).expect("green under its own claim");
    // A second entry point, whose row nobody restated, is still judged by the first one's claim.
    machine.set_declared_footprint(Footprint::empty());
    assert_eq!(
        diagnostic(machine.eval_test(1)).code,
        codes::HOST_FOOTPRINT_ESCAPE
    );
    assert_eq!(handler.writes.load(Ordering::SeqCst), 1);
}

/// A handler may refuse, and it does not get to choose the class its failure is reported under.
#[test]
fn a_handler_may_not_choose_the_code_its_failure_is_classified_under() {
    struct Impersonates(&'static str);

    impl HostHandler for Impersonates {
        fn call(
            &self,
            _: &dyn HostRuntime,
            req: &HostRequest<'_>,
        ) -> Result<HostAnswer, Diagnostic> {
            Err(Diagnostic::error(self.0, "the engines disagree").primary(req.span, "here"))
        }
    }

    let compiled = compile(
        r#"
nondet effect net {
  write send[s](payload: Int) -> Int
}

test/nondet "a handler that lies about why" { assert_eq(net.send[socket](1), 1) }
"#,
    );

    for claimed in [
        codes::INTERNAL_ERROR,
        codes::ENGINE_DIVERGENCE,
        codes::SIMULATION_DIVERGENCE,
        codes::HOST_FOOTPRINT_ESCAPE,
        codes::HERMETIC_BOUNDARY,
        codes::MACHINE_ONLY_CLAUSE,
        codes::DEADLOCK,
    ] {
        assert!(
            ply_eval::host::is_reserved_code(claimed),
            "{claimed} is not in the reserved set, so this case tests nothing"
        );
        let mut machine =
            compiled.bound(vec![(any("net", "send"), Arc::new(Impersonates(claimed)))]);
        let d = diagnostic(machine.eval_test(0));
        assert_eq!(
            d.code,
            codes::RUNTIME_ERROR,
            "a handler minted `{claimed}` and it reached the runner"
        );
        assert_eq!(
            d.message, "the engines disagree",
            "the handler's own account of the failure was thrown away"
        );
        assert!(
            d.notes.iter().any(|n| n.contains(claimed)),
            "the code the handler claimed is not reported: {:?}",
            d.notes
        );
        assert!(
            d.notes.iter().any(|n| n.contains("audit::hostile")),
            "nothing names the handler the failure came from: {:?}",
            d.notes
        );
        assert!(
            d.labels.iter().any(|l| l.span != Span::DUMMY),
            "and it keeps the span of the `perform` it was raised at"
        );
    }
}

/// And an ordinary refusal is left alone except for the attribution, which every host failure gets:
/// a reader must never have to guess whether a diagnostic came from the evaluator or from a member
/// of the trusted computing base.
#[test]
fn an_unreserved_code_from_a_handler_is_kept_and_attributed() {
    struct Refuses;

    impl HostHandler for Refuses {
        fn call(
            &self,
            _: &dyn HostRuntime,
            req: &HostRequest<'_>,
        ) -> Result<HostAnswer, Diagnostic> {
            Err(
                Diagnostic::error(codes::RUNTIME_ERROR, "the socket refused the connection")
                    .primary(req.span, "here"),
            )
        }
    }

    let compiled = compile(
        r#"
nondet effect net {
  write send[s](payload: Int) -> Int
}

test/nondet "a handler that refuses honestly" { assert_eq(net.send[socket](1), 1) }
"#,
    );
    let mut machine = compiled.bound(vec![(any("net", "send"), Arc::new(Refuses))]);
    let d = diagnostic(machine.eval_test(0));
    assert_eq!(d.code, codes::RUNTIME_ERROR);
    assert_eq!(d.message, "the socket refused the connection");
    assert!(
        d.notes
            .iter()
            .any(|n| n.contains("audit::hostile") && n.contains("net.send[socket]")),
        "an honest refusal is not attributed either: {:?}",
        d.notes
    );
    assert!(
        !d.notes.iter().any(|n| n.contains("only the run itself")),
        "an unreserved code was reported as though it had been rewritten: {:?}",
        d.notes
    );
}

/// The one blocking failure the runtime *can* see, pinned because it is the only one.
#[test]
fn a_token_nothing_resolves_is_diagnosed_inside_a_production_region() {
    struct NeverReady;

    impl HostHandler for NeverReady {
        fn call(&self, _: &dyn HostRuntime, _: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
            Ok(HostAnswer::Value(Value::Int(0)))
        }
    }

    struct Waits;

    impl HostHandler for Waits {
        fn call(&self, _: &dyn HostRuntime, _: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
            Ok(HostAnswer::Pending(ply_eval::host::Pending {
                token: 1,
                label: "accept",
            }))
        }
    }

    /// A reactor that always wakes and never resolves: the shape a broken host runtime takes, and
    /// the shape a livelock takes.
    struct NeverResolves;

    impl HostRuntime for NeverResolves {
        fn poll(&self, _: &ply_eval::host::Pending) -> Result<Option<Value>, Diagnostic> {
            Ok(None)
        }

        fn park(&self) -> Result<(), Diagnostic> {
            Ok(())
        }

        fn block_on(&self, _: ply_eval::host::Pending) -> Result<Value, Diagnostic> {
            panic!("a task inside a production region must park, never block the thread")
        }
    }

    let compiled = compile(
        r#"
nondet effect net {
  write accept[s](listener: Int) -> Int
}

fn waits() -> Int / {net.write[socket]} = net.accept[socket](1)

test/nondet "waits on a token nothing resolves" {
  let slow = task.spawn(|| waits());
  assert_eq(task.join(slow), 0)
}
"#,
    );
    let mut entries: Vec<(HostOp, Arc<dyn HostHandler>)> = ["spawn", "join", "yield"]
        .into_iter()
        .map(|name| {
            (
                any("task", name),
                Arc::new(NeverReady) as Arc<dyn HostHandler>,
            )
        })
        .collect();
    entries.push((any("net", "accept"), Arc::new(Waits)));

    let mut machine = compiled.bound(entries);
    machine.set_host_runtime(std::rc::Rc::new(NeverResolves));
    let d = diagnostic(machine.eval_test(0));
    assert_eq!(
        d.code,
        codes::INTERNAL_ERROR,
        "a run that cannot progress must say so: {}",
        d.message
    );
}
