//! The real one: `net` over loopback TCP.
//!
//! `listen` and `close` are syscalls that do not wait, so they answer
//! [`HostAnswer::Value`]. `accept`, `recv` and `send` do wait, so they answer
//! [`HostAnswer::Pending`] over [`Pool`] and never occupy the thread the
//! scheduler is running tasks on.

use super::pool::{Done, Pool};
use super::{Handles, Net, Op, not_a_listener, not_a_stream, unknown_handle};
use ply_core::ty::Resource;
use ply_eval::{HostAnswer, HostRuntime, Pending, Value};
use ply_span::{Diagnostic, Span, codes};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex, MutexGuard};

enum Sock {
    Listener(Arc<TcpListener>),
    Stream(Arc<TcpStream>),
}

/// The socket table and the handle allocator, in one [`Arc`] because a pool
/// thread completing an `accept` has to insert into both.
struct Sockets {
    open: Mutex<BTreeMap<i64, Sock>>,
    handles: Handles,
}

impl Sockets {
    fn insert(&self, at: Option<&Resource>, sock: Sock) -> i64 {
        let handle = self.handles.open(at);
        lock(&self.open).insert(handle, sock);
        handle
    }

    fn listener(
        &self,
        handle: i64,
        at: &Resource,
        span: Span,
    ) -> Result<Arc<TcpListener>, Diagnostic> {
        self.handles.check(handle, at, span)?;
        match lock(&self.open).get(&handle) {
            Some(Sock::Listener(l)) => Ok(Arc::clone(l)),
            Some(Sock::Stream(_)) => Err(not_a_listener(handle, span)),
            None => Err(unknown_handle(handle, span)),
        }
    }

    fn stream(&self, handle: i64, at: &Resource, span: Span) -> Result<Arc<TcpStream>, Diagnostic> {
        self.handles.check(handle, at, span)?;
        match lock(&self.open).get(&handle) {
            Some(Sock::Stream(s)) => Ok(Arc::clone(s)),
            Some(Sock::Listener(_)) => Err(not_a_stream(handle, span)),
            None => Err(unknown_handle(handle, span)),
        }
    }
}

/// The TCP host handler's state: one socket table and one blocking pool.
///
/// Also the [`HostRuntime`] that resolves the tokens it mints. That is not where
/// the boundary's design puts the runtime, and the reason it is here is that
/// `HostRuntime` has no submission API: a handler that answers `Pending` has to
/// own the facility that resolves it. [`TcpHost::owns`] exists so a composed
/// runtime can route a token to whoever minted it.
pub struct TcpHost {
    sockets: Arc<Sockets>,
    pool: Pool,
}

impl Default for TcpHost {
    fn default() -> TcpHost {
        TcpHost::new()
    }
}

impl TcpHost {
    pub fn new() -> TcpHost {
        TcpHost {
            sockets: Arc::new(Sockets {
                open: Mutex::new(BTreeMap::new()),
                handles: Handles::new(),
            }),
            pool: Pool::new(),
        }
    }

    /// The address a listening handle actually bound.
    ///
    /// Rust-side only: W1's `net` has no operation for it, so a Ply program that
    /// binds port 0 cannot discover the port it got. A test can, which is what
    /// this is for.
    pub fn local_addr(&self, handle: i64) -> Option<SocketAddr> {
        match lock(&self.sockets.open).get(&handle) {
            Some(Sock::Listener(l)) => l.local_addr().ok(),
            _ => None,
        }
    }

    /// Whether this host minted the token. A composed [`HostRuntime`] routes on
    /// it; polling the wrong facility loses a result rather than waiting for it.
    pub fn owns(&self, pending: &Pending) -> bool {
        self.pool.owns(pending)
    }

    pub fn outstanding(&self) -> usize {
        self.pool.outstanding()
    }

    fn waiting(
        &self,
        span: Span,
        label: &'static str,
        what: &'static str,
        job: impl FnOnce() -> Done + Send + 'static,
    ) -> Result<HostAnswer, Diagnostic> {
        self.pool
            .submit(span, label, what, Box::new(job))
            .map(HostAnswer::Pending)
    }
}

impl Net for TcpHost {
    fn waits(&self) -> bool {
        true
    }

    fn path(&self, op: Op) -> &'static str {
        match op {
            Op::Listen => "ply_host::tcp::listen",
            Op::Accept => "ply_host::tcp::accept",
            Op::Recv => "ply_host::tcp::recv",
            Op::Send => "ply_host::tcp::send",
            Op::Close => "ply_host::tcp::close",
        }
    }

    /// Loopback only. W1 has no address argument, and a handler that bound
    /// `0.0.0.0` by default would put a test's fixed response on the network of
    /// whoever ran the suite.
    fn listen(&self, at: &Resource, port: u16, span: Span) -> Result<HostAnswer, Diagnostic> {
        let listener = TcpListener::bind(("127.0.0.1", port)).map_err(|e| {
            Diagnostic::error(
                codes::RUNTIME_ERROR,
                format!("`net.listen` could not bind 127.0.0.1:{port}: {e}"),
            )
            .primary(span, "this listen reached the host and the host refused")
        })?;
        Ok(HostAnswer::Value(Value::Int(
            self.sockets
                .insert(Some(at), Sock::Listener(Arc::new(listener))),
        )))
    }

    fn accept(&self, at: &Resource, listener: i64, span: Span) -> Result<HostAnswer, Diagnostic> {
        let listener = self.sockets.listener(listener, at, span)?;
        let sockets = Arc::clone(&self.sockets);
        self.waiting(span, "accept", Op::Accept.what(), move || {
            match listener.accept() {
                // No label: `accept` names the listener's, and the connection's
                // is whichever one the program first reads or writes it under.
                Ok((stream, _)) => Done::Int(sockets.insert(None, Sock::Stream(Arc::new(stream)))),
                Err(e) => Done::Failed(e.to_string()),
            }
        })
    }

    fn recv(
        &self,
        at: &Resource,
        conn: i64,
        max: usize,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic> {
        let stream = self.sockets.stream(conn, at, span)?;
        self.waiting(span, "recv", Op::Recv.what(), move || {
            let mut buffer = vec![0u8; max];
            // One `read`, deliberately: a short answer is what a partial read
            // looks like and an empty one is what a peer's close looks like, and
            // a loop here would hide both from the program that has to handle
            // them.
            match (&*stream).read(&mut buffer) {
                Ok(n) => {
                    buffer.truncate(n);
                    Done::Bytes(buffer)
                }
                Err(e) => Done::Failed(e.to_string()),
            }
        })
    }

    fn send(
        &self,
        at: &Resource,
        conn: i64,
        payload: &[u8],
        span: Span,
    ) -> Result<HostAnswer, Diagnostic> {
        let stream = self.sockets.stream(conn, at, span)?;
        let payload = payload.to_vec();
        self.waiting(span, "send", Op::Send.what(), move || {
            // The whole buffer or a failure. Backpressure and partial writes are
            // W3's; answering a byte count short of the payload here would be a
            // second protocol for a program to get wrong.
            match (&*stream).write_all(&payload) {
                Ok(()) => Done::Int(payload.len() as i64),
                Err(e) => Done::Failed(e.to_string()),
            }
        })
    }

    fn close(&self, at: &Resource, socket: i64, span: Span) -> Result<HostAnswer, Diagnostic> {
        self.sockets.handles.check(socket, at, span)?;
        let sock = lock(&self.sockets.open).remove(&socket);
        self.sockets.handles.close(socket);
        match sock {
            // Shut down rather than only dropping: another Arc of this stream may
            // be parked in a `recv` on a pool thread, and closing the fd is what
            // returns that thread.
            Some(Sock::Stream(s)) => {
                let _ = s.shutdown(Shutdown::Both);
                Ok(HostAnswer::Value(Value::Unit))
            }
            Some(Sock::Listener(_)) => Ok(HostAnswer::Value(Value::Unit)),
            None => Err(unknown_handle(socket, span)),
        }
    }
}

impl HostRuntime for TcpHost {
    fn poll(&self, pending: &Pending) -> Result<Option<Value>, Diagnostic> {
        self.pool.poll(pending)
    }

    fn park(&self) -> Result<(), Diagnostic> {
        self.pool.park()
    }

    fn block_on(&self, pending: Pending) -> Result<Value, Diagnostic> {
        self.pool.block_on(pending)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}
