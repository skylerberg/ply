mod socket;
pub mod wire;

pub use socket::TcpHost;
pub(crate) use socket::apply;
pub use wire::{Credentials, Endpoint, NetError, Probing, SocketOption};

use ply_core::{Diagnostic, Span, codes};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

pub const MAX_RECV: usize = 1 << 20;

pub struct Upgrade {
    pub alpn: Vec<String>,
    pub unread: usize,
    pub timeout: Duration,
}

pub struct Handles {
    open: Mutex<BTreeMap<i64, Option<String>>>,
    next: AtomicI64,
}

impl Default for Handles {
    fn default() -> Handles {
        Handles::new()
    }
}

impl Handles {
    pub fn new() -> Handles {
        Handles {
            open: Mutex::new(BTreeMap::new()),
            next: AtomicI64::new(1),
        }
    }

    pub fn open(&self, label: Option<&str>) -> i64 {
        let handle = self.next.fetch_add(1, Ordering::Relaxed);
        lock(&self.open).insert(handle, label.map(str::to_string));
        handle
    }

    pub fn check(&self, handle: i64, label: &str, span: Span) -> Result<(), Diagnostic> {
        let mut open = lock(&self.open);
        let Some(held) = open.get_mut(&handle) else {
            return Err(unknown_handle(handle, span));
        };
        match held {
            Some(existing) if existing == label => Ok(()),
            Some(existing) => Err(wrong_label(handle, existing, label, span)),
            None => {
                *held = Some(label.to_string());
                Ok(())
            }
        }
    }

    pub fn close(&self, handle: i64) {
        lock(&self.open).remove(&handle);
    }
}

#[cold]
fn wrong_label(handle: i64, existing: &str, label: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("socket {handle} is being used as `net[{label}]` and `net[{existing}]`"),
    )
    .primary(span, format!("this operation names `net[{label}]`"))
    .note(format!(
        "it was first used as `net[{existing}]`, and the resource label is what decides whether two computations conflict"
    ))
    .note("one socket under two labels is two resources the scheduler will not serialise, over one socket it must; give each socket a label and keep it")
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn bind(what: &str, port: u16, span: Span) -> Result<std::net::TcpListener, Diagnostic> {
    std::net::TcpListener::bind(("127.0.0.1", port)).map_err(|e| {
        Diagnostic::error(
            codes::RUNTIME_ERROR,
            format!("{what} could not bind 127.0.0.1:{port}: {e}"),
        )
        .primary(
            span,
            "this listen reached the host and the host rejected it",
        )
    })
}

#[cold]
fn unknown_handle(handle: i64, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("there is no open socket with handle {handle}"),
    )
    .primary(span, "this handle was closed, or never opened")
    .note("handles ascend and are never reused, so a handle past its `net.close` names nothing rather than naming whatever opened next")
}

#[cold]
fn not_a_listener(handle: i64, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("socket {handle} is a connection, and `net.accept` wants a listener"),
    )
    .primary(span, "this handle came from `net.accept`, not `net.listen`")
}

#[cold]
fn not_a_stream(handle: i64, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("socket {handle} is a listener, and this operation wants a connection"),
    )
    .primary(span, "this handle came from `net.listen`, not `net.accept`")
}

#[cold]
fn not_upgradable(what: &str, handle: i64, reason: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("{what} cannot secure socket {handle}: {reason}"),
    )
    .primary(span, "an upgrade takes a plaintext connection nothing else is waiting on")
    .note("the handle is the secured connection afterwards, so a read or a write still in flight on the plaintext would be one the upgrade could not account for")
}

#[cold]
fn err_datagram_for_stream_socket(handle: i64, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("socket {handle} is a datagram socket, and this operation wants a `net` one"),
    )
    .primary(span, "this handle came from `udp.bind`")
    .note("a datagram socket is read with `udp.recv_from` and written with `udp.send_to`")
}

#[cold]
fn not_a_datagram_socket(handle: i64, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("socket {handle} is not a datagram socket, and this operation wants one"),
    )
    .primary(span, "this handle did not come from `udp.bind`")
}
