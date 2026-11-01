use crate::fs::{Roots, confine};
use crate::jobs::Job;
use crate::native::{found, int, int_of, parts, parts_of};
use ply_core::host::MachineId;
use ply_core::sqlite::{Control, Engine, Failure, Limits, Mode, Options, Reply, Scalar, unopened};
use ply_core::{Diagnostic, Resource, Span, Symbol, codes};
use rusqlite::OpenFlags;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

pub const MAX_CONNECTIONS: usize = 64;

const SIDE_FILES: [&str; 3] = ["-journal", "-wal", "-shm"];

enum WorkerRequest {
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

enum Response {
    Reply(Reply),
    Done(bool),
}

struct Command {
    ask: WorkerRequest,
    result: Sender<Response>,
}

struct Handle {
    root: PathBuf,
    machine: MachineId,
    commands: Sender<Command>,
}

#[derive(Default)]
struct Connections {
    last: i64,
    open: BTreeMap<i64, Handle>,
}

#[derive(Default)]
pub struct SqliteHost {
    connections: Arc<Mutex<Connections>>,
}

impl SqliteHost {
    pub fn new() -> SqliteHost {
        SqliteHost::default()
    }

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
            asked(&handle.commands, WorkerRequest::Close);
        }
    }

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
                        Err(failure) => [&[0u8][..], &encode_failure(&failure)].concat(),
                    }
                }
                None => {
                    let failure = Failure::new(
                        "unopened",
                        "",
                        format!("`{path}` passes through a symbolic link"),
                    );
                    [&[0u8][..], &encode_failure(&failure)].concat()
                }
            })
        }))
    }

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
            let closed = handle
                .is_some_and(|handle| asked(&handle.commands, WorkerRequest::Close).is_some());
            Ok(vec![u8::from(closed)])
        })
    }

    pub fn run(
        &self,
        root: PathBuf,
        id: i64,
        sql: String,
        params: Vec<Scalar>,
        limits: Limits,
        reads_only: bool,
    ) -> Job {
        self.reply_job(
            root,
            id,
            WorkerRequest::Run {
                sql,
                params,
                limits,
                reads_only,
            },
        )
    }

    pub fn control(&self, root: PathBuf, id: i64, step: Control) -> Job {
        self.reply_job(root, id, WorkerRequest::Control(step))
    }

    pub fn cursor(
        &self,
        root: PathBuf,
        id: i64,
        sql: String,
        params: Vec<Scalar>,
        limits: Limits,
    ) -> Job {
        self.reply_job(
            root,
            id,
            WorkerRequest::Cursor {
                sql,
                params,
                limits,
            },
        )
    }

    pub fn fetch(&self, root: PathBuf, id: i64, rows: i64) -> Job {
        self.reply_job(root, id, WorkerRequest::Fetch(rows))
    }

    pub fn finish(&self, root: PathBuf, id: i64) -> Job {
        let connections = Arc::clone(&self.connections);
        Box::new(move || {
            let done = commands_of(&connections, &root, id)
                .and_then(|commands| asked(&commands, WorkerRequest::Finish));
            Ok(vec![u8::from(matches!(done, Some(Response::Done(true))))])
        })
    }

    pub fn backup(
        &self,
        root: PathBuf,
        id: i64,
        path: String,
        span: Span,
    ) -> Result<Job, Diagnostic> {
        let to = placed(&root, &path, span)?;
        let connections = Arc::clone(&self.connections);
        Ok(Box::new(move || {
            let reply = match to {
                Some(to) if std::fs::symlink_metadata(&to).is_err() => {
                    reply_of(&connections, &root, id, WorkerRequest::Backup(to))
                }
                _ => Reply::failed(Failure::new(
                    "exists",
                    "",
                    format!("`{path}` is there already, and a backup writes a new file"),
                )),
            };
            Ok(encode_reply(&reply))
        }))
    }

    fn reply_job(&self, root: PathBuf, id: i64, ask: WorkerRequest) -> Job {
        let connections = Arc::clone(&self.connections);
        Box::new(move || Ok(encode_reply(&reply_of(&connections, &root, id, ask))))
    }
}

pub fn root_of(roots: &Roots, label: &str, what: &str, span: Span) -> Result<PathBuf, Diagnostic> {
    match roots.get(&Resource::Named(Symbol::new(label))) {
        Some(root) => Ok(root.to_path_buf()),
        None => Err(unbound(what, label, span)),
    }
}

pub fn decode_scalars(flat: &[u8]) -> Option<Vec<Scalar>> {
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

pub fn decode_limits(flat: &[u8]) -> Option<Limits> {
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

fn encode_scalar(scalar: &Scalar) -> Vec<u8> {
    match scalar {
        Scalar::Null => vec![0],
        Scalar::Int(n) => [&[1u8][..], &int(*n)].concat(),
        Scalar::Real(x) => [&[2u8][..], &int(ply_core::float::bits(*x) as i64)].concat(),
        Scalar::Text(text) => [&[3u8][..], text.as_bytes()].concat(),
        Scalar::Blob(blob) => [&[4u8][..], blob].concat(),
    }
}

fn encode_failure(failure: &Failure) -> Vec<u8> {
    parts(&[
        failure.kind.as_bytes(),
        failure.name.as_bytes(),
        failure.detail.as_bytes(),
    ])
}

pub fn encode_reply(reply: &Reply) -> Vec<u8> {
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
            .map(|row| listed(row.iter().map(encode_scalar).collect()))
            .collect(),
    );
    let failure = found(reply.failure.as_ref().map(encode_failure));
    parts(&[
        &columns,
        &rows,
        &[u8::from(reply.wrote)],
        &int(reply.changed),
        &int(reply.last_row_id),
        &failure,
    ])
}

fn placed(root: &Path, path: &str, span: Span) -> Result<Option<PathBuf>, Diagnostic> {
    let spelled = root.join(path);
    Ok(confine(root, path, span)?.filter(|target| *target == spelled))
}

fn commands_of(connections: &Mutex<Connections>, root: &Path, id: i64) -> Option<Sender<Command>> {
    lock(connections)
        .open
        .get(&id)
        .filter(|handle| handle.root == root)
        .map(|handle| handle.commands.clone())
}

fn asked(commands: &Sender<Command>, ask: WorkerRequest) -> Option<Response> {
    let (result, returned) = channel();
    commands.send(Command { ask, result }).ok()?;
    returned.recv().ok()
}

fn reply_of(connections: &Mutex<Connections>, root: &Path, id: i64, ask: WorkerRequest) -> Reply {
    match commands_of(connections, root, id).and_then(|commands| asked(&commands, ask)) {
        Some(Response::Reply(reply)) => reply,
        _ => Reply::failed(Failure::new(
            "closed",
            "",
            "no connection is open under this number",
        )),
    }
}

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
    let (returned, result) = channel();
    let at = target.to_path_buf();
    std::thread::Builder::new()
        .name("ply-host-sqlite-connection".to_string())
        .spawn(move || match connect(&at, mode, options) {
            Err(failure) => {
                let _ = returned.send(Err(failure));
            }
            Ok(engine) => {
                let _ = returned.send(Ok(()));
                serve(engine, inbox);
            }
        })
        .map_err(|e| Failure::new("unopened", "", format!("no thread for the connection: {e}")))?;
    match result.recv() {
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

fn serve(mut engine: Engine, commands: Receiver<Command>) {
    let mut folded = false;
    let mut next: Option<Command> = None;
    while let Some(Command { ask, result }) = next.take().or_else(|| commands.recv().ok()) {
        let said = match ask {
            WorkerRequest::Close => {
                drop(engine);
                let _ = result.send(Response::Done(true));
                return;
            }
            WorkerRequest::Run {
                sql,
                params,
                limits,
                reads_only,
            } => Response::Reply(engine.run(&sql, &params, limits, reads_only)),
            WorkerRequest::Control(step) => Response::Reply(engine.control(step)),
            WorkerRequest::Backup(to) => Response::Reply(engine.backup(&to)),
            WorkerRequest::Fetch(_) if folded => Response::Reply(Reply::default()),
            WorkerRequest::Fetch(_) => Response::Reply(Reply::failed(Failure::new(
                "closed",
                "",
                "no fold is open on this connection",
            ))),
            WorkerRequest::Finish => Response::Done(std::mem::take(&mut folded)),
            WorkerRequest::Cursor {
                sql,
                params,
                limits,
            } => {
                let mut waiting = Some(result);
                let mut opened = false;
                let mut finished = false;
                engine.run_in_pages(&sql, &params, limits, true, |page| {
                    let over = page.ended;
                    finished = !opened && page.failure.is_some();
                    opened = true;
                    if let Some(to) = waiting.take() {
                        let _ = to.send(Response::Reply(page));
                    }
                    if over {
                        return None;
                    }
                    loop {
                        match commands.recv() {
                            Err(_) => return None,
                            Ok(Command {
                                ask: WorkerRequest::Fetch(rows),
                                result,
                            }) => {
                                waiting = Some(result);
                                return Some(rows.max(0));
                            }
                            Ok(Command {
                                ask: WorkerRequest::Finish,
                                result,
                            }) => {
                                finished = true;
                                let _ = result.send(Response::Done(true));
                                return None;
                            }
                            Ok(close @ Command { ask: WorkerRequest::Close, .. }) => {
                                finished = true;
                                next = Some(close);
                                return None;
                            }
                            Ok(Command { result, .. }) => {
                                let _ = result.send(Response::Reply(Reply::failed(Failure::new(
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
        let _ = result.send(said);
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
