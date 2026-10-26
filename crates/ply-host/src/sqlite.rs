//! SQLite databases under the roots `--fs` binds, which `std.sqlite`'s real handler opens and asks
//! through its natives, each operation a job for the process's waiting pool.
//!
//! A connection is a thread that owns its engine and takes one operation at a time, so a
//! statement waiting on another connection's lock holds no scheduler, and a fold's statement
//! stays open between its pages without leaving that thread.
//!
//! What the natives hand and answer is flat. A value is its kind's byte, 0 for null, 1 an integer,
//! 2 a float and 3 text, each number in eight bytes, the most significant first, and 4 bytes,
//! then its bytes; a statement's values are parts. Limits are five integers. A failure is parts of
//! its kind, its name and its detail. A reply is parts: the columns, the rows, each parts of its
//! values, byte 1 where the statement could have written, the rows it changed, the row id it last
//! inserted, and its failure found.

use crate::fs::{Roots, confine};
use crate::jobs::Job;
use crate::native::{found, int, int_of, parts, parts_of};
use crate::observe::{self, Read};
use ply_core::host::MachineId;
use ply_core::sqlite::{Control, Engine, Failure, Limits, Mode, Options, Reply, Scalar, unopened};
use ply_core::{Diagnostic, Resource, Span, Symbol, codes};
use rusqlite::OpenFlags;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

/// The connections one run holds open at once.
pub const MAX_CONNECTIONS: usize = 64;

/// What the engine keeps beside a database file.
const SIDE_FILES: [&str; 3] = ["-journal", "-wal", "-shm"];

/// What a connection's thread is asked.
enum Ask {
    Run {
        sql: String,
        params: Vec<Scalar>,
        limits: Limits,
        reads_only: bool,
    },
    Control(Control),
    Backup(PathBuf),
    Cursor {
        sql: String,
        params: Vec<Scalar>,
        limits: Limits,
    },
    Fetch(i64),
    Finish,
    Close,
}

enum Answer {
    Reply(Reply),
    Done(bool),
}

struct Command {
    ask: Ask,
    answer: Sender<Answer>,
}

/// A connection a run holds open, under the root it was opened under.
struct Handle {
    root: PathBuf,
    machine: MachineId,
    commands: Sender<Command>,
}

/// The connections a run holds open, each under the number `sqlite.open` answered. A number is
/// never answered twice, so one that was closed names nothing afterwards.
#[derive(Default)]
struct Connections {
    last: i64,
    open: BTreeMap<i64, Handle>,
}

/// The connections a run holds open.
#[derive(Default)]
pub struct SqliteHost {
    connections: Arc<Mutex<Connections>>,
}

impl SqliteHost {
    pub fn new() -> SqliteHost {
        SqliteHost::default()
    }

    /// Closes every connection `machine` opened and did not close.
    pub fn end_machine(&self, machine: MachineId) {
        let left: Vec<Handle> = {
            let mut held = lock(&self.connections);
            let ids: Vec<i64> = held
                .open
                .iter()
                .filter(|(_, handle)| handle.machine == machine)
                .map(|(id, _)| *id)
                .collect();
            ids.into_iter()
                .filter_map(|id| held.open.remove(&id))
                .collect()
        };
        for handle in left {
            asked(&handle.commands, Ask::Close);
        }
    }

    /// The database at `path` under `root`, opened as `mode` and `options` say on a thread of its
    /// own: byte 1 and the connection's number, or byte 0 and why it did not open.
    ///
    /// # Errors
    /// `path` leaves `root`.
    pub fn open(
        &self,
        root: PathBuf,
        path: String,
        mode: Mode,
        options: Options,
        machine: MachineId,
        span: Span,
    ) -> Result<Job, Diagnostic> {
        let target = placed(&root, &path, span)?;
        let connections = Arc::clone(&self.connections);
        Ok(Box::new(move || {
            Ok(match target {
                Some(target) => {
                    match opened(&connections, &root, &target, &path, mode, options, machine) {
                        Ok(id) => [&[1u8][..], &int(id)].concat(),
                        Err(failure) => [&[0u8][..], &failure_written(&failure)].concat(),
                    }
                }
                None => {
                    let failure = Failure::new(
                        "unopened",
                        "",
                        format!("`{path}` passes through a symbolic link"),
                    );
                    [&[0u8][..], &failure_written(&failure)].concat()
                }
            })
        }))
    }

    /// Closes a connection, rolling back what it left open: byte 1, or byte 0 for a number that
    /// names none under `root`.
    pub fn close(&self, root: PathBuf, id: i64) -> Job {
        let connections = Arc::clone(&self.connections);
        Box::new(move || {
            let handle = {
                let mut held = lock(&connections);
                match held.open.get(&id) {
                    Some(handle) if handle.root == root => held.open.remove(&id),
                    _ => None,
                }
            };
            let closed = handle.is_some_and(|handle| asked(&handle.commands, Ask::Close).is_some());
            Ok(vec![u8::from(closed)])
        })
    }

    /// A statement run on a connection, reading only where `reads_only` says: its reply.
    pub fn run(
        &self,
        root: PathBuf,
        id: i64,
        sql: String,
        params: Vec<Scalar>,
        limits: Limits,
        reads_only: bool,
    ) -> Job {
        self.replying(
            root,
            id,
            Ask::Run {
                sql,
                params,
                limits,
                reads_only,
            },
        )
    }

    /// A step of a connection's transaction: its reply.
    pub fn control(&self, root: PathBuf, id: i64, step: Control) -> Job {
        self.replying(root, id, Ask::Control(step))
    }

    /// A statement that only reads, opened to be fetched a page at a time: its reply.
    pub fn cursor(
        &self,
        root: PathBuf,
        id: i64,
        sql: String,
        params: Vec<Scalar>,
        limits: Limits,
    ) -> Job {
        self.replying(
            root,
            id,
            Ask::Cursor {
                sql,
                params,
                limits,
            },
        )
    }

    /// The next `rows` rows of the connection's open statement: its reply.
    pub fn fetch(&self, root: PathBuf, id: i64, rows: i64) -> Job {
        self.replying(root, id, Ask::Fetch(rows))
    }

    /// Ends the connection's open statement: byte 1, or byte 0 where none is open.
    pub fn finish(&self, root: PathBuf, id: i64) -> Job {
        let connections = Arc::clone(&self.connections);
        Box::new(move || {
            let done = commands_of(&connections, &root, id)
                .and_then(|commands| asked(&commands, Ask::Finish));
            Ok(vec![u8::from(matches!(done, Some(Answer::Done(true))))])
        })
    }

    /// A compacted copy of the database written to `path` under `root`, which holds nothing yet:
    /// its reply.
    ///
    /// # Errors
    /// `path` leaves `root`.
    pub fn backup(
        &self,
        root: PathBuf,
        id: i64,
        path: String,
        machine: MachineId,
        span: Span,
    ) -> Result<Job, Diagnostic> {
        let to = placed(&root, &path, span)?;
        let connections = Arc::clone(&self.connections);
        Ok(Box::new(move || {
            let reply = match to {
                Some(to) if std::fs::symlink_metadata(&to).is_err() => {
                    let reply = reply_of(&connections, &root, id, Ask::Backup(to.clone()));
                    if reply.failure.is_none() {
                        observe::wrote(machine, &to);
                    }
                    reply
                }
                _ => Reply::failed(Failure::new(
                    "exists",
                    "",
                    format!("`{path}` is there already, and a backup writes a new file"),
                )),
            };
            Ok(reply_written(&reply))
        }))
    }

    fn replying(&self, root: PathBuf, id: i64, ask: Ask) -> Job {
        let connections = Arc::clone(&self.connections);
        Box::new(move || Ok(reply_written(&reply_of(&connections, &root, id, ask))))
    }
}

/// Where the databases labelled `label` lie: the root `--fs` bound to that name.
///
/// # Errors
/// No root is bound to it.
pub fn root_of(roots: &Roots, label: &str, what: &str, span: Span) -> Result<PathBuf, Diagnostic> {
    match roots.get(&Resource::Named(Symbol::new(label))) {
        Some(root) => Ok(root.to_path_buf()),
        None => Err(unbound(what, label, span)),
    }
}

/// The values `flat` writes as parts; `None` for bytes that write none.
pub fn scalars_read(flat: &[u8]) -> Option<Vec<Scalar>> {
    parts_of(flat)?
        .into_iter()
        .map(|scalar| {
            let (kind, rest) = scalar.split_first()?;
            Some(match kind {
                0 => Scalar::Null,
                1 => Scalar::Int(int_of(rest)?),
                2 => Scalar::Real(f64::from_bits(int_of(rest)? as u64)),
                3 => match std::str::from_utf8(rest) {
                    Ok(text) => Scalar::Text(text.to_string()),
                    Err(_) => Scalar::Blob(rest.to_vec()),
                },
                4 => Scalar::Blob(rest.to_vec()),
                _ => return None,
            })
        })
        .collect()
}

/// The limits `flat` writes, five integers; `None` for bytes that write none.
pub fn limits_read(flat: &[u8]) -> Option<Limits> {
    if flat.len() != 40 {
        return None;
    }
    let at = |i: usize| int_of(&flat[i * 8..i * 8 + 8]);
    Some(Limits {
        sql_bytes: at(0)?,
        params: at(1)?,
        rows: at(2)?,
        bytes: at(3)?,
        steps: at(4)?,
    })
}

fn scalar_written(scalar: &Scalar) -> Vec<u8> {
    match scalar {
        Scalar::Null => vec![0],
        Scalar::Int(n) => [&[1u8][..], &int(*n)].concat(),
        Scalar::Real(x) => [&[2u8][..], &int(ply_core::float::bits(*x) as i64)].concat(),
        Scalar::Text(text) => [&[3u8][..], text.as_bytes()].concat(),
        Scalar::Blob(blob) => [&[4u8][..], blob].concat(),
    }
}

fn failure_written(failure: &Failure) -> Vec<u8> {
    parts(&[
        failure.kind.as_bytes(),
        failure.name.as_bytes(),
        failure.detail.as_bytes(),
    ])
}

pub fn reply_written(reply: &Reply) -> Vec<u8> {
    let listed = |each: Vec<Vec<u8>>| {
        let slices: Vec<&[u8]> = each.iter().map(Vec::as_slice).collect();
        parts(&slices)
    };
    let columns = listed(
        reply
            .columns
            .iter()
            .map(|c| c.as_bytes().to_vec())
            .collect(),
    );
    let rows = listed(
        reply
            .rows
            .iter()
            .map(|row| listed(row.iter().map(scalar_written).collect()))
            .collect(),
    );
    let failure = found(reply.failure.as_ref().map(failure_written));
    parts(&[
        &columns,
        &rows,
        &[u8::from(reply.wrote)],
        &int(reply.changed),
        &int(reply.last_row_id),
        &failure,
    ])
}

/// Where `path` lies under `root`: `None` where a link along it leads anywhere but where it is
/// spelled, which a database's path may not, and where it leads nowhere.
fn placed(root: &Path, path: &str, span: Span) -> Result<Option<PathBuf>, Diagnostic> {
    let spelled = root.join(path);
    Ok(confine(root, path, span)?.filter(|target| *target == spelled))
}

/// The connection `id` names under `root`. One another root opened is not open under this one,
/// and answers as a number that names nothing does.
fn commands_of(connections: &Mutex<Connections>, root: &Path, id: i64) -> Option<Sender<Command>> {
    lock(connections)
        .open
        .get(&id)
        .filter(|handle| handle.root == root)
        .map(|handle| handle.commands.clone())
}

/// What a connection's thread answers `ask`; `None` when the thread is gone.
fn asked(commands: &Sender<Command>, ask: Ask) -> Option<Answer> {
    let (answer, answered) = channel();
    commands.send(Command { ask, answer }).ok()?;
    answered.recv().ok()
}

fn reply_of(connections: &Mutex<Connections>, root: &Path, id: i64, ask: Ask) -> Reply {
    match commands_of(connections, root, id).and_then(|commands| asked(&commands, ask)) {
        Some(Answer::Reply(reply)) => reply,
        _ => Reply::failed(Failure::new(
            "closed",
            "",
            "no connection is open under this number",
        )),
    }
}

/// The file at `path`, which its caller has confined, opened as `mode` says and handed to the
/// engine. No part of the path may be a symbolic link, and a statement waits `options.busy_ms`
/// on another connection's lock.
fn connect(path: &Path, mode: Mode, options: Options) -> Result<Engine, Failure> {
    let access = match mode {
        Mode::ReadOnly => OpenFlags::SQLITE_OPEN_READ_ONLY,
        Mode::ReadWrite => OpenFlags::SQLITE_OPEN_READ_WRITE,
        Mode::Create => OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
    };
    let flags = access
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
        | OpenFlags::SQLITE_OPEN_NOFOLLOW
        | OpenFlags::SQLITE_OPEN_EXRESCODE;
    let conn = rusqlite::Connection::open_with_flags(path, flags).map_err(|e| {
        let mut failure = unopened(e);
        // Where a root lies on this machine is not the program's to read.
        if let Some(words) = failure
            .detail
            .strip_suffix(&format!(": {}", path.display()))
        {
            failure.detail = words.to_string();
        }
        failure
    })?;
    conn.busy_timeout(Duration::from_millis(
        u64::try_from(options.busy_ms).unwrap_or(0),
    ))
    .map_err(unopened)?;
    Engine::file(conn, mode, options)
}

/// The database at `target`, `path` under `root`, opened on a thread of its own: its connection's
/// number.
fn opened(
    connections: &Mutex<Connections>,
    root: &Path,
    target: &Path,
    path: &str,
    mode: Mode,
    options: Options,
    machine: MachineId,
) -> Result<i64, Failure> {
    let beside = |suffix: &str| {
        let mut name = target.to_path_buf().into_os_string();
        name.push(suffix);
        PathBuf::from(name)
    };
    // The engine opens these by name and follows a link, which would carry its writes wherever
    // the link leads.
    for suffix in SIDE_FILES {
        if std::fs::symlink_metadata(beside(suffix)).is_ok_and(|meta| meta.is_symlink()) {
            return Err(Failure::new(
                "unopened",
                "",
                format!(
                    "`{path}{suffix}` is a symbolic link, and the engine would write through it"
                ),
            ));
        }
    }
    if lock(connections).open.len() >= MAX_CONNECTIONS {
        return Err(Failure::new(
            "unopened",
            "",
            format!("a run holds {MAX_CONNECTIONS} connections open at most"),
        ));
    }
    let (commands, inbox) = channel();
    let (answered, answer) = channel();
    let at = target.to_path_buf();
    std::thread::Builder::new()
        .name("ply-host-sqlite-connection".to_string())
        .spawn(move || match connect(&at, mode, options) {
            Err(failure) => {
                let _ = answered.send(Err(failure));
            }
            Ok(engine) => {
                let _ = answered.send(Ok(()));
                serve(engine, inbox);
            }
        })
        .map_err(|e| Failure::new("unopened", "", format!("no thread for the connection: {e}")))?;
    match answer.recv() {
        Ok(Ok(())) => {}
        Ok(Err(failure)) => return Err(failure),
        Err(_) => {
            return Err(Failure::new(
                "unopened",
                "",
                "the connection's thread ended as it opened",
            ));
        }
    }
    if mode == Mode::ReadOnly {
        observe::read(machine, Read::File, target);
        observe::read(machine, Read::File, &beside("-wal"));
    } else {
        observe::wrote(machine, target);
        for suffix in SIDE_FILES {
            observe::wrote(machine, &beside(suffix));
        }
    }
    let mut held = lock(connections);
    held.last += 1;
    let id = held.last;
    held.open.insert(
        id,
        Handle {
            root: root.to_path_buf(),
            machine,
            commands,
        },
    );
    Ok(id)
}

/// A connection's thread: one operation at a time until it is closed or its run lets go of it.
fn serve(mut engine: Engine, commands: Receiver<Command>) {
    // Whether a fold's statement ran to its end and has not been finished.
    let mut folded = false;
    let mut next: Option<Command> = None;
    while let Some(Command { ask, answer }) = next.take().or_else(|| commands.recv().ok()) {
        let said = match ask {
            Ask::Close => {
                drop(engine);
                let _ = answer.send(Answer::Done(true));
                return;
            }
            Ask::Run {
                sql,
                params,
                limits,
                reads_only,
            } => Answer::Reply(engine.run(&sql, &params, limits, reads_only)),
            Ask::Control(step) => Answer::Reply(engine.control(step)),
            Ask::Backup(to) => Answer::Reply(engine.backup(&to)),
            Ask::Fetch(_) if folded => Answer::Reply(Reply::default()),
            Ask::Fetch(_) => Answer::Reply(Reply::failed(Failure::new(
                "closed",
                "",
                "no fold is open on this connection",
            ))),
            Ask::Finish => Answer::Done(std::mem::take(&mut folded)),
            Ask::Cursor {
                sql,
                params,
                limits,
            } => {
                let mut waiting = Some(answer);
                let mut opened = false;
                let mut finished = false;
                engine.paged(&sql, &params, limits, true, |page| {
                    let over = page.ended;
                    // A statement that did not open leaves no fold to finish.
                    finished = !opened && page.failure.is_some();
                    opened = true;
                    if let Some(to) = waiting.take() {
                        let _ = to.send(Answer::Reply(page));
                    }
                    if over {
                        return None;
                    }
                    loop {
                        match commands.recv() {
                            Err(_) => return None,
                            Ok(Command {
                                ask: Ask::Fetch(rows),
                                answer,
                            }) => {
                                waiting = Some(answer);
                                return Some(rows.max(0));
                            }
                            Ok(Command {
                                ask: Ask::Finish,
                                answer,
                            }) => {
                                finished = true;
                                let _ = answer.send(Answer::Done(true));
                                return None;
                            }
                            Ok(close @ Command { ask: Ask::Close, .. }) => {
                                finished = true;
                                next = Some(close);
                                return None;
                            }
                            Ok(Command { answer, .. }) => {
                                let _ = answer.send(Answer::Reply(Reply::failed(Failure::new(
                                    "refused",
                                    "",
                                    "a fold is reading this connection: nothing else runs on it until the fold ends",
                                ))));
                            }
                        }
                    }
                });
                folded = !finished;
                continue;
            }
        };
        let _ = answer.send(said);
    }
}

fn lock(connections: &Mutex<Connections>) -> MutexGuard<'_, Connections> {
    connections.lock().unwrap_or_else(|e| e.into_inner())
}

#[cold]
fn unbound(what: &str, label: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::FS_ROOT_UNBOUND,
        format!("{what} names `{label}`, and no root is bound to it"),
    )
    .primary(span, format!("`{label}` has no root"))
    .note(format!(
        "bind the directory its database lives in beside the run: `--fs {label}=<directory>`"
    ))
    .note("a resource label is the capability: where a database may live is named where the run is configured, never in the program")
}
