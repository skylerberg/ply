//! The socket host over real loopback: addresses, options, Unix sockets, the upgrade to TLS,
//! datagrams and the resolver.

use ply_eval::host::MachineId;
use ply_eval::{
    Diagnostic, HostAnswer, HostBinding, HostRequest, HostRuntime, Span, Symbol, Value, codes,
};
use ply_host::tcp::wire::{Credentials as PeerCredentials, Endpoint, Probing, SocketOption};
use ply_host::{Credentials, Host};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::rc::Rc;
use std::time::Duration;

const NET: &str = "std.net.net";
const UDP: &str = "std.udp.udp";
const DNS: &str = "std.dns.dns";

/// Names every operation the tests perform, under the labels they perform it under.
const DRIVER: &str = r#"
import std.dns (dns)
import std.net (net)
import std.udp (udp)

fn driven() -> Unit / {net.read[listener], net.write[listener], net.read[conn], net.write[conn], net.read[client], net.write[client], udp.read[socket], udp.write[socket], udp.read[other], udp.write[other], dns.lookup, dns.reverse, dns.servers} = ()
"#;

/// One host, bound to the driver, with the runtime that settles what it leaves pending.
struct Bound {
    host: Host,
    binding: HostBinding,
    runtime: Rc<dyn HostRuntime>,
}

fn bound(host: Host) -> Bound {
    let check = crate::support::answered::checked("app", DRIVER).check;
    let binding = host
        .registry()
        .bind(&check)
        .expect("the declarations and the registrations agree");
    let runtime = host.runtime();
    Bound {
        host,
        binding,
        runtime,
    }
}

impl Bound {
    /// The handler's own answer, which a waiting operation leaves pending.
    fn call(
        &self,
        effect: &str,
        op: &str,
        label: Option<&str>,
        args: Vec<Value>,
    ) -> Result<HostAnswer, Diagnostic> {
        let label = label.map(Symbol::new);
        let served = self
            .binding
            .resolve(&Symbol::new(effect), &Symbol::new(op), label.as_ref())
            .unwrap_or_else(|| panic!("the registry serves `{effect}.{op}` under {label:?}"));
        let request = HostRequest {
            machine: MachineId(1),
            atom: served.atom.clone(),
            op: served.op,
            args: &args,
            span: Span::DUMMY,
            task: None,
            declared: None,
        };
        served.handler.call(self.runtime.as_ref(), &request)
    }

    fn settle(&self, answer: HostAnswer) -> Value {
        match answer {
            HostAnswer::Value(v) => v,
            HostAnswer::Pending(pending) => self
                .runtime
                .block_on(pending)
                .expect("the operation settles"),
        }
    }

    fn perform(
        &self,
        effect: &str,
        op: &str,
        label: Option<&str>,
        args: Vec<Value>,
    ) -> Result<Value, Diagnostic> {
        self.call(effect, op, label, args).map(|a| self.settle(a))
    }

    fn net(&self, op: &str, label: &str, args: Vec<Value>) -> Value {
        self.perform(NET, op, Some(label), args)
            .unwrap_or_else(|d| panic!("`net.{op}` is served: {d:?}"))
    }

    fn udp(&self, op: &str, label: &str, args: Vec<Value>) -> Value {
        self.perform(UDP, op, Some(label), args)
            .unwrap_or_else(|d| panic!("`udp.{op}` is served: {d:?}"))
    }

    /// A listener on a loopback port of the kernel's choosing, and where it is.
    fn listening(&self, ip: IpAddr) -> (i64, Endpoint) {
        let listener = int(ok(self.net(
            "listen_on",
            "listener",
            vec![at(ip, 0).value(), Value::list(Vec::new())],
        )));
        let here = self.address("local_address", "listener", listener);
        (listener, here.expect("a listener has an address"))
    }

    /// A connection to `to` and the listener's end of it.
    fn joined(&self, listener: i64, to: &Endpoint) -> (i64, i64) {
        let client = int(ok(self.net(
            "connect_to",
            "client",
            vec![to.value(), Value::list(Vec::new()), Value::Int(5000)],
        )));
        let server = int(self.net("accept", "listener", vec![Value::Int(listener)]));
        (client, server)
    }

    fn address(&self, op: &str, label: &str, socket: i64) -> Option<Endpoint> {
        maybe(self.net(op, label, vec![Value::Int(socket)]))
            .map(|v| Endpoint::read(&v, Span::DUMMY).expect("a socket address"))
    }

    fn options(&self, label: &str, socket: i64) -> Vec<SocketOption> {
        SocketOption::list(
            &self.net("options", label, vec![Value::Int(socket)]),
            Span::DUMMY,
        )
        .expect("a list of options")
    }

    fn send(&self, label: &str, conn: i64, payload: &[u8]) -> i64 {
        int(some(self.net(
            "send",
            label,
            vec![Value::Int(conn), Value::bytes(payload), Value::Int(5000)],
        )))
    }

    /// `None` is the deadline, which these tests set short where they expect it.
    fn recv(&self, label: &str, conn: i64, timeout_ms: i64) -> Option<Vec<u8>> {
        maybe(self.net(
            "recv",
            label,
            vec![Value::Int(conn), Value::Int(4096), Value::Int(timeout_ms)],
        ))
        .map(bytes)
    }

    fn close(&self, label: &str, socket: i64) {
        self.net("close", label, vec![Value::Int(socket)]);
    }
}

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

fn int(v: Value) -> i64 {
    v.as_int(Span::DUMMY, "a test expectation")
        .expect("an Int answer")
}

fn bytes(v: Value) -> Vec<u8> {
    v.as_bytes(Span::DUMMY, "a test expectation")
        .expect("a Bytes answer")
        .to_vec()
}

fn maybe(v: Value) -> Option<Value> {
    match &v {
        Value::Ctor { name, args } if name.as_str() == "Some" => Some(args[0].clone()),
        Value::Ctor { name, .. } if name.as_str() == "None" => None,
        other => panic!("not an `Option`: {other:?}"),
    }
}

fn some(v: Value) -> Value {
    maybe(v).expect("the operation answered `None`")
}

/// A `Result` whose refusal is told by its constructor's own name: `Refused`, `Injected`.
fn result(v: Value) -> Result<Value, String> {
    match &v {
        Value::Ctor { name, args } if name.as_str() == "Ok" => Ok(args[0].clone()),
        Value::Ctor { name, args } if name.as_str() == "Err" => match &args[0] {
            Value::Ctor { name, .. } => {
                let full = name.as_str();
                Err(full.rsplit('.').next().unwrap_or(full).to_string())
            }
            other => panic!("not a refusal: {other:?}"),
        },
        other => panic!("not a `Result`: {other:?}"),
    }
}

fn ok(v: Value) -> Value {
    result(v).unwrap_or_else(|why| panic!("the operation was refused: {why}"))
}

fn refused(v: Value) -> String {
    match result(v) {
        Ok(answer) => panic!("the operation was not refused: {answer:?}"),
        Err(why) => why,
    }
}

fn text(s: &str) -> Value {
    Value::str(s)
}

fn strings(items: &[&str]) -> Value {
    Value::list(items.iter().map(Value::str).collect())
}

/// The protocol a `std.net.Secured` says the two ends agreed on.
fn protocol(secured: Value) -> Option<String> {
    let Value::Record(fields) = &secured else {
        panic!("not a `Secured`: {secured:?}");
    };
    maybe(fields.named("protocol").expect("a protocol").clone()).map(|p| {
        p.as_str(Span::DUMMY, "a protocol")
            .expect("a String")
            .to_string()
    })
}

#[test]
fn a_connection_knows_both_its_ends() {
    let b = bound(Host::new());
    let (listener, here) = b.listening(loopback());
    assert_eq!(here.ip, loopback());
    assert_ne!(here.port, 0, "the kernel chose a port");
    let (client, server) = b.joined(listener, &here);

    let from = b.address("peer_address", "conn", server).expect("a peer");
    assert_eq!(from.ip, loopback(), "the client came from loopback");
    assert_eq!(
        Some(from),
        b.address("local_address", "client", client),
        "the server's peer is the client's own end"
    );
    assert_eq!(
        b.address("peer_address", "client", client),
        Some(here.clone())
    );
    assert_eq!(b.address("local_address", "conn", server), Some(here));
    assert_eq!(
        b.address("peer_address", "listener", listener),
        None,
        "a listener has no far end"
    );
    b.close("client", client);
    b.close("conn", server);
    b.close("listener", listener);
}

#[test]
fn an_ipv6_connection_knows_both_its_ends_where_the_machine_has_ipv6() {
    if std::net::TcpListener::bind((Ipv6Addr::LOCALHOST, 0)).is_err() {
        return;
    }
    let b = bound(Host::new());
    let (listener, here) = b.listening(IpAddr::V6(Ipv6Addr::LOCALHOST));
    assert_eq!(here.ip, IpAddr::V6(Ipv6Addr::LOCALHOST));
    let (client, server) = b.joined(listener, &here);
    let from = b.address("peer_address", "conn", server).expect("a peer");
    assert_eq!(from.ip, IpAddr::V6(Ipv6Addr::LOCALHOST));
    assert_eq!(from.zone, None, "loopback is reached through no zone");
    assert_eq!(Some(from), b.address("local_address", "client", client));
    b.close("client", client);
    b.close("conn", server);
    b.close("listener", listener);
}

#[test]
fn a_connection_nothing_answers_is_refused_and_says_so() {
    let b = bound(Host::new());
    let free = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .and_then(|l| l.local_addr())
        .expect("a port that was free a moment ago");
    let answer = b.net(
        "connect_to",
        "client",
        vec![
            Endpoint::of(free).value(),
            Value::list(Vec::new()),
            Value::Int(5000),
        ],
    );
    assert_eq!(refused(answer), "Refused");
}

#[test]
fn a_listener_binds_loopback_only_and_one_address_once() {
    let b = bound(Host::new());
    let anywhere = b.net(
        "listen_on",
        "listener",
        vec![
            at(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0).value(),
            Value::list(Vec::new()),
        ],
    );
    assert_eq!(refused(anywhere), "PermissionDenied");

    let (listener, here) = b.listening(loopback());
    let again = b.net(
        "listen_on",
        "listener",
        vec![here.value(), Value::list(Vec::new())],
    );
    assert_eq!(refused(again), "AddressInUse");
    b.close("listener", listener);
}

#[test]
fn options_set_as_a_socket_opens_and_afterwards_are_read_back() {
    let b = bound(Host::new());
    let (listener, here) = b.listening(loopback());
    assert!(
        b.options("listener", listener)
            .contains(&SocketOption::ReuseAddress(true)),
        "a listener reuses its address unless told otherwise"
    );

    let probing = Probing {
        idle: Duration::from_secs(30),
        interval: Some(Duration::from_secs(5)),
        count: Some(4),
    };
    let asked = [
        SocketOption::NoDelay(false),
        SocketOption::KeepAlive(Some(probing)),
    ];
    let client = int(ok(b.net(
        "connect_to",
        "client",
        vec![
            here.value(),
            Value::list(asked.iter().map(SocketOption::value).collect()),
            Value::Int(5000),
        ],
    )));
    let server = int(b.net("accept", "listener", vec![Value::Int(listener)]));
    let opened = b.options("client", client);
    assert!(opened.contains(&SocketOption::NoDelay(false)), "{opened:?}");
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    assert!(
        opened.contains(&SocketOption::KeepAlive(Some(probing))),
        "the idle time, the interval and the count are the ones asked for: {opened:?}"
    );
    assert!(
        b.options("conn", server)
            .contains(&SocketOption::NoDelay(true)),
        "a connection sends small writes at once unless told otherwise"
    );

    for option in [SocketOption::NoDelay(true), SocketOption::KeepAlive(None)] {
        ok(b.net(
            "set_option",
            "client",
            vec![Value::Int(client), option.value()],
        ));
    }
    let now = b.options("client", client);
    assert!(now.contains(&SocketOption::NoDelay(true)), "{now:?}");
    assert!(now.contains(&SocketOption::KeepAlive(None)), "{now:?}");

    ok(b.net(
        "set_option",
        "client",
        vec![
            Value::Int(client),
            SocketOption::Linger(Some(Duration::from_secs(1))).value(),
        ],
    ));
    assert!(
        b.options("client", client)
            .contains(&SocketOption::Linger(Some(Duration::from_secs(1))))
    );
    b.close("client", client);
    b.close("conn", server);
    b.close("listener", listener);
}

#[test]
fn ending_ones_sending_is_the_peers_end_of_stream_and_leaves_reading_open() {
    let b = bound(Host::new());
    let (listener, here) = b.listening(loopback());
    let (client, server) = b.joined(listener, &here);
    assert_eq!(b.send("client", client, b"question"), 8);
    b.net("close_write", "client", vec![Value::Int(client)]);

    assert_eq!(b.recv("conn", server, 5000), Some(b"question".to_vec()));
    assert_eq!(
        b.recv("conn", server, 5000),
        Some(Vec::new()),
        "the half-close is the end of what the client sends"
    );
    assert_eq!(b.send("conn", server, b"answer"), 6);
    assert_eq!(
        b.recv("client", client, 5000),
        Some(b"answer".to_vec()),
        "the end that stopped sending still reads"
    );
    assert_eq!(
        b.send("client", client, b"more"),
        0,
        "nothing more is sent from an end that ended its sending"
    );
    b.close("client", client);
    b.close("conn", server);
    b.close("listener", listener);
}

#[cfg(unix)]
mod unix {
    use super::*;
    use ply_host::fs::{RootSpec, Roots};

    fn rooted(dir: &std::path::Path) -> Bound {
        let roots = Roots::load(
            &[RootSpec {
                name: "run".to_string(),
                path: dir.to_path_buf(),
            }],
            Span::DUMMY,
        )
        .expect("the temp dir is a root");
        bound(Host::new().rooted(roots))
    }

    fn at_path(path: &str) -> Vec<Value> {
        vec![text("run"), text(path)]
    }

    #[test]
    fn a_unix_socket_is_a_path_under_a_root_and_goes_with_its_listener() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::tempdir().expect("a temp dir");
        let b = rooted(dir.path());
        let file = dir.path().join("app.sock");

        let listener = int(ok(b.net("listen_unix", "listener", at_path("app.sock"))));
        assert!(file.exists(), "the socket is a file under the root");
        assert_eq!(
            refused(b.net("listen_unix", "listener", at_path("app.sock"))),
            "AddressInUse"
        );

        let mut dial = at_path("app.sock");
        dial.push(Value::Int(5000));
        let client = int(ok(b.net("connect_unix", "client", dial)));
        let server = int(b.net("accept", "listener", vec![Value::Int(listener)]));
        assert_eq!(b.send("client", client, b"over a path"), 11);
        assert_eq!(b.recv("conn", server, 5000), Some(b"over a path".to_vec()));

        assert_eq!(b.address("peer_address", "conn", server), None);
        assert_eq!(b.address("local_address", "client", client), None);
        assert_eq!(
            maybe(b.net("local_port", "listener", vec![Value::Int(listener)])).map(int),
            None
        );
        let owner = maybe(b.net("peer_credentials", "conn", vec![Value::Int(server)]))
            .expect("the far end of a Unix socket has an owner");
        let Value::Record(fields) = &owner else {
            panic!("not a `PeerCredentials`: {owner:?}");
        };
        let mine = std::fs::metadata(dir.path()).expect("the temp dir").uid();
        assert_eq!(
            int(fields.named("user").expect("a user").clone()),
            i64::from(mine),
            "the client is this process, which made the temp dir"
        );
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        assert_eq!(
            maybe(fields.named("process").expect("a process").clone()).map(int),
            Some(i64::from(std::process::id()))
        );

        b.close("client", client);
        b.close("conn", server);
        b.close("listener", listener);
        assert!(!file.exists(), "closing the listener removes its socket");
        let mut absent = at_path("app.sock");
        absent.push(Value::Int(5000));
        assert_eq!(refused(b.net("connect_unix", "client", absent)), "Refused");
    }

    #[test]
    fn a_tcp_connection_has_no_owner_to_report() {
        let b = bound(Host::new());
        let (listener, here) = b.listening(loopback());
        let (client, server) = b.joined(listener, &here);
        assert!(maybe(b.net("peer_credentials", "conn", vec![Value::Int(server)])).is_none());
        b.close("client", client);
        b.close("conn", server);
        b.close("listener", listener);
    }

    #[test]
    fn a_unix_socket_stays_inside_a_root_the_run_bound() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let b = rooted(dir.path());
        let escaping = b
            .perform(
                NET,
                "listen_unix",
                Some("listener"),
                vec![text("run"), text("../outside.sock")],
            )
            .expect_err("`..` leaves the root");
        assert_eq!(escaping.code, codes::FS_PATH_ESCAPES_ROOT);
        let unbound = b
            .perform(
                NET,
                "listen_unix",
                Some("listener"),
                vec![text("elsewhere"), text("app.sock")],
            )
            .expect_err("no root is bound under that name");
        assert_eq!(unbound.code, codes::FS_ROOT_UNBOUND);
    }

    #[test]
    fn a_unix_socket_is_not_secured() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let b = rooted(dir.path());
        let listener = int(ok(b.net("listen_unix", "listener", at_path("tls.sock"))));
        let mut dial = at_path("tls.sock");
        dial.push(Value::Int(5000));
        let client = int(ok(b.net("connect_unix", "client", dial)));
        let refusal = b
            .perform(
                NET,
                "start_tls",
                Some("client"),
                vec![
                    Value::Int(client),
                    text("localhost"),
                    strings(&[]),
                    Value::bytes(b""),
                    Value::Int(1000),
                ],
            )
            .expect_err("an upgrade takes a TCP connection");
        assert_eq!(refusal.code, codes::RUNTIME_ERROR);
        b.close("client", client);
        b.close("listener", listener);
    }
}

/// A certificate for `localhost` on disk, as `--tls` and `--trust` name one.
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
    /// A host that serves this certificate as `mail`, and trusts it when `trusting`.
    fn host(&self, trusting: bool) -> Bound {
        let spec = ply_host::CredentialSpec {
            name: "mail".to_string(),
            certificate: self.certificate.clone(),
            key: self.key.clone(),
        };
        let trusted = if trusting {
            vec![self.certificate.clone()]
        } else {
            Vec::new()
        };
        bound(Host::with_credentials(
            Credentials::load(&[spec], &trusted).expect("the generated material loads"),
        ))
    }
}

fn start(b: &Bound, conn: i64, name: &str, alpn: &[&str], unread: &[u8], ms: i64) -> HostAnswer {
    b.call(
        NET,
        "start_tls",
        Some("client"),
        vec![
            Value::Int(conn),
            text(name),
            strings(alpn),
            Value::bytes(unread),
            Value::Int(ms),
        ],
    )
    .expect("the client's half is served")
}

fn serve(b: &Bound, conn: i64, alpn: &[&str], unread: &[u8], ms: i64) -> HostAnswer {
    b.call(
        NET,
        "serve_tls",
        Some("conn"),
        vec![
            Value::Int(conn),
            text("mail"),
            strings(alpn),
            Value::bytes(unread),
            Value::Int(ms),
        ],
    )
    .expect("the server's half is served")
}

/// Both halves of an upgrade, each waiting on the other, settled together.
fn upgraded(
    b: &Bound,
    client: i64,
    server: i64,
    name: &str,
    offered: &[&str],
    accepted: &[&str],
) -> (Result<Value, String>, Result<Value, String>) {
    let serving = serve(b, server, accepted, b"", 5000);
    let starting = start(b, client, name, offered, b"", 5000);
    (result(b.settle(starting)), result(b.settle(serving)))
}

#[test]
fn an_open_connection_is_secured_at_both_ends_and_carries_bytes_after() {
    let material = material();
    let b = material.host(true);
    let (listener, here) = b.listening(loopback());
    let (client, server) = b.joined(listener, &here);

    // The plaintext exchange that asks for the upgrade, as a STARTTLS does.
    assert_eq!(b.send("client", client, b"STARTTLS\r\n"), 10);
    assert_eq!(b.recv("conn", server, 5000), Some(b"STARTTLS\r\n".to_vec()));
    assert_eq!(b.send("conn", server, b"220 go ahead\r\n"), 14);
    assert_eq!(
        b.recv("client", client, 5000),
        Some(b"220 go ahead\r\n".to_vec())
    );
    assert!(
        maybe(b.net("handshake", "client", vec![Value::Int(client)])).is_none(),
        "a plaintext connection has no handshake to complete"
    );

    let (started, served) = upgraded(&b, client, server, "localhost", &[], &[]);
    assert_eq!(protocol(started.expect("the client secured")), None);
    assert_eq!(protocol(served.expect("the server secured")), None);
    assert!(
        maybe(b.net("handshake", "client", vec![Value::Int(client)])).is_some(),
        "the same handle is the secured connection"
    );

    assert_eq!(b.send("client", client, b"EHLO secured\r\n"), 14);
    assert_eq!(
        b.recv("conn", server, 5000),
        Some(b"EHLO secured\r\n".to_vec())
    );
    assert_eq!(b.send("conn", server, b"250 ok\r\n"), 8);
    assert_eq!(b.recv("client", client, 5000), Some(b"250 ok\r\n".to_vec()));
    assert_eq!(
        b.address("peer_address", "conn", server),
        b.address("local_address", "client", client),
        "a secured connection still knows its ends"
    );

    b.close("client", client);
    assert_eq!(
        b.recv("conn", server, 5000),
        Some(Vec::new()),
        "the handle's close ends the session"
    );
    b.close("conn", server);
    b.close("listener", listener);
    assert_eq!(b.host.handshakes().completed, 2);
}

#[test]
fn an_upgrade_agrees_on_a_protocol_both_ends_name() {
    let material = material();
    let b = material.host(true);
    let (listener, here) = b.listening(loopback());
    let (client, server) = b.joined(listener, &here);
    let (started, served) = upgraded(
        &b,
        client,
        server,
        "localhost",
        &["postgresql", "other"],
        &["postgresql"],
    );
    assert_eq!(
        protocol(started.expect("the client secured")),
        Some("postgresql".to_string())
    );
    assert_eq!(
        protocol(served.expect("the server secured")),
        Some("postgresql".to_string())
    );
    b.close("client", client);
    b.close("conn", server);
    b.close("listener", listener);
}

#[test]
fn plaintext_waiting_in_the_socket_is_refused_and_ends_the_connection() {
    let material = material();
    let b = material.host(true);
    let (listener, here) = b.listening(loopback());
    let (client, server) = b.joined(listener, &here);
    // A go-ahead with a second line behind it, which the client reads only the first of.
    assert_eq!(
        b.send("conn", server, b"220 go ahead\r\n250 injected\r\n"),
        28
    );
    let read = maybe(b.net(
        "recv",
        "client",
        vec![Value::Int(client), Value::Int(14), Value::Int(5000)],
    ))
    .map(bytes);
    assert_eq!(read, Some(b"220 go ahead\r\n".to_vec()));

    let started = result(b.settle(start(&b, client, "localhost", &[], b"", 5000)));
    assert_eq!(started.expect_err("plaintext was waiting"), "Injected");
    assert_eq!(
        b.recv("client", client, 5000),
        Some(Vec::new()),
        "what was injected is never read as if it had come secured"
    );
    assert_eq!(b.send("client", client, b"AUTH"), 0);
    b.close("client", client);
    b.close("conn", server);
    b.close("listener", listener);
}

#[test]
fn plaintext_the_caller_still_holds_is_refused_at_either_end() {
    let material = material();
    let b = material.host(true);
    let (listener, here) = b.listening(loopback());
    let (client, server) = b.joined(listener, &here);
    let started = result(b.settle(start(
        &b,
        client,
        "localhost",
        &[],
        b"250 injected\r\n",
        5000,
    )));
    assert_eq!(started.expect_err("the caller held plaintext"), "Injected");
    let served = result(b.settle(serve(&b, server, &[], b"MAIL FROM:<m@x>\r\n", 5000)));
    assert_eq!(served.expect_err("the caller held plaintext"), "Injected");
    assert_eq!(b.send("conn", server, b"250 ok\r\n"), 0);
    b.close("client", client);
    b.close("conn", server);
    b.close("listener", listener);
    let refusals: u64 = b
        .host
        .handshakes()
        .reasons
        .iter()
        .filter(|(reason, _)| *reason == ply_host::tls::REASON_INJECTED)
        .map(|(_, n)| *n)
        .sum();
    assert_eq!(refusals, 2, "each refusal is counted under its reason");
}

#[test]
fn a_certificate_no_root_vouches_for_is_untrusted() {
    let material = material();
    let b = material.host(false);
    let (listener, here) = b.listening(loopback());
    let (client, server) = b.joined(listener, &here);
    let (started, served) = upgraded(&b, client, server, "localhost", &[], &[]);
    assert_eq!(started.expect_err("no root vouches for it"), "Untrusted");
    assert!(served.is_err(), "the server's half fails with the client's");
    assert_eq!(
        b.send("client", client, b"AUTH"),
        0,
        "the connection is over"
    );
    b.close("client", client);
    b.close("conn", server);
    b.close("listener", listener);
}

#[test]
fn a_name_the_certificate_does_not_cover_is_untrusted() {
    let material = material();
    let b = material.host(true);
    let (listener, here) = b.listening(loopback());
    let (client, server) = b.joined(listener, &here);
    let (started, _) = upgraded(&b, client, server, "mail.example", &[], &[]);
    assert_eq!(
        started.expect_err("the name is not the certificate's"),
        "Untrusted"
    );
    b.close("client", client);
    b.close("conn", server);
    b.close("listener", listener);
}

#[test]
fn a_name_no_certificate_could_cover_is_refused_before_anything_is_sent() {
    let material = material();
    let b = material.host(true);
    let (listener, here) = b.listening(loopback());
    let (client, server) = b.joined(listener, &here);
    let started = result(b.settle(start(&b, client, "not a name", &[], b"", 5000)));
    assert_eq!(
        started.expect_err("no server is verified as that"),
        "Handshake"
    );
    assert_eq!(
        b.recv("conn", server, 5000),
        Some(Vec::new()),
        "the refusal ended the connection"
    );
    b.close("client", client);
    b.close("conn", server);
    b.close("listener", listener);
}

#[test]
fn a_peer_that_never_handshakes_is_a_deadline() {
    let material = material();
    let b = material.host(true);
    let (listener, here) = b.listening(loopback());
    let (client, server) = b.joined(listener, &here);
    let began = std::time::Instant::now();
    let started = result(b.settle(start(&b, client, "localhost", &[], b"", 200)));
    assert_eq!(started.expect_err("the server never answers"), "TimedOut");
    assert!(
        began.elapsed() < Duration::from_secs(4),
        "the deadline is the whole handshake's, not each read's"
    );
    assert_eq!(
        b.send("client", client, b"AUTH"),
        0,
        "a timed-out upgrade is over"
    );
    b.close("client", client);
    b.close("conn", server);
    b.close("listener", listener);
}

#[test]
fn an_upgrade_is_refused_under_a_read_still_waiting_and_twice() {
    let material = material();
    let b = material.host(true);
    let (listener, here) = b.listening(loopback());
    let (client, server) = b.joined(listener, &here);

    let waiting = b
        .call(
            NET,
            "recv",
            Some("client"),
            vec![Value::Int(client), Value::Int(64), Value::Int(5000)],
        )
        .expect("a read");
    let under = b
        .call(
            NET,
            "start_tls",
            Some("client"),
            vec![
                Value::Int(client),
                text("localhost"),
                strings(&[]),
                Value::bytes(b""),
                Value::Int(5000),
            ],
        )
        .err()
        .expect("a read still holds the plaintext");
    assert_eq!(under.code, codes::RUNTIME_ERROR);
    assert_eq!(b.send("conn", server, b"x"), 1);
    assert_eq!(maybe(b.settle(waiting)).map(bytes), Some(b"x".to_vec()));

    let (started, served) = upgraded(&b, client, server, "localhost", &[], &[]);
    assert!(started.is_ok() && served.is_ok());
    let twice = b
        .call(
            NET,
            "start_tls",
            Some("client"),
            vec![
                Value::Int(client),
                text("localhost"),
                strings(&[]),
                Value::bytes(b""),
                Value::Int(5000),
            ],
        )
        .err()
        .expect("the connection is already TLS");
    assert_eq!(twice.code, codes::RUNTIME_ERROR);
    let unknown = b
        .call(
            NET,
            "serve_tls",
            Some("conn"),
            vec![
                Value::Int(server),
                text("absent"),
                strings(&[]),
                Value::bytes(b""),
                Value::Int(5000),
            ],
        )
        .err()
        .expect("the run holds no such credential");
    assert_eq!(unknown.code, codes::TLS_CREDENTIAL_UNKNOWN);
    b.close("client", client);
    b.close("conn", server);
    b.close("listener", listener);
}

/// A datagram socket at loopback, on a port of the kernel's choosing, and where it is.
fn datagram_socket(b: &Bound, label: &str) -> (i64, Endpoint) {
    let socket = int(ok(b.udp(
        "bind",
        label,
        vec![at(loopback(), 0).value(), Value::list(Vec::new())],
    )));
    let here = maybe(b.udp("local_address", label, vec![Value::Int(socket)]))
        .map(|v| Endpoint::read(&v, Span::DUMMY).expect("a socket address"))
        .expect("a bound socket has an address");
    (socket, here)
}

/// What a `std.udp.Datagram` holds: who sent it, its bytes, and whether it was cut.
fn datagram(v: Value) -> (Endpoint, Vec<u8>, bool) {
    let Value::Record(fields) = &v else {
        panic!("not a `Datagram`: {v:?}");
    };
    (
        Endpoint::read(fields.named("from").expect("a sender"), Span::DUMMY)
            .expect("a socket address"),
        bytes(fields.named("payload").expect("a payload").clone()),
        fields
            .named("truncated")
            .expect("a flag")
            .as_bool(Span::DUMMY, "a flag")
            .expect("a Bool"),
    )
}

fn receive(b: &Bound, label: &str, socket: i64, max: i64, ms: i64) -> Result<Value, String> {
    result(b.udp(
        "recv_from",
        label,
        vec![Value::Int(socket), Value::Int(max), Value::Int(ms)],
    ))
}

#[test]
fn a_datagram_crosses_loopback_and_names_its_sender() {
    let b = bound(Host::new());
    let (server, there) = datagram_socket(&b, "socket");
    let (client, here) = datagram_socket(&b, "other");
    let sent = int(ok(b.udp(
        "send_to",
        "other",
        vec![
            Value::Int(client),
            there.value(),
            Value::bytes(b"ping"),
            Value::Int(5000),
        ],
    )));
    assert_eq!(sent, 4);
    let (from, payload, truncated) =
        datagram(receive(&b, "socket", server, 512, 5000).expect("a datagram"));
    assert_eq!(from, here);
    assert_eq!(payload, b"ping");
    assert!(!truncated);
    assert_eq!(
        receive(&b, "socket", server, 512, 50).expect_err("nothing more was sent"),
        "TimedOut"
    );
    b.udp("close", "socket", vec![Value::Int(server)]);
    b.udp("close", "other", vec![Value::Int(client)]);
}

#[test]
fn a_datagram_longer_than_its_reader_holds_is_cut_and_says_so() {
    let b = bound(Host::new());
    let (server, there) = datagram_socket(&b, "socket");
    let (client, _) = datagram_socket(&b, "other");
    for payload in [&b"0123456789"[..], &b"0123"[..]] {
        ok(b.udp(
            "send_to",
            "other",
            vec![
                Value::Int(client),
                there.value(),
                Value::bytes(payload),
                Value::Int(5000),
            ],
        ));
    }
    let (_, cut, truncated) = datagram(receive(&b, "socket", server, 4, 5000).expect("a datagram"));
    assert_eq!(cut, b"0123");
    assert!(
        truncated,
        "six bytes of it are gone, and the answer says so"
    );
    let (_, whole, truncated) =
        datagram(receive(&b, "socket", server, 4, 5000).expect("a datagram"));
    assert_eq!(whole, b"0123");
    assert!(!truncated, "a datagram that fits exactly is whole");
}

#[test]
fn a_connected_socket_sends_without_naming_its_far_end() {
    let b = bound(Host::new());
    let (server, there) = datagram_socket(&b, "socket");
    let (client, here) = datagram_socket(&b, "other");
    let unconnected = b
        .perform(
            UDP,
            "send",
            Some("other"),
            vec![Value::Int(client), Value::bytes(b"x"), Value::Int(5000)],
        )
        .expect_err("the socket has no far end yet");
    assert_eq!(unconnected.code, codes::RUNTIME_ERROR);
    ok(b.udp("connect", "other", vec![Value::Int(client), there.value()]));
    let sent = int(ok(b.udp(
        "send",
        "other",
        vec![Value::Int(client), Value::bytes(b"hello"), Value::Int(5000)],
    )));
    assert_eq!(sent, 5);
    let (from, payload, _) =
        datagram(receive(&b, "socket", server, 512, 5000).expect("a datagram"));
    assert_eq!((from, payload), (here, b"hello".to_vec()));
}

#[test]
fn a_run_binds_a_datagram_socket_at_loopback_or_nowhere() {
    let b = bound(Host::new());
    let unspecified = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
    let fixed = b.udp(
        "bind",
        "socket",
        vec![at(unspecified, 5353).value(), Value::list(Vec::new())],
    );
    assert_eq!(refused(fixed), "PermissionDenied");
    let asking = int(ok(b.udp(
        "bind",
        "socket",
        vec![at(unspecified, 0).value(), Value::list(Vec::new())],
    )));
    let (_, here) = datagram_socket(&b, "other");
    let taken = b.udp("bind", "other", vec![here.value(), Value::list(Vec::new())]);
    assert_eq!(refused(taken), "AddressInUse");
    b.udp("close", "socket", vec![Value::Int(asking)]);
}

#[test]
fn a_datagram_sockets_options_are_set_and_read_back() {
    let b = bound(Host::new());
    let (socket, _) = datagram_socket(&b, "socket");
    let options = |b: &Bound| {
        SocketOption::list(
            &b.udp("options", "socket", vec![Value::Int(socket)]),
            Span::DUMMY,
        )
        .expect("a list of options")
    };
    assert!(options(&b).contains(&SocketOption::Broadcast(false)));
    ok(b.udp(
        "set_option",
        "socket",
        vec![Value::Int(socket), SocketOption::Broadcast(true).value()],
    ));
    let now = options(&b);
    assert!(now.contains(&SocketOption::Broadcast(true)), "{now:?}");
    assert!(
        !now.iter().any(|o| matches!(o, SocketOption::NoDelay(_))),
        "a datagram socket has no stream's options: {now:?}"
    );

    // Whether a machine routes multicast is its own affair, so a join may be refused; a group
    // that was joined is one that can be left, once.
    let group = IpAddr::V4(Ipv4Addr::new(239, 255, 71, 3));
    let join = SocketOption::JoinGroup(group, None).value();
    let leave = SocketOption::LeaveGroup(group, None).value();
    if result(b.udp("set_option", "socket", vec![Value::Int(socket), join])).is_ok() {
        ok(b.udp(
            "set_option",
            "socket",
            vec![Value::Int(socket), leave.clone()],
        ));
    }
    assert!(
        result(b.udp("set_option", "socket", vec![Value::Int(socket), leave])).is_err(),
        "a group not joined cannot be left"
    );
}

#[test]
fn a_stream_operation_refuses_a_datagram_socket_and_a_datagram_one_a_stream() {
    let b = bound(Host::new());
    let (listener, _) = b.listening(loopback());
    let as_datagram = b
        .perform(
            UDP,
            "recv_from",
            Some("socket"),
            vec![Value::Int(listener), Value::Int(64), Value::Int(100)],
        )
        .expect_err("a listener is no datagram socket");
    assert_eq!(as_datagram.code, codes::RUNTIME_ERROR);
    b.close("listener", listener);
}

fn lookup(b: &Bound, name: &str) -> Result<Vec<IpAddr>, String> {
    let answer = b
        .perform(DNS, "lookup", None, vec![text(name), Value::Int(10_000)])
        .expect("a lookup is served");
    result(answer).map(|found| {
        found
            .as_list(Span::DUMMY, "the addresses")
            .expect("a List")
            .iter()
            .map(|one| {
                let Value::Record(fields) = one else {
                    panic!("not a `Found`: {one:?}");
                };
                assert!(
                    maybe(fields.named("ttl").expect("a lifetime").clone()).is_none(),
                    "the system's resolver says nothing of lifetimes"
                );
                ply_host::tcp::wire::ip_of(
                    fields.named("address").expect("an address"),
                    Span::DUMMY,
                )
                .expect("an address")
            })
            .collect()
    })
}

#[test]
fn an_address_written_as_text_answers_itself() {
    let b = bound(Host::new());
    assert_eq!(
        lookup(&b, "192.0.2.7"),
        Ok(vec!["192.0.2.7".parse().unwrap()])
    );
    assert_eq!(
        lookup(&b, "2001:db8::7"),
        Ok(vec!["2001:db8::7".parse().unwrap()])
    );
}

#[test]
fn localhost_resolves_to_loopback_and_each_address_once() {
    let b = bound(Host::new());
    let found = lookup(&b, "localhost").expect("every machine names itself");
    assert!(!found.is_empty());
    assert!(found.iter().all(IpAddr::is_loopback), "{found:?}");
    let mut once = found.clone();
    once.sort();
    once.dedup();
    assert_eq!(once.len(), found.len(), "{found:?}");
}

#[test]
fn a_name_that_cannot_exist_is_a_failure_and_not_an_empty_answer() {
    let b = bound(Host::new());
    let failed = lookup(&b, "no-such-host.invalid").expect_err("`.invalid` never resolves");
    assert!(
        ["NoSuchName", "NoData", "ServerFailure", "TimedOut"].contains(&failed.as_str()),
        "{failed}"
    );
}

#[test]
fn an_address_answers_to_a_name_or_says_it_has_none() {
    let b = bound(Host::new());
    let answer = b
        .perform(
            DNS,
            "reverse",
            None,
            vec![
                ply_host::tcp::wire::ip_value(loopback()),
                Value::Int(10_000),
            ],
        )
        .expect("a reverse lookup is served");
    match result(answer) {
        Ok(names) => assert!(
            !names
                .as_list(Span::DUMMY, "names")
                .expect("a List")
                .is_empty()
        ),
        Err(why) => assert!(
            ["NoSuchName", "ServerFailure", "TimedOut"].contains(&why.as_str()),
            "{why}"
        ),
    }
}

#[test]
fn the_servers_of_a_resolver_configuration_are_read_in_its_order() {
    let found = ply_host::dns::servers(
        "# a comment\nsearch example.org\nnameserver 192.0.2.53\nnameserver fe80::1%en0\nnameserver not-an-address\noptions ndots:1\nnameserver 2001:db8::53\n",
    );
    assert_eq!(
        found,
        [
            Endpoint {
                ip: "192.0.2.53".parse().unwrap(),
                port: 53,
                zone: None
            },
            Endpoint {
                ip: "fe80::1".parse().unwrap(),
                port: 53,
                zone: Some("en0".to_string())
            },
            Endpoint {
                ip: "2001:db8::53".parse().unwrap(),
                port: 53,
                zone: None
            },
        ]
    );
    let b = bound(Host::new());
    let listed = b
        .perform(DNS, "servers", None, Vec::new())
        .expect("the servers are served");
    for server in listed
        .as_list(Span::DUMMY, "servers")
        .expect("a List")
        .iter()
    {
        assert_eq!(
            Endpoint::read(server, Span::DUMMY)
                .expect("a socket address")
                .port,
            53
        );
    }
}

#[test]
fn the_script_keeps_the_sockets_rules_without_a_socket() {
    use ply_eval::Resource;
    use ply_host::tcp::{Net, SimNet, Upgrade};
    let net = SimNet::with_credentials(
        vec![vec![b"220 go ahead\r\n".to_vec()], vec![b"S".to_vec()]],
        vec!["mail"],
    );
    let label = |name: &str| Resource::Named(Symbol::new(name));
    let value = |answer: Result<HostAnswer, Diagnostic>| match answer.expect("the script serves it")
    {
        HostAnswer::Value(v) => v,
        HostAnswer::Pending(_) => panic!("the script never waits"),
    };
    let upgrade = |alpn: &[&str], unread: usize| Upgrade {
        alpn: alpn.iter().map(|p| p.to_string()).collect(),
        unread,
        timeout: Duration::from_secs(1),
    };

    let listener = int(value(net.listen(&label("listener"), 8080, Span::DUMMY)));
    let accepted = int(value(net.accept(&label("listener"), listener, Span::DUMMY)));
    let from = maybe(value(net.peer_address(
        &label("conn"),
        accepted,
        Span::DUMMY,
    )))
    .map(|v| Endpoint::read(&v, Span::DUMMY).expect("a socket address"))
    .expect("an accepted connection has a far end");
    assert_eq!(from.ip, loopback());
    let held = value(net.serve_tls(
        &label("conn"),
        accepted,
        "mail",
        upgrade(&[], 3),
        Span::DUMMY,
    ));
    assert_eq!(refused(held), "Injected");
    let after = value(net.send(
        &label("conn"),
        accepted,
        b"250 ok\r\n",
        Duration::from_secs(1),
        Span::DUMMY,
    ));
    assert_eq!(int(some(after)), 0, "a refused upgrade ends the connection");

    let to = at("192.0.2.5".parse().unwrap(), 5432);
    let dialled = int(ok(value(net.connect_to(
        &label("client"),
        to.clone(),
        vec![SocketOption::NoDelay(false)],
        Duration::from_secs(1),
        Span::DUMMY,
    ))));
    assert_eq!(
        maybe(value(net.peer_address(
            &label("client"),
            dialled,
            Span::DUMMY
        )))
        .map(|v| Endpoint::read(&v, Span::DUMMY).expect("a socket address")),
        Some(to.clone()),
        "the far end is where the connection was sent"
    );
    let options = SocketOption::list(
        &value(net.options(&label("client"), dialled, Span::DUMMY)),
        Span::DUMMY,
    )
    .expect("a list of options");
    assert_eq!(options, [SocketOption::NoDelay(false)]);
    let secured = ok(value(net.start_tls(
        &label("client"),
        dialled,
        "db.example",
        upgrade(&["postgresql"], 0),
        Span::DUMMY,
    )));
    assert_eq!(protocol(secured), Some("postgresql".to_string()));
    let twice = net
        .start_tls(
            &label("client"),
            dialled,
            "db.example",
            upgrade(&[], 0),
            Span::DUMMY,
        )
        .err()
        .expect("the connection is already TLS");
    assert_eq!(twice.code, codes::RUNTIME_ERROR);
    let nobody = value(net.connect_to(
        &label("client"),
        to,
        Vec::new(),
        Duration::from_secs(1),
        Span::DUMMY,
    ));
    assert_eq!(refused(nobody), "Refused", "no connection is left scripted");
}

/// What the program sees is what `std.ip` and `std.net` declare: a shape the handler builds by
/// name is one a rename would otherwise break without a compile error.
#[test]
fn the_shapes_the_handlers_build_are_the_ones_the_library_declares() {
    let front = crate::support::answered::checked("app", DRIVER);
    let declared = |name: &str| {
        front
            .emitter_ctors
            .iter()
            .find(|(constructor, _)| constructor.as_str() == name)
            .map(|(_, fields)| *fields)
    };
    for (constructor, fields) in [
        ("std.ip.SocketAddress", 3),
        ("std.ip.V4", 1),
        ("std.ip.V6", 1),
        ("std.ip.Ipv4", 1),
        ("std.ip.Ipv6", 1),
        ("std.net.Refused", 0),
        ("std.net.Unreachable", 0),
        ("std.net.TimedOut", 0),
        ("std.net.Reset", 0),
        ("std.net.NameNotFound", 0),
        ("std.net.AddressInUse", 0),
        ("std.net.PermissionDenied", 0),
        ("std.net.Injected", 0),
        ("std.net.Untrusted", 0),
        ("std.net.Handshake", 1),
        ("std.net.Other", 2),
        ("std.net.NoDelay", 1),
        ("std.net.KeepAlive", 1),
        ("std.net.ReuseAddress", 1),
        ("std.net.ReusePort", 1),
        ("std.net.SendBuffer", 1),
        ("std.net.ReceiveBuffer", 1),
        ("std.net.Linger", 1),
        ("std.net.Broadcast", 1),
        ("std.net.MulticastLoop", 1),
        ("std.net.JoinGroup", 2),
        ("std.net.LeaveGroup", 2),
        ("std.dns.NoSuchName", 0),
        ("std.dns.NoData", 0),
        ("std.dns.ServerFailure", 0),
        ("std.dns.TimedOut", 0),
        ("std.dns.Other", 2),
    ] {
        assert_eq!(
            declared(constructor),
            Some(fields),
            "a handler builds `{constructor}` with {fields} field(s)"
        );
    }
    let owner = PeerCredentials {
        user: 501,
        group: 20,
        process: None,
    }
    .value();
    let Value::Record(fields) = &owner else {
        panic!("not a record: {owner:?}");
    };
    assert_eq!(
        fields.keys().map(Symbol::as_str).collect::<Vec<_>>(),
        ["group", "process", "user"]
    );
}
