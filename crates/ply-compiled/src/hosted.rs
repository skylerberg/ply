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

use crate::c::{MachineNative, runtime_native};
use crate::emitted::SecretHandle;
use crate::native::{Addr, I64, TakenBytes};
use crate::rt::{Ctx, borrowed};
use crate::secret;
use ply_core::host::Determinism::{self, Deterministic, Nondeterministic};
use ply_core::host::{Pending, err_host_in_search, err_host_in_simulation};
use ply_core::{Diagnostic, Resource, codes};
use ply_host::jobs::{self, Failure, Job, Work};
use ply_host::native::{found, parts, parts_of, text_or_error};
use ply_host::password;
use ply_host::registry::Facilities;
use std::path::Path;
use zeroize::Zeroizing;

pub fn natives() -> Vec<MachineNative> {
    let mut natives = vec![
        runtime_native("job_wait", rt_job_wait as unsafe extern "C" fn(_, _) -> _),
        runtime_native(
            "observed",
            rt_observed as unsafe extern "C" fn(_, _, _, _, _, _),
        ),
        runtime_native(
            "certgen_issue",
            rt_certgen_issue as unsafe extern "C" fn(_) -> _,
        ),
        runtime_native(
            "config_get",
            rt_config_get as unsafe extern "C" fn(_, _, _) -> _,
        ),
        runtime_native(
            "config_secret_held",
            rt_config_secret_held as unsafe extern "C" fn(_, _, _) -> _,
        ),
        runtime_native(
            "config_secret",
            rt_config_secret as unsafe extern "C" fn(_, _, _) -> _,
        ),
        runtime_native(
            "kdf_argon2",
            rt_kdf_argon2 as unsafe extern "C" fn(_, _, _, _, _, _, _, _, _, _, _, _, _) -> _,
        ),
        runtime_native(
            "kdf_scrypt",
            rt_kdf_scrypt as unsafe extern "C" fn(_, _, _, _, _, _, _, _) -> _,
        ),
        runtime_native(
            "kdf_bcrypt",
            rt_kdf_bcrypt as unsafe extern "C" fn(_, _, _, _, _) -> _,
        ),
        runtime_native(
            "kdf_pbkdf2",
            rt_kdf_pbkdf2 as unsafe extern "C" fn(_, _, _, _, _, _, _, _) -> _,
        ),
        runtime_native("os_system", rt_os_system as unsafe extern "C" fn(_) -> _),
        runtime_native("os_machine", rt_os_machine as unsafe extern "C" fn(_) -> _),
        runtime_native("os_uname", rt_os_uname as unsafe extern "C" fn(_) -> _),
        runtime_native(
            "os_hostname",
            rt_os_hostname as unsafe extern "C" fn(_) -> _,
        ),
        runtime_native("os_cpus", rt_os_cpus as unsafe extern "C" fn(_) -> _),
        runtime_native("os_uid", rt_os_uid as unsafe extern "C" fn(_) -> _),
        runtime_native(
            "os_user_name",
            rt_os_user_name as unsafe extern "C" fn(_, _) -> _,
        ),
        runtime_native("os_pid", rt_os_pid as unsafe extern "C" fn(_) -> _),
        runtime_native(
            "os_executable",
            rt_os_executable as unsafe extern "C" fn(_) -> _,
        ),
        runtime_native(
            "os_big_endian",
            rt_os_big_endian as unsafe extern "C" fn(_) -> _,
        ),
        runtime_native(
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
    fn check_host_allowed(&mut self, native: &'static str, determinism: Determinism) -> bool {
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

    fn submitted(
        &mut self,
        native: &'static str,
        what: &'static str,
        work: Work,
        job: impl FnOnce() -> Result<Vec<u8>, String> + Send + 'static,
    ) -> I64 {
        self.queued_fallible(native, what, work, move || job().map_err(Failure::Message))
    }

    fn queued(
        &mut self,
        native: &'static str,
        what: &'static str,
        work: Work,
        job: impl FnOnce() -> Result<Vec<u8>, String> + Send + 'static,
    ) -> I64 {
        self.ticketed(native, what, work, false, move || {
            job().map_err(Failure::Message)
        })
    }

    fn queued_fallible(
        &mut self,
        native: &'static str,
        what: &'static str,
        work: Work,
        job: impl FnOnce() -> Result<Vec<u8>, Failure> + Send + 'static,
    ) -> I64 {
        let here = work != Work::Waiting && crate::tasks::running_unseeded_task(self).is_none();
        self.ticketed(native, what, work, here, job)
    }

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
                jobs::queue_fallible(work, run, span, native, what, job)
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

fn if_host_allowed<R>(
    ctx: *mut Ctx,
    native: &'static str,
    determinism: Determinism,
    rejected: R,
    result: impl FnOnce(&mut Ctx) -> R,
) -> R {
    let c = unsafe { &mut *ctx };
    if c.check_host_allowed(native, determinism) {
        result(c)
    } else {
        rejected
    }
}

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
    if_host_allowed(ctx, native, determinism, rejected, result)
}

fn with_host_facilities<R>(c: &mut Ctx, read: impl FnOnce(&Facilities) -> R) -> Option<R> {
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

fn required_argument<T>(c: &mut Ctx, native: &str, what: &str, read: Option<T>) -> Option<T> {
    if read.is_none() {
        c.fail(Diagnostic::error(
            codes::INTERNAL_ERROR,
            format!("the native `{native}` was handed {what} it cannot be"),
        ));
    }
    read
}

fn given(c: &mut Ctx, result: Result<Vec<u8>, Diagnostic>) -> TakenBytes {
    match result {
        Ok(bytes) => c.given(bytes),
        Err(d) => {
            c.fail(d);
            TakenBytes(0)
        }
    }
}

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

fn text_at(c: &mut Ctx, p: Addr, n: I64) -> Option<String> {
    c.text(unsafe { borrowed(p, n) }).map(str::to_string)
}

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

fn done(c: &mut Ctx, done: Result<(), Diagnostic>) {
    if let Err(d) = done {
        c.fail(d);
    }
}

pub unsafe extern "C" fn rt_job_wait(ctx: *mut Ctx, ticket: I64) -> TakenBytes {
    let c = unsafe { &mut *ctx };
    match crate::tasks::waited(c, crate::tasks::REQUEST_JOB_RESULT, ticket.0) {
        Some(true) => crate::tasks::take_ticket_bytes(ctx, ticket),
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
            required_argument(c, "observed", "a kind of read", None::<()>);
            return;
        }
    };
    let resource = Resource::Named(ply_core::Symbol::new(label));
    let Some(Some(root)) = with_host_facilities(c, |facilities| {
        facilities.roots().get(&resource).map(Path::to_path_buf)
    }) else {
        return;
    };
    let target = root.join(path);
    match how {
        Some(how) => recorder.read(how, &target),
        None => recorder.wrote(&target),
    }
}

pub unsafe extern "C" fn rt_certgen_issue(ctx: *mut Ctx) -> TakenBytes {
    if_host_allowed(ctx, "certgen_issue", Nondeterministic, TakenBytes(0), |c| {
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

pub unsafe extern "C" fn rt_config_get(ctx: *mut Ctx, p: Addr, n: I64) -> TakenBytes {
    if_host_allowed(ctx, "config_get", Nondeterministic, TakenBytes(0), |c| {
        let Some(key) = c.text(unsafe { borrowed(p, n) }) else {
            return TakenBytes(0);
        };
        let value = configured(c, key, false);
        c.given(found(value))
    })
}

pub unsafe extern "C" fn rt_config_secret_held(ctx: *mut Ctx, p: Addr, n: I64) -> I64 {
    if_host_allowed(ctx, "config_secret_held", Nondeterministic, I64(0), |c| {
        let Some(key) = c.text(unsafe { borrowed(p, n) }) else {
            return I64(0);
        };
        I64(i64::from(configured(c, key, true).is_some()))
    })
}

pub unsafe extern "C" fn rt_config_secret(ctx: *mut Ctx, p: Addr, n: I64) -> SecretHandle {
    if_host_allowed(
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
    if_host_allowed(ctx, "kdf_argon2", Deterministic, I64(0), |c| {
        let (Some(password), Some(key)) = (secret::sealed_value(c, &password), secret::sealed_value(c, &key))
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
    if_host_allowed(ctx, "kdf_scrypt", Deterministic, I64(0), |c| {
        let Some(password) = secret::sealed_value(c, &password) else {
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

pub unsafe extern "C" fn rt_kdf_bcrypt(
    ctx: *mut Ctx,
    password: SecretHandle,
    salt_at: Addr,
    salt_len: I64,
    cost: I64,
) -> I64 {
    if_host_allowed(ctx, "kdf_bcrypt", Deterministic, I64(0), |c| {
        let Some(password) = secret::sealed_value(c, &password) else {
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
    if_host_allowed(ctx, "kdf_pbkdf2", Deterministic, I64(0), |c| {
        let Some(password) = secret::sealed_value(c, &password) else {
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

pub unsafe extern "C" fn rt_os_system(ctx: *mut Ctx) -> TakenBytes {
    if_host_allowed(ctx, "os_system", Nondeterministic, TakenBytes(0), |c| {
        c.given(std::env::consts::OS.as_bytes().to_vec())
    })
}

pub unsafe extern "C" fn rt_os_machine(ctx: *mut Ctx) -> TakenBytes {
    if_host_allowed(ctx, "os_machine", Nondeterministic, TakenBytes(0), |c| {
        c.given(std::env::consts::ARCH.as_bytes().to_vec())
    })
}

pub unsafe extern "C" fn rt_os_uname(ctx: *mut Ctx) -> TakenBytes {
    if_host_allowed(ctx, "os_uname", Nondeterministic, TakenBytes(0), |c| {
        c.given(text_or_error(ply_host::os::name()))
    })
}

pub unsafe extern "C" fn rt_os_hostname(ctx: *mut Ctx) -> TakenBytes {
    if_host_allowed(ctx, "os_hostname", Nondeterministic, TakenBytes(0), |c| {
        c.given(text_or_error(ply_host::os::hostname()))
    })
}

pub unsafe extern "C" fn rt_os_cpus(ctx: *mut Ctx) -> I64 {
    if_host_allowed(ctx, "os_cpus", Nondeterministic, I64(0), |_| {
        I64(ply_host::os::cpus())
    })
}

pub unsafe extern "C" fn rt_os_uid(ctx: *mut Ctx) -> I64 {
    if_host_allowed(ctx, "os_uid", Nondeterministic, I64(0), |_| {
        I64(ply_host::os::uid())
    })
}

pub unsafe extern "C" fn rt_os_user_name(ctx: *mut Ctx, id: I64) -> TakenBytes {
    if_host_allowed(ctx, "os_user_name", Nondeterministic, TakenBytes(0), |c| {
        c.given(found(ply_host::os::user_name(id.0)))
    })
}

pub unsafe extern "C" fn rt_os_pid(ctx: *mut Ctx) -> I64 {
    if_host_allowed(ctx, "os_pid", Nondeterministic, I64(0), |_| {
        I64(i64::from(std::process::id()))
    })
}

pub unsafe extern "C" fn rt_os_executable(ctx: *mut Ctx) -> TakenBytes {
    if_host_allowed(ctx, "os_executable", Nondeterministic, TakenBytes(0), |c| {
        c.given(found(ply_host::os::executable()))
    })
}

pub unsafe extern "C" fn rt_os_big_endian(ctx: *mut Ctx) -> I64 {
    if_host_allowed(ctx, "os_big_endian", Nondeterministic, I64(0), |_| {
        I64(i64::from(cfg!(target_endian = "big")))
    })
}

pub unsafe extern "C" fn rt_os_pointer_bits(ctx: *mut Ctx) -> I64 {
    if_host_allowed(ctx, "os_pointer_bits", Nondeterministic, I64(0), |_| {
        I64(i64::from(usize::BITS))
    })
}
