use super::{Nondeterministic, asked, handed, waited, with_host_facilities};
use crate::c::{MachineNative, runtime_native};
use crate::native::{Addr, I64, TakenBytes};
use crate::rt::{Ctx, borrowed};
use ply_core::sqlite::{Control, FUNCTIONS, Limits, Mode, Options, Scalar, run_on_image};
use ply_host::native::{found, parts};
use ply_host::sqlite::{SqliteHost, decode_limits, decode_scalars, encode_reply, root_of};
use std::path::PathBuf;
use std::sync::Arc;

pub(super) fn natives() -> Vec<MachineNative> {
    vec![
        runtime_native(
            "sqlite_run",
            rt_sqlite_run as unsafe extern "C" fn(_, _, _, _, _, _, _, _, _, _, _) -> _,
        ),
        runtime_native(
            "sqlite_functions",
            rt_sqlite_functions as unsafe extern "C" fn(_) -> _,
        ),
        runtime_native(
            "sqlite_open",
            rt_sqlite_open as unsafe extern "C" fn(_, _, _, _, _, _, _, _, _) -> _,
        ),
        runtime_native(
            "sqlite_close",
            rt_sqlite_close as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
        runtime_native(
            "sqlite_query",
            rt_sqlite_query as unsafe extern "C" fn(_, _, _, _, _, _, _, _, _, _) -> _,
        ),
        runtime_native(
            "sqlite_execute",
            rt_sqlite_execute as unsafe extern "C" fn(_, _, _, _, _, _, _, _, _, _) -> _,
        ),
        runtime_native(
            "sqlite_cursor",
            rt_sqlite_cursor as unsafe extern "C" fn(_, _, _, _, _, _, _, _, _, _) -> _,
        ),
        runtime_native(
            "sqlite_control",
            rt_sqlite_control as unsafe extern "C" fn(_, _, _, _, _) -> _,
        ),
        runtime_native(
            "sqlite_backup",
            rt_sqlite_backup as unsafe extern "C" fn(_, _, _, _, _, _) -> _,
        ),
        runtime_native(
            "sqlite_fetch",
            rt_sqlite_fetch as unsafe extern "C" fn(_, _, _, _, _) -> _,
        ),
        runtime_native(
            "sqlite_finish",
            rt_sqlite_finish as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
    ]
}

#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn rt_sqlite_run(
    ctx: *mut Ctx,
    image_addr: Addr,
    image_len: I64,
    sql_addr: Addr,
    sql_len: I64,
    params_addr: Addr,
    params_len: I64,
    reads_only: I64,
    strict: I64,
    limits_addr: Addr,
    limits_len: I64,
) -> TakenBytes {
    let c = unsafe { &mut *ctx };
    let Some((sql, params, limits)) = statement(
        c,
        "sqlite_run",
        unsafe { borrowed(sql_addr, sql_len) },
        unsafe { borrowed(params_addr, params_len) },
        unsafe { borrowed(limits_addr, limits_len) },
    ) else {
        return TakenBytes(0);
    };
    let (reply, left) = run_on_image(
        unsafe { borrowed(image_addr, image_len) },
        &sql,
        &params,
        limits,
        reads_only.0 != 0,
        strict.0 != 0,
    );
    c.given(parts(&[&found(left), &encode_reply(&reply)]))
}

pub unsafe extern "C" fn rt_sqlite_functions(ctx: *mut Ctx) -> TakenBytes {
    let names: Vec<&[u8]> = FUNCTIONS.iter().map(|name| name.as_bytes()).collect();
    unsafe { &mut *ctx }.given(parts(&names))
}

fn rooted(c: &mut Ctx, label: &[u8], what: &'static str) -> Option<(Arc<SqliteHost>, PathBuf)> {
    let label = c.text(label)?;
    let span = c.site();
    let found = with_host_facilities(c, |facilities| {
        root_of(facilities.roots(), label, what, span)
            .map(|root| (Arc::clone(facilities.sqlite_host()), root))
    })?;
    match found {
        Ok(found) => Some(found),
        Err(d) => {
            c.fail(d);
            None
        }
    }
}

fn statement(
    c: &mut Ctx,
    native: &str,
    sql: &[u8],
    params: &[u8],
    limits: &[u8],
) -> Option<(String, Vec<Scalar>, Limits)> {
    let sql = c.text(sql)?.to_string();
    let params = handed(c, native, "a statement's values", decode_scalars(params))?;
    let limits = handed(c, native, "a statement's limits", decode_limits(limits))?;
    Some((sql, params, limits))
}

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
    asked(
        ctx,
        "sqlite_open",
        Nondeterministic,
        I64(0),
        |c: &mut Ctx| {
            let what = "`sqlite.open`";
            let Some((sqlite_host, root)) = rooted(c, unsafe { borrowed(label_at, label_n) }, what)
            else {
                return I64(0);
            };
            let Some(path) = c.text(unsafe { borrowed(path_at, path_n) }) else {
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
            let job = sqlite_host.open(root, path, mode, options, c.id, c.site());
            waited(c, "sqlite_open", what, job)
        },
    )
}

pub unsafe extern "C" fn rt_sqlite_close(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    conn: I64,
) -> I64 {
    asked(
        ctx,
        "sqlite_close",
        Nondeterministic,
        I64(0),
        |c: &mut Ctx| {
            let what = "`sqlite.close`";
            let Some((sqlite_host, root)) = rooted(c, unsafe { borrowed(label_at, label_n) }, what)
            else {
                return I64(0);
            };
            waited(c, "sqlite_close", what, Ok(sqlite_host.close(root, conn.0)))
        },
    )
}

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
    asked(ctx, native, Nondeterministic, I64(0), |c: &mut Ctx| {
        let Some((sqlite_host, root)) = rooted(c, unsafe { borrowed(label.0, label.1) }, what)
        else {
            return I64(0);
        };
        let Some((sql, params, limits)) = statement(
            c,
            native,
            unsafe { borrowed(sql.0, sql.1) },
            unsafe { borrowed(params.0, params.1) },
            unsafe { borrowed(limits.0, limits.1) },
        ) else {
            return I64(0);
        };
        let job = if cursor {
            sqlite_host.cursor(root, conn.0, sql, params, limits)
        } else {
            sqlite_host.run(root, conn.0, sql, params, limits, reads_only)
        };
        waited(c, native, what, Ok(job))
    })
}

#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn rt_sqlite_query(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    conn: I64,
    sql_addr: Addr,
    sql_len: I64,
    params_addr: Addr,
    params_len: I64,
    limits_addr: Addr,
    limits_len: I64,
) -> I64 {
    ran(
        ctx,
        "sqlite_query",
        "`sqlite.query`",
        (label_at, label_n),
        conn,
        (sql_addr, sql_len),
        (params_addr, params_len),
        (limits_addr, limits_len),
        true,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn rt_sqlite_execute(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    conn: I64,
    sql_addr: Addr,
    sql_len: I64,
    params_addr: Addr,
    params_len: I64,
    limits_addr: Addr,
    limits_len: I64,
) -> I64 {
    ran(
        ctx,
        "sqlite_execute",
        "`sqlite.execute`",
        (label_at, label_n),
        conn,
        (sql_addr, sql_len),
        (params_addr, params_len),
        (limits_addr, limits_len),
        false,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn rt_sqlite_cursor(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    conn: I64,
    sql_addr: Addr,
    sql_len: I64,
    params_addr: Addr,
    params_len: I64,
    limits_addr: Addr,
    limits_len: I64,
) -> I64 {
    ran(
        ctx,
        "sqlite_cursor",
        "`sqlite.cursor`",
        (label_at, label_n),
        conn,
        (sql_addr, sql_len),
        (params_addr, params_len),
        (limits_addr, limits_len),
        true,
        true,
    )
}

pub unsafe extern "C" fn rt_sqlite_control(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    conn: I64,
    step: I64,
) -> I64 {
    asked(
        ctx,
        "sqlite_control",
        Nondeterministic,
        I64(0),
        |c: &mut Ctx| {
            let what = "`sqlite.control`";
            let Some((sqlite_host, root)) = rooted(c, unsafe { borrowed(label_at, label_n) }, what)
            else {
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
                Ok(sqlite_host.control(root, conn.0, step)),
            )
        },
    )
}

pub unsafe extern "C" fn rt_sqlite_backup(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    conn: I64,
    path_at: Addr,
    path_n: I64,
) -> I64 {
    asked(
        ctx,
        "sqlite_backup",
        Nondeterministic,
        I64(0),
        |c: &mut Ctx| {
            let what = "`sqlite.backup`";
            let Some((sqlite_host, root)) = rooted(c, unsafe { borrowed(label_at, label_n) }, what)
            else {
                return I64(0);
            };
            let Some(path) = c.text(unsafe { borrowed(path_at, path_n) }) else {
                return I64(0);
            };
            let job = sqlite_host.backup(root, conn.0, path.to_string(), c.site());
            waited(c, "sqlite_backup", what, job)
        },
    )
}

pub unsafe extern "C" fn rt_sqlite_fetch(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    conn: I64,
    rows: I64,
) -> I64 {
    asked(
        ctx,
        "sqlite_fetch",
        Nondeterministic,
        I64(0),
        |c: &mut Ctx| {
            let what = "`sqlite.fetch`";
            let Some((sqlite_host, root)) = rooted(c, unsafe { borrowed(label_at, label_n) }, what)
            else {
                return I64(0);
            };
            waited(
                c,
                "sqlite_fetch",
                what,
                Ok(sqlite_host.fetch(root, conn.0, rows.0)),
            )
        },
    )
}

pub unsafe extern "C" fn rt_sqlite_finish(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    conn: I64,
) -> I64 {
    asked(
        ctx,
        "sqlite_finish",
        Nondeterministic,
        I64(0),
        |c: &mut Ctx| {
            let what = "`sqlite.finish`";
            let Some((sqlite_host, root)) = rooted(c, unsafe { borrowed(label_at, label_n) }, what)
            else {
                return I64(0);
            };
            waited(
                c,
                "sqlite_finish",
                what,
                Ok(sqlite_host.finish(root, conn.0)),
            )
        },
    )
}
