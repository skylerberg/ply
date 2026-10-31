use std::io::{Error, ErrorKind, Result};
use std::sync::{Mutex, MutexGuard};
use std::time::Instant;

pub const OUT_BUFFER_BYTES: usize = 64 * 1024;

pub const MAX_READ_BYTES: usize = 1024 * 1024;

pub const BROKEN_PIPE_STATUS: i32 = 128 + libc::SIGPIPE;

const IN: i32 = libc::STDIN_FILENO;
const OUT: i32 = libc::STDOUT_FILENO;
const ERR: i32 = libc::STDERR_FILENO;

static OUTPUT_BUFFER: Mutex<Output> = Mutex::new(Output {
    held: Vec::new(),
    gone: false,
    untold: false,
});

static INPUT_BUFFER: Mutex<Input> = Mutex::new(Input {
    held: Vec::new(),
    at: 0,
});

static TERMINAL: Mutex<Terminal> = Mutex::new(Terminal {
    original: None,
    mode: Mode::Cooked,
});

pub fn reader_gone(e: &Error) -> bool {
    e.kind() == ErrorKind::BrokenPipe
}

struct Output {
    held: Vec<u8>,
    gone: bool,
    untold: bool,
}

impl Output {
    fn written(&mut self) -> Result<()> {
        if self.gone {
            return Err(ErrorKind::BrokenPipe.into());
        }
        let wrote = write_all(OUT, &self.held);
        self.held.clear();
        if wrote.as_ref().is_err_and(reader_gone) {
            self.gone = true;
        }
        wrote
    }

    fn told(&mut self, wrote: Result<()>) -> Result<()> {
        if wrote.is_err() {
            self.untold = false;
        }
        wrote
    }
}

pub fn write_out(bytes: &[u8]) -> Result<()> {
    let mut out = lock(&OUTPUT_BUFFER);
    if !out.gone {
        out.held.extend_from_slice(bytes);
    }
    let wrote = if out.gone || out.held.len() >= OUT_BUFFER_BYTES {
        out.written()
    } else {
        Ok(())
    };
    out.told(wrote)
}

pub fn flush_out() -> Result<()> {
    let mut out = lock(&OUTPUT_BUFFER);
    let wrote = out.written();
    out.told(wrote)
}

pub fn make_way() {
    let mut out = lock(&OUTPUT_BUFFER);
    if !out.gone && !out.held.is_empty() && out.written().is_err_and(|e| reader_gone(&e)) {
        out.untold = true;
    }
}

pub fn write_err(bytes: &[u8]) -> Result<()> {
    make_way();
    write_all(ERR, bytes)
}

fn write_all(fd: i32, mut bytes: &[u8]) -> Result<()> {
    while !bytes.is_empty() {
        let n = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
        if n < 0 {
            let e = Error::last_os_error();
            if e.kind() == ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        bytes = &bytes[n as usize..];
    }
    Ok(())
}

struct Input {
    held: Vec<u8>,
    at: usize,
}

impl Input {
    fn unread(&self) -> &[u8] {
        &self.held[self.at..]
    }

    fn take(&mut self, n: usize) -> Vec<u8> {
        let taken = self.held[self.at..self.at + n].to_vec();
        self.at += n;
        if self.at == self.held.len() {
            self.held.clear();
            self.at = 0;
        }
        taken
    }

    fn fill(&mut self, max: usize) -> Result<usize> {
        self.held.drain(..self.at);
        self.at = 0;
        let had = self.held.len();
        self.held.resize(had + max, 0);
        let n = loop {
            let n = unsafe { libc::read(IN, self.held[had..].as_mut_ptr().cast(), max) };
            if n >= 0 {
                break n as usize;
            }
            let e = Error::last_os_error();
            if e.kind() != ErrorKind::Interrupted {
                self.held.truncate(had);
                return Err(e);
            }
        };
        self.held.truncate(had + n);
        Ok(n)
    }
}

pub fn read(max: usize) -> Result<Vec<u8>> {
    read_within(max, None).map(Option::unwrap_or_default)
}

pub fn read_within(max: usize, deadline: Option<Instant>) -> Result<Option<Vec<u8>>> {
    make_way();
    let max = max.min(MAX_READ_BYTES);
    let mut input = lock(&INPUT_BUFFER);
    if input.unread().is_empty() {
        if max == 0 {
            return Ok(Some(Vec::new()));
        }
        if let Some(deadline) = deadline
            && !readable_by(deadline)?
        {
            return Ok(None);
        }
        input.fill(max)?;
    }
    let n = input.unread().len().min(max);
    Ok(Some(input.take(n)))
}

pub fn read_line() -> Result<Option<String>> {
    make_way();
    let mut input = lock(&INPUT_BUFFER);
    let mut searched = 0;
    let line = loop {
        if let Some(i) = input.unread()[searched..].iter().position(|b| *b == b'\n') {
            break input.take(searched + i + 1);
        }
        searched = input.unread().len();
        if input.fill(8 * 1024)? == 0 {
            if searched == 0 {
                return Ok(None);
            }
            break input.take(searched);
        }
    };
    let mut text = String::from_utf8(line)
        .map_err(|_| Error::new(ErrorKind::InvalidData, "stream did not contain valid UTF-8"))?;
    while text.ends_with('\n') || text.ends_with('\r') {
        text.pop();
    }
    Ok(Some(text))
}

pub type SecretLine = zeroize::Zeroizing<String>;

pub fn read_secret_line() -> Result<Option<SecretLine>> {
    Ok(read_line()?.map(zeroize::Zeroizing::new))
}

fn readable_by(deadline: Instant) -> Result<bool> {
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let ms = left.as_millis().min(i32::MAX as u128) as i32;
        let mut asked = libc::pollfd {
            fd: IN,
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut asked, 1, ms) };
        if ready > 0 {
            return Ok(true);
        }
        if ready == 0 {
            if Instant::now() >= deadline {
                return Ok(false);
            }
            continue;
        }
        let e = Error::last_os_error();
        if e.kind() != ErrorKind::Interrupted {
            return Err(e);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Standard {
    In,
    Out,
    Err,
}

impl Standard {
    fn descriptor(self) -> i32 {
        match self {
            Standard::In => IN,
            Standard::Out => OUT,
            Standard::Err => ERR,
        }
    }
}

pub fn is_terminal(stream: Standard) -> bool {
    unsafe { libc::isatty(stream.descriptor()) == 1 }
}

pub fn terminal_size() -> Option<(u16, u16)> {
    [OUT, ERR, IN].into_iter().find_map(|fd| {
        let size = unsafe {
            let mut size: libc::winsize = std::mem::zeroed();
            (libc::ioctl(fd, libc::TIOCGWINSZ, &mut size) == 0).then_some(size)
        }?;
        (size.ws_col > 0 && size.ws_row > 0).then_some((size.ws_col, size.ws_row))
    })
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Cooked,
    NoEcho,
    Raw,
}

struct Terminal {
    original: Option<libc::termios>,
    mode: Mode,
}

pub fn set_mode(mode: Mode) -> Result<Option<Mode>> {
    let mut terminal = lock(&TERMINAL);
    let original = match terminal.original {
        Some(original) => original,
        None => {
            let found = unsafe {
                let mut found: libc::termios = std::mem::zeroed();
                (libc::tcgetattr(IN, &mut found) == 0).then_some(found)
            };
            let Some(found) = found else {
                return Ok(None);
            };
            terminal.original = Some(found);
            found
        }
    };
    let mut wanted = original;
    match mode {
        Mode::Cooked => {}
        Mode::NoEcho => wanted.c_lflag &= !libc::ECHO,
        Mode::Raw => {
            unsafe { libc::cfmakeraw(&mut wanted) };
            wanted.c_cc[libc::VMIN] = 1;
            wanted.c_cc[libc::VTIME] = 0;
        }
    }
    if unsafe { libc::tcsetattr(IN, libc::TCSADRAIN, &wanted) } != 0 {
        return Err(Error::last_os_error());
    }
    Ok(Some(std::mem::replace(&mut terminal.mode, mode)))
}

pub fn settle() -> bool {
    make_way();
    let untold = std::mem::take(&mut lock(&OUTPUT_BUFFER).untold);
    restore_terminal();
    untold
}

pub fn restore_terminal() {
    let mut terminal = lock(&TERMINAL);
    if let Some(original) = &terminal.original
        && terminal.mode != Mode::Cooked
    {
        unsafe { libc::tcsetattr(IN, libc::TCSADRAIN, original) };
        terminal.mode = Mode::Cooked;
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}
