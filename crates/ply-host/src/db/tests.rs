use super::*;
use ply_core::ty::{EffectAtom, Resource};
use ply_eval::{Determinism, Linearity};
use ply_syntax::ast::Mode;

fn label(name: &str) -> Resource {
    Resource::Named(Symbol::new(name))
}

fn atom(table: &str, mode: Mode) -> EffectAtom {
    EffectAtom::new(Symbol::new(EFFECT), label(table), mode)
}

fn row(atoms: impl IntoIterator<Item = EffectAtom>) -> Footprint {
    Footprint::from_atoms(atoms)
}

fn scanned(sql: &str) -> Scan {
    scan::scan(sql, Span::DUMMY).unwrap_or_else(|d| panic!("`{sql}`: {}", d.message))
}

/// Every column `ply hosts` prints is decided by the declaration, and each is a
/// claim someone has to have made on purpose.
#[test]
fn the_registration_declares_what_a_database_actually_is() {
    for op in Op::ALL {
        let declaration = op.declaration();
        assert_eq!(declaration.effect.as_str(), EFFECT);
        assert_eq!(
            declaration.determinism,
            Determinism::Nondeterministic,
            "`{}` claims to be a function of the program's state",
            op.name()
        );
        assert_eq!(
            declaration.linearity,
            Linearity::AtMostOnce,
            "`{}` claims a resumption replays it; a resumed query is a second query",
            op.name()
        );
        assert!(
            declaration.blocking,
            "`{}` claims to answer without leaving the machine's thread",
            op.name()
        );
        assert!(
            declaration.path.starts_with("ply_host::db::"),
            "`{}` names no Rust path a reviewer can find",
            op.name()
        );
    }
}

/// The data operations carry a table and the transaction control operations do
/// not. That asymmetry is the milestone: a row that said `db.write[db]` for
/// every statement would have thrown away the only reason to do this.
#[test]
fn a_row_says_which_table_for_the_operations_that_have_one() {
    for op in [Op::Query, Op::Execute, Op::Returning] {
        assert_eq!(
            op.declaration().resource,
            HostResource::Any,
            "{}",
            op.name()
        );
    }
    for op in [Op::Begin, Op::Commit, Op::Abort] {
        assert_eq!(
            op.declaration().resource,
            HostResource::Only(Resource::Singleton),
            "{}",
            op.name()
        );
    }
}

/// `rollback` is handled in Ply by `transaction` and never reaches the boundary.
/// If it is ever registered, something has bound it and that is a defect.
#[test]
fn rollback_is_not_a_host_operation() {
    assert!(Op::ALL.iter().all(|op| op.name() != "rollback"));
}

#[test]
fn a_statement_publishes_the_tables_it_touches_and_not_the_label_alone() {
    let scan = scanned("select o.id, i.sku from orders o join items i on i.sku = o.sku");
    let footprint = check_footprint(&scan, Op::Query, &label("orders"), None, Span::DUMMY)
        .expect("the label is one of the tables");
    let mut atoms: Vec<String> = footprint.atoms().map(|a| a.to_string()).collect();
    atoms.sort();
    assert_eq!(atoms.len(), 2, "{atoms:?}");
    assert!(atoms.iter().any(|a| a.contains("items")), "{atoms:?}");
    assert!(atoms.iter().any(|a| a.contains("orders")), "{atoms:?}");
}

/// The preventer. A join whose second table is missing from the entry point's
/// row is refused **before** a row moves, which is the half the machine's
/// answer-time check cannot be.
#[test]
fn a_table_outside_the_declared_row_is_refused_before_the_statement_runs() {
    let scan = scanned("select * from orders join items on items.sku = orders.sku");
    let declared = row([atom("orders", Mode::Read)]);
    let d = check_footprint(
        &scan,
        Op::Query,
        &label("orders"),
        Some(&declared),
        Span::DUMMY,
    )
    .expect_err("`items` is not declared");
    assert_eq!(d.code, codes::DB_FOOTPRINT_UNDECLARED);
    assert!(d.message.contains("items"), "{}", d.message);

    // Declared, it runs and both atoms are recorded.
    let declared = row([atom("orders", Mode::Read), atom("items", Mode::Read)]);
    let footprint = check_footprint(
        &scan,
        Op::Query,
        &label("orders"),
        Some(&declared),
        Span::DUMMY,
    )
    .expect("both tables are declared");
    assert_eq!(footprint.atoms().count(), 2);
}

/// A declared *read* does not cover a write of the same table: the conflict
/// graph runs two readers side by side, and a write among them is the race the
/// footprint exists to prevent.
#[test]
fn a_write_is_not_covered_by_a_declared_read_of_the_same_table() {
    let scan = scanned("update items set on_hand = 0");
    let declared = row([atom("items", Mode::Read)]);
    let d = check_footprint(
        &scan,
        Op::Execute,
        &label("items"),
        Some(&declared),
        Span::DUMMY,
    )
    .expect_err("a read does not cover a write");
    assert_eq!(d.code, codes::DB_FOOTPRINT_UNDECLARED);

    let declared = row([atom("items", Mode::Write)]);
    assert!(
        check_footprint(
            &scan,
            Op::Execute,
            &label("items"),
            Some(&declared),
            Span::DUMMY
        )
        .is_ok()
    );
}

/// `db.query` is the only `read`, and it is what two read-only endpoints over
/// one table are scheduled side by side on. A write performed through it would
/// record a read atom for something that writes.
#[test]
fn a_write_statement_performed_as_a_query_is_refused() {
    for sql in [
        "insert into items values ($1)",
        "update items set a = 1",
        "delete from items",
        "with gone as (delete from items returning id) select * from gone",
    ] {
        let scan = scanned(sql);
        let d = check_footprint(&scan, Op::Query, &label("items"), None, Span::DUMMY)
            .expect_err("a write through `db.query`");
        assert_eq!(d.code, codes::DB_STATEMENT_REFUSED, "{sql}");
    }
    // The same statements through `db.execute` are fine.
    for sql in ["insert into items values ($1)", "delete from items"] {
        assert!(
            check_footprint(
                &scanned(sql),
                Op::Execute,
                &label("items"),
                None,
                Span::DUMMY
            )
            .is_ok(),
            "{sql}"
        );
    }
}

/// A label naming a table the statement never touches is a footprint claim about
/// nothing — a rename that moved the label and left the statement behind.
#[test]
fn a_label_that_is_not_one_of_the_statements_tables_is_refused() {
    let scan = scanned("select * from items");
    let d = check_footprint(&scan, Op::Query, &label("orders"), None, Span::DUMMY)
        .expect_err("`orders` is not touched");
    assert_eq!(d.code, codes::DB_FOOTPRINT_UNDECLARED);
    assert!(d.message.contains("orders"), "{}", d.message);
}

#[test]
fn the_declaration_this_binds_against_is_the_one_that_ships() {
    assert_eq!(DECLARATION, ply_std::DB);
    assert!(DECLARATION.contains("pub nondet effect db"));
    // The names the registration uses have to be in the source it binds
    // against, or `bind` is `E0421` at the first run rather than here.
    for op in Op::ALL {
        assert!(
            DECLARATION.contains(&format!(" {}", op.name())),
            "`{}` is not declared by `std.db`",
            op.name()
        );
    }
    assert!(DECLARATION.contains("rollback"));
    assert_eq!(EFFECT, format!("{MODULE}.db"));
}
