use super::*;
use crate::db::stmt::Answer;
use crate::db::types::Datum;
use ply_core::ty::{EffectAtom, Footprint};
use ply_eval::Value;
use ply_eval::host::{Determinism, HostOp, Linearity};
use ply_span::Symbol;

/// A `std.db` constructor, qualified as a `Value` carries one.
fn ctor(name: &str, args: Vec<Value>) -> Value {
    Value::ctor(format!("{}.{name}", crate::db::MODULE), args)
}
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};

/// A poisoned lock here holds one `Option`, which has no invariant a panicking test thread can
/// break.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// An implementation that answers nothing and counts what reached it.
#[derive(Default)]
struct Counting {
    statements: AtomicUsize,
    begins: AtomicUsize,
    last: Mutex<Option<(String, Vec<String>)>>,
}

impl Driver for Counting {
    fn path(&self, _: Op) -> &'static str {
        "ply_host::db::tests::counting"
    }

    fn statement(&self, request: Statement<'_>) -> Result<HostAnswer, Diagnostic> {
        self.statements.fetch_add(1, Ordering::Relaxed);
        let mut atoms: Vec<String> = request.touched.atoms().map(|a| a.to_string()).collect();
        atoms.sort();
        *lock(&self.last) = Some((request.sql.to_string(), atoms));
        Ok(HostAnswer::Value(value::answer(&Answer::Rows(vec![vec![
            ("one".to_string(), Datum::Int(1)),
        ]]))))
    }

    fn begin(
        &self,
        level: crate::db::scope::Isolation,
        _: crate::db::scope::Access,
        _: crate::db::scope::Owner,
        _: Span,
    ) -> Result<HostAnswer, Diagnostic> {
        self.begins.fetch_add(1, Ordering::Relaxed);
        Ok(HostAnswer::Value(Value::str(level.as_str())))
    }

    fn commit(&self, _: crate::db::scope::Owner, _: Span) -> Result<HostAnswer, Diagnostic> {
        Ok(HostAnswer::Value(Value::Unit))
    }

    fn abort(&self, _: crate::db::scope::Owner, _: Span) -> Result<HostAnswer, Diagnostic> {
        Ok(HostAnswer::Value(Value::Unit))
    }
}

fn stmt(sql: &str) -> Value {
    let mut fields = BTreeMap::new();
    fields.insert(Symbol::new("sql"), Value::str(sql));
    Value::Record(Arc::new(fields.into_iter().collect()))
}

fn declaration(op: Op) -> HostOp {
    op.declaration()
}

fn perform(
    driver: &Arc<Counting>,
    op: Op,
    at: &str,
    args: Vec<Value>,
) -> Result<HostAnswer, Diagnostic> {
    perform_declared(driver, op, at, args, None)
}

/// The same, with the entry point's row, which is what `check_footprint` refuses against before a
/// connection is acquired.
fn perform_declared(
    driver: &Arc<Counting>,
    op: Op,
    at: &str,
    args: Vec<Value>,
    declared: Option<&Footprint>,
) -> Result<HostAnswer, Diagnostic> {
    struct NoRuntime;
    impl HostRuntime for NoRuntime {
        fn poll(&self, _: &ply_eval::host::Pending) -> Result<Option<Value>, Diagnostic> {
            unreachable!("nothing here answers `Pending`")
        }
        fn park(&self) -> Result<(), Diagnostic> {
            unreachable!("nothing here answers `Pending`")
        }
        fn block_on(&self, _: ply_eval::host::Pending) -> Result<Value, Diagnostic> {
            unreachable!("nothing here answers `Pending`")
        }
    }

    let handler = Operation {
        op,
        driver: Arc::clone(driver) as Arc<dyn Driver>,
        cache: Arc::new(crate::db::stmt::Cache::default()),
    };
    let declaration = declaration(op);
    let atom = EffectAtom::new(
        Symbol::new(crate::db::EFFECT),
        Resource::Named(Symbol::new(at)),
        ply_syntax::ast::Mode::Read,
    );
    handler.call(
        &NoRuntime,
        &HostRequest {
            machine: ply_eval::host::MachineId(1),
            atom,
            op: &declaration,
            args: &args,
            span: Span::DUMMY,
            task: None,
            declared,
        },
    )
}

#[test]
fn a_statement_reaches_the_implementation_with_every_table_it_touches() {
    let driver = Arc::new(Counting::default());
    perform(
        &driver,
        Op::Query,
        "orders",
        vec![
            stmt("select o.id from orders o join items i on i.sku = o.sku"),
            Value::list(Vec::new()),
        ],
    )
    .unwrap_or_else(|d| panic!("it runs: {}", d.message));
    assert_eq!(driver.statements.load(Ordering::Relaxed), 1);
    let (_, atoms) = lock(&driver.last).clone().expect("a statement");
    assert_eq!(atoms.len(), 2, "{atoms:?}");
    assert!(atoms.iter().any(|a| a.contains("items")), "{atoms:?}");
    assert!(atoms.iter().any(|a| a.contains("orders")), "{atoms:?}");
}

/// ADR 0014 §2.3's preventer, end to end through the handler: a join reaches a table the entry
/// point's row never declared, and the refusal happens at prepare — before a connection is acquired
/// and before a row is read.
#[test]
fn a_join_outside_the_declared_row_is_refused_before_the_statement_runs() {
    let join = || {
        vec![
            stmt("select o.id from orders o join items i on i.sku = o.sku"),
            Value::list(Vec::new()),
        ]
    };
    let atom = |table: &str| {
        EffectAtom::new(
            Symbol::new(crate::db::EFFECT),
            Resource::Named(Symbol::new(table)),
            ply_syntax::ast::Mode::Read,
        )
    };

    let narrow = Footprint::from_atoms([atom("orders")]);
    let driver = Arc::new(Counting::default());
    let d = perform_declared(&driver, Op::Query, "orders", join(), Some(&narrow))
        .err()
        .expect("`items` is not in the row");
    assert_eq!(d.code, codes::DB_FOOTPRINT_UNDECLARED);
    assert!(d.message.contains("items"), "{}", d.message);
    assert_eq!(
        driver.statements.load(Ordering::Relaxed),
        0,
        "the statement ran anyway"
    );

    let wide = Footprint::from_atoms([atom("orders"), atom("items")]);
    let driver = Arc::new(Counting::default());
    perform_declared(&driver, Op::Query, "orders", join(), Some(&wide))
        .unwrap_or_else(|d| panic!("the declared row covers it: {}", d.message));
    assert_eq!(driver.statements.load(Ordering::Relaxed), 1);
    let (_, atoms) = lock(&driver.last).clone().expect("a statement");
    assert_eq!(atoms.len(), 2, "{atoms:?}");
}

/// The ordering that makes the refusal a preventer rather than a report: the implementation is
/// never called, so nothing was acquired and no row moved.
#[test]
fn a_refused_statement_never_reaches_the_implementation() {
    for (sql, code) in [
        (
            "select 1 from items; drop table items",
            codes::DB_STATEMENT_REFUSED,
        ),
        ("drop table items", codes::DB_STATEMENT_REFUSED),
        (
            "select * from items where at < now()",
            codes::DB_STATEMENT_REFUSED,
        ),
        (
            "select * from generate_series(1, 3)",
            codes::DB_STATEMENT_REFUSED,
        ),
        // The label names a table the statement never touches.
        ("select * from orders", codes::DB_FOOTPRINT_UNDECLARED),
    ] {
        let driver = Arc::new(Counting::default());
        let d = perform(
            &driver,
            Op::Query,
            "items",
            vec![stmt(sql), Value::list(Vec::new())],
        )
        .err()
        .unwrap_or_else(|| panic!("`{sql}` was not refused"));
        assert_eq!(d.code, code, "{sql}: {}", d.message);
        assert_eq!(
            driver.statements.load(Ordering::Relaxed),
            0,
            "`{sql}` reached the implementation before it was refused"
        );
    }
}

/// `db.query` is the only `read`, so a statement that changes rows performed through it is refused
/// rather than recorded as a read.
#[test]
fn a_write_through_query_is_refused_before_anything_runs() {
    let driver = Arc::new(Counting::default());
    let d = perform(
        &driver,
        Op::Query,
        "items",
        vec![
            stmt("delete from items where sku = $1"),
            Value::list(vec![ctor("PText", vec![Value::str("bolt")])]),
        ],
    )
    .err()
    .expect("a write through a read");
    assert_eq!(d.code, codes::DB_STATEMENT_REFUSED);
    assert_eq!(driver.statements.load(Ordering::Relaxed), 0);

    // The same statement through `db.execute` runs.
    let driver = Arc::new(Counting::default());
    perform(
        &driver,
        Op::Execute,
        "items",
        vec![
            stmt("delete from items where sku = $1"),
            Value::list(vec![ctor("PText", vec![Value::str("bolt")])]),
        ],
    )
    .expect("a write through a write");
    assert_eq!(driver.statements.load(Ordering::Relaxed), 1);
}

#[test]
fn transaction_control_takes_no_statement_and_no_table() {
    let driver = Arc::new(Counting::default());
    perform(
        &driver,
        Op::Begin,
        "items",
        vec![
            ctor("Serializable", Vec::new()),
            ctor("ReadOnly", Vec::new()),
        ],
    )
    .expect("it begins");
    assert_eq!(driver.begins.load(Ordering::Relaxed), 1);
    assert_eq!(driver.statements.load(Ordering::Relaxed), 0);
    perform(&driver, Op::Commit, "items", Vec::new()).expect("it commits");
    perform(&driver, Op::Abort, "items", Vec::new()).expect("it aborts");
}

/// Inference checks a perform's arity, so this is Ply's fault rather than the program's and it says
/// which.
#[test]
fn a_perform_of_the_wrong_arity_is_plys_fault() {
    let driver = Arc::new(Counting::default());
    let d = perform(
        &driver,
        Op::Query,
        "items",
        vec![stmt("select 1 from items")],
    )
    .err()
    .expect("one argument, not two");
    assert_eq!(d.code, codes::INTERNAL_ERROR);
    assert_eq!(driver.statements.load(Ordering::Relaxed), 0);
}

/// The listing is the artifact ADR 0008 §2 exists to produce, and the implementation gets a say in
/// exactly one column of it.
#[test]
fn every_operation_a_driver_serves_appears_with_the_implementations_own_path() {
    let registry = registry(Arc::new(Counting::default()) as Arc<dyn Driver>);
    assert_eq!(registry.len(), Op::ALL.len());
    for op in registry.ops() {
        assert_eq!(op.path, "ply_host::db::tests::counting");
        assert_eq!(op.determinism, Determinism::Nondeterministic);
        assert_eq!(op.linearity, Linearity::AtMostOnce);
        assert!(op.blocking);
    }
    let names: Vec<String> = registry.ops().map(|op| op.op.to_string()).collect();
    assert!(!names.contains(&"rollback".to_string()), "{names:?}");
}
