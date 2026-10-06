//! The process's own standard streams and terminal. They are the process's, not a run's: a `ply`
//! that enters a program shares them with it, so there is one buffer of output, one of input and
//! one saved terminal state, whichever host writes, reads or changes them.

use std::io::{Error, ErrorKind, Result};
use std::sync::{Mutex, MutexGuard};
use std::time::Instant;

/// What standard output's buffer holds before it is written without being asked.
pub const OUT_BUFFER_BYTES: usize = 64 * 1024;

/// What one read of standard input takes at most, however much it was asked for.
pub const MAX_READ_BYTES: usize = 1024 * 1024;

/// What a shell reports of a process a closed pipe ended: 128 and `SIGPIPE`.
pub const BROKEN_PIPE_STATUS: i32 = 128 + libc::SIGPIPE;

const IN: i32 = libc::STDIN_FILENO;
const OUT: i32 = libc::STDOUT_FILENO;
const ERR: i32 = libc::STDERR_FILENO;

static HELD_OUT: Mutex<Output> = Mutex::new(Output {
    held: Vec::new(),
    gone: false,
    untold: false,
});

static HELD_IN: Mutex<Input> = Mutex::new(Input {
    held: Vec::new(),
    at: 0,
});

static TERMINAL: Mutex<Terminal> = Mutex::new(Terminal {
    original: None,
    mode: Mode::Cooked,
});

/// Whether a write's reader has gone away: a closed pipe, or a socket whose peer hung up.
pub fn reader_gone(e: &Error) -> bool {
    e.kind() == ErrorKind::BrokenPipe
}

struct Output {
    held: Vec<u8>,
    /// Standard output's reader has gone away, and it takes nothing more.
    gone: bool,
    /// Bytes a write had answered for went nowhere, and no answer since has said so.
    untold: bool,
}

impl Output {
    /// Writes what is held. The buffer is emptied whether or not its bytes were taken: a reader
    /// that went away takes none of what follows either.
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

    /// What an operation that answers for the stream is refused with, which tells its caller.
    fn told(&mut self, wrote: Result<()>) -> Result<()> {
        if wrote.is_err() {
            self.untold = false;
        }
        wrote
    }
}

/// Adds `bytes` to standard output's buffer, which is written once it holds
/// [`OUT_BUFFER_BYTES`]. An error is the caller's to tell the program.
pub fn write_out(bytes: &[u8]) -> Result<()> {
    let mut out = lock(&HELD_OUT);
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

/// Writes what standard output's buffer holds. An error is the caller's to tell the program.
pub fn flush_out() -> Result<()> {
    let mut out = lock(&HELD_OUT);
    let wrote = out.written();
    out.told(wrote)
}

/// Writes what standard output's buffer holds ahead of something that must come after it. Nobody
/// is answered here, so bytes that go nowhere are owed a telling: [`settle`] ends the process
/// over them unless a later answer told the program first.
pub fn make_way() {
    let mut out = lock(&HELD_OUT);
    if !out.gone && !out.held.is_empty() && out.written().is_err_and(|e| reader_gone(&e)) {
        out.untold = true;
    }
}

/// Writes `bytes` to standard error once what standard output's buffer held is written, so the
/// two streams carry what was written in the order it was written.
pub fn write_err(bytes: &[u8]) -> Result<()> {
    make_way();
    write_all(ERR, bytes)
}

fn write_all(fd: i32, mut bytes: &[u8]) -> Result<()> {
    while !bytes.is_empty() {
        // SAFETY: the pointer and length are one live slice's.
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
    /// Where the unread bytes of `held` start.
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

    /// One read of the descriptor onto what is held: how many bytes it gave, none at the end.
    fn fill(&mut self, max: usize) -> Result<usize> {
        // What was read is let go here, so a long input read a line at a time holds one line.
        self.held.drain(..self.at);
        self.at = 0;
        let had = self.held.len();
        self.held.resize(had + max, 0);
        let n = loop {
            // SAFETY: the pointer and length are the bytes just added to `held`.
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

/// What comes next on standard input, at most `max` bytes: what a line read left unread first,
/// else one read of the stream, so a short answer is not the end and an empty one is. Standard
/// output's buffer is written first, so a prompt is seen before its answer is waited for.
pub fn read(max: usize) -> Result<Vec<u8>> {
    read_within(max, None).map(Option::unwrap_or_default)
}

/// [`read`], giving up at `deadline`: `None` when nothing came by then.
pub fn read_within(max: usize, deadline: Option<Instant>) -> Result<Option<Vec<u8>>> {
    make_way();
    let max = max.min(MAX_READ_BYTES);
    let mut input = lock(&HELD_IN);
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

/// One line of standard input without its ending, `None` at the end; a last line needs no
/// newline. What follows the line stays for the next read, of a line or of bytes.
pub fn read_line() -> Result<Option<String>> {
    make_way();
    let mut input = lock(&HELD_IN);
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

/// Whether standard input has something to read before `deadline`; its end is something to read.
fn readable_by(deadline: Instant) -> Result<bool> {
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let ms = left.as_millis().min(i32::MAX as u128) as i32;
        let mut asked = libc::pollfd {
            fd: IN,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one `pollfd` this call alone reads and writes.
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

/// `std.process.Stream`.
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
    // SAFETY: a plain system call on a descriptor number.
    unsafe { libc::isatty(stream.descriptor()) == 1 }
}

/// The columns and rows of the terminal the program writes to or reads from: standard output's
/// if that is one, else standard error's, else standard input's.
pub fn terminal_size() -> Option<(u16, u16)> {
    [OUT, ERR, IN].into_iter().find_map(|fd| {
        // SAFETY: `size` is plain data the call only writes.
        let size = unsafe {
            let mut size: libc::winsize = std::mem::zeroed();
            (libc::ioctl(fd, libc::TIOCGWINSZ, &mut size) == 0).then_some(size)
        }?;
        (size.ws_col > 0 && size.ws_row > 0).then_some((size.ws_col, size.ws_row))
    })
}

/// `std.term.Mode`: how the terminal hands over what is typed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// As the terminal was found: a line at a time, echoed.
    Cooked,
    /// A line at a time, and nothing typed is shown.
    Silent,
    /// Each byte as it is typed, none echoed and none read as a signal.
    Raw,
}

struct Terminal {
    /// The settings standard input's terminal had before the first change.
    original: Option<libc::termios>,
    mode: Mode,
}

/// Puts standard input's terminal in `mode` and answers the mode it was in; `None`, and nothing
/// changes, when standard input is not a terminal.
pub fn set_mode(mode: Mode) -> Result<Option<Mode>> {
    let mut terminal = lock(&TERMINAL);
    let original = match terminal.original {
        Some(original) => original,
        None => {
            // SAFETY: `found` is plain data the call only writes.
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
        Mode::Silent => wanted.c_lflag &= !libc::ECHO,
        Mode::Raw => {
            // SAFETY: `wanted` is a `termios` this call alone edits.
            unsafe { libc::cfmakeraw(&mut wanted) };
            wanted.c_cc[libc::VMIN] = 1;
            wanted.c_cc[libc::VTIME] = 0;
        }
    }
    // SAFETY: `wanted` is a `termios` the call only reads. `TCSADRAIN` keeps what was typed ahead.
    if unsafe { libc::tcsetattr(IN, libc::TCSADRAIN, &wanted) } != 0 {
        return Err(Error::last_os_error());
    }
    Ok(Some(std::mem::replace(&mut terminal.mode, mode)))
}

/// Leaves the process's streams as a program that ended should: what standard output's buffer
/// holds written, and the terminal as it was found. Answers whether bytes the program was
/// answered for went nowhere without its being told, which is how a closed pipe ends a program.
pub fn settle() -> bool {
    make_way();
    let untold = std::mem::take(&mut lock(&HELD_OUT).untold);
    let mut terminal = lock(&TERMINAL);
    if let Some(original) = &terminal.original
        && terminal.mode != Mode::Cooked
    {
        // SAFETY: `original` is a `termios` the call only reads.
        unsafe { libc::tcsetattr(IN, libc::TCSADRAIN, original) };
        terminal.mode = Mode::Cooked;
    }
    untold
}

/// The guarded state has no invariant a panicking caller can break, so recovering is correct.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}
