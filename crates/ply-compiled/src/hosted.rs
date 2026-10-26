//! The natives the real handlers of the standard library's host effects call (`host for`): each asks
//! the machine the program runs on and answers machine data, integers, bytes or a secret, which the
//! effect's module reads into its own types. Each refuses where a run may not reach the host,
//! inside a `simulate` region or in a test the search re-runs, as a host operation does.
//!
//! A native that would block the program's thread hands a job to the process's pools and answers a
//! ticket, which `job_wait` reads the job's bytes by: the running task waits in a production
//! region while the region's other tasks run, and the thread waits outside one.
//!
//! A native answering structured text writes one of three flat forms. Told: byte 1 and then the
//! text, or byte 0 and then why the system said nothing. Found: byte 1 and then the text, or the
//! one byte 0. Parts: each part behind its count, four bytes, most significant first.

use crate::c::{MachineNative, machine_native};
use crate::emitted::SecretHandle;
use crate::host::HostMark;
use crate::native::{Addr, I64, TakenBytes};
use crate::rt::{Ctx, lent};
use ply_core::host::{Determinism, Linearity, Pending, err_host_in_search};
use ply_core::{Diagnostic, EffectAtom, Mode, Resource, Value, codes, err_host_in_simulation};
use ply_host::jobs::{self, Work};
use ply_host::password;

/// The effect every native here performs, under its program-wide name (`std.internal.native`).
const NATIVE: &str = "std.internal.native.native";

/// What a native says of itself when it asks the host: whether what it answers is a function of
/// what it is handed, and whether asking again changes anything outside the program.
#[derive(Clone, Copy)]
struct Asking {
    determinism: Determinism,
    linearity: Linearity,
}

/// Asking the machine what it may answer otherwise the next time, changing nothing.
const READS: Asking = Asking {
    determinism: Determinism::Nondeterministic,
    linearity: Linearity::Repeatable,
};

/// Computing a function of what the native is handed, off the program's thread.
const COMPUTES: Asking = Asking {
    determinism: Determinism::Deterministic,
    linearity: Linearity::Repeatable,
};

/// Every native here, under the name its module declares it by.
pub fn natives() -> Vec<MachineNative> {
    vec![
        machine_native("job_wait", rt_job_wait as unsafe extern "C" fn(_, _) -> _),
        machine_native(
            "certgen_issue",
            rt_certgen_issue as unsafe extern "C" fn(_) -> _,
        ),
        machine_native(
            "config_get",
            rt_config_get as unsafe extern "C" fn(_, _, _) -> _,
        ),
        machine_native(
            "config_secret_held",
            rt_config_secret_held as unsafe extern "C" fn(_, _, _) -> _,
        ),
        machine_native(
            "config_secret",
            rt_config_secret as unsafe extern "C" fn(_, _, _) -> _,
        ),
        machine_native(
            "kdf_argon2",
            rt_kdf_argon2 as unsafe extern "C" fn(_, _, _, _, _, _, _, _, _, _, _, _, _) -> _,
        ),
        machine_native(
            "kdf_scrypt",
            rt_kdf_scrypt as unsafe extern "C" fn(_, _, _, _, _, _, _, _) -> _,
        ),
        machine_native(
            "kdf_bcrypt",
            rt_kdf_bcrypt as unsafe extern "C" fn(_, _, _, _, _) -> _,
        ),
        machine_native(
            "kdf_pbkdf2",
            rt_kdf_pbkdf2 as unsafe extern "C" fn(_, _, _, _, _, _, _, _) -> _,
        ),
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
    /// Whether the native `native` may ask the host now. Where it may, the run has asked it as
    /// `asking` says, as a host operation is asked: a nondeterministic answer means its pass
    /// answers only for the binding it ran under, and an at-most-once one is not replayed by a
    /// second resumption. Where it may not, the run fails as a host operation's would, and the
    /// native answers nothing it reads.
    fn reaches_host(&mut self, native: &'static str, asking: Asking) -> bool {
        let operation = format!("the native `{native}`");
        let span = self.site();
        if let Some(region) = crate::simulate::seeded_region(self) {
            self.fail(err_host_in_simulation(span, &operation, region));
            return false;
        }
        if self.re_executed {
            self.fail(err_host_in_search(span, &operation, native));
            return false;
        }
        self.host_use.record(
            &EffectAtom::new(NATIVE, Resource::Singleton, Mode::Read),
            asking.determinism,
        );
        if asking.linearity.is_linear() {
            self.host_ops = self.host_ops.saturating_add(1);
            self.last_linear = Some(HostMark {
                operation,
                path: native,
                span,
            });
        }
        true
    }

    /// The ticket of `job` on `work`'s pool, which `native` asked for as `what`, a diagnostic names
    /// it by; 0, with the run failed, where no thread could be had for it.
    fn submitted(
        &mut self,
        native: &'static str,
        what: &'static str,
        work: Work,
        job: impl FnOnce() -> Result<Vec<u8>, String> + Send + 'static,
    ) -> I64 {
        let span = self.site();
        match jobs::submit(work, span, native, what, job) {
            Ok(pending) => I64(pending.token as i64),
            Err(d) => {
                self.fail(d);
                I64(0)
            }
        }
    }

    /// The ticket of `work`'s hash, which `native` asked for as `what`; 0, with the run failed
    /// where the hash asked for is past its bounds.
    fn hashed(
        &mut self,
        native: &'static str,
        what: &'static str,
        work: Result<password::Work, String>,
    ) -> I64 {
        match work {
            Ok(work) => self.submitted(native, what, Work::Computing, move || work.run()),
            Err(why) => {
                let span = self.site();
                self.fail(password::refused(what, &why, span));
                I64(0)
            }
        }
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

/// `parts` one after another, each behind its count.
fn parts(parts: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for part in parts {
        out.extend((part.len() as u32).to_be_bytes());
        out.extend(*part);
    }
    out
}

/// What `answer` answers, where the native `native` may ask the host as `asking` says; `refused`
/// where it may not.
fn asked<R>(
    ctx: *mut Ctx,
    native: &'static str,
    asking: Asking,
    refused: R,
    answer: impl FnOnce(&mut Ctx) -> R,
) -> R {
    let c = unsafe { &mut *ctx };
    if c.reaches_host(native, asking) {
        answer(c)
    } else {
        refused
    }
}

/// The secret the lent handle `s` holds.
fn secret_of(c: &mut Ctx, s: &SecretHandle) -> Option<Value> {
    crate::machine::secret_held(c, s)
}

/// A secret's bytes, which only this side of the boundary reads.
fn opened(v: &Value) -> &[u8] {
    match v {
        Value::Secret(held) => held.bytes(),
        _ => &[],
    }
}

/// The bytes the job of `ticket` answered, as taken bytes: inside a production region the running
/// task waits for them while the region's other tasks run, and outside one the thread waits. A job
/// that failed fails the run, naming what asked for it.
pub unsafe extern "C" fn rt_job_wait(ctx: *mut Ctx, ticket: I64) -> TakenBytes {
    let pending = Pending {
        token: ticket.0 as u64,
        label: "a native's job",
    };
    let c = unsafe { &mut *ctx };
    let answered = if crate::simulate::running_task_of_production(c).is_some() {
        match unsafe { crate::simulate::park(ctx, pending) } {
            Some(value) => value,
            None => return TakenBytes(0),
        }
    } else {
        match c.host_runtime().block_on(pending) {
            Ok(value) => value,
            Err(d) => {
                c.fail(d);
                return TakenBytes(0);
            }
        }
    };
    let c = unsafe { &mut *ctx };
    match &answered {
        Value::Bytes(bytes) => c.given(bytes.to_vec()),
        _ => {
            c.fail(Diagnostic::error(
                codes::INTERNAL_ERROR,
                "a native's job was answered with no bytes",
            ));
            TakenBytes(0)
        }
    }
}

/// A new self-signed certificate for `localhost`, told: its certificate, its key, its DER and its
/// fingerprint as parts.
pub unsafe extern "C" fn rt_certgen_issue(ctx: *mut Ctx) -> TakenBytes {
    asked(ctx, "certgen_issue", READS, TakenBytes(0), |c| {
        let said = match ply_host::certgen::issue(&[]) {
            Ok(issued) => [
                &[1u8][..],
                &parts(&[
                    issued.certificate.as_bytes(),
                    issued.key.as_bytes(),
                    &issued.der,
                    issued.fingerprint.as_bytes(),
                ]),
            ]
            .concat(),
            Err(why) => [&[0u8][..], why.as_bytes()].concat(),
        };
        c.given(said)
    })
}

/// The run's plain value of the key at `p`, found.
pub unsafe extern "C" fn rt_config_get(ctx: *mut Ctx, p: Addr, n: I64) -> TakenBytes {
    asked(ctx, "config_get", READS, TakenBytes(0), |c| {
        let Some(key) = c.text(unsafe { lent(p, n) }) else {
            return TakenBytes(0);
        };
        let value = c.host_runtime().configured(key, false);
        c.given(found(value))
    })
}

/// 1 where the run holds a secret value of the key at `p`, else 0.
pub unsafe extern "C" fn rt_config_secret_held(ctx: *mut Ctx, p: Addr, n: I64) -> I64 {
    asked(ctx, "config_secret_held", READS, I64(0), |c| {
        let Some(key) = c.text(unsafe { lent(p, n) }) else {
            return I64(0);
        };
        I64(i64::from(c.host_runtime().configured(key, true).is_some()))
    })
}

/// The run's secret value of the key at `p`, empty where it holds none.
pub unsafe extern "C" fn rt_config_secret(ctx: *mut Ctx, p: Addr, n: I64) -> SecretHandle {
    asked(ctx, "config_secret", READS, SecretHandle::of(0), |c| {
        let Some(key) = c.text(unsafe { lent(p, n) }) else {
            return SecretHandle::of(0);
        };
        let plain = c.host_runtime().configured(key, true).unwrap_or_default();
        SecretHandle::of(c.heap.bridge(Value::secret_text(&plain)))
    })
}

/// The ticket of Argon2 version 1.3 over what `kdf.argon2` is handed.
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn rt_kdf_argon2(
    ctx: *mut Ctx,
    variant_at: Addr,
    variant_len: I64,
    password: SecretHandle,
    salt_at: Addr,
    salt_len: I64,
    key: SecretHandle,
    data_at: Addr,
    data_len: I64,
    memory: I64,
    passes: I64,
    lanes: I64,
    length: I64,
) -> I64 {
    asked(ctx, "kdf_argon2", COMPUTES, I64(0), |c| {
        let (Some(password), Some(key)) = (secret_of(c, &password), secret_of(c, &key)) else {
            return I64(0);
        };
        let Some(variant) = c.text(unsafe { lent(variant_at, variant_len) }) else {
            return I64(0);
        };
        let work = password::Work::argon2(
            variant,
            opened(&password),
            unsafe { lent(salt_at, salt_len) },
            opened(&key),
            unsafe { lent(data_at, data_len) },
            memory.0,
            passes.0,
            lanes.0,
            length.0,
        );
        c.hashed("kdf_argon2", "`kdf.argon2`", work)
    })
}

/// The ticket of scrypt over what `kdf.scrypt` is handed.
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn rt_kdf_scrypt(
    ctx: *mut Ctx,
    password: SecretHandle,
    salt_at: Addr,
    salt_len: I64,
    log_n: I64,
    r: I64,
    p: I64,
    length: I64,
) -> I64 {
    asked(ctx, "kdf_scrypt", COMPUTES, I64(0), |c| {
        let Some(password) = secret_of(c, &password) else {
            return I64(0);
        };
        let work = password::Work::scrypt(
            opened(&password),
            unsafe { lent(salt_at, salt_len) },
            log_n.0,
            r.0,
            p.0,
            length.0,
        );
        c.hashed("kdf_scrypt", "`kdf.scrypt`", work)
    })
}

/// The ticket of bcrypt over what `kdf.bcrypt` is handed.
pub unsafe extern "C" fn rt_kdf_bcrypt(
    ctx: *mut Ctx,
    password: SecretHandle,
    salt_at: Addr,
    salt_len: I64,
    cost: I64,
) -> I64 {
    asked(ctx, "kdf_bcrypt", COMPUTES, I64(0), |c| {
        let Some(password) = secret_of(c, &password) else {
            return I64(0);
        };
        let work = password::Work::bcrypt(
            opened(&password),
            unsafe { lent(salt_at, salt_len) },
            cost.0,
        );
        c.hashed("kdf_bcrypt", "`kdf.bcrypt`", work)
    })
}

/// The ticket of PBKDF2 over what `kdf.pbkdf2` is handed.
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn rt_kdf_pbkdf2(
    ctx: *mut Ctx,
    hash_at: Addr,
    hash_len: I64,
    password: SecretHandle,
    salt_at: Addr,
    salt_len: I64,
    iterations: I64,
    length: I64,
) -> I64 {
    asked(ctx, "kdf_pbkdf2", COMPUTES, I64(0), |c| {
        let Some(password) = secret_of(c, &password) else {
            return I64(0);
        };
        let Some(hash) = c.text(unsafe { lent(hash_at, hash_len) }) else {
            return I64(0);
        };
        let work = password::Work::pbkdf2(
            hash,
            opened(&password),
            unsafe { lent(salt_at, salt_len) },
            iterations.0,
            length.0,
        );
        c.hashed("kdf_pbkdf2", "`kdf.pbkdf2`", work)
    })
}

/// The name the toolchain gives the system this binary was built for.
pub unsafe extern "C" fn rt_os_system(ctx: *mut Ctx) -> TakenBytes {
    asked(ctx, "os_system", READS, TakenBytes(0), |c| {
        c.given(std::env::consts::OS.as_bytes().to_vec())
    })
}

/// The name the toolchain gives the architecture this binary was built for.
pub unsafe extern "C" fn rt_os_machine(ctx: *mut Ctx) -> TakenBytes {
    asked(ctx, "os_machine", READS, TakenBytes(0), |c| {
        c.given(std::env::consts::ARCH.as_bytes().to_vec())
    })
}

/// The system's name and release, as `uname -sr` prints them, told.
pub unsafe extern "C" fn rt_os_uname(ctx: *mut Ctx) -> TakenBytes {
    asked(ctx, "os_uname", READS, TakenBytes(0), |c| {
        c.given(told(ply_host::os::name()))
    })
}

/// The name the kernel holds for the machine, told.
pub unsafe extern "C" fn rt_os_hostname(ctx: *mut Ctx) -> TakenBytes {
    asked(ctx, "os_hostname", READS, TakenBytes(0), |c| {
        c.given(told(ply_host::os::hostname()))
    })
}

/// The processors the process may run on at once.
pub unsafe extern "C" fn rt_os_cpus(ctx: *mut Ctx) -> I64 {
    asked(ctx, "os_cpus", READS, I64(0), |_| I64(ply_host::os::cpus()))
}

/// The id of the user the process runs as.
pub unsafe extern "C" fn rt_os_uid(ctx: *mut Ctx) -> I64 {
    asked(ctx, "os_uid", READS, I64(0), |_| I64(ply_host::os::uid()))
}

/// The name the system's records give the user of id `id`, found.
pub unsafe extern "C" fn rt_os_user_name(ctx: *mut Ctx, id: I64) -> TakenBytes {
    asked(ctx, "os_user_name", READS, TakenBytes(0), |c| {
        c.given(found(ply_host::os::user_name(id.0)))
    })
}

/// The process's id.
pub unsafe extern "C" fn rt_os_pid(ctx: *mut Ctx) -> I64 {
    asked(ctx, "os_pid", READS, I64(0), |_| {
        I64(i64::from(std::process::id()))
    })
}

/// The executable the process runs, its links followed, found.
pub unsafe extern "C" fn rt_os_executable(ctx: *mut Ctx) -> TakenBytes {
    asked(ctx, "os_executable", READS, TakenBytes(0), |c| {
        c.given(found(ply_host::os::executable()))
    })
}

/// 1 where the machine's words are big-endian, else 0.
pub unsafe extern "C" fn rt_os_big_endian(ctx: *mut Ctx) -> I64 {
    asked(ctx, "os_big_endian", READS, I64(0), |_| {
        I64(i64::from(cfg!(target_endian = "big")))
    })
}

/// The width of an address on the machine, in bits.
pub unsafe extern "C" fn rt_os_pointer_bits(ctx: *mut Ctx) -> I64 {
    asked(ctx, "os_pointer_bits", READS, I64(0), |_| {
        I64(i64::from(usize::BITS))
    })
}
