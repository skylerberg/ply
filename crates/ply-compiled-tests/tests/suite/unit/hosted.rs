mod meter;
mod time;
mod toolchain;

use crate::bare::context;
use ply_compiled::hosted::signal::rt_signal_arrived;
use ply_compiled::hosted::term::rt_term_secret_taken;
use ply_compiled::hosted::{rt_job_wait, rt_kdf_bcrypt, rt_observed, rt_os_cpus};
use ply_compiled::native::{Addr, I64};
use ply_compiled::rt::{Ctx, rt_taken};
use ply_compiled::secret::{rt_secret_len, rt_secret_of};
use ply_core::host::MachineId;
use ply_core::{HostBinding, HostRuntime, Span};
use ply_host::jobs::{self, Work};
use ply_host::observe::{Binary, Recorder, World};
use ply_host::pool::Kept;
use std::rc::Rc;
use std::sync::Arc;

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

/// The ticket of `job`, submitted for `ctx`'s run as a native submits one.
fn submitted(
    ctx: &mut Ctx,
    work: Work,
    what: &'static str,
    job: impl FnOnce() -> Result<Vec<u8>, String> + Send + 'static,
) -> I64 {
    let runtime = hosted(ctx);
    let run = Arc::clone(ply_host::registry::run_of(&*runtime).expect("a host made it"));
    let ticket = jobs::submit(work, &run, Span::DUMMY, "test", what, job).expect("a thread is had");
    I64(ticket.token as i64)
}

/// `ctx` over a runtime of a host that holds nothing: no stop, no roots, no process.
fn hosted(ctx: &mut Ctx) -> Rc<dyn HostRuntime> {
    let runtime = ply_host::Host::new().runtime();
    ctx.set_host(
        Arc::new(HostBinding::unbound()),
        Some(Rc::clone(&runtime)),
        None,
    );
    runtime
}

/// A line read for a `Secret` is kept by the run that asked: the second native takes it from that
/// run, and one whose entry point ended between the two natives is gone.
#[test]
fn a_secret_line_is_taken_from_its_run_and_none_outlives_the_entry_point() {
    let mut ctx = context();
    let runtime = hosted(&mut ctx);
    let run = Arc::clone(ply_host::registry::run_of(&*runtime).expect("a host made it"));
    let kept = run.keep_secret(Kept::new(b"hunter2".to_vec()));
    let secret = unsafe { rt_term_secret_taken(&mut ctx, I64(kept)) };
    assert!(ctx.take_failure().is_none());
    assert_eq!(unsafe { rt_secret_len(&mut ctx, secret) }.0, 7);

    let left = run.keep_secret(Kept::new(b"hunter3".to_vec()));
    let _ = runtime.end_entry_point(MachineId::next());
    unsafe { rt_term_secret_taken(&mut ctx, I64(left)) };
    assert!(
        ctx.take_failure().is_some(),
        "a line outlived the entry point that read it"
    );
}

/// A run handed no stop hears no signal: its ask answers `false` and installs no handler, so the
/// platform's handling stays for every other run of the process, a `ctrl-C` on `ply test` among
/// them.
#[test]
fn a_run_that_listens_for_no_stop_installs_no_signal_handler() {
    let mut ctx = context();
    let _runtime = hosted(&mut ctx);
    assert_eq!(unsafe { rt_signal_arrived(&mut ctx, I64(1)) }.0, 0);
    assert!(ctx.take_failure().is_none());
    // SAFETY: with no action handed, the call only writes `found`.
    let found = unsafe {
        let mut found: libc::sigaction = std::mem::zeroed();
        assert_eq!(
            libc::sigaction(libc::SIGUSR1, std::ptr::null(), &mut found),
            0
        );
        found
    };
    assert_eq!(
        found.sa_sigaction,
        libc::SIG_DFL,
        "`SIGUSR1` was given a handler"
    );
}

/// What a real handler read goes into its run's record under the root its label names, once the run
/// is observed; a label no root is bound to, and a run nobody observes, record nothing.
#[test]
fn a_real_handler_s_read_is_entered_into_its_run_s_record_by_its_root() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.db"), b"data").unwrap();
    let spec = ply_host::fs::RootSpec {
        name: "app".to_string(),
        path: dir.path().to_path_buf(),
    };
    let roots = ply_host::fs::Roots::load(&[spec], Span::DUMMY).expect("a directory");
    let runtime = ply_host::Host::new().rooted(roots).runtime();
    let mut ctx = context();
    ctx.set_host(
        Arc::new(HostBinding::unbound()),
        Some(Rc::clone(&runtime)),
        None,
    );
    let note = |ctx: &mut Ctx, label: &[u8], path: &[u8], kind: i64| {
        let (label_at, label_n) = addr(label);
        let (path_at, path_n) = addr(path);
        unsafe { rt_observed(ctx, label_at, label_n, path_at, path_n, I64(kind)) };
    };
    note(&mut ctx, b"app", b"before.db", 0);
    let recorder = Arc::new(Recorder::default());
    ply_host::observe::records_into(&*runtime, &recorder);
    note(&mut ctx, b"app", b"a.db", 0);
    note(&mut ctx, b"elsewhere", b"a.db", 0);
    note(&mut ctx, b"app", b"made.db", 4);
    note(&mut ctx, b"app", b"made.db", 0);
    assert!(ctx.take_failure().is_none());
    // Where the root lies with its links resolved, as a binding lends it.
    let lent = [("app".to_string(), dir.path().canonicalize().unwrap())];
    let shipped = |_: &str| None;
    let config = std::collections::BTreeMap::new();
    let world = World {
        roots: &lent,
        binding: "b".to_string(),
        config: &config,
        binary: Binary {
            shipped: &shipped,
            program: "p".to_string(),
        },
    };
    let trace = ply_host::observe::finished(&recorder, &world, false).expect("a trace");
    let read: Vec<&str> = trace
        .lines()
        .map(|line| line.rsplit_once('\t').map_or(line, |(at, _)| at))
        .collect();
    assert_eq!(read, ["file\tapp\ta.db\t"], "{trace}");
}

/// What a real handler read of the binary goes into its run's record without a root: a shipped
/// module by its name and the program the launcher entered, each digested by the binary.
#[test]
fn a_real_handler_s_read_of_the_binary_is_entered_into_its_run_s_record() {
    let runtime = ply_host::Host::new().runtime();
    let mut ctx = context();
    ctx.set_host(
        Arc::new(HostBinding::unbound()),
        Some(Rc::clone(&runtime)),
        None,
    );
    let recorder = Arc::new(Recorder::default());
    ply_host::observe::records_into(&*runtime, &recorder);
    for (path, kind) in [(&b"std.list"[..], 5), (b"", 6)] {
        let (label_at, label_n) = addr(b"");
        let (path_at, path_n) = addr(path);
        unsafe { rt_observed(&mut ctx, label_at, label_n, path_at, path_n, I64(kind)) };
    }
    assert!(ctx.take_failure().is_none());
    let shipped = |name: &str| (name == "std.list").then(|| "listed".to_string());
    let config = std::collections::BTreeMap::new();
    let world = World {
        roots: &[],
        binding: "b".to_string(),
        config: &config,
        binary: Binary {
            shipped: &shipped,
            program: "entered".to_string(),
        },
    };
    let trace = ply_host::observe::finished(&recorder, &world, false).expect("a trace");
    assert!(trace.contains("shipped\tstd.list\tlisted"), "{trace}");
    assert!(trace.contains("program\tentered"), "{trace}");
}

/// Outside a production region the thread waits for a job, and is handed its bytes.
#[test]
fn a_jobs_bytes_come_back_through_job_wait() {
    let mut ctx = context();
    let ticket = submitted(&mut ctx, Work::Computing, "a test job", || {
        Ok(b"computed".to_vec())
    });
    let n = unsafe { rt_job_wait(&mut ctx, ticket) };
    assert!(ctx.take_failure().is_none());
    assert_eq!(taken(&mut ctx, n.0), b"computed");
}

/// A job that failed fails the run, naming what asked for it.
#[test]
fn a_failed_job_fails_the_run_that_waits_on_it() {
    let mut ctx = context();
    let ticket = submitted(&mut ctx, Work::Waiting, "`kdf.scrypt`", || {
        Err("no memory to be had".to_string())
    });
    unsafe { rt_job_wait(&mut ctx, ticket) };
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
