use ply_core::{
    Determinism, Diagnostic, HostAnswer, HostHandler, HostOp, HostRequest, HostRuntime, Linearity,
    Resource, Span, Symbol, Value, codes,
};
use ply_host::signal::*;
use ply_host::tcp::TcpHost;
use std::io::Read;
use std::net::TcpListener;
use std::net::TcpStream;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

const LISTENER: &str = "listener";

fn listen(host: &TcpHost, port: u16) -> i64 {
    host.listen(LISTENER, port, Span::DUMMY)
        .expect("a loopback port")
}

/// What a listener's next `accept` answers once it does, its job waiting on a thread of its own as
/// a run's would.
fn accepting(host: &TcpHost, listener: i64) -> mpsc::Receiver<i64> {
    let job = host
        .accept(LISTENER, listener, Span::DUMMY)
        .expect("the accept is taken");
    let (answered, answer) = mpsc::channel();
    std::thread::spawn(move || {
        let handle = ply_host::native::int_of(&job().expect("the accept ran")).expect("a handle");
        let _ = answered.send(handle);
    });
    answer
}

fn accepted(host: &TcpHost, listener: i64) -> i64 {
    accepting(host, listener)
        .recv_timeout(Duration::from_secs(10))
        .expect("the accept answered")
}

fn bounds(lead_ms: u64, drain_ms: u64) -> Bounds {
    Bounds {
        lead: Duration::from_millis(lead_ms),
        drain: Duration::from_millis(drain_ms),
    }
}

fn until_stopped_accepting(shutdown: &Arc<Shutdown>) {
    let until = Instant::now() + Duration::from_secs(5);
    while !shutdown.stopped_accepting() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(
        shutdown.stopped_accepting(),
        "the shutdown never reached its stop"
    );
}

fn int(value: &Value) -> i64 {
    match value {
        Value::Int(i) => *i,
        other => panic!("not an Int: {}", other.type_name()),
    }
}

/// `-1` rather than `None` because every call site compares it against a duration.
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
    assert!(shutdown.request(ShutdownSignal::Terminate));
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
    assert!(shutdown.request(ShutdownSignal::Interrupt));
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
    assert!(
        shutdown.request(ShutdownSignal::Terminate),
        "the first one starts"
    );
    assert!(
        !shutdown.request(ShutdownSignal::Interrupt),
        "the second one is the caller's cue to exit"
    );
    assert!(shutdown.second_requested());
    assert_eq!(
        shutdown.signal(),
        Some(ShutdownSignal::Terminate),
        "the run reports the signal that started the drain, not the one that ended the wait"
    );
}

#[test]
fn the_exit_codes_are_the_shell_convention() {
    assert_eq!(ShutdownSignal::Interrupt.exit_code(), 130);
    assert_eq!(ShutdownSignal::Terminate.exit_code(), 143);
}

#[test]
fn accept_answers_zero_once_the_run_has_stopped_accepting() {
    let host = Arc::new(TcpHost::new());
    let listener = listen(&host, 0);
    assert!(listener > 0);

    let shutdown = Shutdown::new(bounds(0, 5_000));
    shutdown.attach_net(Arc::clone(&host) as Arc<dyn Accepting>);
    assert!(shutdown.request(ShutdownSignal::Terminate));
    until_stopped_accepting(&shutdown);

    assert_eq!(
        accepted(&host, listener),
        0,
        "a listener the drain closed answers 0, which is what ends a sequential accept loop"
    );
    // And again, because a program that loops until zero may ask twice.
    assert_eq!(accepted(&host, listener), 0);
}

/// Programs close the listener after their accept loop returns, so the handle survives the drain.
#[test]
fn the_program_can_still_close_a_listener_the_drain_closed() {
    let host = Arc::new(TcpHost::new());
    let listener = listen(&host, 0);
    let shutdown = Shutdown::new(bounds(0, 5_000));
    shutdown.attach_net(Arc::clone(&host) as Arc<dyn Accepting>);
    shutdown.request(ShutdownSignal::Terminate);
    until_stopped_accepting(&shutdown);

    host.close(LISTENER, listener, Span::DUMMY)
        .expect("the program's own `net.close` still succeeds");
}

#[test]
fn a_parked_accept_returns_when_the_run_stops_accepting() {
    let host = Arc::new(TcpHost::new());
    let listener = listen(&host, 0);

    // Parked, with nothing connecting: exactly an idle service.
    let answer = accepting(&host, listener);
    assert_eq!(
        host.accepts_in_flight(),
        1,
        "the accept is counted as parked"
    );
    assert!(
        answer.recv_timeout(Duration::from_millis(100)).is_err(),
        "nothing is connecting, so it is still waiting"
    );

    let shutdown = Shutdown::new(bounds(0, 5_000));
    shutdown.attach_net(Arc::clone(&host) as Arc<dyn Accepting>);
    shutdown.request(ShutdownSignal::Terminate);
    until_stopped_accepting(&shutdown);

    let answered = answer
        .recv_timeout(Duration::from_secs(10))
        .expect("the parked accept returned, so an idle service observes a stop");
    assert_eq!(
        answered, 0,
        "the connection the drain dialled is closed rather than served"
    );
    let until = Instant::now() + Duration::from_secs(2);
    while host.accepts_in_flight() > 0 && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        host.accepts_in_flight(),
        0,
        "a returned accept is no longer counted"
    );
}

#[test]
fn a_connection_accepted_at_the_stop_is_closed_rather_than_served() {
    let host = Arc::new(TcpHost::new());
    let listener = listen(&host, 0);
    let address = host.local_addr(listener).expect("a bound address");

    let answer = accepting(&host, listener);

    let shutdown = Shutdown::new(bounds(0, 5_000));
    shutdown.attach_net(Arc::clone(&host) as Arc<dyn Accepting>);
    shutdown.request(ShutdownSignal::Terminate);
    until_stopped_accepting(&shutdown);

    let mut client = TcpStream::connect_timeout(&address, Duration::from_millis(500))
        .or_else(|_| TcpStream::connect(address));
    assert_eq!(
        answer
            .recv_timeout(Duration::from_secs(10))
            .expect("the accept returned"),
        0
    );
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

/// A second signal, a `Drop` and an explicit teardown can all reach it.
#[test]
fn stopping_twice_closes_the_listeners_once() {
    let host = TcpHost::new();
    listen(&host, 0);
    listen(&host, 0);
    assert_eq!(host.stop_accepting(), 2);
    assert_eq!(
        host.stop_accepting(),
        0,
        "the second call has nothing left to close"
    );
}

/// The drain dials the addresses after closing them, so they must outlive the sockets.
#[test]
fn the_drain_can_still_name_a_listener_it_closed() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let wanted = listener.local_addr().expect("a bound address");
    drop(listener);

    let host = TcpHost::new();
    let handle = listen(&host, wanted.port());
    assert!(handle > 0);
    assert_eq!(host.listening_at(), vec![wanted]);
    host.stop_accepting();
    assert_eq!(
        host.listening_at(),
        vec![wanted],
        "the stop dials after it closes, so the address has to outlive the socket"
    );
}

/// The stop's two are flag reads: replaying changes nothing outside the program. An ask takes the
/// arrival it answers, so a second one answers otherwise. None waits on a peer.
#[test]
fn the_registrations_declare_what_a_reviewer_relies_on() {
    let shutdown = Shutdown::new(Bounds::default());
    for (op, _) in registrations(Some(&shutdown)) {
        assert_eq!(op.effect.as_str(), EFFECT);
        assert_eq!(op.determinism, Determinism::Nondeterministic);
        let reads = matches!(op.op.as_str(), "stopping" | "deadline_ms");
        let expected = if reads {
            Linearity::Repeatable
        } else {
            Linearity::AtMostOnce
        };
        assert_eq!(op.linearity, expected, "{op}");
        assert!(!op.blocking);
        assert!(!op.secrets, "a stop flag is handed no value at all");
        assert!(op.path.starts_with("ply_host::signal::"));
    }
}

/// `SIGUSR2` sent to this process, which is safe once an ask has installed its handler. The
/// shell's own `kill` sends it, which every system has.
fn send_user2() {
    let sent = std::process::Command::new("/bin/sh")
        .args(["-c", &format!("kill -USR2 {}", std::process::id())])
        .status()
        .expect("a shell runs");
    assert!(sent.success());
}

fn arrives(shutdown: &Shutdown, which: Asked) -> bool {
    let until = Instant::now() + Duration::from_secs(10);
    while Instant::now() < until {
        if shutdown.arrived(which).expect("the handler is installed") {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
}

#[test]
fn a_signal_asked_about_is_seen_once_however_often_it_arrived() {
    let shutdown = Shutdown::new(Bounds::default());
    assert!(
        !shutdown
            .arrived(Asked::User2)
            .expect("the first ask installs the handler"),
        "nothing arrived before anybody asked"
    );
    send_user2();
    assert!(arrives(&shutdown, Asked::User2), "one arrival is seen");
    send_user2();
    send_user2();
    assert!(arrives(&shutdown, Asked::User2), "two arrivals are seen");
    std::thread::sleep(Duration::from_millis(200));
    // The second of the two may have landed after the ask that saw the first.
    let _ = shutdown.arrived(Asked::User2);
    assert!(
        !shutdown.arrived(Asked::User2).expect("installed"),
        "arrivals are not counted: nothing is owed once they were answered"
    );
    assert!(
        !shutdown.stopping(),
        "a signal asked about is no request to stop"
    );
}

#[test]
fn an_ask_is_answered_through_its_handler() {
    let shutdown = Shutdown::new(Bounds::default());
    let handlers = registrations(Some(&shutdown));
    let (declaration, handler) = handlers
        .iter()
        .find(|(op, _)| op.op.as_str() == "resized")
        .expect("`resized` is registered");
    let atom = ply_core::EffectAtom::new(
        Symbol::new(EFFECT),
        Resource::Singleton,
        ply_core::Mode::Write,
    );
    struct Nothing;
    impl HostRuntime for Nothing {
        fn watch(&self, _: &ply_core::Pending) -> Result<(), Diagnostic> {
            Ok(())
        }
        fn resolved(&self) -> Vec<(u64, Result<Value, Diagnostic>)> {
            Vec::new()
        }
        fn park(&self) -> Result<(), Diagnostic> {
            Ok(())
        }
        fn block_on(&self, _: ply_core::Pending) -> Result<Value, Diagnostic> {
            Ok(Value::Unit)
        }
    }
    let answered = handler
        .call(
            &Nothing,
            &HostRequest {
                atom,
                op: declaration,
                args: &[],
                span: Span::DUMMY,
                machine: ply_core::host::MachineId(0),
                task: None,
                declared: None,
            },
        )
        .expect("an ask is answered");
    assert!(matches!(answered, HostAnswer::Value(Value::Bool(false))));
}

#[test]
fn the_handler_answers_the_flag_and_the_clock() {
    let shutdown = Shutdown::new(bounds(0, 5_000));
    let handlers = registrations(Some(&shutdown));
    let stopping = &handlers[0].1;
    let deadline = &handlers[1].1;
    let atom = ply_core::EffectAtom::new(
        Symbol::new(EFFECT),
        Resource::Singleton,
        ply_core::Mode::Read,
    );
    let declaration = Op::Stopping.declaration();
    fn request<'a>(
        atom: &ply_core::EffectAtom,
        declaration: &'a HostOp,
        args: &'a [Value],
    ) -> HostRequest<'a> {
        HostRequest {
            atom: atom.clone(),
            op: declaration,
            args,
            span: Span::DUMMY,
            machine: ply_core::host::MachineId(0),
            task: None,
            declared: None,
        }
    }

    struct Nothing;
    impl HostRuntime for Nothing {
        fn watch(&self, _: &ply_core::Pending) -> Result<(), Diagnostic> {
            Ok(())
        }
        fn resolved(&self) -> Vec<(u64, Result<Value, Diagnostic>)> {
            Vec::new()
        }
        fn park(&self) -> Result<(), Diagnostic> {
            Ok(())
        }
        fn block_on(&self, _: ply_core::Pending) -> Result<Value, Diagnostic> {
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

    shutdown.request(ShutdownSignal::Interrupt);
    assert_eq!(answer(stopping), Value::Bool(true));
    assert!(int(&answer(deadline)) > 0);

    // Arity is inference's, so the wrong count means the module was never checked.
    let extra = [Value::Unit];
    match stopping.call(&Nothing, &request(&atom, &declaration, &extra)) {
        Err(d) => assert_eq!(d.code, codes::INTERNAL_ERROR),
        Ok(_) => panic!("an argument to a nullary operation was accepted"),
    }
}
