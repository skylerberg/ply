pub fn cpus() -> i64 {
    std::thread::available_parallelism().map_or(1, |n| n.get() as i64)
}

#[cfg(unix)]
fn c_text(bytes: &[libc::c_char]) -> String {
    let text: Vec<u8> = bytes
        .iter()
        .take_while(|c| **c != 0)
        .map(|c| *c as u8)
        .collect();
    String::from_utf8_lossy(&text).into_owned()
}

#[cfg(unix)]
pub fn name() -> std::io::Result<String> {
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

#[cfg(unix)]
pub fn hostname() -> std::io::Result<String> {
    let mut name = [0 as libc::c_char; 256];
    if unsafe { libc::gethostname(name.as_mut_ptr(), name.len() - 1) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(c_text(&name))
}

#[cfg(not(unix))]
pub fn hostname() -> std::io::Result<String> {
    Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
}

#[cfg(unix)]
pub fn uid() -> i64 {
    i64::from(unsafe { libc::getuid() })
}

#[cfg(not(unix))]
pub fn uid() -> i64 {
    0
}

#[cfg(unix)]
pub fn user_name(id: i64) -> Option<String> {
    let id = libc::uid_t::try_from(id).ok()?;
    let mut text = vec![0 as libc::c_char; 16 * 1024];
    unsafe {
        let mut entry: libc::passwd = std::mem::zeroed();
        let mut found: *mut libc::passwd = std::ptr::null_mut();
        let returned = libc::getpwuid_r(id, &mut entry, text.as_mut_ptr(), text.len(), &mut found);
        (returned == 0 && !found.is_null() && !entry.pw_name.is_null()).then(|| {
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

pub fn executable() -> Option<String> {
    let started_as = std::env::current_exe().ok()?;
    let path = started_as.canonicalize().ok()?;
    Some(path.to_string_lossy().into_owned())
}
