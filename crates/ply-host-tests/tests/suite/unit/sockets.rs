use ply_core::{Span, codes};
use ply_host::jobs::Job;
use ply_host::native::{int_of, parts_of};
use ply_host::tcp::wire::{Endpoint, NetError, Probing, SocketOption};
use ply_host::tcp::{TcpHost, Upgrade};
use ply_host::{CredentialSpec, Credentials};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

const LONG: Duration = Duration::from_secs(5);

fn at(ip: IpAddr, port: u16) -> Endpoint {
    Endpoint {
        ip,
        port,
        zone: None,
    }
}

fn loopback() -> IpAddr {
    IpAddr::V4(Ipv4Addr::LOCALHOST)
}

fn outcome(result: Vec<u8>) -> Result<Vec<u8>, Vec<u8>> {
    match result.split_first() {
        Some((1, rest)) => Ok(rest.to_vec()),
        Some((0, rest)) => Err(rest.to_vec()),
        _ => panic!("not a result: {result:?}"),
    }
}

fn found(result: Vec<u8>) -> Option<Vec<u8>> {
    match result.split_first() {
        Some((1, rest)) => Some(rest.to_vec()),
        Some((0, [])) => None,
        _ => panic!("not found nor none: {result:?}"),
    }
}

fn int(bytes: &[u8]) -> i64 {
    int_of(bytes).expect("eight bytes")
}

fn ran(job: Job) -> Vec<u8> {
    job().expect("the job ran")
}

struct Net {
    host: TcpHost,
}

impl Net {
    fn new() -> Net {
        Net {
            host: TcpHost::new(),
        }
    }

    fn listening(&self, ip: IpAddr) -> (i64, Endpoint) {
        let listener = int(
            &outcome(self.host.listen_on("listener", &at(ip, 0), &[])).expect("loopback binds")
        );
        let here = self.address("listener", listener).expect("a bound address");
        (listener, here)
    }

    fn address(&self, label: &str, socket: i64) -> Option<Endpoint> {
        let flat = self.host.local_address(label, socket, Span::DUMMY);
        found(flat.expect("a socket")).map(|e| Endpoint::read(&e).expect("an address"))
    }

    fn peer(&self, label: &str, conn: i64) -> Option<Endpoint> {
        let flat = self.host.peer_address(label, conn, Span::DUMMY);
        found(flat.expect("a socket")).map(|e| Endpoint::read(&e).expect("an address"))
    }

    fn joined(&self, listener: i64, here: &Endpoint) -> (i64, i64) {
        let connecting = self
            .host
            .connect_to("client", here.clone(), Vec::new(), LONG);
        let dialled = std::thread::spawn(move || ran(connecting));
        let server = int(&ran(self
            .host
            .accept("listener", listener, Span::DUMMY)
            .expect("an accept")));
        let client = int(&outcome(dialled.join().expect("the dial")).expect("connected"));
        (client, server)
    }

    fn send(&self, label: &str, conn: i64, payload: &[u8]) -> i64 {
        let job = self
            .host
            .send(label, conn, payload.to_vec(), LONG, Span::DUMMY)
            .expect("a send");
        int(&found(ran(job)).expect("not a deadline"))
    }

    fn recv(&self, label: &str, conn: i64, max: usize, timeout: Duration) -> Option<Vec<u8>> {
        let job = self
            .host
            .recv(label, conn, max, timeout, Span::DUMMY)
            .expect("a recv");
        found(ran(job))
    }

    fn close(&self, label: &str, socket: i64) {
        self.host
            .close(label, socket, Span::DUMMY)
            .expect("an open socket closes");
    }

    fn options(&self, label: &str, socket: i64) -> Vec<SocketOption> {
        let listed = self.host.options(label, socket, Span::DUMMY);
        SocketOption::list(&listed.expect("a socket")).expect("options")
    }
}

#[test]
fn a_connection_reports_its_local_and_peer_addresses_and_carries_bytes_in_both_directions() {
    let net = Net::new();
    let (listener, here) = net.listening(loopback());
    let (client, server) = net.joined(listener, &here);
    assert_eq!(net.peer("client", client), Some(here.clone()));
    assert_eq!(net.peer("conn", server), net.address("client", client));

    assert_eq!(net.send("client", client, b"ping"), 4);
    assert_eq!(net.recv("conn", server, 64, LONG), Some(b"ping".to_vec()));
    net.host
        .close_write("client", client, Span::DUMMY)
        .expect("an open connection");
    assert_eq!(
        net.recv("conn", server, 64, LONG),
        Some(Vec::new()),
        "ending one's sending is the peer's end of stream"
    );
    assert_eq!(net.send("conn", server, b"pong"), 4);
    assert_eq!(
        net.recv("client", client, 64, LONG),
        Some(b"pong".to_vec()),
        "and leaves reading open"
    );
    assert_eq!(
        net.recv("client", client, 64, Duration::from_millis(50)),
        None,
        "nothing more is a deadline"
    );
    net.close("client", client);
    net.close("conn", server);
    net.close("listener", listener);
}

#[test]
fn connecting_to_an_address_with_no_listener_fails_and_listening_on_a_used_address_fails() {
    let net = Net::new();
    let (listener, here) = net.listening(loopback());
    assert_eq!(
        outcome(net.host.listen_on("listener", &here, &[])),
        Err(NetError::AddressInUse.written())
    );
    net.close("listener", listener);
    let dialled = ran(net.host.connect_to("client", here, Vec::new(), LONG));
    assert_eq!(outcome(dialled), Err(NetError::Rejected.written()));
}

#[test]
fn an_ipv6_connection_reports_its_local_and_peer_addresses_where_the_machine_supports_ipv6() {
    let net = Net::new();
    let six = IpAddr::V6(Ipv6Addr::LOCALHOST);
    let Ok(listening) = outcome(net.host.listen_on("listener", &at(six, 0), &[])) else {
        return;
    };
    let listener = int(&listening);
    let here = net.address("listener", listener).expect("a bound address");
    let (client, server) = net.joined(listener, &here);
    assert_eq!(net.peer("client", client), Some(here));
    assert_eq!(net.address("client", client).map(|e| e.ip), Some(six));
    net.close("client", client);
    net.close("conn", server);
    net.close("listener", listener);
}

#[test]
fn a_read_shorter_than_the_available_bytes_leaves_the_remaining_bytes_for_the_next_read() {
    let net = Net::new();
    let (listener, here) = net.listening(loopback());
    let (client, server) = net.joined(listener, &here);
    assert_eq!(net.send("client", client, b"0123456789"), 10);
    let first = net.recv("conn", server, 4, LONG).expect("bytes");
    assert_eq!(first, b"0123");
    let mut rest = Vec::new();
    while rest.len() < 6 {
        rest.extend(net.recv("conn", server, 64, LONG).expect("the rest"));
    }
    assert_eq!(rest, b"456789");
    net.close("client", client);
    assert_eq!(
        net.recv("conn", server, 64, LONG),
        Some(Vec::new()),
        "a peer that closed reads empty rather than failing"
    );
    net.close("conn", server);
    net.close("listener", listener);
}

#[test]
fn options_set_as_a_socket_opens_and_afterwards_are_read_back() {
    let net = Net::new();
    let (listener, here) = net.listening(loopback());
    assert!(
        net.options("listener", listener)
            .contains(&SocketOption::ReuseAddress(true)),
        "a listener reuses its address"
    );
    let connecting = net.host.connect_to(
        "client",
        here.clone(),
        vec![
            SocketOption::NoDelay(false),
            SocketOption::Presenting("absent".to_string()),
        ],
        LONG,
    );
    let dialled = std::thread::spawn(move || ran(connecting));
    let server = int(&ran(net
        .host
        .accept("listener", listener, Span::DUMMY)
        .expect("an accept")));
    let client = int(&outcome(dialled.join().expect("the dial")).expect("connected"));
    let options = net.options("client", client);
    assert!(
        options.contains(&SocketOption::NoDelay(false)),
        "{options:?}"
    );
    assert!(
        !options
            .iter()
            .any(|o| matches!(o, SocketOption::Presenting(_))),
        "a credential to present is no setting"
    );
    let probing = Probing {
        idle: Duration::from_secs(60),
        interval: None,
        count: None,
    };
    for option in [
        SocketOption::NoDelay(true),
        SocketOption::KeepAlive(Some(probing)),
    ] {
        let set = net.host.set_option("client", client, option, Span::DUMMY);
        assert_eq!(outcome(set.expect("a socket")), Ok(Vec::new()));
    }
    let options = net.options("client", client);
    assert!(
        options.contains(&SocketOption::NoDelay(true)),
        "{options:?}"
    );
    assert!(
        options
            .iter()
            .any(|o| matches!(o, SocketOption::KeepAlive(Some(_)))),
        "{options:?}"
    );
    net.close("client", client);
    net.close("conn", server);
    net.close("listener", listener);
}

#[test]
fn using_a_socket_handle_under_a_different_name_or_as_the_wrong_kind_of_socket_or_after_closing_fails()
 {
    let net = Net::new();
    let (listener, here) = net.listening(loopback());
    let (client, server) = net.joined(listener, &here);
    let twice = net
        .host
        .recv("other", client, 8, LONG, Span::DUMMY)
        .err()
        .expect("one socket under two labels");
    assert!(
        twice.message.contains("net[other]") && twice.message.contains("net[client]"),
        "{}",
        twice.message
    );
    let read_a_listener = net
        .host
        .recv("listener", listener, 8, LONG, Span::DUMMY)
        .err()
        .expect("a listener is no connection");
    assert_eq!(read_a_listener.code, codes::RUNTIME_ERROR);
    let accept_a_connection = net
        .host
        .accept("client", client, Span::DUMMY)
        .err()
        .expect("a connection is no listener");
    assert_eq!(accept_a_connection.code, codes::RUNTIME_ERROR);
    net.close("client", client);
    let closed = net
        .host
        .send("client", client, b"x".to_vec(), LONG, Span::DUMMY)
        .err()
        .expect("a closed handle names nothing");
    assert!(
        closed.message.contains("no open socket"),
        "{}",
        closed.message
    );
    net.close("conn", server);
    net.close("listener", listener);
}

#[test]
fn connecting_to_localhost_succeeds_and_connecting_to_an_unresolvable_host_name_returns_none() {
    let net = Net::new();
    let (listener, here) = net.listening(loopback());
    let connecting = net
        .host
        .connect("client", "localhost".to_string(), here.port, LONG);
    let dialled = std::thread::spawn(move || ran(connecting));
    let server = int(&ran(net
        .host
        .accept("listener", listener, Span::DUMMY)
        .expect("an accept")));
    let client = int(&found(dialled.join().expect("the dial")).expect("reached"));
    net.close("client", client);
    net.close("conn", server);
    for name in ["\u{ff4c}ocalhost", "no-such-host.invalid"] {
        let dialled = ran(net.host.connect(
            "client",
            name.to_string(),
            here.port,
            Duration::from_millis(500),
        ));
        assert_eq!(found(dialled), None, "{name}");
    }
    net.close("listener", listener);
}

#[cfg(unix)]
mod unix {
    use super::*;
    use ply_host::fs::{RootSpec, Roots};

    fn rooted(dir: &std::path::Path) -> Net {
        let roots = Roots::load(
            &[RootSpec {
                name: "run".to_string(),
                path: dir.to_path_buf(),
            }],
            Span::DUMMY,
        )
        .expect("the temp dir is a root");
        let net = Net::new();
        net.host.rooted(roots);
        net
    }

    #[test]
    fn a_unix_socket_is_a_file_under_its_root_directory_and_is_removed_when_its_listener_closes() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::tempdir().expect("a temp dir");
        let net = rooted(dir.path());
        let file = dir.path().join("app.sock");
        let listen = || {
            outcome(
                net.host
                    .listen_unix("listener", "run", "app.sock", Span::DUMMY)
                    .expect("the root is bound"),
            )
        };
        let listener = int(&listen().expect("the path is free"));
        assert!(file.exists(), "the socket is a file under the root");
        assert_eq!(listen(), Err(NetError::AddressInUse.written()));

        let connecting = net
            .host
            .connect_unix("client", "run", "app.sock", LONG, Span::DUMMY)
            .expect("the root is bound");
        let dialled = std::thread::spawn(move || ran(connecting));
        let server = int(&ran(net
            .host
            .accept("listener", listener, Span::DUMMY)
            .expect("an accept")));
        let client = int(&outcome(dialled.join().expect("the dial")).expect("connected"));
        assert_eq!(net.send("client", client, b"over a path"), 11);
        assert_eq!(
            net.recv("conn", server, 64, LONG),
            Some(b"over a path".to_vec())
        );
        assert_eq!(net.peer("conn", server), None);
        assert_eq!(net.address("client", client), None);

        let owner = found(
            net.host
                .peer_credentials("conn", server, Span::DUMMY)
                .expect("a socket"),
        )
        .expect("the far end of a Unix socket has an owner");
        let parts = parts_of(&owner).expect("parts");
        let mine = std::fs::metadata(dir.path()).expect("the temp dir").uid();
        assert_eq!(
            int(parts[0]),
            i64::from(mine),
            "the client is this process, which made the temp dir"
        );
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        assert_eq!(int(parts[2]), i64::from(std::process::id()));
        let start = net.host.start_tls(
            "client",
            client,
            "localhost",
            Upgrade {
                alpn: Vec::new(),
                unread: 0,
                timeout: LONG,
            },
            Span::DUMMY,
        );
        assert_eq!(
            start.err().expect("an upgrade takes a TCP connection").code,
            codes::RUNTIME_ERROR
        );

        net.close("client", client);
        net.close("conn", server);
        net.close("listener", listener);
        assert!(!file.exists(), "closing the listener removes its socket");
        let after = net
            .host
            .connect_unix("client", "run", "app.sock", LONG, Span::DUMMY)
            .expect("the root is bound");
        assert_eq!(outcome(ran(after)), Err(NetError::Rejected.written()));
    }

    #[test]
    fn a_unix_socket_path_cannot_leave_its_root_directory_or_use_a_root_name_that_does_not_exist() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let net = rooted(dir.path());
        let escaping = net
            .host
            .listen_unix("listener", "run", "../outside.sock", Span::DUMMY)
            .expect_err("`..` leaves the root");
        assert_eq!(escaping.code, codes::FS_PATH_ESCAPES_ROOT);
        let unbound = net
            .host
            .listen_unix("listener", "elsewhere", "app.sock", Span::DUMMY)
            .expect_err("no root is bound under that name");
        assert_eq!(unbound.code, codes::FS_ROOT_UNBOUND);
    }
}

struct Material {
    _dir: tempfile::TempDir,
    certificate: std::path::PathBuf,
    key: std::path::PathBuf,
}

fn material() -> Material {
    let dir = tempfile::tempdir().expect("a temp dir");
    let issued = ply_host::certgen::issue(&[]).expect("a certificate is issued");
    let certificate = dir.path().join("cert.pem");
    let key = dir.path().join("key.pem");
    std::fs::write(&certificate, &issued.certificate).expect("the certificate is written");
    std::fs::write(&key, &issued.key).expect("the key is written");
    Material {
        _dir: dir,
        certificate,
        key,
    }
}

impl Material {
    fn net(&self, trusting: bool) -> Net {
        let spec = CredentialSpec {
            name: "mail".to_string(),
            certificate: self.certificate.clone(),
            key: self.key.clone(),
        };
        let trusted = if trusting {
            vec![self.certificate.clone()]
        } else {
            Vec::new()
        };
        Net {
            host: TcpHost::with_credentials(
                Credentials::load(&[spec], &trusted, &[]).expect("the generated material loads"),
            ),
        }
    }
}

fn upgrade(alpn: &[&str], unread: usize, timeout: Duration) -> Upgrade {
    Upgrade {
        alpn: alpn.iter().map(|p| p.to_string()).collect(),
        unread,
        timeout,
    }
}

type Finished = Result<Option<String>, Vec<u8>>;

fn finished(result: Vec<u8>) -> Finished {
    outcome(result)
        .map(|agreed| found(agreed).map(|p| String::from_utf8(p).expect("a protocol is text")))
}

fn upgraded(
    net: &Net,
    client: i64,
    server: i64,
    name: &str,
    offered: &[&str],
    accepted: &[&str],
) -> (Finished, Finished) {
    let serving = net
        .host
        .serve_tls(
            "conn",
            server,
            "mail",
            upgrade(accepted, 0, LONG),
            Span::DUMMY,
        )
        .expect("the server's half is taken");
    let served = std::thread::spawn(move || ran(serving));
    let starting = net
        .host
        .start_tls(
            "client",
            client,
            name,
            upgrade(offered, 0, LONG),
            Span::DUMMY,
        )
        .expect("the client's half is taken");
    let started = finished(ran(starting));
    (started, finished(served.join().expect("the server's half")))
}

fn handshake(net: &Net, label: &str, conn: i64) -> Option<Vec<u8>> {
    found(ran(net
        .host
        .handshake(label, conn, Span::DUMMY)
        .expect("a connection")))
}

fn certificate(net: &Net, label: &str, conn: i64) -> Option<Vec<u8>> {
    found(
        net.host
            .peer_certificate(label, conn, Span::DUMMY)
            .expect("a connection"),
    )
}

#[test]
fn an_open_connection_upgraded_to_tls_is_secured_at_both_ends_and_still_sends_and_receives_bytes() {
    let material = material();
    let net = material.net(true);
    let (listener, here) = net.listening(loopback());
    let (client, server) = net.joined(listener, &here);
    assert_eq!(net.send("client", client, b"STARTTLS\r\n"), 10);
    assert_eq!(
        net.recv("conn", server, 64, LONG),
        Some(b"STARTTLS\r\n".to_vec())
    );
    assert_eq!(
        handshake(&net, "client", client),
        None,
        "a plaintext connection has no handshake to complete"
    );

    let (started, served) = upgraded(&net, client, server, "localhost", &[], &[]);
    assert_eq!(started, Ok(None));
    assert_eq!(served, Ok(None));
    assert!(
        handshake(&net, "client", client).is_some(),
        "the same handle is the secured connection"
    );
    assert!(certificate(&net, "client", client).is_some());
    assert_eq!(
        certificate(&net, "conn", server),
        None,
        "the client presented none"
    );

    assert_eq!(net.send("client", client, b"EHLO secured\r\n"), 14);
    assert_eq!(
        net.recv("conn", server, 64, LONG),
        Some(b"EHLO secured\r\n".to_vec())
    );
    assert_eq!(net.peer("conn", server), net.address("client", client));
    net.close("client", client);
    assert_eq!(
        net.recv("conn", server, 64, LONG),
        Some(Vec::new()),
        "the handle's close ends the session"
    );
    net.close("conn", server);
    net.close("listener", listener);
    assert_eq!(net.host.handshakes().completed, 2);
}

#[test]
fn a_socket_presenting_a_credential_shows_its_certificate_to_the_server() {
    let material = material();
    let net = material.net(true);
    let (listener, here) = net.listening(loopback());
    let (client, server) = net.joined(listener, &here);
    let presenting = SocketOption::Presenting("mail".to_string());
    let set = net
        .host
        .set_option("client", client, presenting, Span::DUMMY);
    assert_eq!(outcome(set.expect("a socket")), Ok(Vec::new()));
    let (started, served) = upgraded(&net, client, server, "localhost", &[], &[]);
    assert!(started.is_ok() && served.is_ok());
    let seen = certificate(&net, "conn", server);
    assert!(seen.is_some(), "the server sees the certificate presented");
    assert_eq!(seen, certificate(&net, "client", client));
    net.close("client", client);
    net.close("conn", server);

    let (client, server) = net.joined(listener, &here);
    let absent = SocketOption::Presenting("absent".to_string());
    let set = net.host.set_option("client", client, absent, Span::DUMMY);
    assert_eq!(outcome(set.expect("a socket")), Ok(Vec::new()));
    let unknown = net
        .host
        .start_tls(
            "client",
            client,
            "localhost",
            upgrade(&[], 0, LONG),
            Span::DUMMY,
        )
        .err()
        .expect("the run holds no credential of that name");
    assert_eq!(unknown.code, codes::TLS_CREDENTIAL_UNKNOWN);
    let unknown = net
        .host
        .serve_tls("conn", server, "absent", upgrade(&[], 0, LONG), Span::DUMMY)
        .err()
        .expect("the run holds no credential of that name");
    assert_eq!(unknown.code, codes::TLS_CREDENTIAL_UNKNOWN);
    net.close("client", client);
    net.close("conn", server);
    net.close("listener", listener);
}

#[test]
fn an_upgrade_agrees_on_a_protocol_both_ends_name() {
    let material = material();
    let net = material.net(true);
    let (listener, here) = net.listening(loopback());
    let (client, server) = net.joined(listener, &here);
    let (started, served) = upgraded(
        &net,
        client,
        server,
        "localhost",
        &["postgresql", "other"],
        &["postgresql"],
    );
    assert_eq!(started, Ok(Some("postgresql".to_string())));
    assert_eq!(served, Ok(Some("postgresql".to_string())));
    let agreed = net.host.protocol("client", client, Span::DUMMY);
    assert_eq!(
        found(agreed.expect("a connection")),
        Some(b"postgresql".to_vec())
    );
    net.close("client", client);
    net.close("conn", server);
    net.close("listener", listener);
}

#[test]
fn plaintext_ahead_of_a_handshake_is_rejected_and_ends_the_connection() {
    let material = material();
    let net = material.net(true);
    let (listener, here) = net.listening(loopback());

    let (client, server) = net.joined(listener, &here);
    assert_eq!(
        net.send("conn", server, b"220 go ahead\r\n250 injected\r\n"),
        28
    );
    assert_eq!(
        net.recv("client", client, 14, LONG),
        Some(b"220 go ahead\r\n".to_vec())
    );
    let started = net
        .host
        .start_tls(
            "client",
            client,
            "localhost",
            upgrade(&[], 0, LONG),
            Span::DUMMY,
        )
        .expect("the upgrade is taken");
    assert_eq!(finished(ran(started)), Err(NetError::Injected.written()));
    assert_eq!(
        net.recv("client", client, 64, LONG),
        Some(Vec::new()),
        "what was injected is never read as if it had come secured"
    );
    net.close("client", client);
    net.close("conn", server);

    let (client, server) = net.joined(listener, &here);
    let started = net
        .host
        .start_tls(
            "client",
            client,
            "localhost",
            upgrade(&[], 14, LONG),
            Span::DUMMY,
        )
        .expect("the upgrade is taken");
    assert_eq!(finished(ran(started)), Err(NetError::Injected.written()));
    let served = net
        .host
        .serve_tls("conn", server, "mail", upgrade(&[], 17, LONG), Span::DUMMY)
        .expect("the upgrade is taken");
    assert_eq!(finished(ran(served)), Err(NetError::Injected.written()));
    net.close("client", client);
    net.close("conn", server);
    net.close("listener", listener);
    let injected: u64 = net
        .host
        .handshakes()
        .reasons
        .iter()
        .filter(|(reason, _)| *reason == ply_host::tls::REASON_EARLY_PLAINTEXT)
        .map(|(_, n)| *n)
        .sum();
    assert_eq!(injected, 3, "each rejection is counted under its reason");
}

#[test]
fn a_server_the_client_cannot_verify_as_its_name_is_rejected() {
    let material = material();
    for (trusting, name) in [(false, "localhost"), (true, "mail.example")] {
        let net = material.net(trusting);
        let (listener, here) = net.listening(loopback());
        let (client, server) = net.joined(listener, &here);
        let (started, served) = upgraded(&net, client, server, name, &[], &[]);
        assert_eq!(started, Err(NetError::Untrusted.written()), "{name}");
        assert!(served.is_err(), "the server's half fails with the client's");
        net.close("client", client);
        net.close("conn", server);
        net.close("listener", listener);
    }
    let net = material.net(true);
    let (listener, here) = net.listening(loopback());
    let (client, server) = net.joined(listener, &here);
    let started = net
        .host
        .start_tls(
            "client",
            client,
            "not a name",
            upgrade(&[], 0, LONG),
            Span::DUMMY,
        )
        .expect("the upgrade is taken");
    let rejected = finished(ran(started)).expect_err("no server is verified as that");
    assert_eq!(rejected[0], 10, "a handshake rejected for its reason");
    assert_eq!(
        net.recv("conn", server, 64, LONG),
        Some(Vec::new()),
        "the rejection ended the connection"
    );
    net.close("client", client);
    net.close("conn", server);
    net.close("listener", listener);
}

#[test]
fn a_tls_upgrade_times_out_when_the_peer_never_completes_the_handshake() {
    let material = material();
    let net = material.net(true);
    let (listener, here) = net.listening(loopback());
    let (client, server) = net.joined(listener, &here);
    let began = std::time::Instant::now();
    let started = net
        .host
        .start_tls(
            "client",
            client,
            "localhost",
            upgrade(&[], 0, Duration::from_millis(200)),
            Span::DUMMY,
        )
        .expect("the upgrade is taken");
    assert_eq!(finished(ran(started)), Err(NetError::TimedOut.written()));
    assert!(began.elapsed() < Duration::from_secs(4));
    net.close("client", client);
    net.close("conn", server);
    net.close("listener", listener);
}

#[test]
fn a_tls_upgrade_is_rejected_while_a_read_on_the_connection_is_pending_and_when_the_connection_is_already_secured()
 {
    let material = material();
    let net = material.net(true);
    let (listener, here) = net.listening(loopback());
    let (client, server) = net.joined(listener, &here);
    let waiting = net
        .host
        .recv("client", client, 64, LONG, Span::DUMMY)
        .expect("a read");
    let under = net
        .host
        .start_tls(
            "client",
            client,
            "localhost",
            upgrade(&[], 0, LONG),
            Span::DUMMY,
        )
        .err()
        .expect("a read still holds the plaintext");
    assert_eq!(under.code, codes::RUNTIME_ERROR);
    drop(waiting);
    let (started, served) = upgraded(&net, client, server, "localhost", &[], &[]);
    assert!(started.is_ok() && served.is_ok());
    let twice = net
        .host
        .start_tls(
            "client",
            client,
            "localhost",
            upgrade(&[], 0, LONG),
            Span::DUMMY,
        )
        .err()
        .expect("the connection is already TLS");
    assert_eq!(twice.code, codes::RUNTIME_ERROR);
    net.close("client", client);
    net.close("conn", server);
    net.close("listener", listener);
}

#[test]
fn a_tls_listener_serves_a_client_that_connects_over_tls() {
    let material = material();
    let net = material.net(true);
    let listener = net
        .host
        .listen_tls("listener", 0, "mail", Span::DUMMY)
        .expect("the credential is held");
    let port = net.host.local_addr(listener).expect("a bound port").port();
    let connecting = net
        .host
        .connect_tls("client", "localhost".to_string(), port, LONG);
    let dialled = std::thread::spawn(move || ran(connecting));
    let server = int(&ran(net
        .host
        .accept("listener", listener, Span::DUMMY)
        .expect("an accept")));
    let client = int(&found(dialled.join().expect("the dial")).expect("reached"));
    let sending = net
        .host
        .send(
            "client",
            client,
            b"GET / HTTP/1.1\r\n".to_vec(),
            LONG,
            Span::DUMMY,
        )
        .expect("a send");
    let sent = std::thread::spawn(move || ran(sending));
    assert_eq!(
        net.recv("conn", server, 64, LONG),
        Some(b"GET / HTTP/1.1\r\n".to_vec()),
        "the first read is where the handshake happens"
    );
    assert_eq!(
        found(sent.join().expect("the send")).map(|n| int(&n)),
        Some(16)
    );
    let unknown = net
        .host
        .listen_tls("listener", 0, "absent", Span::DUMMY)
        .expect_err("the run holds no such credential");
    assert_eq!(unknown.code, codes::TLS_CREDENTIAL_UNKNOWN);
    net.close("client", client);
    net.close("conn", server);
    net.close("listener", listener);
}

fn datagram_socket(net: &Net, label: &str) -> (i64, Endpoint) {
    let bound = ply_host::udp::bind(&net.host, label, &at(loopback(), 0), &[]);
    let socket = int(&outcome(bound).expect("loopback binds"));
    let here = net.address(label, socket).expect("a bound address");
    (socket, here)
}

fn received(
    net: &Net,
    label: &str,
    socket: i64,
    max: usize,
    timeout: Duration,
) -> Result<(Endpoint, Vec<u8>, bool), Vec<u8>> {
    let job = ply_host::udp::recv_from(&net.host, label, socket, max, timeout, Span::DUMMY)
        .expect("a datagram socket");
    outcome(ran(job)).map(|flat| {
        let parts = parts_of(&flat).expect("parts");
        (
            Endpoint::read(parts[0]).expect("an address"),
            parts[1].to_vec(),
            parts[2] == [1],
        )
    })
}

fn sent_to(net: &Net, label: &str, socket: i64, to: &Endpoint, payload: &[u8]) -> i64 {
    let job = ply_host::udp::send_to(
        &net.host,
        label,
        socket,
        to.clone(),
        payload.to_vec(),
        LONG,
        Span::DUMMY,
    )
    .expect("a datagram socket");
    int(&outcome(ran(job)).expect("sent"))
}

#[test]
fn a_datagram_sent_over_loopback_arrives_with_its_sender_address_and_a_truncated_flag_set_when_it_was_cut_to_fit_the_buffer()
 {
    let net = Net::new();
    let (server, there) = datagram_socket(&net, "socket");
    let (client, here) = datagram_socket(&net, "other");
    assert_eq!(sent_to(&net, "other", client, &there, b"ping"), 4);
    assert_eq!(
        received(&net, "socket", server, 512, LONG),
        Ok((here.clone(), b"ping".to_vec(), false))
    );
    assert_eq!(
        received(&net, "socket", server, 512, Duration::from_millis(50)),
        Err(NetError::TimedOut.written())
    );
    sent_to(&net, "other", client, &there, b"0123456789");
    sent_to(&net, "other", client, &there, b"0123");
    assert_eq!(
        received(&net, "socket", server, 4, LONG),
        Ok((here.clone(), b"0123".to_vec(), true)),
        "six bytes of it are gone, and the result says so"
    );
    assert_eq!(
        received(&net, "socket", server, 4, LONG),
        Ok((here, b"0123".to_vec(), false)),
        "a datagram that fits exactly is whole"
    );
    net.close("socket", server);
    net.close("other", client);
}

#[test]
fn a_connected_datagram_socket_sends_without_naming_its_far_end() {
    let net = Net::new();
    let (server, there) = datagram_socket(&net, "socket");
    let (client, here) = datagram_socket(&net, "other");
    let unconnected =
        ply_host::udp::send(&net.host, "other", client, b"x".to_vec(), LONG, Span::DUMMY)
            .err()
            .expect("the socket has no far end yet");
    assert_eq!(unconnected.code, codes::RUNTIME_ERROR);
    let connected = ply_host::udp::connect(&net.host, "other", client, &there, Span::DUMMY);
    assert_eq!(
        outcome(connected.expect("a datagram socket")),
        Ok(Vec::new())
    );
    let job = ply_host::udp::send(
        &net.host,
        "other",
        client,
        b"hello".to_vec(),
        LONG,
        Span::DUMMY,
    )
    .expect("a connected socket");
    assert_eq!(int(&outcome(ran(job)).expect("sent")), 5);
    assert_eq!(
        received(&net, "socket", server, 512, LONG),
        Ok((here, b"hello".to_vec(), false))
    );
    let (listener, _) = net.listening(loopback());
    let as_datagram =
        ply_host::udp::recv_from(&net.host, "listener", listener, 64, LONG, Span::DUMMY)
            .err()
            .expect("a listener is no datagram socket");
    assert_eq!(as_datagram.code, codes::RUNTIME_ERROR);
    let as_stream = net
        .host
        .recv("socket", server, 64, LONG, Span::DUMMY)
        .err()
        .expect("a datagram socket is no connection");
    assert_eq!(as_stream.code, codes::RUNTIME_ERROR);
}

#[test]
fn a_datagram_sockets_options_are_set_and_read_back() {
    let net = Net::new();
    let (socket, here) = datagram_socket(&net, "socket");
    assert!(
        net.options("socket", socket)
            .contains(&SocketOption::Broadcast(false))
    );
    let set = net
        .host
        .set_option("socket", socket, SocketOption::Broadcast(true), Span::DUMMY);
    assert_eq!(outcome(set.expect("a socket")), Ok(Vec::new()));
    let now = net.options("socket", socket);
    assert!(now.contains(&SocketOption::Broadcast(true)), "{now:?}");
    assert!(
        !now.iter().any(|o| matches!(o, SocketOption::NoDelay(_))),
        "a datagram socket has no stream's options: {now:?}"
    );
    let taken = ply_host::udp::bind(&net.host, "other", &here, &[]);
    assert_eq!(outcome(taken), Err(NetError::AddressInUse.written()));
}

#[test]
fn every_socket_option_converts_to_a_flat_list_of_values_and_back_to_the_same_option() {
    let probing = Probing {
        idle: Duration::from_secs(60),
        interval: Some(Duration::from_secs(5)),
        count: Some(3),
    };
    let options = [
        SocketOption::NoDelay(true),
        SocketOption::KeepAlive(Some(probing)),
        SocketOption::KeepAlive(None),
        SocketOption::ReuseAddress(false),
        SocketOption::ReusePort(true),
        SocketOption::SendBuffer(65536),
        SocketOption::ReceiveBuffer(0),
        SocketOption::Linger(Some(Duration::from_millis(250))),
        SocketOption::Linger(None),
        SocketOption::Broadcast(true),
        SocketOption::MulticastLoop(false),
        SocketOption::JoinGroup(
            IpAddr::V4(Ipv4Addr::new(239, 0, 0, 1)),
            Some("en0".to_string()),
        ),
        SocketOption::LeaveGroup(IpAddr::V6(Ipv6Addr::LOCALHOST), None),
        SocketOption::Presenting("client".to_string()),
    ];
    assert_eq!(
        SocketOption::list(&SocketOption::listed(&options)),
        Some(options.to_vec())
    );
    let zoned = Endpoint {
        ip: IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1)),
        port: 8080,
        zone: Some("3".to_string()),
    };
    assert_eq!(Endpoint::read(&zoned.written()), Some(zoned));
    assert_eq!(Endpoint::read(b"\x00\x00\x00\x03abc"), None);
    assert_eq!(SocketOption::read(&[12]), None);
}

#[test]
fn localhost_resolves_to_loopback_addresses_without_duplicates_and_an_invalid_host_name_fails() {
    let result = ply_host::dns::lookup("localhost");
    let (kind, rest) = result.split_first().expect("a result");
    assert_eq!(*kind, 1, "every machine names itself");
    let found: Vec<IpAddr> = parts_of(rest)
        .expect("parts")
        .into_iter()
        .map(|b| ply_host::dns::ip_address_from_bytes(b).expect("an address"))
        .collect();
    assert!(
        !found.is_empty() && found.iter().all(IpAddr::is_loopback),
        "{found:?}"
    );
    let mut once = found.clone();
    once.sort();
    once.dedup();
    assert_eq!(once.len(), found.len(), "{found:?}");
    assert_eq!(ply_host::dns::lookup("no-such-host.invalid")[0], 0);
    let reversed = ply_host::dns::reverse(loopback());
    assert!(matches!(reversed.first(), Some(0 | 1)));
}
