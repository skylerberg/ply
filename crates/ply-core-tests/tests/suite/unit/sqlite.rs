use ply_core::sqlite::{Control, Engine, Failure, Limits, Reply, Scalar};

fn roomy() -> Limits {
    Limits {
        sql_bytes: 10_000,
        params: 100,
        rows: 1_000,
        bytes: 1_000_000,
        steps: 10_000,
    }
}

fn empty() -> Engine {
    Engine::memory(b"", false).expect("an empty database opens")
}

fn run(engine: &mut Engine, sql: &str) -> Reply {
    engine.run(sql, &[], roomy(), false)
}

fn done(engine: &mut Engine, sql: &str) -> Reply {
    let reply = run(engine, sql);
    assert_eq!(reply.failure, None, "{sql}");
    reply
}

fn rejected(engine: &mut Engine, sql: &str) -> Failure {
    let reply = run(engine, sql);
    reply
        .failure
        .unwrap_or_else(|| panic!("`{sql}` ran, and returned {:?}", reply.rows))
}

fn kind(engine: &mut Engine, sql: &str) -> &'static str {
    rejected(engine, sql).kind
}

fn text(s: &str) -> Scalar {
    Scalar::Text(s.to_string())
}

fn first(reply: &Reply) -> Scalar {
    reply.rows[0][0].clone()
}

#[test]
fn the_sqlite_engine_is_compiled_with_the_options_ply_requires() {
    let mut engine = empty();
    let options: Vec<String> = done(&mut engine, "PRAGMA compile_options")
        .rows
        .into_iter()
        .map(|row| match &row[0] {
            Scalar::Text(option) => option.clone(),
            other => panic!("an option that is {other:?}"),
        })
        .collect();
    for wanted in [
        "OMIT_LOAD_EXTENSION",
        "OMIT_SHARED_CACHE",
        "DQS=0",
        "TEMP_STORE=3",
        "MAX_MMAP_SIZE=0",
        "DEFAULT_MEMSTATUS=0",
        "LIKE_DOESNT_MATCH_BLOBS",
        "STRICT_SUBTYPE",
        "DEFAULT_FOREIGN_KEYS",
        "THREADSAFE=1",
    ] {
        assert!(
            options.iter().any(|o| o == wanted),
            "`{wanted}` is not among {options:?}"
        );
    }
    for unwanted in [
        "ENABLE_FTS",
        "ENABLE_RTREE",
        "ENABLE_DBSTAT",
        "SOUNDEX",
        "USE_URI",
    ] {
        assert!(
            !options.iter().any(|o| o.starts_with(unwanted)),
            "`{unwanted}` is among {options:?}"
        );
    }
    assert_eq!(
        first(&done(&mut engine, "SELECT sqlite_version()")),
        text("3.53.2")
    );
    assert_eq!(
        first(&done(&mut engine, "SELECT length(printf('%.200000d', 1))")),
        Scalar::Int(100_000)
    );
}

#[test]
fn every_kind_of_value_is_read_back_as_it_was_bound() {
    let mut engine = empty();
    done(
        &mut engine,
        "CREATE TABLE v (i INTEGER, r REAL, t TEXT, b BLOB, n ANY) STRICT",
    );
    let bound = vec![
        Scalar::Int(i64::MIN),
        Scalar::Real(-0.1),
        text("a\0b \u{1F600} \u{10FFFF}"),
        Scalar::Blob(vec![0, 255, 0]),
        Scalar::Null,
    ];
    let reply = engine.run(
        "INSERT INTO v VALUES (?1, ?2, ?3, ?4, ?5)",
        &bound,
        roomy(),
        false,
    );
    assert_eq!(reply.failure, None);
    assert_eq!((reply.changed, reply.last_row_id), (1, 1));
    let read = engine.run("SELECT i, r, t, b, n FROM v", &[], roomy(), true);
    assert_eq!(read.columns, ["i", "r", "t", "b", "n"]);
    assert_eq!(read.rows, vec![bound]);
    assert!(!read.wrote);
}

#[test]
fn a_statement_cannot_attach_vacuum_into_read_or_write_another_file_or_load_an_extension() {
    let mut engine = empty();
    for sql in [
        "ATTACH DATABASE '/tmp/ply-sqlite-escape.db' AS other",
        "ATTACH DATABASE 'file:/tmp/ply-sqlite-escape.db?mode=rwc' AS other",
        "ATTACH DATABASE ':memory:' AS other",
        "DETACH DATABASE main",
        "VACUUM",
        "VACUUM INTO '/tmp/ply-sqlite-escape.db'",
        "SELECT load_extension('/tmp/evil')",
        "SELECT readfile('/etc/passwd')",
        "SELECT writefile('/tmp/ply-sqlite-escape', 'x')",
    ] {
        let failure = rejected(&mut engine, sql);
        assert!(
            failure.kind == "refused" || failure.kind == "statement",
            "{sql}: {failure:?}"
        );
    }
    assert!(!std::path::Path::new("/tmp/ply-sqlite-escape.db").exists());
}

#[test]
fn a_statement_cannot_change_what_the_connection_was_opened_with() {
    let mut engine = empty();
    for sql in [
        "PRAGMA journal_mode = DELETE",
        "PRAGMA foreign_keys = OFF",
        "PRAGMA trusted_schema = ON",
        "PRAGMA writable_schema = ON",
        "PRAGMA temp_store_directory = '/tmp'",
        "PRAGMA database_list",
        "PRAGMA schema_version = 7",
        "SELECT * FROM pragma_database_list",
        "BEGIN",
        "BEGIN IMMEDIATE",
        "COMMIT",
        "ROLLBACK",
        "SAVEPOINT s",
        "RELEASE s",
        "CREATE TEMP TABLE t (a)",
        "CREATE VIRTUAL TABLE f USING fts5(a)",
    ] {
        assert_eq!(kind(&mut engine, sql), "refused", "{sql}");
    }
    assert_eq!(
        first(&done(&mut engine, "PRAGMA foreign_keys")),
        Scalar::Int(1)
    );
    done(&mut engine, "PRAGMA user_version = 7");
    assert_eq!(
        first(&done(&mut engine, "PRAGMA user_version")),
        Scalar::Int(7)
    );
}

#[test]
fn time_and_randomness_are_rejected_wherever_they_are_written() {
    let mut engine = empty();
    for sql in [
        "SELECT random()",
        "SELECT randomblob(8)",
        "SELECT CURRENT_TIMESTAMP",
        "SELECT CURRENT_DATE",
        "SELECT CURRENT_TIME",
        "SELECT datetime('now')",
        "SELECT datetime('NOW')",
        "SELECT datetime()",
        "SELECT date()",
        "SELECT time()",
        "SELECT julianday()",
        "SELECT unixepoch()",
        "SELECT strftime('%s')",
        "SELECT strftime('%s', 'now')",
        "SELECT timediff('now', '2020-01-01')",
        "SELECT datetime('n' || 'ow')",
        "SELECT datetime('2020-01-01 00:00:00', 'localtime')",
        "SELECT datetime('2020-01-01 00:00:00', 'utc')",
        "SELECT last_insert_rowid()",
        "SELECT changes()",
        "SELECT total_changes()",
    ] {
        assert_eq!(kind(&mut engine, sql), "refused", "{sql}");
    }
    let bound = engine.run("SELECT datetime(?1)", &[text("now")], roomy(), true);
    assert_eq!(bound.failure.map(|f| f.kind), Some("refused"));
}

#[test]
fn date_functions_that_read_no_clock_return_the_expected_values() {
    let mut engine = empty();
    for (sql, wanted) in [
        (
            "SELECT datetime('2020-02-28 23:00:00', '+1 day')",
            "2020-02-29 23:00:00",
        ),
        ("SELECT date('2020-03-01', '-1 day')", "2020-02-29"),
        ("SELECT strftime('%Y-%m', '2021-07-04')", "2021-07"),
        ("SELECT time('12:00', '+90 minutes')", "13:30:00"),
        ("SELECT datetime(0, 'unixepoch')", "1970-01-01 00:00:00"),
        (
            "SELECT timediff('2020-01-02', '2020-01-01')",
            "+0000-00-01 00:00:00.000",
        ),
    ] {
        assert_eq!(first(&done(&mut engine, sql)), text(wanted), "{sql}");
    }
    assert_eq!(
        first(&done(&mut engine, "SELECT unixepoch('2020-01-01')")),
        Scalar::Int(1_577_836_800)
    );
    assert_eq!(
        first(&done(
            &mut engine,
            "SELECT julianday('2000-01-01 12:00:00')"
        )),
        Scalar::Real(2_451_545.0)
    );
}

#[test]
fn a_view_or_a_trigger_cannot_smuggle_a_rejected_function() {
    let mut engine = empty();
    done(&mut engine, "CREATE TABLE a (x INTEGER) STRICT");
    run(&mut engine, "CREATE VIEW dice AS SELECT random() AS r");
    assert_ne!(run(&mut engine, "SELECT r FROM dice").failure, None);
    run(
        &mut engine,
        "CREATE TRIGGER roll AFTER INSERT ON a BEGIN UPDATE a SET x = random(); END",
    );
    run(&mut engine, "INSERT INTO a VALUES (1)");
    let held = done(&mut engine, "SELECT x FROM a").rows;
    assert!(
        held.is_empty() || held == vec![vec![Scalar::Int(1)]],
        "{held:?}"
    );

    let mut clocked = empty();
    done(&mut clocked, "CREATE TABLE a (x TEXT) STRICT");
    done(
        &mut clocked,
        "CREATE VIEW stamp AS SELECT datetime('now') AS at",
    );
    assert_eq!(kind(&mut clocked, "SELECT at FROM stamp"), "refused");
    done(
        &mut clocked,
        "CREATE TRIGGER stamped AFTER INSERT ON a BEGIN UPDATE a SET x = datetime(); END",
    );
    assert_eq!(
        kind(&mut clocked, "INSERT INTO a VALUES ('then')"),
        "refused"
    );
    assert!(done(&mut clocked, "SELECT x FROM a").rows.is_empty());
}

#[test]
fn a_schema_object_may_call_a_date_function_that_reads_no_clock() {
    let mut engine = empty();
    done(
        &mut engine,
        "CREATE TABLE e (at TEXT NOT NULL, day TEXT) STRICT",
    );
    done(
        &mut engine,
        "CREATE VIEW days AS SELECT date(at) AS day FROM e",
    );
    done(
        &mut engine,
        "CREATE TRIGGER dated AFTER INSERT ON e BEGIN UPDATE e SET day = date(at) WHERE at = new.at; END",
    );
    done(&mut engine, "CREATE INDEX by_day ON e (date(at))");
    done(
        &mut engine,
        "INSERT INTO e (at) VALUES ('2024-05-06 07:08:09')",
    );
    assert_eq!(
        first(&done(&mut engine, "SELECT day FROM days")),
        text("2024-05-06")
    );
    assert_eq!(
        first(&done(&mut engine, "SELECT day FROM e")),
        text("2024-05-06")
    );
}

#[test]
fn ordinary_schema_and_query_statements_such_as_create_alter_drop_and_select_run_successfully() {
    let mut engine = empty();
    for sql in [
        "CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT NOT NULL, note TEXT) STRICT",
        "CREATE UNIQUE INDEX t_name ON t (name)",
        "INSERT INTO t (name) VALUES ('a'), ('b')",
        "INSERT INTO t (name) VALUES ('a') ON CONFLICT (name) DO UPDATE SET note = 'again'",
        "INSERT OR IGNORE INTO t (name) VALUES ('b')",
        "ALTER TABLE t RENAME COLUMN note TO remark",
        "ALTER TABLE t ADD COLUMN extra INTEGER",
        "ALTER TABLE t DROP COLUMN extra",
        "ALTER TABLE t RENAME TO things",
        "ANALYZE",
        "REINDEX",
        "PRAGMA optimize",
        "CREATE VIEW names AS SELECT name FROM things",
        "DROP VIEW names",
        "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 5) SELECT sum(i) FROM n",
        "SELECT name, row_number() OVER (ORDER BY name) FROM things",
        "SELECT json_extract('{\"a\": [1, 2]}', '$.a[1]'), '{\"a\": 3}' ->> 'a'",
        "SELECT group_concat(name, ',') FROM things WHERE name LIKE 'a%' OR name GLOB 'b*'",
        "PRAGMA integrity_check",
        "PRAGMA foreign_key_check",
        "PRAGMA table_info(things)",
        "DROP INDEX t_name",
        "DROP TABLE things",
    ] {
        done(&mut engine, sql);
    }
}

#[test]
fn each_write_statement_reports_its_own_changed_row_count_and_last_inserted_row_id() {
    let mut engine = empty();
    let made = done(
        &mut engine,
        "CREATE TABLE t (id INTEGER PRIMARY KEY, n INTEGER) STRICT",
    );
    assert_eq!((made.changed, made.last_row_id, made.wrote), (0, 0, true));
    let inserted = done(&mut engine, "INSERT INTO t (n) VALUES (1), (2), (3)");
    assert_eq!((inserted.changed, inserted.last_row_id), (3, 3));
    let updated = done(&mut engine, "UPDATE t SET n = n + 1 WHERE id > 1");
    assert_eq!((updated.changed, updated.last_row_id), (2, 0));
    let none = done(&mut engine, "DELETE FROM t WHERE id > 9");
    assert_eq!((none.changed, none.last_row_id), (0, 0));
    let returned = done(&mut engine, "DELETE FROM t WHERE id = 1 RETURNING n");
    assert_eq!(
        (returned.changed, returned.rows),
        (1, vec![vec![Scalar::Int(1)]])
    );
    let read = done(&mut engine, "SELECT count(*) FROM t");
    assert_eq!((read.changed, read.last_row_id, read.wrote), (0, 0, false));
}

#[test]
fn a_constraint_failure_says_its_kind_and_what_it_names() {
    let mut engine = empty();
    done(
        &mut engine,
        "CREATE TABLE p (id INTEGER PRIMARY KEY, code TEXT NOT NULL UNIQUE) STRICT",
    );
    done(
        &mut engine,
        "CREATE TABLE c (id INTEGER PRIMARY KEY, p INTEGER NOT NULL REFERENCES p (id), \
         n INTEGER, CONSTRAINT positive CHECK (n > 0)) STRICT",
    );
    done(&mut engine, "INSERT INTO p VALUES (1, 'a')");
    for (sql, kind, name) in [
        ("INSERT INTO p VALUES (2, 'a')", "unique", "p.code"),
        ("INSERT INTO p VALUES (1, 'b')", "unique", "p.id"),
        ("INSERT INTO p (code) VALUES (NULL)", "not_null", "p.code"),
        ("INSERT INTO c (p, n) VALUES (1, 0)", "check", "positive"),
        ("INSERT INTO c (p, n) VALUES (9, 1)", "foreign_key", ""),
        ("INSERT INTO c (p, n) VALUES (1, 'many')", "datatype", "c.n"),
    ] {
        let failure = rejected(&mut engine, sql);
        assert_eq!(
            (failure.kind, failure.name.as_str()),
            (kind, name),
            "{sql}: {failure:?}"
        );
    }
    assert_eq!(
        first(&done(&mut engine, "SELECT count(*) FROM c")),
        Scalar::Int(0)
    );
}

#[test]
fn a_read_only_query_cannot_write_and_sql_text_with_more_than_one_statement_is_rejected() {
    let mut engine = empty();
    done(&mut engine, "CREATE TABLE t (a INTEGER) STRICT");
    let wrote = engine.run("INSERT INTO t VALUES (1)", &[], roomy(), true);
    assert_eq!(wrote.failure.map(|f| f.kind), Some("writes"));
    assert_eq!(kind(&mut engine, "SELECT 1; SELECT 2"), "refused");
    assert_eq!(
        kind(&mut engine, "INSERT INTO t VALUES (1); DROP TABLE t"),
        "refused"
    );
    assert_eq!(kind(&mut engine, "SELEC 1"), "statement");
    assert_eq!(kind(&mut engine, "SELECT * FROM missing"), "statement");
    assert_eq!(kind(&mut engine, "SELECT \"no such column\""), "statement");
    assert_eq!(kind(&mut engine, "SELECT ?1"), "refused");
    assert_eq!(kind(&mut engine, "SELECT cast(x'ff' AS TEXT)"), "text");
    assert_eq!(
        first(&done(&mut engine, "SELECT count(*) FROM t")),
        Scalar::Int(0)
    );
}

#[test]
fn each_query_limit_fails_a_query_whose_result_is_larger_than_the_limit_and_allows_a_result_within_the_limit()
 {
    let mut engine = empty();
    done(&mut engine, "CREATE TABLE t (a INTEGER) STRICT");
    done(&mut engine, "INSERT INTO t VALUES (1), (2), (3)");
    let limited = |engine: &mut Engine, sql: &str, params: &[Scalar], limits: Limits| {
        let failure = engine.run(sql, params, limits, false).failure;
        failure.map(|f| (f.kind, f.name))
    };
    let too = |name: &str| Some(("too_large", name.to_string()));
    assert_eq!(
        limited(
            &mut engine,
            "SELECT a FROM t",
            &[],
            Limits { rows: 2, ..roomy() }
        ),
        too("rows")
    );
    assert_eq!(
        limited(
            &mut engine,
            "SELECT a FROM t",
            &[],
            Limits { rows: 3, ..roomy() }
        ),
        None
    );
    assert_eq!(
        limited(
            &mut engine,
            "SELECT 'abcdef'",
            &[],
            Limits {
                bytes: 5,
                ..roomy()
            }
        ),
        too("bytes")
    );
    assert_eq!(
        limited(
            &mut engine,
            "SELECT 1",
            &[],
            Limits {
                sql_bytes: 7,
                ..roomy()
            }
        ),
        too("sql_bytes")
    );
    assert_eq!(
        limited(
            &mut engine,
            "SELECT ?1, ?2",
            &[Scalar::Int(1), Scalar::Int(2)],
            Limits {
                params: 1,
                ..roomy()
            }
        ),
        too("params")
    );
    let counting = "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 1000000) \
                    SELECT count(*) FROM n";
    assert_eq!(
        limited(
            &mut engine,
            counting,
            &[],
            Limits {
                steps: 10,
                ..roomy()
            }
        ),
        Some(("steps", String::new()))
    );
    assert_eq!(first(&done(&mut engine, "SELECT 7")), Scalar::Int(7));
    assert_eq!(
        limited(
            &mut engine,
            "DELETE FROM t RETURNING a",
            &[],
            Limits { rows: 1, ..roomy() }
        ),
        too("rows")
    );
}

#[test]
fn a_strict_connection_rejects_a_table_that_is_not_declared_strict() {
    let mut strict = Engine::memory(b"", true).unwrap();
    assert_eq!(kind(&mut strict, "CREATE TABLE loose (a)"), "refused");
    assert_eq!(
        kind(&mut strict, "/* why */ create table copy AS SELECT 1 AS a"),
        "refused"
    );
    done(&mut strict, "-- kept\nCREATE TABLE kept (a INTEGER) STRICT");
    done(&mut strict, "CREATE INDEX kept_a ON kept (a)");
    done(
        &mut strict,
        "CREATE TABLE keyed (a INTEGER PRIMARY KEY) STRICT, WITHOUT ROWID",
    );
    let mut loose = empty();
    done(&mut loose, "CREATE TABLE loose (a)");
}

#[test]
fn an_insert_or_update_that_needs_a_row_id_above_the_maximum_integer_is_rejected() {
    let mut engine = empty();
    done(
        &mut engine,
        "CREATE TABLE t (id INTEGER PRIMARY KEY, n INTEGER) STRICT",
    );
    done(&mut engine, "INSERT INTO t VALUES (9223372036854775806, 1)");
    assert_eq!(
        kind(&mut engine, "INSERT INTO t VALUES (9223372036854775807, 2)"),
        "refused"
    );
    assert_eq!(kind(&mut engine, "UPDATE t SET id = id + 1"), "refused");
    assert_eq!(kind(&mut engine, "INSERT INTO t (n) VALUES (3)"), "refused");
}

#[test]
fn a_transaction_rolls_back_to_where_it_began() {
    let mut engine = empty();
    done(&mut engine, "CREATE TABLE t (a INTEGER) STRICT");
    let count = |engine: &mut Engine| first(&done(engine, "SELECT count(*) FROM t"));
    assert_eq!(engine.control(Control::Begin).failure, None);
    done(&mut engine, "INSERT INTO t VALUES (1)");
    assert_eq!(engine.control(Control::Begin).failure, None);
    done(&mut engine, "INSERT INTO t VALUES (2)");
    assert_eq!(count(&mut engine), Scalar::Int(2));
    assert_eq!(engine.control(Control::Rollback).failure, None);
    assert_eq!(count(&mut engine), Scalar::Int(1));
    assert_eq!(engine.control(Control::Commit).failure, None);
    assert_eq!(count(&mut engine), Scalar::Int(1));
    assert_eq!(engine.control(Control::Begin).failure, None);
    done(&mut engine, "DELETE FROM t");
    assert_eq!(engine.control(Control::Rollback).failure, None);
    assert_eq!(count(&mut engine), Scalar::Int(1));
    assert_eq!(
        engine.control(Control::Commit).failure.map(|f| f.kind),
        Some("refused")
    );
    assert_eq!(
        engine.control(Control::Rollback).failure.map(|f| f.kind),
        Some("refused")
    );
}

#[test]
fn a_statement_is_read_a_page_at_a_time() {
    let mut engine = empty();
    done(&mut engine, "CREATE TABLE t (a INTEGER) STRICT");
    done(&mut engine, "INSERT INTO t VALUES (1), (2), (3), (4), (5)");
    let mut pages = Vec::new();
    engine.run_in_pages("SELECT a FROM t ORDER BY a", &[], roomy(), true, |page| {
        pages.push(page);
        Some(2)
    });
    let sizes: Vec<usize> = pages.iter().map(|page| page.rows.len()).collect();
    assert_eq!(sizes, [0, 2, 2, 1]);
    assert_eq!(pages[0].columns, ["a"]);
    assert_eq!(pages[3].rows, vec![vec![Scalar::Int(5)]]);

    let mut stopped = Vec::new();
    engine.run_in_pages("SELECT a FROM t ORDER BY a", &[], roomy(), true, |page| {
        stopped.push(page);
        (stopped.len() < 2).then_some(2)
    });
    assert_eq!(stopped.len(), 2);

    let mut over = Vec::new();
    engine.run_in_pages(
        "SELECT a FROM t",
        &[],
        Limits { rows: 3, ..roomy() },
        true,
        |page| {
            over.push(page);
            Some(2)
        },
    );
    assert_eq!(
        over.last().unwrap().failure.as_ref().map(|f| f.kind),
        Some("too_large")
    );
    assert_eq!(
        first(&done(&mut engine, "SELECT count(*) FROM t")),
        Scalar::Int(5)
    );
}

fn over(image: &[u8], sql: &str, params: &[Scalar], reads_only: bool) -> (Reply, Option<Vec<u8>>) {
    ply_core::sqlite::run_on_image(image, sql, params, roomy(), reads_only, true)
}

#[test]
fn running_an_sql_statement_on_the_same_database_bytes_twice_returns_equal_results_and_equal_new_database_bytes()
 {
    let (made, image) = over(b"", "CREATE TABLE t (a INTEGER, b TEXT) STRICT", &[], false);
    assert_eq!(made.failure, None);
    let image = image.expect("a table made is an image left");
    assert!(!image.is_empty());
    let insert = |image: &[u8]| {
        over(
            image,
            "INSERT INTO t VALUES (?1, ?2)",
            &[Scalar::Int(7), text("seven")],
            false,
        )
    };
    let once = insert(&image);
    let again = insert(&image);
    assert_eq!(once, again);
    assert_eq!(once.0.changed, 1);
    assert_eq!(once.0.last_row_id, 1);
    let inserted = once.1.expect("a row written is an image left");

    let (read, left) = over(&inserted, "SELECT a, b FROM t", &[], true);
    assert_eq!(
        left, None,
        "a statement that only reads leaves the image as it was"
    );
    assert_eq!(read.columns, ["a", "b"]);
    assert_eq!(read.rows, [vec![Scalar::Int(7), text("seven")]]);

    let (failed, left) = over(
        &inserted,
        "INSERT INTO t VALUES (random(), 'x')",
        &[],
        false,
    );
    assert_eq!(left, None);
    assert!(failed.failure.is_some());

    let (unread, _) = over(
        b"not a database at all",
        "SELECT count(*) FROM sqlite_schema",
        &[],
        true,
    );
    assert!(unread.failure.is_some());
}

#[test]
fn every_function_on_the_allowed_function_list_exists_in_sqlite_and_the_list_is_sorted() {
    let mut sorted = ply_core::sqlite::FUNCTIONS.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted, ply_core::sqlite::FUNCTIONS, "sorted, and each once");
    let mut engine = empty();
    let known: Vec<Scalar> = done(
        &mut engine,
        "SELECT DISTINCT name FROM pragma_function_list",
    )
    .rows
    .into_iter()
    .map(|row| row[0].clone())
    .collect();
    for name in ply_core::sqlite::FUNCTIONS {
        assert!(known.contains(&text(name)), "the engine has no `{name}`");
    }
}
