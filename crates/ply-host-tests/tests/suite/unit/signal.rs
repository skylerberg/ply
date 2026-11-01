use ply_core::Span;
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

fn accepting(host: &TcpHost, listener: i64) -> mpsc::Receiver<i64> {
    let job = host
        .accept(LISTENER, listener, Span::DUMMY)
        .expect("the accept is taken");
    let (returned, result) = mpsc::channel();
    std::thread::spawn(move || {
        let handle = ply_host::native::int_of(&job().expect("the accept ran")).expect("a handle");
        let _ = returned.send(handle);
    });
    result
}

fn accepted(host: &TcpHost, listener: i64) -> i64 {
    accepting(host, listener)
        .recv_timeout(Duration::from_secs(10))
        .expect("the accept returned")
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

#[test]
fn a_running_service_has_no_deadline() {
    let shutdown = Shutdown::new(Bounds::default());
    assert!(!shutdown.stopping());
    assert_eq!(shutdown.deadline_ms(), -1);
    assert!(!shutdown.drain_expired());
    assert!(shutdown.elapsed().is_none());
}

#[test]
fn the_delay_before_accepting_stops_is_not_counted_against_the_shutdown_grace_period() {
    let shutdown = Shutdown::new(bounds(150, 5_000));
    assert!(shutdown.request(ShutdownSignal::Terminate));
    assert!(shutdown.stopping(), "the flag is set before anything else");
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
fn a_shutdown_grace_period_that_runs_out_reports_expired_and_has_zero_time_left() {
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

#[test]
fn a_second_signal_is_rejected_rather_than_started_again() {
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
fn accept_returns_zero_once_the_run_has_stopped_accepting() {
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
        "a listener the drain closed returns 0, which is what ends a sequential accept loop"
    );
    assert_eq!(accepted(&host, listener), 0);
}

#[test]
fn closing_a_listener_that_shutdown_already_closed_succeeds() {
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
fn a_blocked_accept_returns_when_the_run_stops_accepting_connections() {
    let host = Arc::new(TcpHost::new());
    let listener = listen(&host, 0);

    let result = accepting(&host, listener);
    assert_eq!(
        host.accepts_in_flight(),
        1,
        "the accept is counted as parked"
    );
    assert!(
        result.recv_timeout(Duration::from_millis(100)).is_err(),
        "nothing is connecting, so it is still waiting"
    );

    let shutdown = Shutdown::new(bounds(0, 5_000));
    shutdown.attach_net(Arc::clone(&host) as Arc<dyn Accepting>);
    shutdown.request(ShutdownSignal::Terminate);
    until_stopped_accepting(&shutdown);

    let returned = result
        .recv_timeout(Duration::from_secs(10))
        .expect("the parked accept returned, so an idle service observes a stop");
    assert_eq!(
        returned, 0,
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

    let result = accepting(&host, listener);

    let shutdown = Shutdown::new(bounds(0, 5_000));
    shutdown.attach_net(Arc::clone(&host) as Arc<dyn Accepting>);
    shutdown.request(ShutdownSignal::Terminate);
    until_stopped_accepting(&shutdown);

    let mut client = TcpStream::connect_timeout(&address, Duration::from_millis(500))
        .or_else(|_| TcpStream::connect(address));
    assert_eq!(
        result
            .recv_timeout(Duration::from_secs(10))
            .expect("the accept returned"),
        0
    );
    if let Ok(stream) = &mut client {
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let mut buffer = [0u8; 16];
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

#[test]
fn after_accepting_stops_the_host_still_lists_the_address_of_the_listener_it_closed() {
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

fn send_user2() {
    let sent = std::process::Command::new("/bin/sh")
        .args(["-c", &format!("kill -USR2 {}", std::process::id())])
        .status()
        .expect("a shell runs");
    assert!(sent.success());
}

fn arrives(shutdown: &Shutdown, which: WatchedSignal) -> bool {
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
fn arrived_reports_a_signal_as_received_once_however_many_times_it_was_sent() {
    let shutdown = Shutdown::new(Bounds::default());
    assert!(
        !shutdown
            .arrived(WatchedSignal::User2)
            .expect("the first ask installs the handler"),
        "nothing arrived before anybody asked"
    );
    send_user2();
    assert!(
        arrives(&shutdown, WatchedSignal::User2),
        "one arrival is seen"
    );
    send_user2();
    send_user2();
    assert!(
        arrives(&shutdown, WatchedSignal::User2),
        "two arrivals are seen"
    );
    std::thread::sleep(Duration::from_millis(200));
    let _ = shutdown.arrived(WatchedSignal::User2);
    assert!(
        !shutdown.arrived(WatchedSignal::User2).expect("installed"),
        "arrivals are not counted: nothing is owed once an ask returned them"
    );
    assert!(
        !shutdown.stopping(),
        "a signal asked about is no request to stop"
    );
}
