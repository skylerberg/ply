use ply_eval::{
    Bound, CheckOutput, Diagnostic, EffectAtom, HostAnswer, HostBinding, HostRequest, HostRuntime,
    Linearity, Mode, Pending, Resource, Span, Symbol, Value, codes,
};
use ply_host::pool::Pooled;
use ply_host::tcp::*;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::sync::Arc;

const REQUEST: &[u8] = b"GET / HTTP/1.1\r\nhost: localhost\r\n\r\n";
const RESPONSE: &[u8] = b"HTTP/1.1 200 OK\r\ncontent-length: 3\r\n\r\nply";

/// Registrations resolve against the atoms the program performs; the declaration names no label.
const DRIVER: &str = r#"
fn every_op(port: Int, payload: Bytes) -> Int / {net.write[listener], net.write[conn], net.local_port[listener], net.local_port[conn]} = {
  let l = net.listen[listener](port);
  let _ = net.local_port[listener](l);
  let c = net.accept[listener](l);
  let _ = net.local_port[conn](c);
  let got = net.recv[conn](c, 16, 5000);
  let sent = net.send[conn](c, payload, 5000);
  net.close[conn](c);
  net.close[listener](l);
  bytes_len(bytes_or_empty(got)) + int_or_zero(sent)
}

fn client(host: String, port: Int) -> Int / {net.write[conn]} =
  match net.connect[conn](host, port, 5000) { Some(c) -> c, None -> 0 }

fn bytes_or_empty(answer: Option<Bytes>) -> Bytes =
  match answer { Some(bs) -> bs, None -> b"" }

fn int_or_zero(answer: Option<Int>) -> Int =
  match answer { Some(n) -> n, None -> 0 }
"#;

/// The driver over the shipped `std.net`: only its declaration binds the shipped handlers, and a
/// program reaches it by importing it.
fn fixture() -> String {
    format!("import std.net (net)\n{DRIVER}")
}

fn check(source: &str) -> CheckOutput {
    crate::support::answered::checked("app", source).check
}

/// The shipped declaration as the binder reads it, which a test may change as no source can.
fn declared(check: &mut CheckOutput) -> &mut ply_eval::EffectInfo {
    check
        .effects
        .get_mut(&Symbol::new(EFFECT))
        .expect("the program declares `std.net.net`")
}
fn bind(net: Arc<dyn Net>) -> HostBinding {
    registry(net)
        .bind(&check(&fixture()))
        .expect("the declaration and the registration agree")
}

fn atom(resource: &str) -> EffectAtom {
    EffectAtom::new(EFFECT, Resource::Named(Symbol::new(resource)), Mode::Write)
}

fn perform(
    binding: &HostBinding,
    rt: &dyn HostRuntime,
    op: Op,
    resource: &str,
    args: Vec<Value>,
) -> Result<Value, Diagnostic> {
    let bound: Bound<'_> = binding
        .resolve(
            &Symbol::new(EFFECT),
            &Symbol::new(op.name()),
            Some(&Symbol::new(resource)),
        )
        .expect("the registry serves this triple");
    let request = HostRequest {
        machine: ply_eval::host::MachineId(1),
        atom: bound.atom.clone(),
        op: bound.op,
        args: &args,
        span: Span::DUMMY,
        task: None,
        declared: None,
    };
    match bound.handler.call(rt, &request)? {
        HostAnswer::Value(v) => Ok(v),
        HostAnswer::Pending(pending) => rt.block_on(pending),
    }
}

fn int(v: Value) -> i64 {
    inside(v)
        .as_int(Span::DUMMY, "a test expectation")
        .expect("an Int answer")
}

fn bytes(v: Value) -> Vec<u8> {
    inside(v)
        .as_bytes(Span::DUMMY, "a test expectation")
        .expect("a Bytes answer")
        .to_vec()
}

/// `recv` and `send` answer an `Option`, where `None` is a deadline.
fn inside(v: Value) -> Value {
    match &v {
        Value::Ctor { name, args } if name.as_str() == "Some" => args[0].clone(),
        Value::Ctor { name, .. } if name.as_str() == "None" => {
            panic!("the operation answered `None`: its deadline expired")
        }
        _ => v,
    }
}

#[test]
fn the_declaration_binds_and_names_exactly_the_operations_the_program_performs() {
    let binding = bind(Arc::new(TcpHost::new()));
    let mut atoms: Vec<String> = binding
        .footprint()
        .atoms()
        .map(EffectAtom::to_string)
        .collect();
    let mut rows: Vec<String> = binding
        .listing()
        .rows
        .iter()
        .map(|r| r.to_string())
        .collect();
    atoms.sort();
    rows.sort();
    assert_eq!(atoms, rows);
    for label in ["conn", "listener"] {
        assert!(
            binding.serves(&atom(label)),
            "the written mode atom reaches every row under `[{label}]`"
        );
    }
    assert!(binding.serves(&EffectAtom::operation(
        EFFECT,
        Resource::Named(Symbol::new("conn")),
        Mode::Write,
        "send"
    )));
}

#[test]
fn the_listing_is_one_row_per_triple_and_never_a_star() {
    let binding = bind(Arc::new(TcpHost::new()));
    let rows: Vec<String> = binding
        .listing()
        .rows
        .iter()
        .map(|r| {
            assert_eq!(
                r.atom.to_string(),
                r.to_string(),
                "a row's atom is its operation"
            );
            format!("{r} {}", r.path)
        })
        .collect();
    assert_eq!(
        rows,
        [
            "std.net.net.accept[conn] ply_host::tcp::accept",
            "std.net.net.accept[listener] ply_host::tcp::accept",
            "std.net.net.close[conn] ply_host::tcp::close",
            "std.net.net.close[listener] ply_host::tcp::close",
            "std.net.net.close_write[conn] ply_host::tcp::close_write",
            "std.net.net.close_write[listener] ply_host::tcp::close_write",
            "std.net.net.connect[conn] ply_host::tcp::connect",
            "std.net.net.connect[listener] ply_host::tcp::connect",
            "std.net.net.connect_tls[conn] ply_host::tls::connect",
            "std.net.net.connect_tls[listener] ply_host::tls::connect",
            "std.net.net.connect_to[conn] ply_host::tcp::connect_to",
            "std.net.net.connect_to[listener] ply_host::tcp::connect_to",
            "std.net.net.connect_unix[conn] ply_host::tcp::connect_unix",
            "std.net.net.connect_unix[listener] ply_host::tcp::connect_unix",
            "std.net.net.handshake[conn] ply_host::tls::handshake",
            "std.net.net.handshake[listener] ply_host::tls::handshake",
            "std.net.net.listen[conn] ply_host::tcp::listen",
            "std.net.net.listen[listener] ply_host::tcp::listen",
            "std.net.net.listen_on[conn] ply_host::tcp::listen_on",
            "std.net.net.listen_on[listener] ply_host::tcp::listen_on",
            "std.net.net.listen_tls[conn] ply_host::tls::listen",
            "std.net.net.listen_tls[listener] ply_host::tls::listen",
            "std.net.net.listen_unix[conn] ply_host::tcp::listen_unix",
            "std.net.net.listen_unix[listener] ply_host::tcp::listen_unix",
            "std.net.net.local_port[conn] ply_host::tcp::local_port",
            "std.net.net.local_port[listener] ply_host::tcp::local_port",
            "std.net.net.recv[conn] ply_host::tcp::recv",
            "std.net.net.recv[listener] ply_host::tcp::recv",
            "std.net.net.send[conn] ply_host::tcp::send",
            "std.net.net.send[listener] ply_host::tcp::send",
            "std.net.net.send_secret[conn] ply_host::tcp::send_secret",
            "std.net.net.send_secret[listener] ply_host::tcp::send_secret",
            "std.net.net.serve_tls[conn] ply_host::tls::serve",
            "std.net.net.serve_tls[listener] ply_host::tls::serve",
            "std.net.net.set_option[conn] ply_host::tcp::set_option",
            "std.net.net.set_option[listener] ply_host::tcp::set_option",
            "std.net.net.start_tls[conn] ply_host::tls::start",
            "std.net.net.start_tls[listener] ply_host::tls::start",
        ]
    );
}

#[test]
fn two_labelled_sockets_do_not_conflict_and_one_label_does() {
    assert!(!atom("conn").conflicts_with(&atom("listener")));
    assert!(atom("conn").conflicts_with(&atom("conn")));
}

#[test]
fn the_twin_declares_the_same_signature_and_differs_only_where_it_must() {
    let socket = bind(Arc::new(TcpHost::new()));
    let script = bind(Arc::new(SimNet::new(Vec::new())));
    let rows = socket.listing().rows.iter().zip(&script.listing().rows);
    for (a, b) in rows {
        assert_eq!(
            (&a.effect, &a.op, &a.resource),
            (&b.effect, &b.op, &b.resource)
        );
        assert_eq!(a.atom, b.atom);
        assert_eq!(a.deterministic, b.deterministic);
        assert_eq!(a.linearity, b.linearity);
        assert_eq!(a.declared_nondet, b.declared_nondet);
        assert_ne!(a.path, b.path, "the listing must say which one is bound");
    }
    assert!(socket.listing().rows.iter().any(|r| r.blocking));
    assert!(script.listing().rows.iter().all(|r| !r.blocking));
}

/// Reading what a socket is changes nothing; every other operation opens, moves, tunes or closes.
#[test]
fn only_reading_what_a_socket_is_is_repeatable() {
    let reads = [
        Op::LocalPort,
        Op::LocalAddress,
        Op::PeerAddress,
        Op::PeerCredentials,
        Op::PeerCertificate,
        Op::Protocol,
        Op::Options,
    ];
    for net in [
        Arc::new(TcpHost::new()) as Arc<dyn Net>,
        Arc::new(SimNet::new(Vec::new())),
    ] {
        for op in Op::ALL {
            let expected = if reads.contains(&op) {
                Linearity::Repeatable
            } else {
                Linearity::AtMostOnce
            };
            assert_eq!(op.declaration(net.as_ref()).linearity, expected, "{op:?}");
        }
    }
}

#[test]
fn a_declaration_without_nondet_refuses_the_handler() {
    let mut weakened = check(&fixture());
    declared(&mut weakened).nondet = false;
    let diagnostics = registry(Arc::new(TcpHost::new()))
        .bind(&weakened)
        .expect_err("a socket cannot sit behind an effect that is not `nondet`");
    assert!(
        diagnostics
            .iter()
            .all(|d| d.code == codes::HOST_DETERMINISM_MISMATCH),
        "{:?}",
        diagnostics.iter().map(|d| d.code).collect::<Vec<_>>()
    );
}

/// A declaration that lacks an operation the host registers is a program checked before the
/// operation was added, or after it was renamed: either way it performs none of it, and the rest
/// binds. That the registry names only what this tree declares is `unit::registry`'s to hold.
#[test]
fn an_operation_the_declaration_lacks_binds_nothing_and_the_rest_binds() {
    let mut renamed = check(&fixture());
    let net = declared(&mut renamed);
    let recv = net
        .ops
        .shift_remove(&Symbol::new("recv"))
        .expect("`net.recv` is declared");
    net.ops.insert(Symbol::new("read_bytes"), recv);
    let binding = registry(Arc::new(TcpHost::new()))
        .bind(&renamed)
        .expect("what the declaration still holds binds");
    let served: Vec<&str> = binding
        .listing()
        .rows
        .iter()
        .map(|r| r.op.as_str())
        .collect();
    assert!(served.contains(&"send"), "{served:?}");
    assert!(!served.contains(&"recv"), "{served:?}");
}

#[test]
fn the_script_serves_listen_accept_recv_send_close() {
    let net = Arc::new(SimNet::new(vec![vec![REQUEST.to_vec()]]));
    let binding = bind(net.clone());
    let served = serve_once(&binding, net.as_ref());
    assert_eq!(served.request, REQUEST);
    assert_eq!(served.sent, RESPONSE.len() as i64);
    assert_eq!(net.sent(served.conn), RESPONSE);
}

#[test]
fn a_partial_read_leaves_the_rest_for_the_next_one() {
    let net = Arc::new(SimNet::new(vec![vec![b"abcdefghijk".to_vec()]]));
    let binding = bind(net.clone());
    let (_, conn) = open(&binding, net.as_ref());

    let mut chunks = Vec::new();
    loop {
        let chunk = bytes(
            perform(
                &binding,
                net.as_ref(),
                Op::Recv,
                "conn",
                vec![Value::Int(conn), Value::Int(3), Value::Int(5000)],
            )
            .expect("a read of an open connection"),
        );
        if chunk.is_empty() {
            break;
        }
        assert!(chunk.len() <= 3, "a read never answers more than `max`");
        chunks.push(chunk);
    }
    assert_eq!(
        chunks,
        [
            b"abc".to_vec(),
            b"def".to_vec(),
            b"ghi".to_vec(),
            b"jk".to_vec()
        ]
    );
}

#[test]
fn a_second_read_takes_the_next_bytes_and_never_the_same_ones() {
    let net = Arc::new(SimNet::new(vec![vec![b"one".to_vec(), b"two".to_vec()]]));
    let binding = bind(net.clone());
    let (_, conn) = open(&binding, net.as_ref());
    let read = |n: i64| {
        bytes(
            perform(
                &binding,
                net.as_ref(),
                Op::Recv,
                "conn",
                vec![Value::Int(conn), Value::Int(n), Value::Int(5000)],
            )
            .expect("a read of an open connection"),
        )
    };
    assert_eq!(read(3), b"one");
    assert_eq!(read(3), b"two");
    assert_eq!(read(3), b"");
}

#[test]
fn a_peer_that_stopped_sending_reads_empty_rather_than_failing() {
    let net = Arc::new(SimNet::new(vec![vec![b"hi".to_vec()]]));
    let binding = bind(net.clone());
    let (_, conn) = open(&binding, net.as_ref());
    let read = || {
        bytes(
            perform(
                &binding,
                net.as_ref(),
                Op::Recv,
                "conn",
                vec![Value::Int(conn), Value::Int(64), Value::Int(5000)],
            )
            .expect("a read of an open connection"),
        )
    };
    assert_eq!(read(), b"hi");
    assert!(read().is_empty());
    assert!(read().is_empty(), "end of stream is not a one-shot answer");
}

#[test]
fn a_closed_handle_is_a_diagnostic_rather_than_another_socket() {
    let net = Arc::new(SimNet::new(vec![vec![b"hi".to_vec()]]));
    let binding = bind(net.clone());
    let (_, conn) = open(&binding, net.as_ref());
    perform(
        &binding,
        net.as_ref(),
        Op::Close,
        "conn",
        vec![Value::Int(conn)],
    )
    .expect("a close of an open connection");

    let again = perform(
        &binding,
        net.as_ref(),
        Op::Recv,
        "conn",
        vec![Value::Int(conn), Value::Int(64), Value::Int(5000)],
    )
    .expect_err("the handle is gone");
    assert_eq!(again.code, codes::RUNTIME_ERROR);
}

/// A read bound arrives from the program, so it is an untrusted number.
#[test]
fn an_absurd_read_bound_is_capped_rather_than_wrapped() {
    let net = Arc::new(SimNet::new(vec![vec![b"hi".to_vec()]]));
    let binding = bind(net.clone());
    let (_, conn) = open(&binding, net.as_ref());
    let answer = bytes(
        perform(
            &binding,
            net.as_ref(),
            Op::Recv,
            "conn",
            vec![Value::Int(conn), Value::Int(i64::MAX), Value::Int(5000)],
        )
        .expect("a read of an open connection"),
    );
    assert_eq!(answer, b"hi");
}

#[test]
fn a_port_outside_the_range_is_refused() {
    let net = Arc::new(SimNet::new(Vec::new()));
    let binding = bind(net.clone());
    let refused = perform(
        &binding,
        net.as_ref(),
        Op::Listen,
        "listener",
        vec![Value::Int(70_000)],
    )
    .expect_err("70000 is not a TCP port");
    assert_eq!(refused.code, codes::RUNTIME_ERROR);
}

#[test]
fn a_credential_goes_out_whole_between_its_frame_and_only_in_an_encoding_it_names() {
    let net = Arc::new(SimNet::new(vec![vec![b"hi".to_vec()]]));
    let binding = bind(net.clone());
    let (_, conn) = open(&binding, net.as_ref());
    let send = |encoding: &str| {
        perform(
            &binding,
            net.as_ref(),
            Op::SendSecret,
            "conn",
            vec![
                Value::Int(conn),
                Value::bytes(b"AUTH PLAIN "),
                Value::secret_bytes(b"\0ada\0hunter2"),
                Value::str(encoding),
                Value::bytes(b"\r\n"),
                Value::Int(5000),
            ],
        )
    };
    assert_eq!(
        send("base64").expect("a credential is sent"),
        Value::Bool(true)
    );
    assert_eq!(net.sent(conn), b"AUTH PLAIN AGFkYQBodW50ZXIy\r\n");
    assert_eq!(send("raw").expect("as it is"), Value::Bool(true));
    assert!(net.sent(conn).ends_with(b"AUTH PLAIN \0ada\0hunter2\r\n"));
    let refused = send("rot13").expect_err("no such encoding");
    assert_eq!(refused.code, codes::RUNTIME_ERROR);
    assert!(!format!("{refused:#?}").contains("hunter2"), "{refused:#?}");
    let sent = net.sent(conn);
    perform(
        &binding,
        net.as_ref(),
        Op::CloseWrite,
        "conn",
        vec![Value::Int(conn)],
    )
    .expect("this end stops sending");
    assert_eq!(send("base64").expect("answered"), Value::Bool(false));
    assert_eq!(net.sent(conn), sent);
}

#[test]
fn a_listener_and_a_connection_are_not_interchangeable() {
    let net = Arc::new(SimNet::new(vec![vec![b"hi".to_vec()]]));
    let binding = bind(net.clone());
    let (listener, conn) = open(&binding, net.as_ref());

    let wrong_way = perform(
        &binding,
        net.as_ref(),
        Op::Recv,
        "listener",
        vec![Value::Int(listener), Value::Int(8), Value::Int(5000)],
    )
    .expect_err("a listener has no bytes");
    assert_eq!(wrong_way.code, codes::RUNTIME_ERROR);

    let other_way = perform(
        &binding,
        net.as_ref(),
        Op::Accept,
        "conn",
        vec![Value::Int(conn)],
    )
    .expect_err("a connection accepts nothing");
    assert_eq!(other_way.code, codes::RUNTIME_ERROR);
}

#[test]
fn one_socket_under_two_labels_is_refused() {
    let net = Arc::new(SimNet::new(vec![vec![b"hi".to_vec()]]));
    let binding = bind(net.clone());
    let (_, conn) = open(&binding, net.as_ref());
    perform(
        &binding,
        net.as_ref(),
        Op::Recv,
        "conn",
        vec![Value::Int(conn), Value::Int(8), Value::Int(5000)],
    )
    .expect("the first use fixes the label");

    let relabelled = perform(
        &binding,
        net.as_ref(),
        Op::Recv,
        "listener",
        vec![Value::Int(conn), Value::Int(8), Value::Int(5000)],
    )
    .expect_err("this socket is already `[conn]`");
    assert_eq!(relabelled.code, codes::RUNTIME_ERROR);
    assert!(
        relabelled.message.contains("[conn]") && relabelled.message.contains("[listener]"),
        "both labels are named: {}",
        relabelled.message
    );
}

/// Refused, not clamped: an empty answer already means the peer stopped sending.
#[test]
fn a_read_of_no_bytes_is_refused() {
    let net = Arc::new(SimNet::new(vec![vec![b"hi".to_vec()]]));
    let binding = bind(net.clone());
    let (_, conn) = open(&binding, net.as_ref());
    let refused = perform(
        &binding,
        net.as_ref(),
        Op::Recv,
        "conn",
        vec![Value::Int(conn), Value::Int(0), Value::Int(5000)],
    )
    .expect_err("a read wants at least one byte");
    assert_eq!(refused.code, codes::RUNTIME_ERROR);
}

#[test]
fn an_accept_with_nothing_scripted_is_a_diagnostic_rather_than_a_wait() {
    let net = Arc::new(SimNet::new(Vec::new()));
    let binding = bind(net.clone());
    let listener = int(perform(
        &binding,
        net.as_ref(),
        Op::Listen,
        "listener",
        vec![Value::Int(0)],
    )
    .expect("a listen"));
    let refused = perform(
        &binding,
        net.as_ref(),
        Op::Accept,
        "listener",
        vec![Value::Int(listener)],
    )
    .expect_err("nothing is scripted to connect");
    assert_eq!(refused.code, codes::RUNTIME_ERROR);
}

#[test]
fn a_loopback_connection_is_served_end_to_end() {
    let net = Arc::new(TcpHost::new());
    let binding = bind(net.clone());
    let listener = listen(&binding, net.as_ref());
    let peer = speak(net.local_addr(listener).expect("a bound port"));

    let conn = int(perform(
        &binding,
        net.as_ref(),
        Op::Accept,
        "listener",
        vec![Value::Int(listener)],
    )
    .expect("an accept"));
    let request = read_to_end(&binding, net.as_ref(), conn);
    assert_eq!(request, REQUEST);

    let sent = int(perform(
        &binding,
        net.as_ref(),
        Op::Send,
        "conn",
        vec![Value::Int(conn), Value::bytes(RESPONSE), Value::Int(5000)],
    )
    .expect("a send"));
    assert_eq!(sent, RESPONSE.len() as i64);
    close(&binding, net.as_ref(), conn, "conn");
    close(&binding, net.as_ref(), listener, "listener");

    assert_eq!(peer.join().expect("the peer finished"), RESPONSE);
    assert_eq!(
        net.pool().outstanding(),
        0,
        "every blocking operation was reaped"
    );
}

#[test]
fn a_real_partial_read_returns_what_it_can_and_the_rest_next_time() {
    let net = Arc::new(TcpHost::new());
    let binding = bind(net.clone());
    let listener = listen(&binding, net.as_ref());
    let addr = net.local_addr(listener).expect("a bound port");

    let peer = std::thread::spawn(move || {
        let mut stream = TcpStream::connect(addr).expect("the peer connects");
        stream.write_all(b"abcdefghijk").expect("the peer writes");
        // Held open, so the reads end because `max` ran out, not the stream.
        std::thread::sleep(std::time::Duration::from_millis(200));
        stream
    });

    let conn = accept(&binding, net.as_ref(), listener);
    let mut got = Vec::new();
    while got.len() < 11 {
        let chunk = bytes(
            perform(
                &binding,
                net.as_ref(),
                Op::Recv,
                "conn",
                vec![Value::Int(conn), Value::Int(3), Value::Int(5000)],
            )
            .expect("a read"),
        );
        assert!(!chunk.is_empty(), "the peer has not closed");
        assert!(chunk.len() <= 3, "a read never answers more than `max`");
        got.extend_from_slice(&chunk);
    }
    assert_eq!(
        got, b"abcdefghijk",
        "no byte was delivered twice or dropped"
    );

    close(&binding, net.as_ref(), conn, "conn");
    close(&binding, net.as_ref(), listener, "listener");
    drop(peer.join());
}

#[test]
fn a_connection_closed_mid_read_reads_empty_rather_than_failing() {
    let net = Arc::new(TcpHost::new());
    let binding = bind(net.clone());
    let listener = listen(&binding, net.as_ref());
    let addr = net.local_addr(listener).expect("a bound port");

    let peer = std::thread::spawn(move || {
        let mut stream = TcpStream::connect(addr).expect("the peer connects");
        stream.write_all(b"half").expect("the peer writes");
        stream.shutdown(Shutdown::Both).expect("the peer goes away");
    });

    let conn = accept(&binding, net.as_ref(), listener);
    let mut got = Vec::new();
    loop {
        let chunk = bytes(
            perform(
                &binding,
                net.as_ref(),
                Op::Recv,
                "conn",
                vec![Value::Int(conn), Value::Int(64), Value::Int(5000)],
            )
            .expect("a read of a peer that went away is empty, not an error"),
        );
        if chunk.is_empty() {
            break;
        }
        got.extend_from_slice(&chunk);
    }
    assert_eq!(got, b"half");

    close(&binding, net.as_ref(), conn, "conn");
    close(&binding, net.as_ref(), listener, "listener");
    peer.join().expect("the peer finished");
}

#[test]
fn the_socket_and_the_script_answer_the_same_program() {
    let script = Arc::new(SimNet::new(vec![vec![REQUEST.to_vec()]]));
    let simulated = serve_once(&bind(script.clone()), script.as_ref());

    let net = Arc::new(TcpHost::new());
    let binding = bind(net.clone());
    let listener = listen(&binding, net.as_ref());
    let peer = speak(net.local_addr(listener).expect("a bound port"));
    let real = serve_accepted(&binding, net.as_ref(), listener);
    close(&binding, net.as_ref(), listener, "listener");

    assert_eq!(simulated.listener, real.listener);
    assert_eq!(simulated.conn, real.conn);
    assert_eq!(simulated.request, real.request);
    assert_eq!(simulated.sent, real.sent);
    assert_eq!(script.sent(simulated.conn), RESPONSE);
    assert_eq!(peer.join().expect("the peer finished"), RESPONSE);
}

/// What `local_port` answered, where `None` is a listener the drain has closed.
fn local_port(
    binding: &HostBinding,
    rt: &dyn HostRuntime,
    handle: i64,
    resource: &str,
) -> Option<i64> {
    let answer = perform(
        binding,
        rt,
        Op::LocalPort,
        resource,
        vec![Value::Int(handle)],
    )
    .expect("a port is read of an open socket");
    match &answer {
        Value::Ctor { name, args } if name.as_str() == "Some" => {
            Some(args[0].as_int(Span::DUMMY, "a port").expect("an Int"))
        }
        Value::Ctor { name, .. } if name.as_str() == "None" => None,
        other => panic!("not an `Option`: {other:?}"),
    }
}

#[test]
fn a_listener_asked_for_any_port_answers_the_one_it_was_given() {
    let net = Arc::new(TcpHost::new());
    let binding = bind(net.clone());
    let listener = listen(&binding, net.as_ref());
    let given = net.local_addr(listener).expect("a bound port").port();
    assert_ne!(given, 0, "the kernel chose a port");
    assert_eq!(
        local_port(&binding, net.as_ref(), listener, "listener"),
        Some(i64::from(given))
    );

    let peer = speak(SocketAddr::from(([127, 0, 0, 1], given)));
    let conn = accept(&binding, net.as_ref(), listener);
    assert_eq!(
        local_port(&binding, net.as_ref(), conn, "conn"),
        Some(i64::from(given)),
        "an accepted connection's own end is the port it was accepted on"
    );
    assert_eq!(read_to_end(&binding, net.as_ref(), conn), REQUEST);
    close(&binding, net.as_ref(), conn, "conn");
    close(&binding, net.as_ref(), listener, "listener");
    peer.join().expect("the peer finished");

    let closed = perform(
        &binding,
        net.as_ref(),
        Op::LocalPort,
        "listener",
        vec![Value::Int(listener)],
    )
    .expect_err("a closed socket names nothing");
    assert_eq!(closed.code, codes::RUNTIME_ERROR);
}

#[test]
fn a_listener_the_drain_closed_listens_on_no_port() {
    use ply_host::signal::Accepting;
    let net = Arc::new(TcpHost::new());
    let binding = bind(net.clone());
    let listener = listen(&binding, net.as_ref());
    assert!(local_port(&binding, net.as_ref(), listener, "listener").is_some());
    assert_eq!(net.stop_accepting(), 1);
    assert_eq!(
        local_port(&binding, net.as_ref(), listener, "listener"),
        None
    );
}

/// Deterministic, so a program that listens on any port reads the same one in every simulated run.
#[test]
fn the_script_assigns_ports_as_the_kernel_would_and_keeps_the_ones_asked_for() {
    let net = Arc::new(SimNet::new(vec![
        vec![b"in".to_vec()],
        vec![b"out".to_vec()],
    ]));
    let binding = bind(net.clone());
    let any = listen(&binding, net.as_ref());
    assert_eq!(
        local_port(&binding, net.as_ref(), any, "listener"),
        Some(49152)
    );
    let fixed = int(perform(
        &binding,
        net.as_ref(),
        Op::Listen,
        "listener",
        vec![Value::Int(8080)],
    )
    .expect("a listen"));
    assert_eq!(
        local_port(&binding, net.as_ref(), fixed, "listener"),
        Some(8080)
    );
    let inbound = accept(&binding, net.as_ref(), fixed);
    assert_eq!(
        local_port(&binding, net.as_ref(), inbound, "conn"),
        Some(8080)
    );
    let outbound = int(perform(
        &binding,
        net.as_ref(),
        Op::Connect,
        "conn",
        vec![Value::str("localhost"), Value::Int(9000), Value::Int(1000)],
    )
    .expect("a connect"));
    assert_eq!(
        local_port(&binding, net.as_ref(), outbound, "conn"),
        Some(49153)
    );
}

#[test]
fn a_token_the_runtime_did_not_mint_is_loud_rather_than_lost() {
    let net = TcpHost::new();
    let foreign = Pending {
        token: 4096,
        label: "recv",
    };
    assert!(!net.pool().owns(&foreign));
    let polled = net
        .pool()
        .poll(&foreign)
        .expect_err("this token is someone else's");
    assert_eq!(polled.code, codes::INTERNAL_ERROR);
    let blocked = net
        .block_on(foreign)
        .expect_err("this token is someone else's");
    assert_eq!(blocked.code, codes::INTERNAL_ERROR);
}

#[test]
fn waiting_with_nothing_outstanding_is_a_diagnostic_rather_than_a_deadlock() {
    let net = TcpHost::new();
    let parked = net.park().expect_err("nothing would ever wake it");
    assert_eq!(parked.code, codes::INTERNAL_ERROR);
}

/// The twin mints nothing, so it must never be the runtime a token is taken to.
#[test]
fn the_script_refuses_to_answer_for_a_token() {
    let net = SimNet::new(Vec::new());
    let stray = Pending {
        token: 1,
        label: "recv",
    };
    assert_eq!(
        net.watch(&stray).expect_err("not its token").code,
        codes::INTERNAL_ERROR
    );
    assert!(net.resolved().is_empty(), "the twin resolves nothing");
    assert_eq!(
        net.park().expect_err("nothing to wait for").code,
        codes::INTERNAL_ERROR
    );
}

struct Served {
    listener: i64,
    conn: i64,
    request: Vec<u8>,
    sent: i64,
}

fn serve_once(binding: &HostBinding, rt: &dyn HostRuntime) -> Served {
    let listener = listen(binding, rt);
    let served = serve_accepted(binding, rt, listener);
    close(binding, rt, listener, "listener");
    served
}

fn serve_accepted(binding: &HostBinding, rt: &dyn HostRuntime, listener: i64) -> Served {
    let conn = accept(binding, rt, listener);
    let request = read_to_end(binding, rt, conn);
    let sent = int(perform(
        binding,
        rt,
        Op::Send,
        "conn",
        vec![Value::Int(conn), Value::bytes(RESPONSE), Value::Int(5000)],
    )
    .expect("a send"));
    close(binding, rt, conn, "conn");
    Served {
        listener,
        conn,
        request,
        sent,
    }
}

fn listen(binding: &HostBinding, rt: &dyn HostRuntime) -> i64 {
    int(perform(binding, rt, Op::Listen, "listener", vec![Value::Int(0)]).expect("a listen"))
}

fn accept(binding: &HostBinding, rt: &dyn HostRuntime, listener: i64) -> i64 {
    int(perform(
        binding,
        rt,
        Op::Accept,
        "listener",
        vec![Value::Int(listener)],
    )
    .expect("an accept"))
}

fn close(binding: &HostBinding, rt: &dyn HostRuntime, handle: i64, resource: &str) {
    perform(binding, rt, Op::Close, resource, vec![Value::Int(handle)]).expect("a close");
}

fn read_to_end(binding: &HostBinding, rt: &dyn HostRuntime, conn: i64) -> Vec<u8> {
    let mut got = Vec::new();
    loop {
        let chunk = bytes(
            perform(
                binding,
                rt,
                Op::Recv,
                "conn",
                vec![Value::Int(conn), Value::Int(4096), Value::Int(5000)],
            )
            .expect("a read"),
        );
        if chunk.is_empty() {
            return got;
        }
        got.extend_from_slice(&chunk);
    }
}

#[test]
fn an_outbound_connection_is_made_and_served_end_to_end() {
    let net = Arc::new(TcpHost::new());
    let binding = bind(net.clone());
    let server = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("a port");
    let port = server.local_addr().expect("an address").port();
    let peer = std::thread::spawn(move || {
        let (mut stream, _) = server.accept().expect("the host connects");
        let mut request = vec![0u8; REQUEST.len()];
        stream
            .read_exact(&mut request)
            .expect("the request arrives");
        stream.write_all(RESPONSE).expect("the response goes");
        stream
            .shutdown(Shutdown::Write)
            .expect("the peer stops sending");
        request
    });

    let conn = int(perform(
        &binding,
        net.as_ref(),
        Op::Connect,
        "conn",
        vec![
            Value::str("localhost"),
            Value::Int(i64::from(port)),
            Value::Int(5000),
        ],
    )
    .expect("a connect"));
    assert!(conn > 0);
    let sent = int(perform(
        &binding,
        net.as_ref(),
        Op::Send,
        "conn",
        vec![Value::Int(conn), Value::bytes(REQUEST), Value::Int(5000)],
    )
    .expect("a send"));
    assert_eq!(sent, REQUEST.len() as i64);
    assert_eq!(read_to_end(&binding, net.as_ref(), conn), RESPONSE);
    close(&binding, net.as_ref(), conn, "conn");
    assert_eq!(peer.join().expect("the peer finished"), REQUEST);
    assert_eq!(
        net.pool().outstanding(),
        0,
        "every blocking operation was reaped"
    );
}

#[test]
fn a_host_that_cannot_be_reached_is_none_rather_than_a_failure() {
    let net = Arc::new(TcpHost::new());
    let binding = bind(net.clone());
    // A port nothing listens on: bound, then released, so the connect is refused at once.
    let port = std::net::TcpListener::bind(("127.0.0.1", 0))
        .expect("a port")
        .local_addr()
        .expect("an address")
        .port();
    let answer = perform(
        &binding,
        net.as_ref(),
        Op::Connect,
        "conn",
        vec![
            Value::str("127.0.0.1"),
            Value::Int(i64::from(port)),
            Value::Int(2000),
        ],
    )
    .expect("an answer");
    assert!(
        matches!(&answer, Value::Ctor { name, .. } if name.as_str() == "None"),
        "{answer:?}"
    );
    let answer = perform(
        &binding,
        net.as_ref(),
        Op::Connect,
        "conn",
        vec![
            Value::str("no.such.host.invalid"),
            Value::Int(80),
            Value::Int(2000),
        ],
    )
    .expect("an answer");
    assert!(
        matches!(&answer, Value::Ctor { name, .. } if name.as_str() == "None"),
        "{answer:?}"
    );
    assert_eq!(net.pool().outstanding(), 0);
}

fn speak(addr: SocketAddr) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut stream = TcpStream::connect(addr).expect("the peer connects");
        stream.write_all(REQUEST).expect("the peer writes");
        stream
            .shutdown(Shutdown::Write)
            .expect("the peer stops sending");
        let mut answer = Vec::new();
        stream.read_to_end(&mut answer).expect("the peer reads");
        answer
    })
}

fn open(binding: &HostBinding, rt: &dyn HostRuntime) -> (i64, i64) {
    let listener = listen(binding, rt);
    (listener, accept(binding, rt, listener))
}

/// [`DRIVER`] with its listener created over TLS.
const TLS_DRIVER: &str = r#"
fn serve_tls(port: Int, payload: Bytes) -> Int / {net.write[listener], net.write[conn]} = {
  let l = net.listen_tls[listener](port, "api");
  let c = net.accept[listener](l);
  let got = net.recv[conn](c, 4096, 5000);
  let sent = net.send[conn](c, payload, 5000);
  net.close[conn](c);
  net.close[listener](l);
  bytes_len(unwrap_bytes(got)) + unwrap_int(sent)
}

fn unwrap_bytes(answer: Option<Bytes>) -> Bytes =
  match answer { Some(bs) -> bs, None -> b"" }

fn unwrap_int(answer: Option<Int>) -> Int =
  match answer { Some(n) -> n, None -> 0 }
"#;

fn tls_fixture() -> String {
    format!("import std.net (net)\n{TLS_DRIVER}")
}

fn bind_tls(net: Arc<dyn Net>) -> HostBinding {
    registry(net)
        .bind(&check(&tls_fixture()))
        .expect("the declaration and the registration agree")
}

/// Lists what the run was configured with, because the fix is a `--tls` argument.
#[test]
fn a_credential_the_run_does_not_hold_is_refused_by_both_implementations() {
    let socket = Arc::new(TcpHost::new());
    let script = Arc::new(SimNet::new(Vec::new()));
    let refusals = [
        unconfigured(&bind_tls(socket.clone()), socket.as_ref()),
        unconfigured(&bind_tls(script.clone()), script.as_ref()),
    ];
    for refused in refusals {
        assert_eq!(refused.code, codes::TLS_CREDENTIAL_UNKNOWN);
        assert!(refused.message.contains("`api`"), "{}", refused.message);
        assert!(
            refused
                .notes
                .iter()
                .any(|n| n.contains("no `--tls` credential")),
            "{:?}",
            refused.notes
        );
    }
}

fn unconfigured(binding: &HostBinding, rt: &dyn HostRuntime) -> Diagnostic {
    perform(
        binding,
        rt,
        Op::ListenTls,
        "listener",
        vec![Value::Int(0), Value::str("api")],
    )
    .expect_err("no credential was configured")
}

/// Above the boundary a TLS connection carries the same bytes, so a service runs here unchanged.
#[test]
fn the_script_serves_a_tls_listener_for_a_service_that_never_changed() {
    let net = Arc::new(SimNet::with_credentials(
        vec![vec![REQUEST.to_vec()]],
        vec!["api"],
    ));
    let binding = bind_tls(net.clone());
    let listener = int(perform(
        &binding,
        net.as_ref(),
        Op::ListenTls,
        "listener",
        vec![Value::Int(0), Value::str("api")],
    )
    .expect("the credential this run was configured with"));
    let conn = accept(&binding, net.as_ref(), listener);
    let request = drain_tls(&binding, net.as_ref(), conn);
    assert_eq!(request, REQUEST);
    answer(&binding, net.as_ref(), conn);
    assert_eq!(net.sent(conn), RESPONSE);

    // A plaintext listener answers the same program: TLS is not a separate effect.
    let plain = Arc::new(SimNet::new(vec![vec![REQUEST.to_vec()]]));
    let binding = bind_tls(plain.clone());
    let listener = listen(&binding, plain.as_ref());
    let other = accept(&binding, plain.as_ref(), listener);
    assert_eq!(drain_tls(&binding, plain.as_ref(), other), request);
    answer(&binding, plain.as_ref(), other);
    assert_eq!(plain.sent(other), net.sent(conn));
}

fn drain_tls(binding: &HostBinding, rt: &dyn HostRuntime, conn: i64) -> Vec<u8> {
    let mut got = Vec::new();
    loop {
        let answer = perform(
            binding,
            rt,
            Op::Recv,
            "conn",
            vec![Value::Int(conn), Value::Int(4096), Value::Int(5_000)],
        )
        .expect("a read");
        let chunk = bytes(sent_some(answer));
        if chunk.is_empty() {
            return got;
        }
        got.extend_from_slice(&chunk);
    }
}

fn answer(binding: &HostBinding, rt: &dyn HostRuntime, conn: i64) {
    let written = perform(
        binding,
        rt,
        Op::Send,
        "conn",
        vec![Value::Int(conn), Value::bytes(RESPONSE), Value::Int(5_000)],
    )
    .expect("a send");
    assert_eq!(int(sent_some(written)), RESPONSE.len() as i64);
}

fn sent_some(value: Value) -> Value {
    match &value {
        Value::Ctor { name, args } if name.as_str() == "Some" && args.len() == 1 => args[0].clone(),
        other => panic!("expected `Some(..)`, got {other:?}"),
    }
}

/// It must name the TLS handler, not the plaintext listener's.
#[test]
fn a_hermetic_run_names_the_tls_handler_it_did_not_bind() {
    let hermetic = HostBinding::hermetic_with(registry(Arc::new(TcpHost::new())));
    assert!(hermetic.is_hermetic());
    assert_eq!(
        hermetic.would_serve(
            &Symbol::new(EFFECT),
            &Symbol::new("listen_tls"),
            Some(&Symbol::new("listener")),
        ),
        Some(ply_host::tls::HANDLER)
    );
}

/// A certificate on disk that the host serves under `api` and, through `--trust`, accepts.
fn issued() -> (
    tempfile::TempDir,
    ply_host::tls::CredentialSpec,
    std::path::PathBuf,
) {
    let dir = tempfile::tempdir().expect("a temp dir");
    let issued =
        ply_host::certgen::issue(&["localhost".to_string()]).expect("a certificate is issued");
    let certificate = dir.path().join("cert.pem");
    let key = dir.path().join("key.pem");
    std::fs::write(&certificate, &issued.certificate).expect("the certificate is written");
    std::fs::write(&key, &issued.key).expect("the key is written");
    let spec = ply_host::tls::CredentialSpec {
        name: "api".to_string(),
        certificate: certificate.clone(),
        key,
    };
    (dir, spec, certificate)
}

/// The host's own TLS listener answers its own TLS client: the bytes cross encrypted.
#[test]
fn an_outbound_tls_connection_is_verified_and_served_end_to_end() {
    let (_dir, spec, certificate) = issued();
    let credentials = ply_host::tls::Credentials::load(std::slice::from_ref(&spec), &[certificate])
        .expect("the material loads");
    let net = Arc::new(TcpHost::with_credentials(credentials));
    let binding = bind_tls(net.clone());
    let listener = int(perform(
        &binding,
        net.as_ref(),
        Op::ListenTls,
        "listener",
        vec![Value::Int(0), Value::str("api")],
    )
    .expect("a TLS listener"));
    let port = net.local_addr(listener).expect("a bound port").port();

    let server_net = net.clone();
    let server_binding = bind_tls(server_net.clone());
    let server = std::thread::spawn(move || {
        let conn = int(perform(
            &server_binding,
            server_net.as_ref(),
            Op::Accept,
            "listener",
            vec![Value::Int(listener)],
        )
        .expect("an accept"));
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            let chunk = bytes(
                perform(
                    &server_binding,
                    server_net.as_ref(),
                    Op::Recv,
                    "conn",
                    vec![Value::Int(conn), Value::Int(4096), Value::Int(5000)],
                )
                .expect("a read"),
            );
            assert!(!chunk.is_empty(), "the client has not finished the request");
            request.extend_from_slice(&chunk);
        }
        perform(
            &server_binding,
            server_net.as_ref(),
            Op::Send,
            "conn",
            vec![Value::Int(conn), Value::bytes(RESPONSE), Value::Int(5000)],
        )
        .expect("a send");
        close(&server_binding, server_net.as_ref(), conn, "conn");
        request
    });

    let conn = int(perform(
        &binding,
        net.as_ref(),
        Op::ConnectTls,
        "conn",
        vec![
            Value::str("localhost"),
            Value::Int(i64::from(port)),
            Value::Int(5000),
        ],
    )
    .expect("a connect"));
    assert!(conn > 0);
    let sent = int(perform(
        &binding,
        net.as_ref(),
        Op::Send,
        "conn",
        vec![Value::Int(conn), Value::bytes(REQUEST), Value::Int(5000)],
    )
    .expect("a send"));
    assert_eq!(sent, REQUEST.len() as i64);
    assert_eq!(read_to_end(&binding, net.as_ref(), conn), RESPONSE);
    close(&binding, net.as_ref(), conn, "conn");
    assert_eq!(server.join().expect("the server finished"), REQUEST);
    close(&binding, net.as_ref(), listener, "listener");
    let counts = net.handshakes();
    assert_eq!(counts.completed, 2, "one handshake at each end");
    assert_eq!(counts.refused, 0);
}

/// Without `--trust`, a self-signed server is refused: the connection ends and nothing is read.
#[test]
fn an_untrusted_server_ends_the_connection_at_the_handshake() {
    let (_dir, spec, _certificate) = issued();
    let credentials = ply_host::tls::Credentials::load(std::slice::from_ref(&spec), &[])
        .expect("the material loads");
    let net = Arc::new(TcpHost::with_credentials(credentials));
    let binding = bind_tls(net.clone());
    let listener = int(perform(
        &binding,
        net.as_ref(),
        Op::ListenTls,
        "listener",
        vec![Value::Int(0), Value::str("api")],
    )
    .expect("a TLS listener"));
    let port = net.local_addr(listener).expect("a bound port").port();

    let server_net = net.clone();
    let server_binding = bind_tls(server_net.clone());
    let server = std::thread::spawn(move || {
        let conn = int(perform(
            &server_binding,
            server_net.as_ref(),
            Op::Accept,
            "listener",
            vec![Value::Int(listener)],
        )
        .expect("an accept"));
        let got = bytes(
            perform(
                &server_binding,
                server_net.as_ref(),
                Op::Recv,
                "conn",
                vec![Value::Int(conn), Value::Int(4096), Value::Int(5000)],
            )
            .expect("a read"),
        );
        close(&server_binding, server_net.as_ref(), conn, "conn");
        got
    });

    let conn = int(perform(
        &binding,
        net.as_ref(),
        Op::ConnectTls,
        "conn",
        vec![
            Value::str("localhost"),
            Value::Int(i64::from(port)),
            Value::Int(5000),
        ],
    )
    .expect("the TCP connect succeeds; verification happens on the first write"));
    let sent = int(perform(
        &binding,
        net.as_ref(),
        Op::Send,
        "conn",
        vec![Value::Int(conn), Value::bytes(REQUEST), Value::Int(5000)],
    )
    .expect("a send"));
    assert_eq!(sent, 0, "the handshake failed, so nothing was written");
    assert_eq!(read_to_end(&binding, net.as_ref(), conn), b"");
    close(&binding, net.as_ref(), conn, "conn");
    assert_eq!(server.join().expect("the server finished"), b"");
    close(&binding, net.as_ref(), listener, "listener");
    let counts = net.handshakes();
    assert!(
        counts
            .reasons
            .iter()
            .any(|(reason, _)| *reason == ply_host::tls::REASON_CERTIFICATE),
        "{:?}",
        counts.reasons
    );
}
