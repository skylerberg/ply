//! The simulated twin: `net` over a script instead of a socket.

use super::{
    Handles, Net, Op, no_connection_scripted, not_a_listener, not_a_stream, unknown_handle,
};
use crate::tls;
use ply_core::ty::Resource;
use ply_eval::{HostAnswer, HostRuntime, Pending, Value};
use ply_span::{Diagnostic, Span, codes};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

fn some(v: Value) -> Value {
    Value::ctor("Some", vec![v])
}

#[derive(Default)]
struct SimState {
    listeners: Vec<i64>,
    /// The connections `accept` hands out, in order, whichever listener asks.
    inbound: VecDeque<VecDeque<Vec<u8>>>,
    conns: BTreeMap<i64, VecDeque<Vec<u8>>>,
    sent: BTreeMap<i64, Vec<u8>>,
}

pub struct SimNet {
    state: Mutex<SimState>,
    handles: Handles,
    /// The credential names this simulated run was configured with — the twin's stand-in for what
    /// `--tls` gave the socket handler.
    credentials: BTreeSet<String>,
}

impl SimNet {
    /// Each element is one connection `accept` hands out, in order; each connection is the chunks
    /// `recv` answers with before it reports the peer is done.
    pub fn new(connections: Vec<Vec<Vec<u8>>>) -> SimNet {
        SimNet::with_credentials(connections, Vec::<String>::new())
    }

    /// The same script, for a service whose listener is `net.listen_tls`.
    pub fn with_credentials(
        connections: Vec<Vec<Vec<u8>>>,
        credentials: Vec<impl Into<String>>,
    ) -> SimNet {
        SimNet {
            state: Mutex::new(SimState {
                inbound: connections
                    .into_iter()
                    .map(|chunks| chunks.into_iter().collect())
                    .collect(),
                ..SimState::default()
            }),
            handles: Handles::new(),
            credentials: credentials.into_iter().map(Into::into).collect(),
        }
    }

    fn bind(&self, at: &Resource) -> i64 {
        let handle = self.handles.open(Some(at));
        lock(&self.state).listeners.push(handle);
        handle
    }

    /// Everything the program wrote to a connection, in order.
    pub fn sent(&self, conn: i64) -> Vec<u8> {
        lock(&self.state)
            .sent
            .get(&conn)
            .cloned()
            .unwrap_or_default()
    }
}

impl Net for SimNet {
    /// A script never waits, so every answer is a value and nothing is dispatched off the machine's
    /// thread.
    fn waits(&self) -> bool {
        false
    }

    fn path(&self, op: Op) -> &'static str {
        match op {
            Op::Listen => "ply_host::tcp::sim::listen",
            Op::ListenTls => "ply_host::tls::sim::listen",
            Op::Accept => "ply_host::tcp::sim::accept",
            Op::Recv => "ply_host::tcp::sim::recv",
            Op::Send => "ply_host::tcp::sim::send",
            Op::Close => "ply_host::tcp::sim::close",
        }
    }

    fn listen(&self, at: &Resource, _port: u16, _span: Span) -> Result<HostAnswer, Diagnostic> {
        Ok(HostAnswer::Value(Value::Int(self.bind(at))))
    }

    /// The credential is checked and then the listener is an ordinary one: the bytes a program
    /// reads off a TLS connection are the bytes it reads off any other, and the twin's whole job is
    /// to be that program's other binding.
    fn listen_tls(
        &self,
        at: &Resource,
        _port: u16,
        credential: &str,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic> {
        if !self.credentials.contains(credential) {
            return Err(tls::unknown_credential(
                credential,
                self.credentials.iter().map(String::as_str),
                span,
            ));
        }
        Ok(HostAnswer::Value(Value::Int(self.bind(at))))
    }

    fn accept(&self, at: &Resource, listener: i64, span: Span) -> Result<HostAnswer, Diagnostic> {
        self.handles.check(listener, at, span)?;
        let mut state = lock(&self.state);
        if !state.listeners.contains(&listener) {
            return Err(not_a_listener(listener, span));
        }
        // A real `accept` waits here for as long as it takes.
        let Some(chunks) = state.inbound.pop_front() else {
            return Err(no_connection_scripted(span));
        };
        let handle = self.handles.open(None);
        state.conns.insert(handle, chunks);
        Ok(HostAnswer::Value(Value::Int(handle)))
    }

    /// The twin never answers `None`.
    fn recv(
        &self,
        at: &Resource,
        conn: i64,
        max: usize,
        _timeout: Duration,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic> {
        self.handles.check(conn, at, span)?;
        let mut state = lock(&self.state);
        let Some(chunks) = state.conns.get_mut(&conn) else {
            return Err(not_a_stream(conn, span));
        };
        // An exhausted script is a peer that has stopped sending, which is the empty answer a real
        // `recv` gives at end of stream.
        let Some(mut chunk) = chunks.pop_front() else {
            return Ok(HostAnswer::Value(some(Value::bytes([]))));
        };
        if chunk.len() > max {
            chunks.push_front(chunk.split_off(max));
        }
        Ok(HostAnswer::Value(some(Value::bytes(chunk))))
    }

    fn send(
        &self,
        at: &Resource,
        conn: i64,
        payload: &[u8],
        _timeout: Duration,
        span: Span,
    ) -> Result<HostAnswer, Diagnostic> {
        self.handles.check(conn, at, span)?;
        let mut state = lock(&self.state);
        if !state.conns.contains_key(&conn) {
            return Err(not_a_stream(conn, span));
        }
        state
            .sent
            .entry(conn)
            .or_default()
            .extend_from_slice(payload);
        Ok(HostAnswer::Value(some(Value::Int(payload.len() as i64))))
    }

    fn close(&self, at: &Resource, socket: i64, span: Span) -> Result<HostAnswer, Diagnostic> {
        self.handles.check(socket, at, span)?;
        self.handles.close(socket);
        let mut state = lock(&self.state);
        if state.conns.remove(&socket).is_some() {
            return Ok(HostAnswer::Value(Value::Unit));
        }
        match state.listeners.iter().position(|l| *l == socket) {
            Some(index) => {
                state.listeners.remove(index);
                Ok(HostAnswer::Value(Value::Unit))
            }
            None => Err(unknown_handle(socket, span)),
        }
    }
}

/// The twin mints no token, so any token it is handed belongs to something else.
impl HostRuntime for SimNet {
    fn poll(&self, pending: &Pending) -> Result<Option<Value>, Diagnostic> {
        Err(foreign_token(pending))
    }

    fn park(&self) -> Result<(), Diagnostic> {
        Err(Diagnostic::error(
            codes::INTERNAL_ERROR,
            "the simulated network was asked to wait, and it never has anything outstanding",
        ))
    }

    fn block_on(&self, pending: Pending) -> Result<Value, Diagnostic> {
        Err(foreign_token(&pending))
    }
}

#[cold]
fn foreign_token(pending: &Pending) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("the simulated network was polled for `{pending}`, which it did not mint"),
    )
    .note("every simulated answer is a value; a pending token here means two host facilities were composed and the wrong one was asked")
}

/// See `pool::lock`: the state behind this is maps with no invariant a panicking caller can break.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}
