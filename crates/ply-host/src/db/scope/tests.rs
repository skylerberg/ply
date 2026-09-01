//! Two suites, and the second is the one that matters.

use super::*;
use crate::db::pool::Cleanup;

fn lease(n: u64) -> LeaseId {
    LeaseId::named(n)
}

fn span() -> Span {
    Span::DUMMY
}

/// One machine, since every test in this module is about one entry point's own tasks.
const MACHINE: MachineId = MachineId(1);

fn owner(n: u32) -> Owner {
    (MACHINE, Some(TaskId(n)))
}

/// The entry point that opened no region — one thread of control, and an identity rather than an
/// absence of one.
const ALONE: Owner = (MACHINE, None);

/// Where a statement performed by `who` would run, with the operation's name fixed so a caller
/// reads as the question rather than as the plumbing.
fn route(table: &ScopeTable, who: Owner) -> Result<Option<LeaseId>, String> {
    table
        .route(who, "`db.execute`", span())
        .map_err(|d| d.code.to_string())
}

fn open(table: &mut ScopeTable, who: Owner, level: Isolation, access: Access, on: LeaseId) -> Step {
    let step = table.begin(who, level, access);
    match &step {
        Step::Open { .. } | Step::Nested { .. } => table.opened(who, on, level, access),
        _ => {}
    }
    step
}

/// A level names one thing in three places — the Ply constructor a `Value` carries, the SQL a
/// `BEGIN` writes, and the word a refusal prints — and the round trip is what keeps the three from
/// drifting into two enumerations.
#[test]
fn every_level_and_access_round_trips_through_its_constructor() {
    for level in [
        Isolation::ReadCommitted,
        Isolation::RepeatableRead,
        Isolation::Serializable,
    ] {
        assert_eq!(Isolation::from_ctor(level.as_str()), Some(level));
        assert!(!level.sql().is_empty());
    }
    for access in [Access::ReadWrite, Access::ReadOnly] {
        assert_eq!(Access::from_ctor(access.as_str()), Some(access));
    }
    // `ReadUncommitted` is not offered: postgres implements it as read committed, and a name that
    // promised dirty reads would be a name that lies.
    assert_eq!(Isolation::from_ctor("ReadUncommitted"), None);
}

#[test]
fn the_outermost_begin_carries_its_level_and_its_access() {
    let mut table = ScopeTable::new();
    assert_eq!(
        table.begin(ALONE, Isolation::Serializable, Access::ReadOnly),
        Step::Open {
            sql: "BEGIN ISOLATION LEVEL SERIALIZABLE READ ONLY".to_string()
        }
    );
    // One statement rather than a `BEGIN` followed by two `SET TRANSACTION`s: a `BEGIN` that
    // succeeded and a `SET` that failed would leave a scope open at a level the call site did not
    // ask for, which is the one outcome neither the caller nor the driver could recover from.
    assert_eq!(
        table.begin(ALONE, Isolation::ReadCommitted, Access::ReadWrite),
        Step::Open {
            sql: "BEGIN ISOLATION LEVEL READ COMMITTED READ WRITE".to_string()
        }
    );
}

#[test]
fn nothing_is_open_until_the_server_accepted_the_begin() {
    let mut table = ScopeTable::new();
    table.begin(ALONE, Isolation::ReadCommitted, Access::ReadWrite);
    assert_eq!(table.depth(ALONE), 0, "a `BEGIN` that was only asked for");
    assert_eq!(route(&table, ALONE), Ok(None));

    table.opened(ALONE, lease(1), Isolation::ReadCommitted, Access::ReadWrite);
    assert_eq!(table.depth(ALONE), 1);
    assert_eq!(route(&table, ALONE), Ok(Some(lease(1))));
}

#[test]
fn a_nested_begin_is_a_savepoint_on_the_same_connection() {
    let mut table = ScopeTable::new();
    open(
        &mut table,
        ALONE,
        Isolation::ReadCommitted,
        Access::ReadWrite,
        lease(1),
    );
    assert_eq!(
        open(
            &mut table,
            ALONE,
            Isolation::ReadCommitted,
            Access::ReadWrite,
            lease(1)
        ),
        Step::Nested {
            lease: lease(1),
            sql: "SAVEPOINT ply_sp_1".to_string()
        }
    );
    assert_eq!(
        open(
            &mut table,
            ALONE,
            Isolation::ReadCommitted,
            Access::ReadWrite,
            lease(1)
        ),
        Step::Nested {
            lease: lease(1),
            sql: "SAVEPOINT ply_sp_2".to_string()
        }
    );
    assert_eq!(table.depth(ALONE), 3);
}

#[test]
fn a_nested_commit_releases_and_a_nested_abort_rolls_back_to_the_same_name() {
    let mut table = ScopeTable::new();
    for _ in 0..2 {
        open(
            &mut table,
            ALONE,
            Isolation::ReadCommitted,
            Access::ReadWrite,
            lease(1),
        );
    }
    assert_eq!(
        table.abort(ALONE, span()).expect("a scope is open"),
        Step::Nested {
            lease: lease(1),
            sql: "ROLLBACK TO SAVEPOINT ply_sp_1; RELEASE SAVEPOINT ply_sp_1".to_string()
        }
    );
    assert_eq!(
        table.closed(ALONE, false).lease,
        None,
        "the transaction is still open"
    );
    assert_eq!(table.depth(ALONE), 1);

    open(
        &mut table,
        ALONE,
        Isolation::ReadCommitted,
        Access::ReadWrite,
        lease(1),
    );
    assert_eq!(
        table.commit(ALONE, span()).expect("a scope is open"),
        Step::Nested {
            lease: lease(1),
            sql: "RELEASE SAVEPOINT ply_sp_1".to_string()
        }
    );
}

#[test]
fn closing_the_outermost_scope_hands_the_connection_back() {
    let mut table = ScopeTable::new();
    open(
        &mut table,
        ALONE,
        Isolation::ReadCommitted,
        Access::ReadWrite,
        lease(4),
    );
    assert_eq!(
        table.commit(ALONE, span()).expect("a scope is open"),
        Step::Close {
            lease: lease(4),
            sql: "COMMIT".to_string(),
            cleanup: Cleanup::Clean
        }
    );
    assert_eq!(table.closed(ALONE, true).lease, Some(lease(4)));
    assert!(table.is_empty());
}

#[test]
fn a_nested_begin_at_another_level_is_25001_naming_both() {
    let mut table = ScopeTable::new();
    open(
        &mut table,
        ALONE,
        Isolation::ReadCommitted,
        Access::ReadWrite,
        lease(1),
    );
    let Step::Refused(error) = table.begin(ALONE, Isolation::Serializable, Access::ReadWrite)
    else {
        panic!("a savepoint has no isolation level, so this cannot be honoured");
    };
    assert_eq!(error.code, sqlstate::ACTIVE_TRANSACTION);
    assert!(error.detail.contains("Serializable"), "{}", error.detail);
    assert!(error.detail.contains("ReadCommitted"), "{}", error.detail);
    assert_eq!(table.depth(ALONE), 1, "the refusal opened nothing");
}

/// A narrowing is documentation and not enforcement — postgres has no read-only savepoint and the
/// statements inside one are still writable — and saying so is the only honest thing available.
#[test]
fn a_nested_read_only_narrows_and_a_nested_read_write_widens() {
    let mut table = ScopeTable::new();
    open(
        &mut table,
        ALONE,
        Isolation::ReadCommitted,
        Access::ReadWrite,
        lease(1),
    );
    assert!(matches!(
        table.begin(ALONE, Isolation::ReadCommitted, Access::ReadOnly),
        Step::Nested { .. }
    ));

    let mut table = ScopeTable::new();
    open(
        &mut table,
        ALONE,
        Isolation::ReadCommitted,
        Access::ReadOnly,
        lease(1),
    );
    let Step::Refused(error) = table.begin(ALONE, Isolation::ReadCommitted, Access::ReadWrite)
    else {
        panic!("a savepoint cannot widen what the transaction may do");
    };
    assert_eq!(error.code, sqlstate::ACTIVE_TRANSACTION);
    assert!(error.detail.contains("ReadWrite"), "{}", error.detail);
    assert!(error.detail.contains("ReadOnly"), "{}", error.detail);
}

#[test]
fn nesting_past_the_bound_is_54000_and_not_a_diagnostic() {
    let mut table = ScopeTable::new();
    for _ in 0..=MAX_SAVEPOINTS {
        open(
            &mut table,
            ALONE,
            Isolation::ReadCommitted,
            Access::ReadWrite,
            lease(1),
        );
    }
    assert_eq!(table.depth(ALONE), MAX_SAVEPOINTS + 1);
    let Step::Refused(error) = table.begin(ALONE, Isolation::ReadCommitted, Access::ReadWrite)
    else {
        panic!("the bound is the bound");
    };
    assert_eq!(error.code, sqlstate::PROGRAM_LIMIT_EXCEEDED);
    assert_eq!(
        table.depth(ALONE),
        MAX_SAVEPOINTS + 1,
        "the refusal opened nothing"
    );
}

#[test]
fn a_close_with_nothing_open_is_25p01() {
    let mut table = ScopeTable::new();
    let Ok(Step::Refused(error)) = table.commit(ALONE, span()) else {
        panic!("there is nothing to commit");
    };
    assert_eq!(error.code, sqlstate::NO_ACTIVE_TRANSACTION);
    let Ok(Step::Refused(error)) = table.abort(ALONE, span()) else {
        panic!("there is nothing to abort");
    };
    assert_eq!(error.code, sqlstate::NO_ACTIVE_TRANSACTION);
}

/// Two tasks each in their own transaction are two stacks on two connections, which is what a pool
/// exists to serve.
#[test]
fn two_owners_hold_two_scopes_on_two_connections() {
    let mut table = ScopeTable::new();
    open(
        &mut table,
        owner(1),
        Isolation::ReadCommitted,
        Access::ReadWrite,
        lease(1),
    );
    open(
        &mut table,
        owner(2),
        Isolation::Serializable,
        Access::ReadWrite,
        lease(2),
    );
    assert_eq!(route(&table, owner(1)), Ok(Some(lease(1))));
    assert_eq!(route(&table, owner(2)), Ok(Some(lease(2))));
    assert_eq!(
        table.commit(owner(1), span()).expect("owner 1 has a scope"),
        Step::Close {
            lease: lease(1),
            sql: "COMMIT".to_string(),
            cleanup: Cleanup::Clean
        }
    );
    assert_eq!(table.depth(owner(2)), 1, "and owner 2 is untouched");
}

/// The refusal the fixture for `E0436` is written against: a statement from a task spawned inside
/// somebody else's `transaction` body.
#[test]
fn a_statement_from_a_task_that_owns_no_scope_is_e0436() {
    let mut table = ScopeTable::new();
    assert_eq!(
        route(&table, owner(2)),
        Ok(None),
        "with nothing open anywhere, a statement is simply not in a transaction"
    );

    open(
        &mut table,
        owner(1),
        Isolation::ReadCommitted,
        Access::ReadWrite,
        lease(1),
    );
    assert_eq!(
        route(&table, owner(2)),
        Err(codes::DB_TRANSACTION_SCOPE.to_string())
    );
    assert_eq!(
        route(&table, owner(1)),
        Ok(Some(lease(1))),
        "and the owner's own statements still run on its scope"
    );
}

#[test]
fn closing_a_scope_a_performer_does_not_own_is_e0436() {
    let mut table = ScopeTable::new();
    open(
        &mut table,
        owner(1),
        Isolation::ReadCommitted,
        Access::ReadWrite,
        lease(1),
    );
    let diagnostic = table
        .commit(owner(2), span())
        .expect_err("task 2 owns no scope while task 1's is open");
    assert_eq!(diagnostic.code, codes::DB_TRANSACTION_SCOPE);
    assert!(diagnostic.message.contains("@2"), "{}", diagnostic.message);
    assert!(
        diagnostic.notes.iter().any(|n| n.contains("@1")),
        "the open scope's owner is what a reader needs"
    );
    assert_eq!(table.depth(owner(1)), 1, "and nothing was closed");
}

#[test]
fn end_entry_point_names_every_connection_still_holding_a_scope() {
    let mut table = ScopeTable::new();
    open(
        &mut table,
        owner(1),
        Isolation::ReadCommitted,
        Access::ReadWrite,
        lease(7),
    );
    open(
        &mut table,
        owner(2),
        Isolation::ReadCommitted,
        Access::ReadWrite,
        lease(8),
    );
    // A savepoint inside one of them: `ROLLBACK` discards every savepoint under it, so the
    // connection is named once rather than once per depth.
    open(
        &mut table,
        owner(1),
        Isolation::ReadCommitted,
        Access::ReadWrite,
        lease(7),
    );

    assert_eq!(table.open_leases(), vec![lease(7), lease(8)]);
    assert_eq!(table.end_entry_point(MACHINE), vec![lease(7), lease(8)]);
    assert!(table.is_empty(), "an entry point ends holding nothing");
    assert_eq!(
        table.end_entry_point(MACHINE),
        Vec::new(),
        "and stays that way"
    );
}

// --- Against real postgres ---------------------------------------------------

mod live;
