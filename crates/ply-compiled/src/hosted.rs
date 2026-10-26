//! The natives the real handlers of the standard library's host effects call (`host for`): each asks
//! the machine the program runs on and answers machine data, integers or bytes the effect's module
//! reads into its own types. Each refuses where a run may not reach the host, inside a `simulate`
//! region or in a test the search re-runs, as a host operation does.
//!
//! A native answering structured text writes one of two flat forms. Told: byte 1 and then the
//! text, or byte 0 and then why the system said nothing. Found: byte 1 and then the text, or the
//! one byte 0.

use crate::c::{MachineNative, machine_native};
use crate::native::{I64, TakenBytes};
use crate::rt::Ctx;
use ply_core::host::{Determinism, err_host_in_search};
use ply_core::{EffectAtom, Mode, Resource, err_host_in_simulation};

/// The effect every native here performs, under its program-wide name (`std.internal.native`).
const NATIVE: &str = "std.internal.native.native";

/// Every native here, under the name its module declares it by.
pub fn natives() -> Vec<MachineNative> {
    vec![
        machine_native("os_system", rt_os_system as unsafe extern "C" fn(_) -> _),
        machine_native("os_machine", rt_os_machine as unsafe extern "C" fn(_) -> _),
        machine_native("os_uname", rt_os_uname as unsafe extern "C" fn(_) -> _),
        machine_native(
            "os_hostname",
            rt_os_hostname as unsafe extern "C" fn(_) -> _,
        ),
        machine_native("os_cpus", rt_os_cpus as unsafe extern "C" fn(_) -> _),
        machine_native("os_uid", rt_os_uid as unsafe extern "C" fn(_) -> _),
        machine_native(
            "os_user_name",
            rt_os_user_name as unsafe extern "C" fn(_, _) -> _,
        ),
        machine_native("os_pid", rt_os_pid as unsafe extern "C" fn(_) -> _),
        machine_native(
            "os_executable",
            rt_os_executable as unsafe extern "C" fn(_) -> _,
        ),
        machine_native(
            "os_big_endian",
            rt_os_big_endian as unsafe extern "C" fn(_) -> _,
        ),
        machine_native(
            "os_pointer_bits",
            rt_os_pointer_bits as unsafe extern "C" fn(_) -> _,
        ),
    ]
}

impl Ctx {
    /// Whether the native `native` may ask the host now. Where it may, the run has read the machine,
    /// as a nondeterministic host operation's answer reads it: its pass answers only for the binding
    /// it ran under. Where it may not, the run fails as a host operation's would, and the native
    /// answers nothing it reads.
    fn reaches_host(&mut self, native: &'static str) -> bool {
        let operation = format!("the native `{native}`");
        if let Some(region) = crate::simulate::seeded_region(self) {
            let span = self.site();
            self.fail(err_host_in_simulation(span, &operation, region));
            return false;
        }
        if self.re_executed {
            let span = self.site();
            self.fail(err_host_in_search(span, &operation, native));
            return false;
        }
        self.host_use.record(
            &EffectAtom::new(NATIVE, Resource::Singleton, Mode::Read),
            Determinism::Nondeterministic,
        );
        true
    }

    fn given(&mut self, bytes: Vec<u8>) -> TakenBytes {
        TakenBytes(self.give(bytes).0)
    }
}

/// `text` told: byte 1 and its bytes, or byte 0 and the system's error.
fn told(text: std::io::Result<String>) -> Vec<u8> {
    match text {
        Ok(text) => [&[1u8][..], text.as_bytes()].concat(),
        Err(e) => [&[0u8][..], e.to_string().as_bytes()].concat(),
    }
}

/// `text` found: byte 1 and its bytes, or byte 0 alone.
fn found(text: Option<String>) -> Vec<u8> {
    match text {
        Some(text) => [&[1u8][..], text.as_bytes()].concat(),
        None => vec![0],
    }
}

/// What `answer` answers, where the native `native` may ask the host; `refused` where it may not.
fn asked<R>(
    ctx: *mut Ctx,
    native: &'static str,
    refused: R,
    answer: impl FnOnce(&mut Ctx) -> R,
) -> R {
    let c = unsafe { &mut *ctx };
    if c.reaches_host(native) {
        answer(c)
    } else {
        refused
    }
}

/// The name the toolchain gives the system this binary was built for.
pub unsafe extern "C" fn rt_os_system(ctx: *mut Ctx) -> TakenBytes {
    asked(ctx, "os_system", TakenBytes(0), |c| {
        c.given(std::env::consts::OS.as_bytes().to_vec())
    })
}

/// The name the toolchain gives the architecture this binary was built for.
pub unsafe extern "C" fn rt_os_machine(ctx: *mut Ctx) -> TakenBytes {
    asked(ctx, "os_machine", TakenBytes(0), |c| {
        c.given(std::env::consts::ARCH.as_bytes().to_vec())
    })
}

/// The system's name and release, as `uname -sr` prints them, told.
pub unsafe extern "C" fn rt_os_uname(ctx: *mut Ctx) -> TakenBytes {
    asked(ctx, "os_uname", TakenBytes(0), |c| {
        c.given(told(ply_host::os::name()))
    })
}

/// The name the kernel holds for the machine, told.
pub unsafe extern "C" fn rt_os_hostname(ctx: *mut Ctx) -> TakenBytes {
    asked(ctx, "os_hostname", TakenBytes(0), |c| {
        c.given(told(ply_host::os::hostname()))
    })
}

/// The processors the process may run on at once.
pub unsafe extern "C" fn rt_os_cpus(ctx: *mut Ctx) -> I64 {
    asked(ctx, "os_cpus", I64(0), |_| I64(ply_host::os::cpus()))
}

/// The id of the user the process runs as.
pub unsafe extern "C" fn rt_os_uid(ctx: *mut Ctx) -> I64 {
    asked(ctx, "os_uid", I64(0), |_| I64(ply_host::os::uid()))
}

/// The name the system's records give the user of id `id`, found.
pub unsafe extern "C" fn rt_os_user_name(ctx: *mut Ctx, id: I64) -> TakenBytes {
    asked(ctx, "os_user_name", TakenBytes(0), |c| {
        c.given(found(ply_host::os::user_name(id.0)))
    })
}

/// The process's id.
pub unsafe extern "C" fn rt_os_pid(ctx: *mut Ctx) -> I64 {
    asked(ctx, "os_pid", I64(0), |_| {
        I64(i64::from(std::process::id()))
    })
}

/// The executable the process runs, its links followed, found.
pub unsafe extern "C" fn rt_os_executable(ctx: *mut Ctx) -> TakenBytes {
    asked(ctx, "os_executable", TakenBytes(0), |c| {
        c.given(found(ply_host::os::executable()))
    })
}

/// 1 where the machine's words are big-endian, else 0.
pub unsafe extern "C" fn rt_os_big_endian(ctx: *mut Ctx) -> I64 {
    asked(ctx, "os_big_endian", I64(0), |_| {
        I64(i64::from(cfg!(target_endian = "big")))
    })
}

/// The width of an address on the machine, in bits.
pub unsafe extern "C" fn rt_os_pointer_bits(ctx: *mut Ctx) -> I64 {
    asked(ctx, "os_pointer_bits", I64(0), |_| {
        I64(i64::from(usize::BITS))
    })
}
