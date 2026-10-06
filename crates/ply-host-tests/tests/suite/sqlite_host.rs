//! `std.sqlite` over files: a compiled program's databases under a root the host binds, held to
//! that root by the host's confinement and by the engine's own authorizer.

use ply_eval::{Machine, Span, Symbol, Value, codes};
use ply_host::fs::Roots;
use std::path::Path;
use std::sync::Arc;

const PROGRAM: &str = r#"
import std.db (stmt, PInt, PText, Row)
import std.sqlite
import std.sqlite (sqlite, sql, Busy, Refused, Unopened, Create, ReadOnly, ReadWrite, Limits, Options)

fn roomy() -> Limits = { sql_bytes: 10000, params: 100, rows: 100000, bytes: 10000000, steps: 1000000 }

fn plain() -> Options = { strict: true, busy_ms: 50, statements: 8 }

fn s(text: String) -> sqlite::Bound / {sql.refused, abort.raise} = sqlite::bound(stmt(text), [])

fn refusal<a | e>(go: () -> a / {sql.refused | e}) -> String / e =
  match try[sql.refused] { go() } {
    Ok(_) -> "ran",
    Err(Busy) -> "busy",
    Err(Refused(_)) -> "refused",
    Err(Unopened(_)) -> "unopened",
    Err(_) -> "other",
  }

fn names<[d]>(conn: Hold<sqlite::Connection<[d]>>) -> List<String> / {hold.read, sqlite.query[d], sql.refused, abort.raise} =
  map(sqlite::query(conn, s("select name from person order by id"), roomy()), |row: Row|
    sqlite::text(row, "name"))

// Written, closed, opened again and read back.
pub fn round_trip() -> List<String> / {sqlite.read[app], sqlite.write[app], sql.refused, abort.raise} = {
  with_hold[first](sqlite::opening[app]("data.db", Create, plain()), |c| sqlite::close(c)) { conn -> {
    sqlite::execute(conn, s("create table person (id integer primary key, name text not null) strict"), roomy());
    sqlite::transaction(conn, || {
      sqlite::execute(conn, sqlite::bound(stmt("insert into person (name) values (?1)"), [PText("Ada")]), roomy());
      sqlite::execute(conn, sqlite::bound(stmt("insert into person (name) values (?1)"), [PText("Grace")]), roomy())
    })
  } };
  with_hold[second](sqlite::opening[app]("data.db", ReadOnly, plain()), |c| sqlite::close(c)) { conn -> names(conn) }
}

/// What the engine itself answers a statement the reader would have refused first.
fn raw(text: String) -> String / {sqlite.read[app], sqlite.write[app]} =
  match sqlite.open[app]("escape.db", Create, plain()) {
    Err(f) -> f.kind,
    Ok(conn) -> {
      let said = match sqlite.execute[app](conn, text, [], roomy()).failure {
        Some(f) -> f.kind,
        None -> "ran",
      };
      sqlite.close[app](conn);
      said
    },
  }

pub fn escapes(outside: String) -> List<String> / {sqlite.read[app], sqlite.write[app]} =
  [
    raw("attach database '" ++ outside ++ "/attached.db' as other"),
    raw("attach database 'file:" ++ outside ++ "/uri.db?mode=rwc' as other"),
    raw("vacuum into '" ++ outside ++ "/vacuumed.db'"),
    raw("select readfile('" ++ outside ++ "/secret.txt')"),
    raw("select writefile('" ++ outside ++ "/written.txt', 'x')"),
    raw("pragma temp_store_directory = '" ++ outside ++ "'"),
    raw("select load_extension('" ++ outside ++ "/evil')"),
  ]

pub fn opens(path: String) -> String / {sqlite.read[app], sqlite.write[app], abort.raise} =
  refusal(|| with_hold[opened_at](sqlite::opening[app](path, Create, plain()), |c| sqlite::close(c)) { conn ->
    sqlite::execute(conn, s("create table if not exists t (n integer) strict"), roomy()) })

pub fn backs_up(path: String) -> String / {sqlite.read[app], sqlite.write[app], abort.raise} =
  refusal(|| with_hold[source](sqlite::opening[app]("data.db", ReadWrite, plain()), |c| sqlite::close(c)) { conn ->
    sqlite::backup(conn, path) })

pub fn unbound() -> String / {sqlite.read[nowhere], sqlite.write[nowhere], abort.raise} =
  refusal(|| with_hold[nowhere_db](sqlite::opening[nowhere]("x.db", Create, plain()), |c| sqlite::close(c)) { conn -> 0 })

// One connection writing in a transaction, another reading and then trying to write.
pub fn together() -> List<String> / {sqlite.read[app], sqlite.write[app], sql.refused, abort.raise} =
  with_hold[one](sqlite::opening[app]("data.db", ReadWrite, plain()), |c| sqlite::close(c)) { one ->
    with_hold[two](sqlite::opening[app]("data.db", ReadWrite, plain()), |c| sqlite::close(c)) { two -> {
      let during = sqlite::transaction(one, || {
        sqlite::execute(one, sqlite::bound(stmt("insert into person (name) values (?1)"), [PText("Edsger")]), roomy());
        [
          string_of_int(len(names(two))),
          refusal(|| sqlite::execute(two, s("insert into person (name) values ('Barbara')"), roomy())),
          refusal(|| sqlite::transaction(two, || 0)),
          string_of_int(len(names(one))),
        ]
      });
      push(push(during, string_of_int(len(names(two)))), refusal(|| sqlite::execute(two, s("insert into person (name) values ('Barbara')"), roomy())))
    } } }

fn string_of_int(n: Int) -> String = int_to_string(n)

// A large answer read a page at a time, from a statement the connection keeps open between pages.
pub fn folded(rows: Int) -> Int / {sqlite.read[app], sqlite.write[app], sql.refused, abort.raise} =
  with_hold[folding](sqlite::opening[app]("fold.db", Create, plain()), |c| sqlite::close(c)) { conn -> {
    sqlite::execute(conn, s("create table if not exists n (i integer primary key) strict"), roomy());
    sqlite::execute(conn, s("delete from n"), roomy());
    sqlite::execute(
      conn,
      sqlite::bound(
        stmt("with recursive c(i) as (select 1 union all select i + 1 from c where i < ?1) insert into n select i from c"),
        [PInt(rows)],
      ),
      roomy(),
    );
    sqlite::fold_rows(conn, s("select i from n order by i"), roomy(), 64, 0, |acc: Int, row: Row|
      acc + sqlite::int(row, "i"))
  } }

/// One scenario, written against any root.
fn scenario<[d]>() -> List<String> / {sqlite.read[d], sqlite.write[d], sql.refused, abort.raise} =
  with_hold[scene](sqlite::opening[d]("scene.db", Create, plain()), |c| sqlite::close(c)) { conn -> {
    sqlite::execute(conn, s("create table person (id integer primary key, name text not null unique) strict"), roomy());
    let made = sqlite::execute(conn, s("insert into person (name) values ('Ada'), ('Grace')"), roomy());
    let twice = refusal(|| sqlite::execute(conn, s("insert into person (name) values ('Ada')"), roomy()));
    let undone = refusal(|| sqlite::transaction(conn, || {
      sqlite::execute(conn, s("delete from person"), roomy());
      sqlite::execute(conn, s("insert into person (id, name) values (1, null)"), roomy())
    }));
    let clocked = refusal(|| sqlite::execute(conn, sqlite::bound(stmt("insert into person (name) values (datetime(?1))"), [PText("now")]), roomy()));
    let dated = map(sqlite::query(conn, s("select date('2024-02-28', '+2 days') as d"), roomy()), |row: Row|
      sqlite::text(row, "d"));
    let loose = refusal(|| sqlite::execute(conn, s("create table loose (a)"), roomy()));
    concat_all([[int_to_string(made.rows), int_to_string(made.last_row_id), twice, undone, clocked, loose], dated, names(conn)])
  } }

fn concat_all(lists: List<List<String>>) -> List<String> =
  fold(lists, [], |acc: List<String>, list: List<String>| fold(list, acc, |a: List<String>, x: String| push(a, x)))

pub fn on_a_file() -> List<String> / {sqlite.read[app], sqlite.write[app], sql.refused, abort.raise} =
  scenario[app]()

pub fn on_the_twin() -> List<String> / {sql.refused, abort.raise} =
  sqlite::in_memory[app](|| scenario[app]())

// A connection its entry never closes.
pub fn left_open() -> Int / {sqlite.read[app], sqlite.write[app], sql.refused, abort.raise} = {
  let conn = sqlite::opening[app]("left.db", Create, plain());
  with_hold[left](conn, |_: sqlite::Connection<[app]>| ()) { held -> {
    sqlite::execute(held, s("create table if not exists t (n integer) strict"), roomy());
    sqlite::execute(held, s("insert into t values (1)"), roomy()).rows
  } }
}
"#;

struct Rooted {
    dir: tempfile::TempDir,
    host: Arc<ply_host::Host>,
}

impl Rooted {
    /// A host whose `app` is `<dir>/root`, beside an `outside` no label names.
    fn new() -> Rooted {
        let dir = tempfile::tempdir().expect("a scratch directory");
        std::fs::create_dir(dir.path().join("root")).unwrap();
        std::fs::create_dir(dir.path().join("outside")).unwrap();
        let mut roots = Roots::new();
        roots
            .bind("app", &dir.path().join("root"), Span::DUMMY)
            .expect("the root is a directory");
        Rooted {
            dir,
            host: Arc::new(ply_host::Host::new().rooted(roots)),
        }
    }

    fn root(&self) -> std::path::PathBuf {
        self.dir.path().join("root")
    }

    fn outside(&self) -> std::path::PathBuf {
        self.dir.path().join("outside")
    }

    fn call(&self, entry: &str, args: Vec<Value>) -> Result<Value, ply_eval::Diagnostic> {
        let (front, unit) = crate::support::answered::compiled("m", PROGRAM);
        let binding = self
            .host
            .registry()
            .bind(&front.check)
            .expect("the declaration and the registration agree");
        let mut machine = Machine::new(&front, ply_eval::Provider::attach(unit))
            .expect("the unit was compiled from this program");
        machine.set_host_binding(Arc::new(binding));
        machine.set_host_runtime({
            let host = Arc::clone(&self.host);
            Arc::new(move || host.runtime())
        });
        let declared = front
            .check
            .defs
            .get(&Symbol::new(entry))
            .expect("the entry is a definition of the program");
        machine.set_declared_footprint(declared.footprint.clone());
        machine.call(entry, args, Span::DUMMY).into_parts().0
    }

    fn answered(&self, entry: &str, args: Vec<Value>) -> Value {
        self.call(entry, args)
            .unwrap_or_else(|e| panic!("`{entry}` answers: {e}"))
    }
}

fn strings(values: &[&str]) -> Value {
    Value::list(values.iter().map(Value::str).collect())
}

fn text(path: &Path) -> Value {
    Value::str(path.to_str().expect("a UTF-8 scratch path"))
}

/// Every file below `dir`, by its path from it.
fn files(dir: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(at) = pending.pop() {
        for entry in std::fs::read_dir(&at).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() && !path.is_symlink() {
                pending.push(path);
            } else {
                found.push(path.strip_prefix(dir).unwrap().display().to_string());
            }
        }
    }
    found.sort();
    found
}

#[test]
fn a_database_written_and_closed_is_read_back_from_its_file() {
    let rooted = Rooted::new();
    assert_eq!(
        rooted.answered("m.round_trip", vec![]),
        strings(&["Ada", "Grace"])
    );
    let file = std::fs::read(rooted.root().join("data.db")).expect("the database is a file");
    assert_eq!(&file[..16], b"SQLite format 3\0");
    // Bytes 18 and 19 are the file format's write and read versions: 2 is write-ahead logging.
    assert_eq!((file[18], file[19]), (2, 2));
    // The log stays beside a database whose last connection only read.
    assert_eq!(
        files(&rooted.root()),
        ["data.db", "data.db-shm", "data.db-wal"]
    );
}

#[test]
fn the_engine_itself_refuses_every_way_out_of_the_root() {
    let rooted = Rooted::new();
    std::fs::write(rooted.outside().join("secret.txt"), b"secret").unwrap();
    let kinds = rooted.answered("m.escapes", vec![text(&rooted.outside())]);
    assert_eq!(
        kinds,
        strings(&[
            "refused",
            "refused",
            "refused",
            "statement",
            "statement",
            "refused",
            "statement"
        ]),
        "attach, attach by URI, vacuum into, readfile, writefile, a pragma, load_extension"
    );
    assert_eq!(files(&rooted.outside()), ["secret.txt"]);
    assert_eq!(files(&rooted.root()), ["escape.db"]);
}

#[test]
fn a_path_that_leaves_the_root_is_refused_before_anything_opens() {
    let rooted = Rooted::new();
    let outside = rooted.outside().join("abs.db");
    for path in [
        "../outside/up.db",
        outside.to_str().unwrap(),
        "a/../../outside/up.db",
    ] {
        let refused = rooted
            .call("m.opens", vec![Value::str(path)])
            .expect_err("a path outside the root");
        assert_eq!(
            refused.code,
            codes::FS_PATH_ESCAPES_ROOT,
            "{path}: {refused}"
        );
    }
    rooted.answered("m.round_trip", vec![]);
    for path in ["../outside/copy.db", outside.to_str().unwrap()] {
        let refused = rooted
            .call("m.backs_up", vec![Value::str(path)])
            .expect_err("a backup outside the root");
        assert_eq!(
            refused.code,
            codes::FS_PATH_ESCAPES_ROOT,
            "{path}: {refused}"
        );
    }
    assert!(files(&rooted.outside()).is_empty());
    assert_eq!(
        rooted
            .call("m.unbound", vec![])
            .expect_err("a label no root is bound to")
            .code,
        codes::FS_ROOT_UNBOUND
    );
}

#[test]
fn a_database_is_never_opened_through_a_symbolic_link() {
    let rooted = Rooted::new();
    let (root, outside) = (rooted.root(), rooted.outside());
    assert_eq!(
        rooted.answered("m.opens", vec![Value::str("real.db")]),
        Value::str("ran")
    );

    // A link to a database inside the root, a directory that is a link, and a link out of it.
    std::os::unix::fs::symlink(root.join("real.db"), root.join("link.db")).unwrap();
    std::fs::create_dir(root.join("dir")).unwrap();
    std::os::unix::fs::symlink(root.join("dir"), root.join("linked")).unwrap();
    std::os::unix::fs::symlink(outside.join("out.db"), root.join("out.db")).unwrap();
    std::fs::write(outside.join("out.db"), b"").unwrap();
    for inside in ["link.db", "linked/new.db"] {
        assert_eq!(
            rooted.answered("m.opens", vec![Value::str(inside)]),
            Value::str("unopened"),
            "{inside}"
        );
    }
    let out = rooted
        .call("m.opens", vec![Value::str("out.db")])
        .expect_err("a link out of the root");
    assert_eq!(out.code, codes::FS_PATH_ESCAPES_ROOT);

    // The journal and the write-ahead log the engine would open beside a database, each a link
    // to a file outside the root.
    for suffix in ["-wal", "-journal", "-shm"] {
        let name = format!("side{}.db", suffix.replace('-', "_"));
        let target = outside.join(format!("target{suffix}"));
        std::os::unix::fs::symlink(&target, root.join(format!("{name}{suffix}"))).unwrap();
        assert_eq!(
            rooted.answered("m.opens", vec![Value::str(&name)]),
            Value::str("unopened"),
            "{name}{suffix}"
        );
        assert!(
            !target.exists(),
            "{} was written through the link",
            target.display()
        );
        assert!(!root.join(&name).exists(), "{name} was made");
    }
    assert_eq!(std::fs::read(outside.join("out.db")).unwrap(), b"");
    assert_eq!(files(&outside), ["out.db"]);

    // A backup is written to a new file, never through a link that is already there.
    rooted.answered("m.round_trip", vec![]);
    std::os::unix::fs::symlink(outside.join("copied.db"), root.join("copy.db")).unwrap();
    assert_eq!(
        rooted.answered("m.backs_up", vec![Value::str("copy.db")]),
        Value::str("refused")
    );
    assert_eq!(
        rooted.answered("m.backs_up", vec![Value::str("kept.db")]),
        Value::str("ran")
    );
    assert_eq!(files(&outside), ["out.db"]);
    assert_eq!(
        &std::fs::read(root.join("kept.db")).unwrap()[..16],
        b"SQLite format 3\0"
    );
}

#[test]
fn a_reader_sees_what_was_committed_and_a_second_writer_is_busy() {
    let rooted = Rooted::new();
    rooted.answered("m.round_trip", vec![]);
    assert_eq!(
        rooted.answered("m.together", vec![]),
        strings(&["2", "busy", "busy", "3", "3", "ran"]),
        "the reader's rows inside the writer's transaction, the second writer's statement and \
         transaction, the writer's own rows, then the reader's rows and its write after the commit"
    );
}

#[test]
fn a_fold_reads_a_statement_the_connection_keeps_open_between_pages() {
    let rooted = Rooted::new();
    assert_eq!(
        rooted.answered("m.folded", vec![Value::Int(1000)]),
        Value::Int(500_500)
    );
    assert_eq!(
        rooted.answered("m.folded", vec![Value::Int(64)]),
        Value::Int(2080)
    );
    assert_eq!(
        rooted.answered("m.folded", vec![Value::Int(1)]),
        Value::Int(1)
    );
}

#[test]
fn a_file_and_the_twin_answer_one_scenario_alike() {
    let rooted = Rooted::new();
    let on_a_file = rooted.answered("m.on_a_file", vec![]);
    assert_eq!(on_a_file, rooted.answered("m.on_the_twin", vec![]));
    assert_eq!(
        on_a_file,
        strings(&[
            "2",
            "2",
            "other",
            "other",
            "refused",
            "refused",
            "2024-03-01",
            "Ada",
            "Grace"
        ])
    );
}

#[test]
fn a_connection_an_entry_leaves_open_is_closed_when_the_entry_ends() {
    let rooted = Rooted::new();
    assert_eq!(rooted.answered("m.left_open", vec![]), Value::Int(1));
    // Closed, so the log was folded into the database and removed with its index.
    assert_eq!(files(&rooted.root()), ["left.db"]);
    assert_eq!(rooted.answered("m.left_open", vec![]), Value::Int(1));
}
