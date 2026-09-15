//! The scope table, driven against a real postgres server.

use super::*;
use std::sync::atomic::{AtomicU32, Ordering};

/// A connection, driven synchronously on a current-thread runtime.
struct Pg {
    runtime: tokio::runtime::Runtime,
    client: tokio_postgres::Client,
}

impl Pg {
    fn open(url: &str) -> Pg {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a current-thread runtime");
        let (client, connection) = runtime
            .block_on(tokio_postgres::connect(url, tokio_postgres::NoTls))
            .unwrap_or_else(|e| panic!("PLY_PG_URL is set and `{url}` did not connect: {e}"));
        runtime.spawn(async move {
            let _ = connection.await;
        });
        Pg { runtime, client }
    }

    /// Runs one command.
    fn run(&self, sql: &str) -> Result<(), String> {
        self.runtime
            .block_on(self.client.simple_query(sql))
            .map(|_| ())
            .map_err(|e| {
                e.code()
                    .map(|c| c.code().to_string())
                    .unwrap_or_else(|| format!("no sqlstate: {e}"))
            })
    }

    /// The first column of the first row, as text.
    fn one(&self, sql: &str) -> String {
        let rows = self
            .runtime
            .block_on(self.client.simple_query(sql))
            .unwrap_or_else(|e| panic!("`{sql}` failed: {e}"));
        for message in rows {
            if let tokio_postgres::SimpleQueryMessage::Row(row) = message {
                return row.get(0).unwrap_or("").to_string();
            }
        }
        panic!("`{sql}` returned no row");
    }

    fn backend_pid(&self) -> String {
        self.one("SELECT pg_backend_pid()")
    }
}

/// A scope table, a connection, and a scratch table of its own.
struct Live {
    pg: Pg,
    observer: Pg,
    table: ScopeTable,
    lease: LeaseId,
    rows: String,
    pid: String,
}

/// Distinct scratch table names within one process.
static NEXT: AtomicU32 = AtomicU32::new(0);

impl Live {
    /// `None` when no server is configured, which every test below reports rather than passing
    /// quietly.
    fn open() -> Option<Live> {
        let url = std::env::var("PLY_PG_URL").ok()?;
        let pg = Pg::open(&url);
        let observer = Pg::open(&url);
        let rows = format!(
            "ply_scope_{}_{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        pg.run(&format!("DROP TABLE IF EXISTS {rows}"))
            .expect("a scratch table");
        pg.run(&format!("CREATE TABLE {rows} (id int primary key)"))
            .expect("a scratch table");
        let pid = pg.backend_pid();
        Some(Live {
            pg,
            observer,
            table: ScopeTable::new(),
            lease: LeaseId::named(1),
            rows,
            pid,
        })
    }

    /// Executes whatever `db.begin` asked for.
    fn begin(&mut self, who: Owner, level: Isolation, access: Access) -> Result<(), String> {
        match self.table.begin(who, level, access) {
            Step::Open { sql } | Step::Nested { sql, .. } => {
                self.pg.run(&sql)?;
                self.table.opened(who, self.lease, level, access);
                Ok(())
            }
            Step::Close { .. } => unreachable!("a `begin` never closes a scope"),
            Step::Refused(error) => Err(error.code),
        }
    }

    fn commit(&mut self, who: Owner) -> Result<(), String> {
        let step = self.table.commit(who, Span::DUMMY).expect("owned");
        self.close(step, who)
    }

    fn abort(&mut self, who: Owner) -> Result<(), String> {
        let step = self.table.abort(who, Span::DUMMY).expect("owned");
        self.close(step, who)
    }

    fn close(&mut self, step: Step, who: Owner) -> Result<(), String> {
        match step {
            Step::Close { sql, .. } | Step::Nested { sql, .. } => {
                let outcome = self.pg.run(&sql);
                // Popped whether the server accepted it or not: a failed `COMMIT` has already ended
                // the transaction, and a scope kept after its close would make every later
                // savepoint name wrong.
                self.table.closed(who, true);
                outcome
            }
            Step::Open { .. } => unreachable!("a close never opens a scope"),
            Step::Refused(error) => Err(error.code),
        }
    }

    /// What the machine calls on every exit path from an entry point, and what the driver does with
    /// what it names.
    fn end_entry_point(&mut self) -> Vec<Result<(), String>> {
        self.table
            .end_entry_point(super::MACHINE)
            .into_iter()
            .map(|_| self.pg.run("ROLLBACK"))
            .collect()
    }

    fn insert(&self, id: i32) -> Result<(), String> {
        self.pg
            .run(&format!("INSERT INTO {} (id) VALUES ({id})", self.rows))
    }

    /// Through the connection that wrote them, so an uncommitted row counts.
    fn visible_here(&self) -> i64 {
        self.pg
            .one(&format!("SELECT count(*) FROM {}", self.rows))
            .parse()
            .expect("a count")
    }

    /// Through a **different** connection, which is the only vantage point from which "committed"
    /// means anything.
    fn visible_elsewhere(&self) -> i64 {
        self.observer
            .one(&format!("SELECT count(*) FROM {}", self.rows))
            .parse()
            .expect("a count")
    }

    /// What the server thinks this session is doing.
    fn session_state(&self) -> String {
        self.observer.one(&format!(
            "SELECT state FROM pg_stat_activity WHERE pid = {}",
            self.pid
        ))
    }

    /// Everything a reusable connection has to be able to do: the server says it is idle, and a
    /// statement on it works.
    fn assert_reusable(&self) {
        assert_eq!(
            self.session_state(),
            "idle",
            "the connection is still inside a transaction and would go back to the pool that way"
        );
        self.pg
            .run("SELECT 1")
            .expect("a clean connection answers a statement");
    }
}

/// Every test below shares this preamble, and printing the reason is the point: a live test that
/// vanishes silently is worth less than no live test at all.
macro_rules! live {
    ($name:ident) => {
        let Some(mut $name) = Live::open() else {
            eprintln!(
                "skipped: PLY_PG_URL is unset, so the scope table was not run against real postgres"
            );
            return;
        };
    };
}

#[test]
fn a_committed_transaction_persists_and_the_connection_is_reusable() {
    live!(live);
    live.begin(ALONE, Isolation::ReadCommitted, Access::ReadWrite)
        .expect("a transaction");
    live.insert(1).expect("an insert");
    assert_eq!(live.visible_elsewhere(), 0, "not yet, from outside");
    live.commit(ALONE).expect("a commit");

    assert_eq!(live.visible_elsewhere(), 1);
    assert!(live.table.is_empty());
    live.assert_reusable();
}

/// The property the milestone is about.
#[test]
fn an_aborted_transaction_leaves_nothing_and_the_connection_is_reusable() {
    live!(live);
    live.begin(ALONE, Isolation::ReadCommitted, Access::ReadWrite)
        .expect("a transaction");
    live.insert(1).expect("an insert");
    assert_eq!(live.visible_here(), 1, "the writer sees its own write");
    live.abort(ALONE).expect("a rollback");

    assert_eq!(live.visible_here(), 0, "and now nobody does");
    assert_eq!(live.visible_elsewhere(), 0);
    assert!(live.table.is_empty());
    live.assert_reusable();
}

/// A body that raises propagates past the `handle` that would have committed or aborted it, so the
/// `BEGIN` is still open when the entry point ends.
#[test]
fn a_body_that_raises_leaves_a_scope_that_end_entry_point_rolls_back() {
    live!(live);
    live.begin(ALONE, Isolation::ReadCommitted, Access::ReadWrite)
        .expect("a transaction");
    live.insert(1).expect("an insert");
    // The raise: nothing closes the scope, and control leaves.
    assert_eq!(
        live.session_state(),
        "idle in transaction",
        "which is the state a pooled connection must never be handed back in"
    );

    let closed = live.end_entry_point();
    assert_eq!(closed.len(), 1, "one connection was still holding a scope");
    assert!(closed[0].is_ok());

    assert_eq!(live.visible_elsewhere(), 0, "nothing was committed");
    assert!(live.table.is_empty());
    live.assert_reusable();
}

#[test]
fn an_entry_point_that_ended_cleanly_leaves_end_entry_point_nothing_to_do() {
    live!(live);
    live.begin(ALONE, Isolation::ReadCommitted, Access::ReadWrite)
        .expect("a transaction");
    live.insert(1).expect("an insert");
    live.commit(ALONE).expect("a commit");
    assert!(live.end_entry_point().is_empty());
    live.assert_reusable();
}

#[test]
fn a_nested_rollback_discards_the_inner_writes_and_keeps_the_outer() {
    live!(live);
    live.begin(ALONE, Isolation::ReadCommitted, Access::ReadWrite)
        .expect("a transaction");
    live.insert(1).expect("the outer write");

    live.begin(ALONE, Isolation::ReadCommitted, Access::ReadWrite)
        .expect("a savepoint");
    live.insert(2).expect("the inner write");
    assert_eq!(live.visible_here(), 2);
    live.abort(ALONE).expect("a rollback to the savepoint");

    assert_eq!(live.visible_here(), 1, "the inner write is gone");
    assert_eq!(
        live.table.depth(ALONE),
        1,
        "and the transaction is still open"
    );
    live.commit(ALONE).expect("a commit");

    assert_eq!(live.visible_elsewhere(), 1);
    live.assert_reusable();
}

#[test]
fn a_released_savepoint_survives_until_the_outer_scope_decides() {
    live!(live);
    live.begin(ALONE, Isolation::ReadCommitted, Access::ReadWrite)
        .expect("a transaction");
    live.begin(ALONE, Isolation::ReadCommitted, Access::ReadWrite)
        .expect("a savepoint");
    live.insert(1).expect("the inner write");
    live.commit(ALONE).expect("a release");
    assert_eq!(live.visible_here(), 1);

    // A released savepoint is not a commit, so the outer rollback takes it too.
    live.abort(ALONE).expect("the outer rollback");
    assert_eq!(live.visible_here(), 0);
    assert_eq!(live.visible_elsewhere(), 0);
    live.assert_reusable();
}

/// Every savepoint the bound allows, on a real server, named by the same arithmetic that names them
/// in memory.
#[test]
fn the_savepoint_bound_is_reachable_and_unwinds_in_order() {
    live!(live);
    live.begin(ALONE, Isolation::ReadCommitted, Access::ReadWrite)
        .expect("a transaction");
    live.insert(0).expect("the outermost write");
    for depth in 1..=MAX_SAVEPOINTS {
        live.begin(ALONE, Isolation::ReadCommitted, Access::ReadWrite)
            .expect("a savepoint");
        live.insert(depth as i32).expect("a write inside it");
    }
    assert_eq!(live.visible_here(), MAX_SAVEPOINTS as i64 + 1);
    assert_eq!(
        live.begin(ALONE, Isolation::ReadCommitted, Access::ReadWrite),
        Err(sqlstate::PROGRAM_LIMIT_EXCEEDED.to_string()),
        "the bound is a `Failed` and not a diagnostic"
    );

    // Unwind one savepoint at a time, and every write disappears with the scope that made it.
    for depth in (1..=MAX_SAVEPOINTS).rev() {
        live.abort(ALONE).expect("a rollback to the savepoint");
        assert_eq!(live.visible_here(), depth as i64);
    }
    live.commit(ALONE).expect("a commit");
    assert_eq!(live.visible_elsewhere(), 1);
    live.assert_reusable();
}

/// The mechanical backstop on a row that claims to be read-only, supplied by the one component in
/// the stack that cannot be fooled by an annotation.
#[test]
fn a_write_inside_a_read_only_transaction_is_25006_from_the_server() {
    live!(live);
    live.begin(ALONE, Isolation::ReadCommitted, Access::ReadOnly)
        .expect("a read-only transaction");
    assert_eq!(
        live.insert(1),
        Err("25006".to_string()),
        "the server refuses it, and no check in the driver had to"
    );
    live.abort(ALONE).expect("a rollback");
    live.assert_reusable();
}

/// After a statement fails inside a scope, every later statement in that scope is `25P02` until the
/// scope ends or a savepoint below the failure is rolled back to.
#[test]
fn a_failed_statement_poisons_the_scope_until_a_savepoint_below_it_is_rolled_back_to() {
    live!(live);
    live.begin(ALONE, Isolation::ReadCommitted, Access::ReadWrite)
        .expect("a transaction");
    live.insert(1).expect("a write");
    live.begin(ALONE, Isolation::ReadCommitted, Access::ReadWrite)
        .expect("a savepoint");
    assert_eq!(
        live.insert(1),
        Err("23505".to_string()),
        "the same key twice"
    );
    assert_eq!(
        live.insert(2),
        Err("25P02".to_string()),
        "and the scope is poisoned"
    );

    live.abort(ALONE)
        .expect("a rollback to the savepoint clears it");
    live.insert(2).expect("the scope is usable again");
    live.commit(ALONE).expect("a commit");
    assert_eq!(live.visible_elsewhere(), 2);
    live.assert_reusable();
}

/// `40001` is a value the program matches on and not a diagnostic, and W4 never retries on its
/// behalf: only the program knows whether the body sent an email between two statements.
#[test]
fn a_serialization_failure_is_a_value_and_a_fresh_transaction_succeeds() {
    live!(live);
    live.insert(1).expect("a row both transactions will touch");

    let contender = Pg::open(&std::env::var("PLY_PG_URL").expect("it was set a moment ago"));
    live.begin(ALONE, Isolation::Serializable, Access::ReadWrite)
        .expect("a serializable transaction");
    contender
        .run("BEGIN ISOLATION LEVEL SERIALIZABLE READ WRITE")
        .expect("the other one");

    // Each reads what the other is about to write, which is the shape postgres detects rather than
    // a lock it can wait on.
    live.pg
        .run(&format!("SELECT count(*) FROM {}", live.rows))
        .expect("a read");
    contender
        .run(&format!("SELECT count(*) FROM {}", live.rows))
        .expect("a read");
    live.insert(2).expect("one write");
    contender
        .run(&format!("INSERT INTO {} (id) VALUES (3)", live.rows))
        .expect("the other");

    live.commit(ALONE).expect("the first commit wins");
    let refused = contender
        .run("COMMIT")
        .expect_err("the second cannot be serialized after the first");
    assert_eq!(refused, "40001");
    assert!(
        retryable(&refused),
        "which is what `is_retryable` answers true for"
    );

    // The retry is a fresh transaction rather than a resumption, so it is outside the linearity
    // rule entirely — and it succeeds.
    contender
        .run("BEGIN ISOLATION LEVEL SERIALIZABLE READ WRITE")
        .expect("a fresh transaction");
    contender
        .run(&format!("INSERT INTO {} (id) VALUES (3)", live.rows))
        .expect("the retry");
    contender.run("COMMIT").expect("and it commits");
    assert_eq!(live.visible_elsewhere(), 3);
    live.assert_reusable();
}

/// What `std.db`'s `is_retryable` says, in Rust, so the assertion above is about the same two codes
/// rather than about a string.
fn retryable(code: &str) -> bool {
    code == "40001" || code == "40P01"
}
