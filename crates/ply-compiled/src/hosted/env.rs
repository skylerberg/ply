//! `std.env`'s natives: the variables and the working directory of the process, which every run
//! may read.

use super::{Nondeterministic, asked, text_at};
use crate::c::{MachineNative, machine_native};
use crate::native::{Addr, I64, TakenBytes};
use crate::rt::Ctx;
use ply_host::native::{found, parts};

pub(super) fn natives() -> Vec<MachineNative> {
    vec![
        machine_native(
            "env_variable",
            rt_env_variable as unsafe extern "C" fn(_, _, _) -> _,
        ),
        machine_native(
            "env_variables",
            rt_env_variables as unsafe extern "C" fn(_) -> _,
        ),
        machine_native(
            "env_working_directory",
            rt_env_working_directory as unsafe extern "C" fn(_) -> _,
        ),
    ]
}

/// The value of the variable whose name is at `p`, found where it is set and UTF-8.
pub unsafe extern "C" fn rt_env_variable(ctx: *mut Ctx, p: Addr, n: I64) -> TakenBytes {
    asked(ctx, "env_variable", Nondeterministic, TakenBytes(0), |c| {
        let Some(name) = text_at(c, p, n) else {
            return TakenBytes(0);
        };
        c.given(found(std::env::var(name).ok()))
    })
}

/// Every variable whose name and value are UTF-8, each its name and value as parts, as parts.
pub unsafe extern "C" fn rt_env_variables(ctx: *mut Ctx) -> TakenBytes {
    asked(ctx, "env_variables", Nondeterministic, TakenBytes(0), |c| {
        let variables: Vec<Vec<u8>> = std::env::vars_os()
            .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
            .map(|(name, value)| parts(&[name.as_bytes(), value.as_bytes()]))
            .collect();
        let variables: Vec<&[u8]> = variables.iter().map(Vec::as_slice).collect();
        c.given(parts(&variables))
    })
}

/// The directory the process runs in; `.` where the system does not say.
pub unsafe extern "C" fn rt_env_working_directory(ctx: *mut Ctx) -> TakenBytes {
    asked(
        ctx,
        "env_working_directory",
        Nondeterministic,
        TakenBytes(0),
        |c| {
            let directory = std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| ".".to_string());
            c.given(directory.into_bytes())
        },
    )
}
