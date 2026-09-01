//! The stop flag, the phase machine, and the one thing that gets a parked `accept` to return.

use super::*;
use crate::tcp::{Net, TcpHost};
use ply_core::ty::Resource;
use ply_eval::HostAnswer;
use ply_span::Symbol;
use std::io::Read;
use std::net::TcpListener;

fn at() -> Resource {
    Resource::Named(Symbol::new("listener"))
}

fn bounds(lead_ms: u64, drain_ms: u64) -> Bounds {
    Bounds {
        lead: Duration::from_millis(lead_ms),
        drain: Duration::from_millis(drain_ms),
    }
}

/// Block until the phase machine has run, or give up.
fn until_stopped_accepting(shutdown: &Arc<Shutdown>) {
    let until = Instant::now() + Duration::from_secs(5);
    while !shutdown.stopped_accepting() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(
        shutdown.stopped_accepting(),
        "the phase machine never reached phase 2"
    );
}

fn settle(host: &TcpHost, answered: Result<HostAnswer, Diagnostic>) -> Value {
    match answered.expect("the operation was accepted") {
        HostAnswer::Value(value) => value,
        HostAnswer::Pending(pending) => {
            let until = Instant::now() + Duration::from_secs(10);
            loop {
                if let Some(value) = host.poll(&pending).expect("the token is this host's") {
                    return value;
                }
                assert!(Instant::now() < until, "`{pending}` never resolved");
                let _ = host.park_until(Duration::from_millis(20));
            }
        }
    }
}

fn int(value: &Value) -> i64 {
    match value {
        Value::Int(i) => *i,
        other => panic!("not an Int: {}", other.type_name()),
    }
}

// clock ---------------------------------------------------------------------------

/// `-1` is "no stop has been requested", and it is a number rather than an `Option` because every
/// call site compares it against a duration.
#[test]
fn a_running_service_has_no_deadline() {
    let shutdown = Shutdown::new(Bounds::default());
    assert!(!shutdown.stopping());
    assert_eq!(shutdown.deadline_ms(), -1);
    assert!(!shutdown.drain_expired());
    assert!(shutdown.elapsed().is_none());
}

/// The drain starts when accept stops, not when the signal arrived.
#[test]
fn the_lead_is_not_charged_to_the_drain() {
    let shutdown = Shutdown::new(bounds(150, 5_000));
    assert!(shutdown.request(Signal::Terminate));
    assert!(shutdown.stopping(), "the flag is set before anything else");
    // Still leading: nothing has stopped accepting and the drain has not begun.
    assert!(!shutdown.drain_expired());
    assert!(
        shutdown.deadline_ms() > 5_000,
        "during the lead what is left is the lead plus the whole drain, and it was {}",
        shutdown.deadline_ms()
    );
    until_stopped_accepting(&shutdown);
    let left = shutdown.deadline_ms();
    assert!(
        (0..=5_000).contains(&left),
        "the drain is a whole {}ms from the moment accept stopped, and it was {left}",
        5_000
    );
    assert!(
        shutdown.elapsed().expect("a stop was requested") >= Duration::from_millis(150),
        "the elapsed time is measured from the signal, lead included"
    );
}

#[test]
fn a_drain_that_runs_out_says_so() {
    let shutdown = Shutdown::new(bounds(0, 30));
    assert!(shutdown.request(Signal::Interrupt));
    until_stopped_accepting(&shutdown);
    let until = Instant::now() + Duration::from_secs(5);
    while !shutdown.drain_expired() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(shutdown.drain_expired(), "a 30ms drain never expired");
    assert_eq!(
        shutdown.deadline_ms(),
        0,
        "an expired drain has no time left rather than a negative amount"
    );
}

/// A second signal means a person has decided to stop waiting.
#[test]
fn a_second_signal_is_refused_rather_than_started_again() {
    let shutdown = Shutdown::new(bounds(0, 5_000));
    assert!(shutdown.request(Signal::Terminate), "the first one starts");
    assert!(
        !shutdown.request(Signal::Interrupt),
        "the second one is the caller's cue to exit"
    );
    assert!(shutdown.second_requested());
    assert_eq!(
        shutdown.signal(),
        Some(Signal::Terminate),
        "the run reports the signal that started the drain, not the one that ended the wait"
    );
}

#[test]
fn the_exit_codes_are_the_shell_convention() {
    assert_eq!(Signal::Interrupt.exit_code(), 130);
    assert_eq!(Signal::Terminate.exit_code(), 143);
}

// sockets ---------------------------------------------------------------------------

/// The exit criterion of ADR 0015 §4.3, at the boundary: a program's accept loop ends because
/// `accept` answered `0`, and not one line of it changed.
#[test]
fn accept_answers_zero_once_the_run_has_stopped_accepting() {
    let host = Arc::new(TcpHost::new());
    let listener = int(&settle(&host, host.listen(&at(), 0, Span::DUMMY)));
    assert!(listener > 0);

    let shutdown = Shutdown::new(bounds(0, 5_000));
    shutdown.attach_net(Arc::clone(&host) as Arc<dyn Accepting>);
    assert!(shutdown.request(Signal::Terminate));
    until_stopped_accepting(&shutdown);

    let answered = int(&settle(&host, host.accept(&at(), listener, Span::DUMMY)));
    assert_eq!(
        answered, 0,
        "a listener the drain closed answers 0, which is what ends a sequential accept loop"
    );
    // And again, because a program that loops until zero may ask twice.
    assert_eq!(
        int(&settle(&host, host.accept(&at(), listener, Span::DUMMY))),
        0
    );
}

/// The listener handle stays usable after the drain closed it, because `examples/desk.ply` closes
/// it after the loop returns and §4.3 claims that program needs no source change.
#[test]
fn the_program_can_still_close_a_listener_the_drain_closed() {
    let host = Arc::new(TcpHost::new());
    let listener = int(&settle(&host, host.listen(&at(), 0, Span::DUMMY)));
    let shutdown = Shutdown::new(bounds(0, 5_000));
    shutdown.attach_net(Arc::clone(&host) as Arc<dyn Accepting>);
    shutdown.request(Signal::Terminate);
    until_stopped_accepting(&shutdown);

    host.close(&at(), listener, Span::DUMMY)
        .expect("the program's own `net.close` still succeeds");
}

/// The one that would otherwise hang the drain.
#[test]
fn a_parked_accept_returns_when_the_run_stops_accepting() {
    let host = Arc::new(TcpHost::new());
    let listener = int(&settle(&host, host.listen(&at(), 0, Span::DUMMY)));

    // Parked, with nothing connecting: exactly an idle service.
    let HostAnswer::Pending(pending) = host
        .accept(&at(), listener, Span::DUMMY)
        .expect("the accept is accepted")
    else {
        panic!("a real `accept` waits, so it answers `Pending`");
    };
    let until = Instant::now() + Duration::from_secs(2);
    while host.accepts_in_flight() == 0 && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(host.accepts_in_flight(), 1, "the accept never parked");
    assert_eq!(
        host.poll(&pending).expect("this host's token"),
        None,
        "nothing is connecting, so it is still waiting"
    );

    let shutdown = Shutdown::new(bounds(0, 5_000));
    shutdown.attach_net(Arc::clone(&host) as Arc<dyn Accepting>);
    shutdown.request(Signal::Terminate);
    until_stopped_accepting(&shutdown);

    let deadline = Instant::now() + Duration::from_secs(10);
    let answered = loop {
        if let Some(value) = host.poll(&pending).expect("this host's token") {
            break int(&value);
        }
        assert!(
            Instant::now() < deadline,
            "the parked accept never returned, so an idle service would never observe a stop"
        );
        let _ = host.park_until(Duration::from_millis(20));
    };
    assert_eq!(
        answered, 0,
        "the connection the drain dialled is closed rather than served"
    );
}

/// A client that connects in the instant the run stops accepting gets a closed connection rather
/// than half a response.
#[test]
fn a_connection_accepted_at_the_stop_is_closed_rather_than_served() {
    let host = Arc::new(TcpHost::new());
    let listener = int(&settle(&host, host.listen(&at(), 0, Span::DUMMY)));
    let address = host.local_addr(listener).expect("a bound address");

    let HostAnswer::Pending(pending) = host
        .accept(&at(), listener, Span::DUMMY)
        .expect("the accept is accepted")
    else {
        panic!("a real `accept` waits");
    };
    let until = Instant::now() + Duration::from_secs(2);
    while host.accepts_in_flight() == 0 && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(2));
    }

    let shutdown = Shutdown::new(bounds(0, 5_000));
    shutdown.attach_net(Arc::clone(&host) as Arc<dyn Accepting>);
    shutdown.request(Signal::Terminate);
    until_stopped_accepting(&shutdown);

    let mut client = TcpStream::connect_timeout(&address, Duration::from_millis(500))
        .or_else(|_| TcpStream::connect(address));
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(value) = host.poll(&pending).expect("this host's token") {
            assert_eq!(int(&value), 0);
            break;
        }
        assert!(Instant::now() < deadline, "the accept never returned");
        let _ = host.park_until(Duration::from_millis(20));
    }
    if let Ok(stream) = &mut client {
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let mut buffer = [0u8; 16];
        // End of stream or a reset.
        let read = stream.read(&mut buffer);
        assert!(
            matches!(&read, Ok(0) | Err(_)),
            "the client was written {:?} by a run that had stopped accepting",
            &buffer[..read.unwrap_or(0)]
        );
    }
    assert_eq!(
        host.connections_in_flight(),
        0,
        "a connection taken after the stop is closed rather than entered in the socket table"
    );
}

/// Idempotent, because a second signal, a `Drop` and an explicit teardown can all reach it and none
/// of them may raise on the others' account.
#[test]
fn stopping_twice_closes_the_listeners_once() {
    let host = TcpHost::new();
    settle(&host, host.listen(&at(), 0, Span::DUMMY));
    settle(&host, host.listen(&at(), 0, Span::DUMMY));
    assert_eq!(host.stop_accepting(), 2);
    assert_eq!(
        host.stop_accepting(),
        0,
        "the second call has nothing left to close"
    );
}

/// The addresses survive the close, because the drain dials them *after* it has closed them and a
/// closed entry holds no socket to ask.
#[test]
fn the_drain_can_still_name_a_listener_it_closed() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let wanted = listener.local_addr().expect("a bound address");
    drop(listener);

    let host = TcpHost::new();
    let handle = int(&settle(
        &host,
        host.listen(&at(), wanted.port(), Span::DUMMY),
    ));
    assert!(handle > 0);
    assert_eq!(host.listening_at(), vec![wanted]);
    host.stop_accepting();
    assert_eq!(
        host.listening_at(),
        vec![wanted],
        "phase 2 dials after it closes, so the address has to outlive the socket"
    );
}

/// Both operations are reads of a flag, so replaying one changes nothing outside the program and
/// neither waits on a peer.
#[test]
fn the_registrations_declare_what_a_reviewer_relies_on() {
    let shutdown = Shutdown::new(Bounds::default());
    for (op, _) in registrations(Some(&shutdown)) {
        assert_eq!(op.effect.as_str(), EFFECT);
        assert_eq!(op.determinism, Determinism::Nondeterministic);
        assert_eq!(op.linearity, Linearity::Repeatable);
        assert!(!op.blocking);
        assert!(!op.secrets, "a stop flag is handed no value at all");
        assert!(op.path.starts_with("ply_host::signal::"));
    }
}

#[test]
fn the_handler_answers_the_flag_and_the_clock() {
    let shutdown = Shutdown::new(bounds(0, 5_000));
    let handlers = registrations(Some(&shutdown));
    let stopping = &handlers[0].1;
    let deadline = &handlers[1].1;
    let atom = ply_core::ty::EffectAtom::new(
        Symbol::new(EFFECT),
        Resource::Singleton,
        ply_syntax::ast::Mode::Read,
    );
    let declaration = Op::Stopping.declaration();
    fn request<'a>(
        atom: &ply_core::ty::EffectAtom,
        declaration: &'a HostOp,
        args: &'a [Value],
    ) -> HostRequest<'a> {
        HostRequest {
            atom: atom.clone(),
            op: declaration,
            args,
            span: Span::DUMMY,
            machine: ply_eval::host::MachineId(0),
            task: None,
            declared: None,
        }
    }

    struct Nothing;
    impl HostRuntime for Nothing {
        fn poll(&self, _: &ply_eval::Pending) -> Result<Option<Value>, Diagnostic> {
            Ok(None)
        }
        fn park(&self) -> Result<(), Diagnostic> {
            Ok(())
        }
        fn block_on(&self, _: ply_eval::Pending) -> Result<Value, Diagnostic> {
            Ok(Value::Unit)
        }
    }

    let answer = |h: &Arc<dyn HostHandler>| match h
        .call(&Nothing, &request(&atom, &declaration, &[]))
    {
        Ok(HostAnswer::Value(v)) => v,
        Ok(HostAnswer::Pending(_)) => panic!("a flag read waits on nothing, so it answers inline"),
        Err(d) => panic!("a flag read was refused: {} {}", d.code, d.message),
    };
    assert_eq!(answer(stopping), Value::Bool(false));
    assert_eq!(answer(deadline), Value::Int(-1));

    shutdown.request(Signal::Interrupt);
    assert_eq!(answer(stopping), Value::Bool(true));
    assert!(int(&answer(deadline)) > 0);

    // Arity is inference's, so reaching the handler with the wrong count means the evaluator was
    // handed a module that was never checked.
    let extra = [Value::Unit];
    match stopping.call(&Nothing, &request(&atom, &declaration, &extra)) {
        Err(d) => assert_eq!(d.code, codes::INTERNAL_ERROR),
        Ok(_) => panic!("an argument to a nullary operation was accepted"),
    }
}
