//! What the machine and the process a program runs in say of themselves, as the natives `std.os`'s
//! host handler calls read it. None of it reads the environment, so no variable a user sets changes
//! an answer.

/// The processors this process may run on at once: what its affinity mask and its control
/// group's quota leave of the machine's, where the platform says.
pub fn cpus() -> i64 {
    std::thread::available_parallelism().map_or(1, |n| n.get() as i64)
}

/// A C string a system call wrote into `bytes`, to its first NUL.
#[cfg(unix)]
fn c_text(bytes: &[libc::c_char]) -> String {
    let text: Vec<u8> = bytes
        .iter()
        .take_while(|c| **c != 0)
        .map(|c| *c as u8)
        .collect();
    String::from_utf8_lossy(&text).into_owned()
}

/// The system's own name for itself and its release, as `uname -sr` prints them.
#[cfg(unix)]
pub fn name() -> std::io::Result<String> {
    // SAFETY: `said` is plain data the call only writes.
    let said = unsafe {
        let mut said: libc::utsname = std::mem::zeroed();
        if libc::uname(&mut said) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        said
    };
    Ok(format!(
        "{} {}",
        c_text(&said.sysname),
        c_text(&said.release)
    ))
}

#[cfg(not(unix))]
pub fn name() -> std::io::Result<String> {
    Ok(std::env::consts::OS.to_string())
}

/// The name the kernel holds for this machine; no resolver is asked.
#[cfg(unix)]
pub fn hostname() -> std::io::Result<String> {
    let mut name = [0 as libc::c_char; 256];
    // SAFETY: the pointer and length are `name`'s, less the byte that keeps it terminated.
    if unsafe { libc::gethostname(name.as_mut_ptr(), name.len() - 1) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(c_text(&name))
}

#[cfg(not(unix))]
pub fn hostname() -> std::io::Result<String> {
    Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
}

/// The id of the user this process runs as.
#[cfg(unix)]
pub fn uid() -> i64 {
    // SAFETY: a plain system call that cannot fail.
    i64::from(unsafe { libc::getuid() })
}

#[cfg(not(unix))]
pub fn uid() -> i64 {
    0
}

/// The name the system's own records give the user of id `id`: `None` where they hold no entry
/// for it, as in a container run under an arbitrary id.
#[cfg(unix)]
pub fn user_name(id: i64) -> Option<String> {
    let id = libc::uid_t::try_from(id).ok()?;
    let mut text = vec![0 as libc::c_char; 16 * 1024];
    // SAFETY: `entry` is plain data the call only writes, `text` backs the strings it points at
    // and outlives every read of them, and `found` is null unless the call filled `entry`.
    unsafe {
        let mut entry: libc::passwd = std::mem::zeroed();
        let mut found: *mut libc::passwd = std::ptr::null_mut();
        let answered = libc::getpwuid_r(id, &mut entry, text.as_mut_ptr(), text.len(), &mut found);
        (answered == 0 && !found.is_null() && !entry.pw_name.is_null()).then(|| {
            std::ffi::CStr::from_ptr(entry.pw_name)
                .to_string_lossy()
                .into_owned()
        })
    }
}

#[cfg(not(unix))]
pub fn user_name(_id: i64) -> Option<String> {
    None
}

/// The file this process is running, as an absolute path with its links followed, so two
/// processes of one program answer alike however each was started.
pub fn executable() -> Option<String> {
    let started_as = std::env::current_exe().ok()?;
    let path = started_as.canonicalize().ok()?;
    Some(path.to_string_lossy().into_owned())
}
