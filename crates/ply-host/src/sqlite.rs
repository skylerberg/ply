//! SQLite databases under the roots `--fs` binds: `std.sqlite`'s effect over files.
//!
//! A connection is a thread that owns its engine and takes one operation at a time, so a
//! statement waiting on another connection's lock holds no scheduler, and a fold's statement
//! stays open between its pages without leaving that thread.

use crate::fs::{FsHost, confine};
use crate::observe::{self, Read};
use crate::pool::JobOutput;
use ply_eval::host::{
    Determinism, HostAnswer, HostHandler, HostOp, HostRegistry, HostRequest, HostResource,
    HostRuntime, Linearity, MachineId,
};
use ply_eval::sqlite::{
    Control, Engine, Failure, Limits, Mode, Options, Reply, Scalar, scalars_of, unopened,
};
use ply_eval::{Diagnostic, Resource, Span, Symbol, Value, codes};
use rusqlite::OpenFlags;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

/// The effect `std.sqlite` declares, by its program-wide name.
pub const EFFECT: &str = "std.sqlite.sqlite";

/// The connections one run holds open at once.
pub const MAX_CONNECTIONS: usize = 64;

/// What the engine keeps beside a database file.
const SIDE_FILES: [&str; 3] = ["-journal", "-wal", "-shm"];

// In the order `std.sqlite` declares them.
operations! {
    what "sqlite";
    path "sqlite";
    Open = "open" / 3,
    Close = "close" / 1,
    Query = "query" / 4,
    Execute = "execute" / 4,
    Control = "control" / 2,
    Backup = "backup" / 2,
    Cursor = "cursor" / 4,
    Fetch = "fetch" / 2,
    Finish = "finish" / 1,
}

impl Op {
    fn label(self) -> &'static str {
        match self {
            Op::Open => "sqlite-open",
            Op::Close => "sqlite-close",
            Op::Query => "sqlite-query",
            Op::Execute => "sqlite-execute",
            Op::Control => "sqlite-control",
            Op::Backup => "sqlite-backup",
            Op::Cursor => "sqlite-cursor",
            Op::Fetch => "sqlite-fetch",
            Op::Finish => "sqlite-finish",
        }
    }

    pub fn declaration(self) -> HostOp {
        HostOp {
            effect: Symbol::new(EFFECT),
            op: Symbol::new(self.name()),
            resource: HostResource::Any,
            determinism: Determinism::Nondeterministic,
            linearity: Linearity::AtMostOnce,
            blocking: true,
            secrets: false,
            path: self.path(),
        }
    }
}

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

pub struct SqliteHost {
    fs: Arc<FsHost>,
    connections: Arc<Mutex<Connections>>,
}

impl SqliteHost {
    /// Databases under `fs`'s roots, their operations waiting on its pool.
    pub fn new(fs: Arc<FsHost>) -> SqliteHost {
        SqliteHost {
            fs,
            connections: Arc::new(Mutex::new(Connections::default())),
        }
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
}

pub fn registrations(host: &Arc<SqliteHost>) -> Vec<(HostOp, Arc<dyn HostHandler>)> {
    Op::ALL
        .iter()
        .map(|op| {
            let handler: Arc<dyn HostHandler> = Arc::new(Operation {
                op: *op,
                host: Arc::clone(host),
            });
            (op.declaration(), handler)
        })
        .collect()
}

pub fn register(registry: &mut HostRegistry, host: Arc<SqliteHost>) {
    for (op, handler) in registrations(&host) {
        registry.register(op, handler);
    }
}

struct Operation {
    op: Op,
    host: Arc<SqliteHost>,
}

impl HostHandler for Operation {
    fn call(&self, _: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        let span = req.span;
        let what = self.op.what();
        if req.args.len() != self.op.arity() {
            return Err(arity(self.op, req.args.len(), span));
        }
        let at = &req.atom.resource;
        let root = match self.host.fs.roots().get(at) {
            Some(root) => root.to_path_buf(),
            None => return Err(unbound(self.op, at, span)),
        };
        let connections = Arc::clone(&self.host.connections);
        let machine = req.machine;
        let job: Box<dyn FnOnce() -> JobOutput + Send> = match self.op {
            Op::Open => {
                let path = req.args[0].as_str(span, "a path")?.to_string();
                let mode = mode(&req.args[1], span)?;
                let options = Options::of(&req.args[2], span, what)?;
                Box::new(move || open(&connections, &root, &path, mode, options, machine, span))
            }
            Op::Close => {
                let id = req.args[0].as_int(span, "a connection")?;
                Box::new(move || {
                    let handle = {
                        let mut held = lock(&connections);
                        match held.open.get(&id) {
                            Some(handle) if handle.root == root => held.open.remove(&id),
                            _ => None,
                        }
                    };
                    JobOutput::Bool(
                        handle.is_some_and(|handle| asked(&handle.commands, Ask::Close).is_some()),
                    )
                })
            }
            Op::Backup => {
                let id = req.args[0].as_int(span, "a connection")?;
                let path = req.args[1].as_str(span, "a path")?.to_string();
                Box::new(move || {
                    let to = match confine(&root, &path, span) {
                        Ok(to) => to,
                        Err(refusal) => return JobOutput::Refused(refusal),
                    };
                    if std::fs::symlink_metadata(&to).is_ok() {
                        return JobOutput::Reply(Reply::failed(Failure::new(
                            "exists",
                            "",
                            format!("`{path}` is there already, and a backup writes a new file"),
                        )));
                    }
                    let reply = reply_of(&connections, &root, id, Ask::Backup(to.clone()));
                    if reply.failure.is_none() {
                        observe::wrote(machine, &to);
                    }
                    JobOutput::Reply(reply)
                })
            }
            Op::Finish => {
                let id = req.args[0].as_int(span, "a connection")?;
                Box::new(move || {
                    let done = commands_of(&connections, &root, id)
                        .and_then(|commands| asked(&commands, Ask::Finish));
                    JobOutput::Bool(matches!(done, Some(Answer::Done(true))))
                })
            }
            Op::Query | Op::Execute | Op::Control | Op::Cursor | Op::Fetch => {
                let id = req.args[0].as_int(span, "a connection")?;
                let ask = match self.op {
                    Op::Control => Ask::Control(control(&req.args[1], span)?),
                    Op::Fetch => Ask::Fetch(req.args[1].as_int(span, "a number of rows")?),
                    _ => {
                        let sql = req.args[1].as_str(span, "a statement")?.to_string();
                        let params = scalars_of(&req.args[2], span, what)?;
                        let limits = Limits::of(&req.args[3], span, what)?;
                        match self.op {
                            Op::Cursor => Ask::Cursor {
                                sql,
                                params,
                                limits,
                            },
                            _ => Ask::Run {
                                sql,
                                params,
                                limits,
                                reads_only: self.op == Op::Query,
                            },
                        }
                    }
                };
                Box::new(move || JobOutput::Reply(reply_of(&connections, &root, id, ask)))
            }
        };
        let pending = self
            .host
            .fs
            .submit(span, self.op.label(), self.op.what(), job)?;
        Ok(HostAnswer::Pending(pending))
    }
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

/// The database at `path` under `root`, opened on a thread of its own.
fn open(
    connections: &Mutex<Connections>,
    root: &Path,
    path: &str,
    mode: Mode,
    options: Options,
    machine: MachineId,
    span: Span,
) -> JobOutput {
    let target = match confine(root, path, span) {
        Ok(target) => target,
        Err(refusal) => return JobOutput::Refused(refusal),
    };
    let beside = |suffix: &str| {
        let mut name = target.clone().into_os_string();
        name.push(suffix);
        PathBuf::from(name)
    };
    // The engine opens these by name and follows a link, which would carry its writes wherever
    // the link leads.
    for suffix in SIDE_FILES {
        if std::fs::symlink_metadata(beside(suffix)).is_ok_and(|meta| meta.is_symlink()) {
            return JobOutput::Connected(Err(Failure::new(
                "unopened",
                "",
                format!(
                    "`{path}{suffix}` is a symbolic link, and the engine would write through it"
                ),
            )));
        }
    }
    if lock(connections).open.len() >= MAX_CONNECTIONS {
        return JobOutput::Connected(Err(Failure::new(
            "unopened",
            "",
            format!("a run holds {MAX_CONNECTIONS} connections open at most"),
        )));
    }
    let (commands, inbox) = channel();
    let (opened, opening) = channel();
    let at = target.clone();
    let spawned = std::thread::Builder::new()
        .name("ply-host-sqlite-connection".to_string())
        .spawn(move || match connect(&at, mode, options) {
            Err(failure) => {
                let _ = opened.send(Err(failure));
            }
            Ok(engine) => {
                let _ = opened.send(Ok(()));
                serve(engine, inbox);
            }
        });
    if let Err(e) = spawned {
        return JobOutput::Failed(format!("no thread for the connection: {e}"));
    }
    match opening.recv() {
        Ok(Ok(())) => {}
        Ok(Err(failure)) => return JobOutput::Connected(Err(failure)),
        Err(_) => return JobOutput::Failed("the connection's thread ended as it opened".into()),
    }
    if mode == Mode::ReadOnly {
        observe::read(machine, Read::File, &target);
        observe::read(machine, Read::File, &beside("-wal"));
    } else {
        observe::wrote(machine, &target);
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
    JobOutput::Connected(Ok(id))
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

fn mode(how: &Value, span: Span) -> Result<Mode, Diagnostic> {
    match constructor(how) {
        Some("ReadOnly") => Ok(Mode::ReadOnly),
        Some("ReadWrite") => Ok(Mode::ReadWrite),
        Some("Create") => Ok(Mode::Create),
        _ => Err(ill_typed(
            Op::Open,
            "none of `ReadOnly`, `ReadWrite` and `Create`",
            span,
        )),
    }
}

fn control(step: &Value, span: Span) -> Result<Control, Diagnostic> {
    match constructor(step) {
        Some("Begin") => Ok(Control::Begin),
        Some("BeginRead") => Ok(Control::BeginRead),
        Some("Commit") => Ok(Control::Commit),
        Some("Rollback") => Ok(Control::Rollback),
        _ => Err(ill_typed(
            Op::Control,
            "none of `Begin`, `BeginRead`, `Commit` and `Rollback`",
            span,
        )),
    }
}

/// A constructor's own name, past the module that declares it.
fn constructor(value: &Value) -> Option<&str> {
    match value {
        Value::Ctor { name, .. } => name.as_str().rsplit('.').next(),
        _ => None,
    }
}

fn lock(connections: &Mutex<Connections>) -> MutexGuard<'_, Connections> {
    connections.lock().unwrap_or_else(|e| e.into_inner())
}

#[cold]
fn ill_typed(op: Op, what: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("{} was handed a value it does not read", op.what()),
    )
    .primary(span, format!("this is {what}"))
}

#[cold]
fn unbound(op: Op, at: &Resource, span: Span) -> Diagnostic {
    let label = match at {
        Resource::Named(name) => name.as_str().to_string(),
        Resource::Var(v) => ply_eval::label_var_name(*v),
        Resource::Singleton => "the singleton resource".to_string(),
        Resource::Every => "every label".to_string(),
    };
    Diagnostic::error(
        codes::FS_ROOT_UNBOUND,
        format!("{} names `{label}`, and no root is bound to it", op.what()),
    )
    .primary(span, format!("`{label}` has no root"))
    .note(format!(
        "bind the directory its database lives in beside the run: `--fs {label}=<directory>`"
    ))
    .note("a resource label is the capability: where a database may live is named where the run is configured, never in the program")
}

#[cold]
fn arity(op: Op, given: usize, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!(
            "{} takes {} argument(s) and was given {given}",
            op.what(),
            op.arity()
        ),
    )
    .primary(span, "this call does not match the declaration")
}
