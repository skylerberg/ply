use crate::bare::context;
use ply_compiled::hosted::{rt_job_wait, rt_kdf_bcrypt, rt_os_cpus};
use ply_compiled::machine::rt_secret_of;
use ply_compiled::native::{Addr, I64};
use ply_compiled::rt::{Ctx, rt_taken};
use ply_core::Span;
use ply_host::jobs::{self, Work};

/// The bytes a native left taken, `n` of them, copied out as emitted code copies them.
fn taken(ctx: &mut Ctx, n: i64) -> Vec<u8> {
    let mut out = vec![0u8; usize::try_from(n).expect("a count")];
    unsafe { rt_taken(ctx, Addr(out.as_mut_ptr()), I64(n)) };
    out
}

fn addr(bytes: &[u8]) -> (Addr, I64) {
    (Addr(bytes.as_ptr().cast_mut()), I64(bytes.len() as i64))
}

#[test]
fn a_native_that_asks_the_machine_leaves_the_run_having_read_it() {
    let mut ctx = context();
    assert!(!ctx.take_host_use().0.acted());
    let cpus = unsafe { rt_os_cpus(&mut ctx) };
    assert!(cpus.0 >= 1);
    assert!(ctx.take_failure().is_none());
    let (used, at_most_once) = ctx.take_host_use();
    assert!(
        used.acted(),
        "a read of the machine left the run hermetic: {used:?}"
    );
    assert_eq!(at_most_once, 0);
}

/// Outside a production region the thread waits for a job, and is handed its bytes.
#[test]
fn a_jobs_bytes_come_back_through_job_wait() {
    let mut ctx = context();
    let ticket = jobs::submit(Work::Computing, Span::DUMMY, "test", "a test job", || {
        Ok(b"computed".to_vec())
    })
    .expect("a thread is had");
    let n = unsafe { rt_job_wait(&mut ctx, I64(ticket.token as i64)) };
    assert!(ctx.take_failure().is_none());
    assert_eq!(taken(&mut ctx, n.0), b"computed");
}

/// A job that failed fails the run, naming what asked for it.
#[test]
fn a_failed_job_fails_the_run_that_waits_on_it() {
    let mut ctx = context();
    let ticket = jobs::submit(Work::Waiting, Span::DUMMY, "test", "`kdf.scrypt`", || {
        Err("no memory to be had".to_string())
    })
    .expect("a thread is had");
    unsafe { rt_job_wait(&mut ctx, I64(ticket.token as i64)) };
    let failed = ctx.take_failure().expect("the run failed");
    assert!(
        failed.message.contains("`kdf.scrypt`"),
        "{}",
        failed.message
    );
}

/// A hash is computed off the program's thread and is a function of what it was handed, so the
/// run is recorded as having asked the host and read nothing of it; one past its bounds is refused
/// before any job starts, saying which number and nothing of the password.
#[test]
fn a_hash_is_a_ticket_of_a_deterministic_job_and_one_past_its_bounds_is_refused() {
    let mut ctx = context();
    let (password_at, password_len) = addr(b"");
    let secret = unsafe { rt_secret_of(&mut ctx, password_at, password_len, I64(0)) };
    let salt = [
        0x14, 0x4b, 0x3d, 0x69, 0x1a, 0x7b, 0x4e, 0xcf, 0x39, 0xcf, 0x73, 0x5c, 0x7f, 0xa7, 0xa7,
        0x9c,
    ];
    let (salt_at, salt_len) = addr(&salt);
    let ticket = unsafe { rt_kdf_bcrypt(&mut ctx, secret, salt_at, salt_len, I64(6)) };
    assert!(ctx.take_failure().is_none());
    let n = unsafe { rt_job_wait(&mut ctx, ticket) };
    // jBCrypt's tag of the empty password under that salt at cost 6.
    let tag: Vec<u8> = (0..23)
        .map(|i| {
            u8::from_str_radix(
                &"557e94f34bf286e8719a26be94ac1e16d95ef9f819dee0"[2 * i..2 * i + 2],
                16,
            )
            .expect("hex")
        })
        .collect();
    assert_eq!(taken(&mut ctx, n.0), tag);
    let (used, _) = ctx.take_host_use();
    assert_eq!(used.operations, 1);
    assert!(!used.acted(), "a hash read the machine: {used:?}");

    let (other_at, other_len) = addr(b"correct horse");
    let other = unsafe { rt_secret_of(&mut ctx, other_at, other_len, I64(0)) };
    let refused = unsafe { rt_kdf_bcrypt(&mut ctx, other, salt_at, salt_len, I64(31)) };
    assert_eq!(refused.0, 0);
    let failed = ctx.take_failure().expect("the cost was refused");
    assert!(
        failed.message.contains("a cost of 31"),
        "{}",
        failed.message
    );
    assert!(!failed.message.contains("horse"), "{}", failed.message);
}
