//! `std.sqlite`'s natives: the run's databases, each under the root its label names, every
//! operation a ticket of a job on the process's waiting pool, and the twin's statement over an
//! image, which reaches no host. What they are handed and answer is flat as `ply_host::sqlite`
//! reads and writes it.

use super::{WRITES, asked, from_host, handed, waited};
use crate::c::{MachineNative, machine_native};
use crate::native::{Addr, I64, TakenBytes};
use crate::rt::{Ctx, lent};
use ply_core::sqlite::{Control, FUNCTIONS, Limits, Mode, Options, Scalar, run_over};
use ply_host::native::{found, parts};
use ply_host::sqlite::{SqliteHost, limits_read, reply_written, root_of, scalars_read};
use std::path::PathBuf;
use std::sync::Arc;

pub(super) fn natives() -> Vec<MachineNative> {
    vec![
        machine_native(
            "sqlite_run",
            rt_sqlite_run as unsafe extern "C" fn(_, _, _, _, _, _, _, _, _, _, _) -> _,
        ),
        machine_native(
            "sqlite_functions",
            rt_sqlite_functions as unsafe extern "C" fn(_) -> _,
        ),
        machine_native(
            "sqlite_open",
            rt_sqlite_open as unsafe extern "C" fn(_, _, _, _, _, _, _, _, _) -> _,
        ),
        machine_native(
            "sqlite_close",
            rt_sqlite_close as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
        machine_native(
            "sqlite_query",
            rt_sqlite_query as unsafe extern "C" fn(_, _, _, _, _, _, _, _, _, _) -> _,
        ),
        machine_native(
            "sqlite_execute",
            rt_sqlite_execute as unsafe extern "C" fn(_, _, _, _, _, _, _, _, _, _) -> _,
        ),
        machine_native(
            "sqlite_cursor",
            rt_sqlite_cursor as unsafe extern "C" fn(_, _, _, _, _, _, _, _, _, _) -> _,
        ),
        machine_native(
            "sqlite_control",
            rt_sqlite_control as unsafe extern "C" fn(_, _, _, _, _) -> _,
        ),
        machine_native(
            "sqlite_backup",
            rt_sqlite_backup as unsafe extern "C" fn(_, _, _, _, _, _) -> _,
        ),
        machine_native(
            "sqlite_fetch",
            rt_sqlite_fetch as unsafe extern "C" fn(_, _, _, _, _) -> _,
        ),
        machine_native(
            "sqlite_finish",
            rt_sqlite_finish as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
    ]
}

/// One statement over the database the image at `image_at` holds, as the twin runs it: parts of
/// the image it left, found where it wrote and kept what it wrote, and its reply. A function of
/// what it is handed, so it asks the host nothing.
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn rt_sqlite_run(
    ctx: *mut Ctx,
    image_at: Addr,
    image_n: I64,
    sql_at: Addr,
    sql_n: I64,
    params_at: Addr,
    params_n: I64,
    reads_only: I64,
    strict: I64,
    limits_at: Addr,
    limits_n: I64,
) -> TakenBytes {
    let c = unsafe { &mut *ctx };
    let Some((sql, params, limits)) = statement(
        c,
        "sqlite_run",
        unsafe { lent(sql_at, sql_n) },
        unsafe { lent(params_at, params_n) },
        unsafe { lent(limits_at, limits_n) },
    ) else {
        return TakenBytes(0);
    };
    let (reply, left) = run_over(
        unsafe { lent(image_at, image_n) },
        &sql,
        &params,
        limits,
        reads_only.0 != 0,
        strict.0 != 0,
    );
    c.given(parts(&[&found(left), &reply_written(&reply)]))
}

/// The SQL functions a statement may call, in order, as parts.
pub unsafe extern "C" fn rt_sqlite_functions(ctx: *mut Ctx) -> TakenBytes {
    let names: Vec<&[u8]> = FUNCTIONS.iter().map(|name| name.as_bytes()).collect();
    unsafe { &mut *ctx }.given(parts(&names))
}

/// The run's databases and the root the label at `label` names; `None`, with the run failed,
/// where no root is bound to it.
fn rooted(c: &mut Ctx, label: &[u8], what: &'static str) -> Option<(Arc<SqliteHost>, PathBuf)> {
    let label = c.text(label)?;
    let span = c.site();
    let found = from_host(c, |facilities| {
        root_of(facilities.roots(), label, what, span)
            .map(|root| (Arc::clone(facilities.databases()), root))
    })?;
    match found {
        Ok(found) => Some(found),
        Err(d) => {
            c.fail(d);
            None
        }
    }
}

/// A statement's text, values and limits, as the real handler hands them.
fn statement(
    c: &mut Ctx,
    native: &str,
    sql: &[u8],
    params: &[u8],
    limits: &[u8],
) -> Option<(String, Vec<Scalar>, Limits)> {
    let sql = c.text(sql)?.to_string();
    let params = handed(c, native, "a statement's values", scalars_read(params))?;
    let limits = handed(c, native, "a statement's limits", limits_read(limits))?;
    Some((sql, params, limits))
}

/// The ticket of the database at the path at `path_at` opened under the root the label names:
/// the connection's number, or why it did not open.
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn rt_sqlite_open(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    path_at: Addr,
    path_n: I64,
    mode: I64,
    strict: I64,
    busy_ms: I64,
    statements: I64,
) -> I64 {
    asked(ctx, "sqlite_open", WRITES, I64(0), |c: &mut Ctx| {
        let what = "`sqlite.open`";
        let Some((databases, root)) = rooted(c, unsafe { lent(label_at, label_n) }, what) else {
            return I64(0);
        };
        let Some(path) = c.text(unsafe { lent(path_at, path_n) }) else {
            return I64(0);
        };
        let path = path.to_string();
        let mode = match mode.0 {
            0 => Some(Mode::ReadOnly),
            1 => Some(Mode::ReadWrite),
            2 => Some(Mode::Create),
            _ => None,
        };
        let Some(mode) = handed(c, "sqlite_open", "a mode", mode) else {
            return I64(0);
        };
        let options = Options {
            strict: strict.0 != 0,
            busy_ms: busy_ms.0,
            statements: statements.0,
        };
        let job = databases.open(root, path, mode, options, c.id, c.site());
        waited(c, "sqlite_open", what, job)
    })
}

/// The ticket of a connection closed: byte 1, or byte 0 for a number that names none.
pub unsafe extern "C" fn rt_sqlite_close(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    conn: I64,
) -> I64 {
    asked(ctx, "sqlite_close", WRITES, I64(0), |c: &mut Ctx| {
        let what = "`sqlite.close`";
        let Some((databases, root)) = rooted(c, unsafe { lent(label_at, label_n) }, what) else {
            return I64(0);
        };
        waited(c, "sqlite_close", what, Ok(databases.close(root, conn.0)))
    })
}

/// The ticket of a statement run on a connection: its reply. `reads_only` refuses one that could
/// write, and `cursor` opens it to be fetched a page at a time.
#[allow(clippy::too_many_arguments)]
fn ran(
    ctx: *mut Ctx,
    native: &'static str,
    what: &'static str,
    label: (Addr, I64),
    conn: I64,
    sql: (Addr, I64),
    params: (Addr, I64),
    limits: (Addr, I64),
    reads_only: bool,
    cursor: bool,
) -> I64 {
    asked(ctx, native, WRITES, I64(0), |c: &mut Ctx| {
        let Some((databases, root)) = rooted(c, unsafe { lent(label.0, label.1) }, what) else {
            return I64(0);
        };
        let Some((sql, params, limits)) = statement(
            c,
            native,
            unsafe { lent(sql.0, sql.1) },
            unsafe { lent(params.0, params.1) },
            unsafe { lent(limits.0, limits.1) },
        ) else {
            return I64(0);
        };
        let job = if cursor {
            databases.cursor(root, conn.0, sql, params, limits)
        } else {
            databases.run(root, conn.0, sql, params, limits, reads_only)
        };
        waited(c, native, what, Ok(job))
    })
}

/// The ticket of a statement that only reads: its reply.
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn rt_sqlite_query(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    conn: I64,
    sql_at: Addr,
    sql_n: I64,
    params_at: Addr,
    params_n: I64,
    limits_at: Addr,
    limits_n: I64,
) -> I64 {
    ran(
        ctx,
        "sqlite_query",
        "`sqlite.query`",
        (label_at, label_n),
        conn,
        (sql_at, sql_n),
        (params_at, params_n),
        (limits_at, limits_n),
        true,
        false,
    )
}

/// The ticket of a statement that may write: its reply.
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn rt_sqlite_execute(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    conn: I64,
    sql_at: Addr,
    sql_n: I64,
    params_at: Addr,
    params_n: I64,
    limits_at: Addr,
    limits_n: I64,
) -> I64 {
    ran(
        ctx,
        "sqlite_execute",
        "`sqlite.execute`",
        (label_at, label_n),
        conn,
        (sql_at, sql_n),
        (params_at, params_n),
        (limits_at, limits_n),
        false,
        false,
    )
}

/// The ticket of a statement that only reads, opened to be fetched a page at a time: its reply.
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn rt_sqlite_cursor(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    conn: I64,
    sql_at: Addr,
    sql_n: I64,
    params_at: Addr,
    params_n: I64,
    limits_at: Addr,
    limits_n: I64,
) -> I64 {
    ran(
        ctx,
        "sqlite_cursor",
        "`sqlite.cursor`",
        (label_at, label_n),
        conn,
        (sql_at, sql_n),
        (params_at, params_n),
        (limits_at, limits_n),
        true,
        true,
    )
}

/// The ticket of a step of a connection's transaction: its reply.
pub unsafe extern "C" fn rt_sqlite_control(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    conn: I64,
    step: I64,
) -> I64 {
    asked(ctx, "sqlite_control", WRITES, I64(0), |c: &mut Ctx| {
        let what = "`sqlite.control`";
        let Some((databases, root)) = rooted(c, unsafe { lent(label_at, label_n) }, what) else {
            return I64(0);
        };
        let step = match step.0 {
            0 => Some(Control::Begin),
            1 => Some(Control::BeginRead),
            2 => Some(Control::Commit),
            3 => Some(Control::Rollback),
            _ => None,
        };
        let Some(step) = handed(c, "sqlite_control", "a transaction's step", step) else {
            return I64(0);
        };
        waited(
            c,
            "sqlite_control",
            what,
            Ok(databases.control(root, conn.0, step)),
        )
    })
}

/// The ticket of a compacted copy of a connection's database written to the path at `path_at`:
/// its reply.
pub unsafe extern "C" fn rt_sqlite_backup(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    conn: I64,
    path_at: Addr,
    path_n: I64,
) -> I64 {
    asked(ctx, "sqlite_backup", WRITES, I64(0), |c: &mut Ctx| {
        let what = "`sqlite.backup`";
        let Some((databases, root)) = rooted(c, unsafe { lent(label_at, label_n) }, what) else {
            return I64(0);
        };
        let Some(path) = c.text(unsafe { lent(path_at, path_n) }) else {
            return I64(0);
        };
        let job = databases.backup(root, conn.0, path.to_string(), c.site());
        waited(c, "sqlite_backup", what, job)
    })
}

/// The ticket of the next `rows` rows of a connection's open statement: its reply.
pub unsafe extern "C" fn rt_sqlite_fetch(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    conn: I64,
    rows: I64,
) -> I64 {
    asked(ctx, "sqlite_fetch", WRITES, I64(0), |c: &mut Ctx| {
        let what = "`sqlite.fetch`";
        let Some((databases, root)) = rooted(c, unsafe { lent(label_at, label_n) }, what) else {
            return I64(0);
        };
        waited(
            c,
            "sqlite_fetch",
            what,
            Ok(databases.fetch(root, conn.0, rows.0)),
        )
    })
}

/// The ticket of a connection's open statement ended: byte 1, or byte 0 where none is open.
pub unsafe extern "C" fn rt_sqlite_finish(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    conn: I64,
) -> I64 {
    asked(ctx, "sqlite_finish", WRITES, I64(0), |c: &mut Ctx| {
        let what = "`sqlite.finish`";
        let Some((databases, root)) = rooted(c, unsafe { lent(label_at, label_n) }, what) else {
            return I64(0);
        };
        waited(c, "sqlite_finish", what, Ok(databases.finish(root, conn.0)))
    })
}
