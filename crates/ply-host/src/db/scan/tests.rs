use super::*;

fn ok(sql: &str) -> Scan {
    scan(sql, Span::DUMMY).unwrap_or_else(|d| panic!("`{sql}` was refused: {}", d.message))
}

fn tables(sql: &str) -> Vec<String> {
    ok(sql).tables.all().into_iter().collect()
}

fn refused(sql: &str) -> Diagnostic {
    match scan(sql, Span::DUMMY) {
        Err(d) => {
            assert_eq!(d.code, codes::DB_STATEMENT_REFUSED, "{sql}");
            d
        }
        Ok(s) => panic!("`{sql}` was accepted as {s}"),
    }
}

/// Every refusal has to say where, or a reader is left bisecting their own SQL.
fn refused_at(sql: &str, byte: usize) -> Diagnostic {
    let d = refused(sql);
    let rendered = format!("{} {}", d.message, d.notes.join(" "));
    assert!(
        rendered.contains(&format!("byte {byte}")),
        "`{sql}` was refused without naming byte {byte}: {rendered}"
    );
    d
}

#[test]
fn a_select_names_the_table_it_reads() {
    assert_eq!(tables("select sku from items"), ["items"]);
    assert_eq!(ok("select sku from items").kind, Kind::Select);
    assert_eq!(ok("select sku from items").tables.written.len(), 0);
}

/// The hole the milestone exists to close: one label, two tables. The call site
/// writes `orders` and the statement reads `items` as well, and nothing in the
/// type system can see it because the SQL is a `String`.
#[test]
fn a_join_names_both_tables() {
    assert_eq!(
        tables("select o.id, i.sku from orders o join items i on i.sku = o.sku"),
        ["items", "orders"]
    );
    assert_eq!(
        tables("select * from orders, items where orders.sku = items.sku"),
        ["items", "orders"]
    );
    assert_eq!(
        tables("select * from a left outer join b on a.x = b.x full join c on c.y = a.y"),
        ["a", "b", "c"]
    );
    assert_eq!(tables("select * from a natural join b"), ["a", "b"]);
    assert_eq!(tables("select * from a join b using (x)"), ["a", "b"]);
}

#[test]
fn a_set_operation_names_every_arm() {
    assert_eq!(
        tables("select sku from items union all select sku from orders"),
        ["items", "orders"]
    );
    assert_eq!(
        tables("select a from x intersect select a from y except select a from z"),
        ["x", "y", "z"]
    );
    assert_eq!(
        tables("(select a from x) union (select a from y)"),
        ["x", "y"]
    );
}

#[test]
fn a_subquery_is_followed_rather_than_ignored() {
    assert_eq!(
        tables("select sku from items where sku in (select sku from orders)"),
        ["items", "orders"]
    );
    assert_eq!(
        tables("select (select count(*) from orders) from items"),
        ["items", "orders"]
    );
    assert_eq!(
        tables("select * from (select sku from orders) o"),
        ["orders"]
    );
    assert_eq!(
        tables(
            "select * from items where exists (select 1 from orders where orders.sku = items.sku)"
        ),
        ["items", "orders"]
    );
}

/// A CTE name is not a relation, and the relations it read are. A scanner that
/// reported the CTE name would name a table the database does not have, and one
/// that dropped the whole `with` would miss the tables it read.
#[test]
fn a_cte_resolves_to_its_own_sources() {
    assert_eq!(
        tables("with recent as (select * from orders) select * from recent"),
        ["orders"]
    );
    assert_eq!(
        tables(
            "with a as (select * from x), b as (select * from a join y on true) select * from b"
        ),
        ["x", "y"]
    );
    assert_eq!(
        tables("with recursive t(n) as (select 1 union all select n+1 from t) select n from t"),
        Vec::<String>::new()
    );
}

/// A data-modifying CTE writes, and the write has to reach the footprint or a
/// `db.query[items]` could insert into `orders` with a read atom recorded.
#[test]
fn a_data_modifying_cte_reports_its_write() {
    let s = ok("with gone as (delete from stale returning id) select id from gone");
    assert!(s.tables.written.contains("stale"), "{s}");
    assert_eq!(s.kind, Kind::Select);
}

#[test]
fn an_insert_writes_its_target_and_reads_its_source() {
    let s = ok("insert into items (sku, name) values ($1, $2)");
    assert_eq!(s.kind, Kind::Insert);
    assert_eq!(
        s.tables.written.iter().cloned().collect::<Vec<_>>(),
        ["items"]
    );
    assert!(s.tables.read.is_empty());

    let s = ok("insert into archive select * from items where on_hand = 0");
    assert_eq!(
        s.tables.written.iter().cloned().collect::<Vec<_>>(),
        ["archive"]
    );
    assert_eq!(s.tables.read.iter().cloned().collect::<Vec<_>>(), ["items"]);

    assert_eq!(tables("insert into items default values"), ["items"]);
    assert_eq!(
        tables("insert into items (sku) values ($1) returning sku"),
        ["items"]
    );
}

#[test]
fn an_update_writes_its_target_and_reads_its_from_list() {
    let s = ok("update items set on_hand = on_hand - l.qty from lines l where l.sku = items.sku");
    assert_eq!(s.kind, Kind::Update);
    assert_eq!(
        s.tables.written.iter().cloned().collect::<Vec<_>>(),
        ["items"]
    );
    assert_eq!(s.tables.read.iter().cloned().collect::<Vec<_>>(), ["lines"]);
}

#[test]
fn a_delete_writes_its_target_and_reads_its_using_list() {
    let s = ok("delete from orders using items where items.sku = orders.sku returning orders.id");
    assert_eq!(s.kind, Kind::Delete);
    assert_eq!(
        s.tables.written.iter().cloned().collect::<Vec<_>>(),
        ["orders"]
    );
    assert_eq!(s.tables.read.iter().cloned().collect::<Vec<_>>(), ["items"]);
}

/// A relation that is both read and written is written: the conflict graph has
/// to serialise it against every reader, and calling it a read would not.
#[test]
fn a_table_read_and_written_by_one_statement_is_a_write() {
    let s = ok("update items set n = n + 1 where sku in (select sku from items where n < 3)");
    assert_eq!(
        s.tables.written.iter().cloned().collect::<Vec<_>>(),
        ["items"]
    );
    assert!(s.tables.read.is_empty());
}

#[test]
fn a_schema_qualified_name_is_its_last_segment() {
    assert_eq!(tables("select * from public.items"), ["items"]);
    assert_eq!(tables("insert into public.items values ($1)"), ["items"]);
}

/// Postgres folds an unquoted identifier to lower case and keeps a quoted one,
/// so a scanner that folded both would give `"Items"` the label `items` and
/// schedule two different relations as one.
#[test]
fn case_folding_follows_postgres_rather_than_the_scanner() {
    assert_eq!(tables("SELECT * FROM Items"), ["items"]);
    assert_eq!(tables("select * from \"Items\""), ["Items"]);
    assert_eq!(tables("select * from \"has space\""), ["has space"]);
}

// --- refusals ---------------------------------------------------------------

#[test]
fn a_second_statement_is_refused_and_named() {
    let d = refused_at("select 1 from items; drop table items", 19);
    assert!(
        d.message.contains("more than one statement"),
        "{}",
        d.message
    );
    refused("insert into items values (1); delete from items");
}

/// The whole reason the payload class matters: the same bytes are ordinary text
/// inside a literal and a stacked statement outside one.
#[test]
fn a_semicolon_inside_a_literal_is_text() {
    assert_eq!(
        tables("select * from items where sku = '; drop table items; --'"),
        ["items"]
    );
    assert_eq!(
        tables("select * from items where sku = $tag$; drop table items$tag$"),
        ["items"]
    );
    assert_eq!(tables("select * from items where sku = $$;$$"), ["items"]);
    assert_eq!(
        tables("select * from items where sku = e'\\'; drop table items; --'"),
        ["items"]
    );
    assert_eq!(
        tables("select * from items where name = 'it''s; fine'"),
        ["items"]
    );
    assert_eq!(tables("select * from \"weird;name\""), ["weird;name"]);
}

#[test]
fn a_comment_is_not_statement_text() {
    assert_eq!(
        tables("select 1 from items -- ; drop table items"),
        ["items"]
    );
    assert_eq!(
        tables("select 1 /* ; drop table items */ from items"),
        ["items"]
    );
    assert_eq!(
        tables("select 1 /* outer /* inner ; */ still comment */ from items"),
        ["items"]
    );
    refused("select 1 /* never closed from items");
}

#[test]
fn an_unterminated_literal_is_a_refusal_rather_than_a_guess() {
    refused_at("select * from items where sku = 'open", 32);
    refused_at("select * from \"open", 14);
    refused_at("select * from items where sku = $tag$open", 32);
}

#[test]
fn every_statement_shape_outside_the_admitted_set_is_refused() {
    for sql in [
        "create table t (a int)",
        "alter table t add column b int",
        "drop table t",
        "truncate t",
        "copy t from stdin",
        "do $$ begin end $$",
        "call p()",
        "lock table t",
        "listen chan",
        "notify chan",
        "set statement_timeout = 0",
        "show all",
        "grant select on t to r",
        "explain select * from t",
        "begin",
        "commit",
        "vacuum",
        "table items",
        "",
        "   ",
    ] {
        refused(sql);
    }
}

#[test]
fn constructs_the_scanner_cannot_account_for_are_named_refusals() {
    // A set-returning function's relations live in a function body.
    refused("select * from generate_series(1, 10)");
    // A row lock is a write the scanner has no way to report through a read.
    refused("select * from items for update");
    refused("select * from items limit 1 for share");
    // An upsert's outcome is not reproducible by the engine.
    refused("insert into items values ($1) on conflict do nothing");
    // Window definitions.
    refused("select rank() over w from items window w as (order by sku)");
    // A parenthesised join.
    refused("select * from (items join orders on true)");
}

/// `AS` is a clause boundary everywhere else in the grammar and an output column
/// name in the select list. Treating it as a boundary here stopped the walk at
/// the first alias, never ate the `from`, and came back with an empty table set
/// — so the shape `E0433`'s own advice tells a reader to write ("alias one of
/// them: `select a.id as a_id, b.id as b_id`") was one the scanner refused.
#[test]
fn an_alias_in_the_select_list_is_a_name_and_not_a_clause() {
    assert_eq!(tables("select id as ident from items"), ["items"]);
    assert_eq!(tables("select count(*) as n from items"), ["items"]);
    assert_eq!(tables("select id ident from items"), ["items"]);
    assert_eq!(
        tables("select a.id as a_id, b.id as b_id from items as a join orders as b on a.id = b.id"),
        ["items", "orders"]
    );
    assert_eq!(
        tables("select (select max(n) from orders) as top, sku from items"),
        ["items", "orders"]
    );
    // And the clauses after it are still clauses.
    assert_eq!(
        tables("select n as v from items where n > $1 order by n limit 3"),
        ["items"]
    );
}

/// The second half of §2.4. The scanner decides statement *shapes*; a call lives
/// inside every shape it admits, and it is the one place an admitted statement
/// can reach state no footprint names — the pooled connection's own session,
/// which is the thing two host-backed tests share. So it calls only what it will
/// vouch for.
#[test]
fn a_function_the_scanner_will_not_vouch_for_is_refused() {
    for sql in [
        "select set_config('search_path', 'pg_catalog', false) from items",
        "select pg_advisory_lock(918) from items",
        "select pg_read_file('/etc/hosts') from items",
        "select pg_sleep(0) from items",
        "select id from items where pg_backend_pid() > 0",
        "insert into items (id) values (pg_backend_pid())",
        "update items set n = pg_backend_pid()",
        "delete from items where id = pg_backend_pid()",
    ] {
        let d = refused(sql);
        assert!(
            d.notes.iter().any(|n| n.contains("the scanner calls:")),
            "{sql}: {:?}",
            d.notes
        );
    }
    for sql in [
        "select count(*) from items",
        "select sum(n) from items",
        "select coalesce(n, 0) from items",
        "select lower(sku), length(sku) from items",
        "select id from items where id = any($1)",
        "select id from items where sku in (select sku from orders)",
        "select cast(n as text) from items",
        "select id from items where not (n > $1)",
        "select case when n > $1 then upper(sku) else sku end from items",
    ] {
        ok(sql);
    }
    // Quoted, it is a name and not a call the scanner has an opinion about.
    assert_eq!(tables("select \"pg_sleep\" from items"), ["items"]);
}

#[test]
fn a_nondeterministic_function_in_statement_text_is_refused() {
    for sql in [
        "insert into items (at) values (now())",
        "select * from items where at < current_timestamp",
        "select random() from items",
        "update items set at = clock_timestamp()",
        "select * from items where d = current_date",
    ] {
        let d = refused(sql);
        assert!(
            d.notes.iter().any(|n| n.contains("parameter")),
            "{sql}: {:?}",
            d.notes
        );
    }
    // Quoted, it is a column name and not a call.
    assert_eq!(tables("select \"now\" from items"), ["items"]);
}

#[test]
fn nesting_is_bounded_rather_than_a_stack_overflow() {
    let mut sql = String::from("select * from items");
    for _ in 0..64 {
        sql = format!("select * from ({sql}) t");
    }
    refused(&sql);
}

/// The scanner is handed a `String` a program can build with `++`, so it is
/// handed adversarial input by construction. None of these may panic.
#[test]
fn malformed_input_is_a_diagnostic_and_never_a_panic() {
    for sql in [
        "select",
        "select * from",
        "select * from (",
        "insert into",
        "update",
        "update t",
        "update t set",
        "delete",
        "delete from",
        "with",
        "with a",
        "with a as",
        "with a as (",
        "select * from a join",
        "select * from a join b on",
        "((((",
        "))))",
        "$",
        "$$",
        "'",
        "\"",
        "select * from items order by",
        "select * from items group by",
        "select ,,, from items",
        "select * from items where (",
        "натурально",
        "select * from ünïcode",
    ] {
        let _ = scan(sql, Span::DUMMY);
    }
}

/// Not a proof, and the ADR says so: a value is structurally safe because it
/// crosses in a `Bind`, and statement text is the program's own to get right.
/// What the scanner buys is that the payload class which turns a fragment into a
/// `DROP` needs a `;`, and that a fragment which changes the statement's shape
/// is usually a refusal.
#[test]
fn the_injection_payloads_that_change_a_statements_shape_are_refusals() {
    refused("select * from items where sku = '' ; drop table items --'");
    // Not a refusal, and it does not need to be: the injected arm is a table
    // the scan names, so the footprint check refuses it against a row that
    // never declared `pg_shadow`. The scanner's job is to see it, not to
    // recognise it as an attack.
    assert_eq!(
        tables("select * from items where sku = '' union select * from pg_shadow --'"),
        ["items", "pg_shadow"]
    );
    // This one is *not* a refusal either, and pretending otherwise would be the lie:
    // the fragment is a legal `or`, it reads no new table, and the defence
    // against it is that a parameter never becomes syntax in the first place.
    assert_eq!(
        tables("select * from items where sku = '' or 1=1"),
        ["items"]
    );
}

#[test]
fn a_placeholder_is_a_token_and_never_a_value() {
    assert_eq!(
        tables("select * from items where sku = $1 and n > $22"),
        ["items"]
    );
    assert_eq!(
        tables("insert into items values ($1, $2, $3) returning sku"),
        ["items"]
    );
}

#[test]
fn the_statement_kind_is_reported_so_a_read_label_over_a_write_can_be_refused() {
    assert!(!ok("select * from items").kind.writes());
    assert!(!ok("values (1), (2)").kind.writes());
    assert!(ok("insert into items values ($1)").kind.writes());
    assert!(ok("update items set a = 1").kind.writes());
    assert!(ok("delete from items").kind.writes());
    assert!(
        ok("with gone as (delete from items returning id) select * from gone")
            .tables
            .written
            .contains("items")
    );
}
