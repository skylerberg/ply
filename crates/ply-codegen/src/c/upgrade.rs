//! The embedded bundle's optimised object: `cc -O1` builds it once per bundle and compiler in a
//! detached process, and runs load the development profile's object until it lands.

use super::load::{Library, cache_dir, compile_and_load, ext, key_of, which};
use super::toolchain::Profile;
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

const CC: &str = "cc";

const FLAGS: &[&str] = &["-O1", "-fwrapv", "-fPIC", "-shared", "-fno-strict-aliasing"];

/// A lock this old is abandoned whatever its pid says, since the pid may have been reused.
const STALE: Duration = Duration::from_secs(60 * 60);

/// The compile, run by `sh`: the object lands by a rename, a failure is recorded so this key is
/// not tried again, and the lock goes either way.
const SCRIPT: &str = r#"c=$1 obj=$2 failed=$3 lock=$4
shift 4
echo $$ > "$lock"
nice=
command -v nice >/dev/null 2>&1 && nice="nice -n 19"
if $nice "$@" -o "$obj.$$.tmp" "$c" 2> "$failed.$$.log"; then
  mv -f "$obj.$$.tmp" "$obj"
else
  mv -f "$failed.$$.log" "$failed"
fi
rm -f "$c" "$obj.$$.tmp" "$failed.$$.log" "$lock"
"#;

unsafe extern "C" {
    fn kill(pid: i32, sig: std::ffi::c_int) -> std::ffi::c_int;
}

/// Where `source`'s optimised object lands, or `None` when nothing is upgraded: a profile other
/// than development, a compiler pinned by `PLY_CC` or `PLY_CC_OPT`, or no `cc`.
pub fn object(source: &str) -> Option<PathBuf> {
    Job::of(source).map(|job| job.object)
}

/// `source`'s object: the optimised one once it is built, else the development profile's, with the
/// optimised compile started in the background when nobody has started it.
pub fn load(source: &str, stem: &str) -> Result<Library> {
    super::sweep::once();
    if let Some(job) = Job::of(source) {
        if let Some(lib) = job.open() {
            return Ok(lib);
        }
        job.start(source);
    }
    compile_and_load(source, stem)
}

/// The background compile, run to completion; whether the object is there after it.
pub fn compile(source: &str) -> bool {
    let Some(job) = Job::of(source) else {
        return false;
    };
    let _ = std::fs::create_dir_all(cache_dir());
    let c = scratch();
    let _ = std::fs::write(&c, source).and_then(|()| job.command(&c).status());
    let _ = std::fs::remove_file(&c);
    job.object.is_file()
}

struct Job {
    object: PathBuf,
    lock: PathBuf,
    failed: PathBuf,
}

impl Job {
    fn of(source: &str) -> Option<Job> {
        let pinned =
            std::env::var_os("PLY_CC").is_some() || std::env::var_os("PLY_CC_OPT").is_some();
        if pinned || Profile::current() != Profile::Development || which(CC).is_none() {
            return None;
        }
        let key = key_of(CC, source, &FLAGS.join(" "));
        let at = |suffix: &str| cache_dir().join(format!("{key}.{suffix}"));
        Some(Job {
            object: at(ext()),
            lock: at("lock"),
            failed: at("failed"),
        })
    }

    /// The optimised object, when it is built and loads. One that will not load is recorded as a
    /// failure, so no later run loads it or compiles it again.
    fn open(&self) -> Option<Library> {
        if !self.object.is_file() {
            return None;
        }
        match Library::open(&self.object) {
            Ok(lib) => Some(lib),
            Err(e) => {
                let _ = std::fs::write(&self.failed, format!("{e:#}"));
                let _ = std::fs::remove_file(&self.object);
                None
            }
        }
    }

    fn start(&self, source: &str) {
        let _ = std::fs::create_dir_all(cache_dir());
        if self.failed.exists() || !self.claim() {
            return;
        }
        let c = scratch();
        let spawned = std::fs::write(&c, source).and_then(|()| {
            let mut command = self.command(&c);
            command
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            // A group of its own, so the terminal's interrupt to this run does not reach it.
            #[cfg(unix)]
            std::os::unix::process::CommandExt::process_group(&mut command, 0);
            command.spawn()
        });
        match spawned {
            // Reaped, so a long-lived process is not left holding a zombie.
            Ok(mut child) => {
                let _ = std::thread::Builder::new()
                    .name("ply-c-upgrade".to_string())
                    .spawn(move || child.wait());
            }
            Err(e) => {
                let _ = std::fs::write(&self.failed, e.to_string());
                let _ = std::fs::remove_file(&c);
                let _ = std::fs::remove_file(&self.lock);
            }
        }
    }

    /// Takes the lock, first clearing one whose process is gone or that is older than [`STALE`].
    fn claim(&self) -> bool {
        for _ in 0..2 {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&self.lock)
            {
                Ok(_) => return true,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && stale(&self.lock) => {
                    let _ = std::fs::remove_file(&self.lock);
                }
                Err(_) => return false,
            }
        }
        false
    }

    fn command(&self, c: &Path) -> Command {
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg(SCRIPT)
            .arg("sh")
            .arg(c)
            .arg(&self.object)
            .arg(&self.failed)
            .arg(&self.lock)
            .arg(CC)
            .args(FLAGS);
        command
    }
}

/// Where one compile's copy of the source goes; the script removes it when it is done.
fn scratch() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("ply-c-{}-upgrade-{n}.c", std::process::id()))
}

fn stale(lock: &Path) -> bool {
    let old = match std::fs::metadata(lock).and_then(|m| m.modified()) {
        Ok(written) => written.elapsed().is_ok_and(|age| age > STALE),
        Err(_) => true,
    };
    let gone = std::fs::read_to_string(lock)
        .ok()
        .and_then(|s| s.trim().parse::<i32>().ok())
        .is_some_and(|pid| !alive(pid));
    old || gone
}

/// `kill` with no signal: `EPERM` still means the process exists.
fn alive(pid: i32) -> bool {
    const EPERM: i32 = 1;
    pid > 0
        && (unsafe { kill(pid, 0) } == 0
            || std::io::Error::last_os_error().raw_os_error() == Some(EPERM))
}
