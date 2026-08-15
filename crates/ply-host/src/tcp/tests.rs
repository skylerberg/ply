//! What the socket handler claims, and whether it is telling the truth.
//!
//! Two halves. The first is about the registration — the declared footprint, the
//! determinism flag and the linearity flag — because a wrong entry there is the
//! failure this milestone exists to make loud, and it is wrong before a byte
//! moves. The second runs the protocol, over a real loopback socket and over the
//! script, and asks the two for the same answers.

use super::*;
use ply_core::CheckOutput;
use ply_core::ty::{EffectAtom, Resource};
use ply_eval::{Bound, HostBinding, Pending, Value};
use ply_span::SourceId;
use ply_syntax::ast::{Mode, ModuleName};
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};

const REQUEST: &[u8] = b"GET / HTTP/1.1\r\nhost: localhost\r\n\r\n";
const RESPONSE: &[u8] = b"HTTP/1.1 200 OK\r\ncontent-length: 3\r\n\r\nply";

/// A registration is resolved against the atoms the **program** performs, and
/// the shipped declaration names no resource label on its own. So the fixture is
/// the declaration plus a driver that performs every operation under the two
/// labels these tests use — which is `examples/echo.ply` in miniature.
const DRIVER: &str = "
fn every_op(port: Int, payload: Bytes) -> Int / {net.write[listener], net.write[conn]} = {
  let l = net.listen[listener](port);
  let c = net.accept[listener](l);
  let got = net.recv[conn](c, 16);
  let sent = net.send[conn](c, payload);
  net.close[conn](c);
  net.close[listener](l);
  bytes_len(got) + sent
}
";

/// Checked under the module name it ships as, because an effect's name is
/// qualified: the same text loaded anonymously declares `net` rather than
/// `std.net.net` and would not bind, which is the drift [`EFFECT`] exists to
/// pin.
/// The shipped declaration and the driver, as one module: mutation tests edit
/// this text so that a rename lands on both sides of it, which is the whole
/// point of asserting that a rename is caught.
fn fixture() -> String {
    format!("{DECLARATION}{DRIVER}")
}

fn check(source: &str) -> CheckOutput {
    let module = ply_syntax::parse_module(SourceId(0), ModuleName::from_dotted(MODULE), source)
        .expect("the declaration parses");
    ply_core::check_module(&module).expect("the declaration typechecks")
}

fn bind(net: Arc<dyn Net>) -> HostBinding {
    registry(net)
        .bind(&check(&fixture()))
        .expect("the declaration and the registration agree")
}

fn atom(resource: &str) -> EffectAtom {
    EffectAtom::new(EFFECT, Resource::Named(Symbol::new(resource)), Mode::Write)
}

/// One `perform`, as the machine would make it: resolve the triple, call the
/// handler, and drive a pending answer to a value the way an entry point with no
/// scheduler around it does.
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
        atom: bound.atom.clone(),
        op: bound.op,
        args: &args,
        span: Span::DUMMY,
    };
    match bound.handler.call(rt, &request)? {
        HostAnswer::Value(v) => Ok(v),
        HostAnswer::Pending(pending) => rt.block_on(pending),
    }
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

// --- The registration -------------------------------------------------------

#[test]
fn the_declaration_binds_and_names_exactly_the_atoms_the_program_performs() {
    let binding = bind(Arc::new(TcpHost::new()));
    let atoms: Vec<String> = binding
        .footprint()
        .atoms()
        .map(EffectAtom::to_string)
        .collect();
    assert_eq!(
        atoms,
        ["std.net.net.write[conn]", "std.net.net.write[listener]"]
    );
}

/// The claim `ply hosts` puts in front of a reviewer. `Any` expands per label,
/// never per registration, so a handler that serves every socket still lists the
/// sockets it got.
#[test]
fn the_listing_is_one_row_per_triple_and_never_a_star() {
    let binding = bind(Arc::new(TcpHost::new()));
    let rows: Vec<String> = binding
        .listing()
        .rows
        .iter()
        .map(|r| format!("{r} {} {}", r.atom, r.path))
        .collect();
    assert_eq!(
        rows,
        [
            "std.net.net.accept[conn] std.net.net.write[conn] ply_host::tcp::accept",
            "std.net.net.accept[listener] std.net.net.write[listener] ply_host::tcp::accept",
            "std.net.net.close[conn] std.net.net.write[conn] ply_host::tcp::close",
            "std.net.net.close[listener] std.net.net.write[listener] ply_host::tcp::close",
            "std.net.net.listen[conn] std.net.net.write[conn] ply_host::tcp::listen",
            "std.net.net.listen[listener] std.net.net.write[listener] ply_host::tcp::listen",
            "std.net.net.recv[conn] std.net.net.write[conn] ply_host::tcp::recv",
            "std.net.net.recv[listener] std.net.net.write[listener] ply_host::tcp::recv",
            "std.net.net.send[conn] std.net.net.write[conn] ply_host::tcp::send",
            "std.net.net.send[listener] std.net.net.write[listener] ply_host::tcp::send",
        ]
    );
}

/// The whole reason every operation is `[s]`: two sockets a program labels apart
/// are two resources, so the conflict graph lets the tasks holding them run at
/// once. Two accepted at one source site share a label and do contend, which is
/// the honest limit of a ground resource label.
#[test]
fn two_labelled_sockets_do_not_conflict_and_one_label_does() {
    assert!(!atom("conn").conflicts_with(&atom("listener")));
    assert!(atom("conn").conflicts_with(&atom("conn")));
}

/// ADR 0008 §5. Not asserted in prose: `register` is one function, so the twin
/// cannot drift from the socket in any column the signature owns.
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

/// The flag the machine's E0426 rule keys on. A `Repeatable` socket operation
/// would let a captured continuation send a packet twice, and there is no
/// operation here whose replay changes nothing outside the program.
#[test]
fn no_operation_claims_to_be_repeatable() {
    for net in [
        Arc::new(TcpHost::new()) as Arc<dyn Net>,
        Arc::new(SimNet::new(Vec::new())),
    ] {
        for op in Op::ALL {
            assert_eq!(
                op.declaration(net.as_ref()).linearity,
                Linearity::AtMostOnce
            );
        }
    }
}

/// The arrow points from the declaration to the handler: the source says
/// `nondet` or the handler is refused. This is what keeps E0412 — and therefore
/// the cache key — ignorant of whether `--host` was passed.
#[test]
fn a_declaration_without_nondet_refuses_the_handler() {
    let weakened = fixture().replacen("nondet effect net", "effect net", 1);
    let diagnostics = registry(Arc::new(TcpHost::new()))
        .bind(&check(&weakened))
        .expect_err("a socket cannot sit behind an effect that is not `nondet`");
    assert!(
        diagnostics
            .iter()
            .all(|d| d.code == codes::HOST_DETERMINISM_MISMATCH),
        "{:?}",
        diagnostics.iter().map(|d| d.code).collect::<Vec<_>>()
    );
}

/// A rename on either side. The Rust table is the claim and the declaration is
/// the authority, so the two meeting is checked before anything runs.
#[test]
fn an_operation_renamed_in_the_source_is_refused_at_bind_time() {
    let renamed = fixture().replace("recv", "read_bytes");
    let diagnostics = registry(Arc::new(TcpHost::new()))
        .bind(&check(&renamed))
        .expect_err("`net.recv` is no longer declared");
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code == codes::HOST_OPERATION_UNKNOWN),
        "{:?}",
        diagnostics.iter().map(|d| d.code).collect::<Vec<_>>()
    );
}

// --- The protocol, over a script --------------------------------------------

#[test]
fn the_script_serves_listen_accept_recv_send_close() {
    let net = Arc::new(SimNet::new(vec![vec![REQUEST.to_vec()]]));
    let binding = bind(net.clone());
    let served = serve_once(&binding, net.as_ref());
    assert_eq!(served.request, REQUEST);
    assert_eq!(served.sent, RESPONSE.len() as i64);
    assert_eq!(net.sent(served.conn), RESPONSE);
}

/// A `recv` shorter than `max` is ordinary, and the bytes it did not take are
/// still there. A program that treats a short answer as the whole message is
/// wrong against both handlers.
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
                vec![Value::Int(conn), Value::Int(3)],
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

/// A second `recv` is a second read, not a replay: the bytes the first one took
/// are gone. This is the whole reason every operation is `AtMostOnce` and why
/// resuming a continuation across one has to be refused.
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
                vec![Value::Int(conn), Value::Int(n)],
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
                vec![Value::Int(conn), Value::Int(64)],
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
        vec![Value::Int(conn), Value::Int(64)],
    )
    .expect_err("the handle is gone");
    assert_eq!(again.code, codes::RUNTIME_ERROR);
}

/// A read bound arrives from the program, so it is an untrusted number. Capping
/// it is legal — a short answer always is — but the cap has to happen before the
/// cast, because a wrap to zero would answer empty and empty is how the program
/// learns the peer went away.
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
            vec![Value::Int(conn), Value::Int(i64::MAX)],
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
fn a_listener_and_a_connection_are_not_interchangeable() {
    let net = Arc::new(SimNet::new(vec![vec![b"hi".to_vec()]]));
    let binding = bind(net.clone());
    let (listener, conn) = open(&binding, net.as_ref());

    let wrong_way = perform(
        &binding,
        net.as_ref(),
        Op::Recv,
        "listener",
        vec![Value::Int(listener), Value::Int(8)],
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

/// The handler's one mechanical defence against misreporting its own footprint.
/// The runtime schedules on the label and the handler acts on the handle, and
/// nothing else connects the two: one socket under two labels would be two atoms
/// that do not conflict over a resource that does.
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
        vec![Value::Int(conn), Value::Int(8)],
    )
    .expect("the first use fixes the label");

    let relabelled = perform(
        &binding,
        net.as_ref(),
        Op::Recv,
        "listener",
        vec![Value::Int(conn), Value::Int(8)],
    )
    .expect_err("this socket is already `[conn]`");
    assert_eq!(relabelled.code, codes::RUNTIME_ERROR);
    assert!(
        relabelled.message.contains("[conn]") && relabelled.message.contains("[listener]"),
        "both labels are named: {}",
        relabelled.message
    );
}

/// Zero is refused rather than clamped, because an empty answer already means
/// the peer has stopped sending.
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
        vec![Value::Int(conn), Value::Int(0)],
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

// --- The protocol, over a real socket ---------------------------------------

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
        vec![Value::Int(conn), Value::bytes(RESPONSE)],
    )
    .expect("a send"));
    assert_eq!(sent, RESPONSE.len() as i64);
    close(&binding, net.as_ref(), conn, "conn");
    close(&binding, net.as_ref(), listener, "listener");

    assert_eq!(peer.join().expect("the peer finished"), RESPONSE);
    assert_eq!(net.outstanding(), 0, "every blocking operation was reaped");
}

/// The peer sends more than one `recv` asks for and keeps the connection open,
/// so what comes back is short by construction. How short is the kernel's
/// business — the claim is that the bytes it did not take are still there.
#[test]
fn a_real_partial_read_returns_what_it_can_and_the_rest_next_time() {
    let net = Arc::new(TcpHost::new());
    let binding = bind(net.clone());
    let listener = listen(&binding, net.as_ref());
    let addr = net.local_addr(listener).expect("a bound port");

    let peer = std::thread::spawn(move || {
        let mut stream = TcpStream::connect(addr).expect("the peer connects");
        stream.write_all(b"abcdefghijk").expect("the peer writes");
        // Held open, so the reads below end because `max` ran out rather than
        // because the stream did.
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
                vec![Value::Int(conn), Value::Int(3)],
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
                vec![Value::Int(conn), Value::Int(64)],
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

/// The substitution ADR 0008 §5 is about: one driver, two bindings, no change to
/// what it performs. Chunk boundaries are deliberately not compared — TCP does
/// not preserve them, so a claim about them would be a claim the socket handler
/// cannot keep — but the byte stream, the handles and the byte counts are.
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

// --- The pending token ------------------------------------------------------

#[test]
fn a_token_the_runtime_did_not_mint_is_loud_rather_than_lost() {
    let net = TcpHost::new();
    let foreign = Pending {
        token: 4096,
        label: "recv",
    };
    assert!(!net.owns(&foreign));
    let polled = net
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
        net.poll(&stray).expect_err("not its token").code,
        codes::INTERNAL_ERROR
    );
    assert_eq!(
        net.park().expect_err("nothing to wait for").code,
        codes::INTERNAL_ERROR
    );
}

// --- Drivers ----------------------------------------------------------------

struct Served {
    listener: i64,
    conn: i64,
    request: Vec<u8>,
    sent: i64,
}

/// The program both bindings serve: bind, take one connection, read until the
/// peer is done, answer, close.
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
        vec![Value::Int(conn), Value::bytes(RESPONSE)],
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
                vec![Value::Int(conn), Value::Int(4096)],
            )
            .expect("a read"),
        );
        if chunk.is_empty() {
            return got;
        }
        got.extend_from_slice(&chunk);
    }
}

/// A peer that sends the request, stops sending, and reports what came back.
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

/// Open a connection over whichever binding, so the reads below have one.
fn open(binding: &HostBinding, rt: &dyn HostRuntime) -> (i64, i64) {
    let listener = listen(binding, rt);
    (listener, accept(binding, rt, listener))
}
