//! The natives the host handlers of the standard library's host effects call (`host for`): each asks
//! the machine the program runs on and returns machine data, integers, bytes or a secret, which the
//! effect's module reads into its own types. Each that asks the host fails where a run may not
//! reach it, inside a simulation or in a test the search re-runs, as a host operation does.
//!
//! A native that would block the program's thread returns a ticket, which `job_wait` reads the
//! job's bytes by. In a production scheduler the job runs on the process's pools as the scheduler's
//! other tasks run. Outside one, where no other task could run, a job that reads files or computes
//! runs on the program's thread; one that waits on what is outside the process still runs on a
//! pool, so a shutdown's drain can give up waiting on it.
//!
//! A native returning structured data writes it in the flat forms of `ply_host::native`.

mod dns;
mod env;
mod fs;
mod process;
pub mod random;
pub mod shipped;
pub mod signal;
mod sockets;
mod sqlite;
pub mod term;
pub mod time;
pub mod toolchain;
mod trace;

use crate::c::{MachineNative, machine_native};
use crate::emitted::SecretHandle;
use crate::native::{Addr, I64, TakenBytes};
use crate::rt::{Ctx, borrowed};
use crate::secret;
use ply_core::host::Determinism::{self, Deterministic, Nondeterministic};
use ply_core::host::{Pending, err_host_in_search, err_host_in_simulation};
use ply_core::{Diagnostic, Resource, codes};
use ply_host::jobs::{self, Failure, Job, Work};
use ply_host::native::{found, parts, parts_of, told};
use ply_host::password;
use ply_host::registry::Facilities;
use std::path::Path;
use zeroize::Zeroizing;

/// Every native here, under the name its module declares it by.
pub fn natives() -> Vec<MachineNative> {
    let mut natives = vec![
        machine_native("job_wait", rt_job_wait as unsafe extern "C" fn(_, _) -> _),
        machine_native(
            "observed",
            rt_observed as unsafe extern "C" fn(_, _, _, _, _, _),
        ),
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
    ];
    natives.extend(dns::natives());
    natives.extend(env::natives());
    natives.extend(fs::natives());
    natives.extend(process::natives());
    natives.extend(random::natives());
    natives.extend(shipped::natives());
    natives.extend(signal::natives());
    natives.extend(sockets::natives());
    natives.extend(sqlite::natives());
    natives.extend(term::natives());
    natives.extend(time::natives());
    natives.extend(toolchain::natives());
    natives.extend(trace::natives());
    natives
}

impl Ctx {
    /// Whether the native `native` may ask the host now. Where it may, the run has asked it as a
    /// host operation is asked: a `Nondeterministic` response means its pass holds only for the
    /// binding it ran under. Where it may not, the run fails as a host operation's would, and the
    /// native returns nothing it reads.
    fn reaches_host(&mut self, native: &'static str, determinism: Determinism) -> bool {
        let operation = format!("the native `{native}`");
        let span = self.site();
        if let Some(scheduler) = crate::tasks::seeded_scheduler(self) {
            self.fail(err_host_in_simulation(span, &operation, scheduler));
            return false;
        }
        if self.re_executed {
            self.fail(err_host_in_search(span, &operation, native));
            return false;
        }
        self.host_use.record(determinism);
        true
    }

    /// The ticket of `job`, the job this context's run's, which `native` asked for as `what`, a
    /// diagnostic names it by: run as the module says, on `work`'s pool or here. 0, with the run
    /// failed, where no thread could be had for it.
    fn submitted(
        &mut self,
        native: &'static str,
        what: &'static str,
        work: Work,
        job: impl FnOnce() -> Result<Vec<u8>, String> + Send + 'static,
    ) -> I64 {
        self.refusing(native, what, work, move || job().map_err(Failure::Said))
    }

    /// The ticket of `job` as [`Ctx::submitted`] hands one, but always on `work`'s pool: for a job
    /// the program goes on beside, submitting others before it waits on any.
    fn queued(
        &mut self,
        native: &'static str,
        what: &'static str,
        work: Work,
        job: impl FnOnce() -> Result<Vec<u8>, String> + Send + 'static,
    ) -> I64 {
        self.ticketed(native, what, work, false, move || {
            job().map_err(Failure::Said)
        })
    }

    /// The ticket of `job` as [`Ctx::submitted`] hands one, where the job may fail with a
    /// diagnostic of its own.
    fn refusing(
        &mut self,
        native: &'static str,
        what: &'static str,
        work: Work,
        job: impl FnOnce() -> Result<Vec<u8>, Failure> + Send + 'static,
    ) -> I64 {
        let here = work != Work::Waiting && crate::tasks::production_task(self).is_none();
        self.ticketed(native, what, work, here, job)
    }

    /// The ticket of `job`, run on this thread where `here` and on `work`'s pool otherwise.
    fn ticketed(
        &mut self,
        native: &'static str,
        what: &'static str,
        work: Work,
        here: bool,
        job: impl FnOnce() -> Result<Vec<u8>, Failure> + Send + 'static,
    ) -> I64 {
        let span = self.site();
        let runtime = self.host_runtime();
        let submitted = ply_host::registry::run_of(&*runtime).and_then(|run| {
            if here {
                Ok(jobs::done_here(run, span, native, what, job))
            } else {
                jobs::refusing(work, run, span, native, what, job)
            }
        });
        match submitted {
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
            Err(reason) => {
                let span = self.site();
                self.fail(password::rejected(what, &reason, span));
                I64(0)
            }
        }
    }
}

/// What `result` returns, where the native `native` may ask the host for a response of
/// `determinism`; `rejected` where it may not.
fn asked<R>(
    ctx: *mut Ctx,
    native: &'static str,
    determinism: Determinism,
    rejected: R,
    result: impl FnOnce(&mut Ctx) -> R,
) -> R {
    let c = unsafe { &mut *ctx };
    if c.reaches_host(native, determinism) {
        result(c)
    } else {
        rejected
    }
}

/// What `result` returns where the run is granted `family` and the native `native` may ask the host
/// for a response of `determinism`; `rejected`, with the run failed, where it lacks the family.
fn granted<R>(
    ctx: *mut Ctx,
    family: &'static str,
    native: &'static str,
    determinism: Determinism,
    rejected: R,
    result: impl FnOnce(&mut Ctx) -> R,
) -> R {
    let c = unsafe { &mut *ctx };
    if !c.binding.grants().allows(family) {
        c.fail(
            Diagnostic::error(
                codes::CAPABILITY_UNDECLARED,
                format!(
                    "the native `{native}` is the family `{family}`'s, which this run is not granted"
                ),
            )
            .note(format!("`--allow {family}` grants it")),
        );
        return rejected;
    }
    asked(ctx, native, determinism, rejected, result)
}

/// What `read` takes of the facilities the run's host grants; `None`, with the run failed, for a
/// runtime no host made.
fn from_host<R>(c: &mut Ctx, read: impl FnOnce(&Facilities) -> R) -> Option<R> {
    let runtime = c.host_runtime();
    match ply_host::registry::facilities_of(&*runtime) {
        Some(facilities) => Some(read(facilities)),
        None => {
            c.fail(Diagnostic::error(
                codes::INTERNAL_ERROR,
                "a native asked the host through a runtime no host made",
            ));
            None
        }
    }
}

/// What a native was handed, read; `None`, with the run failed, where it is not what the real
/// handler hands on, which checks what it hands.
fn handed<T>(c: &mut Ctx, native: &str, what: &str, read: Option<T>) -> Option<T> {
    if read.is_none() {
        c.fail(Diagnostic::error(
            codes::INTERNAL_ERROR,
            format!("the native `{native}` was handed {what} it cannot be"),
        ));
    }
    read
}

/// `result`'s bytes as taken bytes; none, with the run failed, where it is a diagnostic.
fn given(c: &mut Ctx, result: Result<Vec<u8>, Diagnostic>) -> TakenBytes {
    match result {
        Ok(bytes) => c.given(bytes),
        Err(d) => {
            c.fail(d);
            TakenBytes(0)
        }
    }
}

/// The ticket of `job` on the waiting pool, which `native` asked for as `what`; 0, with the run
/// failed, where it is a diagnostic.
fn waited(
    c: &mut Ctx,
    native: &'static str,
    what: &'static str,
    job: Result<Job, Diagnostic>,
) -> I64 {
    match job {
        Ok(job) => c.submitted(native, what, Work::Waiting, job),
        Err(d) => {
            c.fail(d);
            I64(0)
        }
    }
}

/// The text a native borrowed at `p`; `None`, with the run failed, where it is not UTF-8.
fn text_at(c: &mut Ctx, p: Addr, n: I64) -> Option<String> {
    c.text(unsafe { borrowed(p, n) }).map(str::to_string)
}

/// The texts borrowed as parts at `p`; `None`, with the run failed, where they are not.
fn texts_at(c: &mut Ctx, p: Addr, n: I64) -> Option<Vec<String>> {
    let read = parts_of(unsafe { borrowed(p, n) }).and_then(|texts| {
        texts
            .into_iter()
            .map(|t| String::from_utf8(t.to_vec()).ok())
            .collect::<Option<Vec<String>>>()
    });
    if read.is_none() {
        c.fail(Diagnostic::error(
            codes::INTERNAL_ERROR,
            "a native was handed a list of texts as parts that are not",
        ));
    }
    read
}

/// The run failed where `done` is a diagnostic.
fn done(c: &mut Ctx, done: Result<(), Diagnostic>) {
    if let Err(d) = done {
        c.fail(d);
    }
}

/// The bytes the job of `ticket` returned, as taken bytes: in a production scheduler the running
/// task waits for them as the scheduler's other tasks run, and outside one the thread waits. A job
/// that failed fails the run, naming what asked for it.
pub unsafe extern "C" fn rt_job_wait(ctx: *mut Ctx, ticket: I64) -> TakenBytes {
    let c = unsafe { &mut *ctx };
    match crate::tasks::waited(c, crate::tasks::ASK_TICKET, ticket.0) {
        Some(true) => crate::tasks::ticket_taken(ctx, ticket),
        Some(false) => TakenBytes(0),
        None => {
            let pending = Pending {
                token: ticket.0 as u64,
                label: "a native's job",
            };
            let runtime = c.host_runtime();
            match ply_host::registry::job_bytes(&*runtime, &pending) {
                Ok(bytes) => c.given(bytes),
                Err(d) => {
                    c.fail(d);
                    TakenBytes(0)
                }
            }
        }
    }
}

/// What a host handler read or wrote, entered into the record of the run asking where it is
/// observed. Under the root the label at `label_at` names, at the path at `path_at` below it,
/// `kind` 0 is a file's bytes read, 1 a directory's names, 2 what is there, 3 every path below,
/// and 4 a write; a label no root is bound to holds nothing to record. Of what the binary ships,
/// the label empty, 5 is the shipped module the path names read (every module's name for the empty
/// path), 6 the program the launcher entered, and 7 and 8 a load's view at the path of the shipped
/// modules it reached, which it pulled itself for 7. It asks nothing of the host itself.
pub unsafe extern "C" fn rt_observed(
    ctx: *mut Ctx,
    label_at: Addr,
    label_n: I64,
    path_at: Addr,
    path_n: I64,
    kind: I64,
) {
    use ply_host::observe::Read;
    let c = unsafe { &mut *ctx };
    let Some(recorder) = ply_host::observe::of(&*c.host_runtime()) else {
        return;
    };
    let (Some(label), Some(path)) = (
        c.text(unsafe { borrowed(label_at, label_n) }),
        c.text(unsafe { borrowed(path_at, path_n) }),
    ) else {
        return;
    };
    let how = match kind.0 {
        0 => Some(Read::File),
        1 => Some(Read::Dir),
        2 => Some(Read::Kind),
        3 => Some(Read::Tree),
        4 => None,
        5 => return recorder.shipped(path),
        6 => return recorder.ran_program(),
        7 | 8 => return recorder.reached(path, kind.0 == 7),
        _ => {
            handed(c, "observed", "a kind of read", None::<()>);
            return;
        }
    };
    let resource = Resource::Named(ply_core::Symbol::new(label));
    let Some(Some(root)) = from_host(c, |facilities| {
        facilities.roots().get(&resource).map(Path::to_path_buf)
    }) else {
        return;
    };
    // The path as the program named it: where a link in it leads is read again when the record is.
    let target = root.join(path);
    match how {
        Some(how) => recorder.read(how, &target),
        None => recorder.wrote(&target),
    }
}

/// A new self-signed certificate for `localhost`, told: its certificate, its key, its DER and its
/// fingerprint as parts.
pub unsafe extern "C" fn rt_certgen_issue(ctx: *mut Ctx) -> TakenBytes {
    asked(ctx, "certgen_issue", Nondeterministic, TakenBytes(0), |c| {
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
            Err(reason) => [&[0u8][..], reason.as_bytes()].concat(),
        };
        c.given(said)
    })
}

/// The run's value of `key`: a plain one, or, where `secret`, the one only `config.secret` reads.
/// `None` where the run was granted no such key. The read is entered into the record of the run
/// asking, where it is observed: a plain value as what it returned, a secret as one read.
fn configured(c: &mut Ctx, key: &str, secret: bool) -> Option<String> {
    let runtime = c.host_runtime();
    let value = ply_host::registry::facilities_of(&*runtime).and_then(|facilities| {
        let config = facilities.configuration();
        if secret {
            config.plaintext(key)
        } else {
            config.get(key)
        }
        .map(str::to_string)
    });
    if let Some(recorder) = ply_host::observe::of(&*runtime) {
        if secret {
            recorder.read_secret(key);
        } else {
            recorder.configured(key, value.as_deref());
        }
    }
    value
}

/// The run's plain value of the key at `p`, found.
pub unsafe extern "C" fn rt_config_get(ctx: *mut Ctx, p: Addr, n: I64) -> TakenBytes {
    asked(ctx, "config_get", Nondeterministic, TakenBytes(0), |c| {
        let Some(key) = c.text(unsafe { borrowed(p, n) }) else {
            return TakenBytes(0);
        };
        let value = configured(c, key, false);
        c.given(found(value))
    })
}

/// 1 where the run holds a secret value of the key at `p`, else 0.
pub unsafe extern "C" fn rt_config_secret_held(ctx: *mut Ctx, p: Addr, n: I64) -> I64 {
    asked(ctx, "config_secret_held", Nondeterministic, I64(0), |c| {
        let Some(key) = c.text(unsafe { borrowed(p, n) }) else {
            return I64(0);
        };
        I64(i64::from(configured(c, key, true).is_some()))
    })
}

/// The run's secret value of the key at `p`, empty where it holds none.
pub unsafe extern "C" fn rt_config_secret(ctx: *mut Ctx, p: Addr, n: I64) -> SecretHandle {
    asked(
        ctx,
        "config_secret",
        Nondeterministic,
        SecretHandle::of(0),
        |c| {
            let Some(key) = c.text(unsafe { borrowed(p, n) }) else {
                return SecretHandle::of(0);
            };
            let plain = Zeroizing::new(configured(c, key, true).unwrap_or_default().into_bytes());
            secret::sealed(c, true, plain)
        },
    )
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
    asked(ctx, "kdf_argon2", Deterministic, I64(0), |c| {
        let (Some(password), Some(key)) = (secret::held(c, &password), secret::held(c, &key))
        else {
            return I64(0);
        };
        let Some(variant) = c.text(unsafe { borrowed(variant_at, variant_len) }) else {
            return I64(0);
        };
        let work = password::Work::argon2(
            variant,
            password.bytes(),
            unsafe { borrowed(salt_at, salt_len) },
            key.bytes(),
            unsafe { borrowed(data_at, data_len) },
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
    asked(ctx, "kdf_scrypt", Deterministic, I64(0), |c| {
        let Some(password) = secret::held(c, &password) else {
            return I64(0);
        };
        let work = password::Work::scrypt(
            password.bytes(),
            unsafe { borrowed(salt_at, salt_len) },
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
    asked(ctx, "kdf_bcrypt", Deterministic, I64(0), |c| {
        let Some(password) = secret::held(c, &password) else {
            return I64(0);
        };
        let work = password::Work::bcrypt(
            password.bytes(),
            unsafe { borrowed(salt_at, salt_len) },
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
    asked(ctx, "kdf_pbkdf2", Deterministic, I64(0), |c| {
        let Some(password) = secret::held(c, &password) else {
            return I64(0);
        };
        let Some(hash) = c.text(unsafe { borrowed(hash_at, hash_len) }) else {
            return I64(0);
        };
        let work = password::Work::pbkdf2(
            hash,
            password.bytes(),
            unsafe { borrowed(salt_at, salt_len) },
            iterations.0,
            length.0,
        );
        c.hashed("kdf_pbkdf2", "`kdf.pbkdf2`", work)
    })
}

/// The name the toolchain gives the system this binary was built for.
pub unsafe extern "C" fn rt_os_system(ctx: *mut Ctx) -> TakenBytes {
    asked(ctx, "os_system", Nondeterministic, TakenBytes(0), |c| {
        c.given(std::env::consts::OS.as_bytes().to_vec())
    })
}

/// The name the toolchain gives the architecture this binary was built for.
pub unsafe extern "C" fn rt_os_machine(ctx: *mut Ctx) -> TakenBytes {
    asked(ctx, "os_machine", Nondeterministic, TakenBytes(0), |c| {
        c.given(std::env::consts::ARCH.as_bytes().to_vec())
    })
}

/// The system's name and release, as `uname -sr` prints them, told.
pub unsafe extern "C" fn rt_os_uname(ctx: *mut Ctx) -> TakenBytes {
    asked(ctx, "os_uname", Nondeterministic, TakenBytes(0), |c| {
        c.given(told(ply_host::os::name()))
    })
}

/// The name the kernel holds for the machine, told.
pub unsafe extern "C" fn rt_os_hostname(ctx: *mut Ctx) -> TakenBytes {
    asked(ctx, "os_hostname", Nondeterministic, TakenBytes(0), |c| {
        c.given(told(ply_host::os::hostname()))
    })
}

/// The processors the process may run on at once.
pub unsafe extern "C" fn rt_os_cpus(ctx: *mut Ctx) -> I64 {
    asked(ctx, "os_cpus", Nondeterministic, I64(0), |_| {
        I64(ply_host::os::cpus())
    })
}

/// The id of the user the process runs as.
pub unsafe extern "C" fn rt_os_uid(ctx: *mut Ctx) -> I64 {
    asked(ctx, "os_uid", Nondeterministic, I64(0), |_| {
        I64(ply_host::os::uid())
    })
}

/// The name the system's records give the user of id `id`, found.
pub unsafe extern "C" fn rt_os_user_name(ctx: *mut Ctx, id: I64) -> TakenBytes {
    asked(ctx, "os_user_name", Nondeterministic, TakenBytes(0), |c| {
        c.given(found(ply_host::os::user_name(id.0)))
    })
}

/// The process's id.
pub unsafe extern "C" fn rt_os_pid(ctx: *mut Ctx) -> I64 {
    asked(ctx, "os_pid", Nondeterministic, I64(0), |_| {
        I64(i64::from(std::process::id()))
    })
}

/// The executable the process runs, its links followed, found.
pub unsafe extern "C" fn rt_os_executable(ctx: *mut Ctx) -> TakenBytes {
    asked(ctx, "os_executable", Nondeterministic, TakenBytes(0), |c| {
        c.given(found(ply_host::os::executable()))
    })
}

/// 1 where the machine's words are big-endian, else 0.
pub unsafe extern "C" fn rt_os_big_endian(ctx: *mut Ctx) -> I64 {
    asked(ctx, "os_big_endian", Nondeterministic, I64(0), |_| {
        I64(i64::from(cfg!(target_endian = "big")))
    })
}

/// The width of an address on the machine, in bits.
pub unsafe extern "C" fn rt_os_pointer_bits(ctx: *mut Ctx) -> I64 {
    asked(ctx, "os_pointer_bits", Nondeterministic, I64(0), |_| {
        I64(i64::from(usize::BITS))
    })
}
