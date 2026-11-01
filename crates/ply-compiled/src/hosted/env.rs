use super::{Nondeterministic, if_host_allowed, text_at};
use crate::c::{MachineNative, runtime_native};
use crate::native::{Addr, I64, TakenBytes};
use crate::rt::Ctx;
use ply_host::native::{found, parts};

pub(super) fn natives() -> Vec<MachineNative> {
    vec![
        runtime_native(
            "env_variable",
            rt_env_variable as unsafe extern "C" fn(_, _, _) -> _,
        ),
        runtime_native(
            "env_variables",
            rt_env_variables as unsafe extern "C" fn(_) -> _,
        ),
        runtime_native(
            "env_working_directory",
            rt_env_working_directory as unsafe extern "C" fn(_) -> _,
        ),
    ]
}

pub unsafe extern "C" fn rt_env_variable(ctx: *mut Ctx, p: Addr, n: I64) -> TakenBytes {
    if_host_allowed(ctx, "env_variable", Nondeterministic, TakenBytes(0), |c| {
        let Some(name) = text_at(c, p, n) else {
            return TakenBytes(0);
        };
        c.given(found(std::env::var(name).ok()))
    })
}

pub unsafe extern "C" fn rt_env_variables(ctx: *mut Ctx) -> TakenBytes {
    if_host_allowed(ctx, "env_variables", Nondeterministic, TakenBytes(0), |c| {
        let variables: Vec<Vec<u8>> = std::env::vars_os()
            .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
            .map(|(name, value)| parts(&[name.as_bytes(), value.as_bytes()]))
            .collect();
        let variables: Vec<&[u8]> = variables.iter().map(Vec::as_slice).collect();
        c.given(parts(&variables))
    })
}

pub unsafe extern "C" fn rt_env_working_directory(ctx: *mut Ctx) -> TakenBytes {
    if_host_allowed(
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
