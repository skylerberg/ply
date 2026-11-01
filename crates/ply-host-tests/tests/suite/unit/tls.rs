use ply_core::{Diagnostic, Span, codes};
use ply_host::tls::*;
use rustls::crypto::CryptoProvider;
use rustls::server::ServerConfig;
use rustls::{ClientConfig, ClientConnection, RootCertStore};
use rustls::{Error as TlsError, PeerIncompatible, StreamOwned};
use std::io::{self, Read, Write};
use std::net::TcpListener;
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const REQUEST: &[u8] = b"GET / HTTP/1.1\r\nhost: localhost\r\n\r\n";
const RESPONSE: &[u8] = b"HTTP/1.1 200 OK\r\ncontent-length: 3\r\n\r\nply";

struct Material {
    dir: tempfile::TempDir,
    certificate: PathBuf,
    key: PathBuf,
    der: rustls::pki_types::CertificateDer<'static>,
}

fn material() -> Material {
    let dir = tempfile::tempdir().expect("a temp dir");
    let issued = ply_host::certgen::issue(&[]).expect("a certificate is issued");
    let certificate = dir.path().join("cert.pem");
    let key = dir.path().join("key.pem");
    std::fs::write(&certificate, &issued.certificate).expect("the certificate is written");
    std::fs::write(&key, &issued.key).expect("the key is written");
    Material {
        dir,
        certificate,
        key,
        der: rustls::pki_types::CertificateDer::from(issued.der),
    }
}

impl Material {
    fn spec(&self, name: &str) -> CredentialSpec {
        CredentialSpec {
            name: name.to_string(),
            certificate: self.certificate.clone(),
            key: self.key.clone(),
        }
    }

    fn credentials(&self, name: &str) -> Credentials {
        Credentials::load(&[self.spec(name)], &[], &[]).expect("the generated material loads")
    }

    fn write(&self, rel: &str, text: &str) -> PathBuf {
        let path = self.dir.path().join(rel);
        std::fs::write(&path, text).expect("a fixture file is written");
        path
    }
}

fn load(spec: CredentialSpec) -> Vec<Diagnostic> {
    Credentials::load(&[spec], &[], &[])
        .err()
        .unwrap_or_default()
}

fn one(diagnostics: Vec<Diagnostic>) -> Diagnostic {
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    diagnostics.into_iter().next().unwrap()
}

#[test]
fn a_generated_certificate_loads_and_is_reported_by_name_and_fingerprint() {
    let material = material();
    let credentials = material.credentials("api");
    assert_eq!(credentials.names().collect::<Vec<_>>(), ["api"]);
    let (_, credential) = credentials.iter().next().expect("one credential");
    assert_eq!(credential.certificates(), 1);
    assert!(
        credential.fingerprint().starts_with("sha256:"),
        "an operator compares this against `openssl x509 -fingerprint -sha256`: {}",
        credential.fingerprint()
    );
    assert_eq!(credential.fingerprint().len(), "sha256:".len() + 64);
    let again = material.credentials("api");
    assert_eq!(
        credential.fingerprint(),
        again.iter().next().expect("one credential").1.fingerprint(),
        "the listing is diffed in CI, so two loads of one file agree"
    );
}

#[test]
fn a_key_that_does_not_match_the_leaf_is_rejected_before_anything_runs() {
    let mine = material();
    let other = material();
    let diagnostic = one(load(CredentialSpec {
        name: "api".to_string(),
        certificate: mine.certificate.clone(),
        key: other.key.clone(),
    }));
    assert_eq!(diagnostic.code, codes::TLS_CREDENTIAL_INVALID);
    assert!(
        diagnostic.message.contains("does not go with"),
        "{}",
        diagnostic.message
    );
    assert!(
        diagnostic.message.contains("cert.pem"),
        "{}",
        diagnostic.message
    );
    assert!(
        diagnostic.message.contains("key.pem"),
        "{}",
        diagnostic.message
    );
}

#[test]
fn a_file_that_cannot_be_read_names_the_file() {
    let material = material();
    let diagnostic = one(load(CredentialSpec {
        name: "api".to_string(),
        certificate: material.dir.path().join("absent.pem"),
        key: material.key.clone(),
    }));
    assert_eq!(diagnostic.code, codes::TLS_CREDENTIAL_INVALID);
    assert!(
        diagnostic.message.contains("absent.pem"),
        "{}",
        diagnostic.message
    );
}

#[test]
fn a_malformed_pem_a_certificate_free_file_and_a_key_free_file_are_each_rejected() {
    let material = material();

    let garbage = material.write("garbage.pem", "-----BEGIN CERTIFICATE-----\nnot base64!\n");
    let rejected = one(load(CredentialSpec {
        name: "api".to_string(),
        certificate: garbage.clone(),
        key: material.key.clone(),
    }));
    assert_eq!(rejected.code, codes::TLS_CREDENTIAL_INVALID);
    assert!(
        rejected.message.contains("garbage.pem"),
        "{}",
        rejected.message
    );

    let empty = one(load(CredentialSpec {
        name: "api".to_string(),
        certificate: material.key.clone(),
        key: material.key.clone(),
    }));
    assert_eq!(empty.code, codes::TLS_CREDENTIAL_INVALID);
    assert!(
        empty.message.contains("no `BEGIN CERTIFICATE` block"),
        "{}",
        empty.message
    );

    let keyless = one(load(CredentialSpec {
        name: "api".to_string(),
        certificate: material.certificate.clone(),
        key: material.certificate.clone(),
    }));
    assert_eq!(keyless.code, codes::TLS_CREDENTIAL_INVALID);
    assert!(
        keyless.message.contains("no private key"),
        "{}",
        keyless.message
    );
}

#[test]
fn every_credential_that_fails_is_reported_rather_than_the_first() {
    let material = material();
    let absent = |name: &str| CredentialSpec {
        name: name.to_string(),
        certificate: material.dir.path().join("absent.pem"),
        key: material.key.clone(),
    };
    let diagnostics = Credentials::load(&[absent("a"), absent("b"), material.spec("c")], &[], &[])
        .expect_err("two of the three cannot load");
    assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
    assert!(
        diagnostics
            .iter()
            .all(|d| d.code == codes::TLS_CREDENTIAL_INVALID)
    );
}

fn issued_into(
    dir: &std::path::Path,
    stem: &str,
    names: &[&str],
) -> (CredentialSpec, ply_host::certgen::Issued) {
    let names: Vec<String> = names.iter().map(|n| n.to_string()).collect();
    let issued = ply_host::certgen::issue(&names).expect("a certificate is issued");
    let certificate = dir.join(format!("{stem}.pem"));
    let key = dir.join(format!("{stem}.key"));
    std::fs::write(&certificate, &issued.certificate).expect("the certificate is written");
    std::fs::write(&key, &issued.key).expect("the key is written");
    (
        CredentialSpec {
            name: "api".to_string(),
            certificate,
            key,
        },
        issued,
    )
}

fn der_of(issued: &ply_host::certgen::Issued) -> rustls::pki_types::CertificateDer<'static> {
    rustls::pki_types::CertificateDer::from(issued.der.clone())
}

fn client_trusting(
    trusted: &[&ply_host::certgen::Issued],
    own: Option<&ply_host::certgen::Issued>,
) -> Arc<ClientConfig> {
    use rustls::pki_types::pem::PemObject;
    let mut roots = RootCertStore::empty();
    for issued in trusted {
        roots.add(der_of(issued)).expect("a generated certificate");
    }
    let builder = ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .expect("the provider supports both versions")
        .with_root_certificates(roots);
    let config = match own {
        Some(issued) => builder
            .with_client_auth_cert(
                vec![der_of(issued)],
                rustls::pki_types::PrivateKeyDer::from_pem_slice(issued.key.as_bytes())
                    .expect("a generated key"),
            )
            .expect("the key goes with the certificate"),
        None => builder.with_no_client_auth(),
    };
    Arc::new(config)
}

fn shaken(
    server: &Arc<ServerConfig>,
    client: &Arc<ClientConfig>,
    name: &str,
) -> (rustls::Connection, rustls::Connection) {
    let (mut s, mut c) = ends(server, client, name);
    for _ in 0..8 {
        pump(&mut c, &mut s);
        pump(&mut s, &mut c);
    }
    assert!(
        !s.is_handshaking() && !c.is_handshaking(),
        "the handshake completes"
    );
    (s, c)
}

fn rejection(server: &Arc<ServerConfig>, client: &Arc<ClientConfig>, name: &str) -> TlsError {
    let (mut s, mut c) = ends(server, client, name);
    for _ in 0..8 {
        if let Err(e) = passed(&mut c, &mut s).and_then(|()| passed(&mut s, &mut c)) {
            return e;
        }
    }
    panic!("the handshake completed")
}

fn ends(
    server: &Arc<ServerConfig>,
    client: &Arc<ClientConfig>,
    name: &str,
) -> (rustls::Connection, rustls::Connection) {
    let s = rustls::ServerConnection::new(Arc::clone(server)).expect("a server connection");
    let c = ClientConnection::new(
        Arc::clone(client),
        rustls::pki_types::ServerName::try_from(name.to_string()).expect("a server name"),
    )
    .expect("a client connection");
    (s.into(), c.into())
}

fn pump(from: &mut rustls::Connection, to: &mut rustls::Connection) {
    passed(from, to).expect("the peer's records are good");
}

fn passed(from: &mut rustls::Connection, to: &mut rustls::Connection) -> Result<(), TlsError> {
    let mut wire = Vec::new();
    while from.wants_write() {
        from.write_tls(&mut wire).expect("written to a buffer");
    }
    let mut reader: &[u8] = &wire;
    while !reader.is_empty() {
        to.read_tls(&mut reader).expect("read from a buffer");
        to.process_new_packets()?;
    }
    Ok(())
}

#[test]
fn one_credential_name_can_have_several_certificates_and_a_client_gets_the_one_matching_its_server_name()
 {
    let dir = tempfile::tempdir().expect("a temp dir");
    let (first, a) = issued_into(dir.path(), "a", &["a.example"]);
    let (second, b) = issued_into(dir.path(), "b", &["b.example"]);
    let credentials = Credentials::load(&[first, second], &[], &[]).expect("both load");
    let (_, credential) = credentials.iter().next().expect("one name");
    assert_eq!(credentials.names().collect::<Vec<_>>(), ["api"]);
    let client = client_trusting(&[&a, &b], None);
    for (name, wanted) in [("a.example", &a), ("b.example", &b)] {
        let (_, c) = shaken(credential.config(), &client, name);
        assert_eq!(
            c.peer_certificates()
                .and_then(|chain| chain.first().cloned()),
            Some(der_of(wanted)),
            "the certificate for {name}"
        );
    }
}

#[test]
fn a_certificate_whose_files_change_is_served_anew_without_a_restart() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let (spec, before) = issued_into(dir.path(), "api", &["localhost"]);
    let credentials = Credentials::load(std::slice::from_ref(&spec), &[], &[]).expect("it loads");
    let (_, credential) = credentials.iter().next().expect("one name");
    assert_eq!(
        credential.chooser().leaves(),
        std::slice::from_ref(&before.der)
    );
    let (_, after) = issued_into(dir.path(), "api", &["localhost"]);
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(10);
    for path in [&spec.certificate, &spec.key] {
        std::fs::File::options()
            .write(true)
            .open(path)
            .and_then(|f| f.set_modified(later))
            .expect("the files' times move");
    }
    credential.chooser().reload_now();
    assert_eq!(
        credential.chooser().leaves(),
        std::slice::from_ref(&after.der)
    );
    let (_, c) = shaken(
        credential.config(),
        &client_trusting(&[&after], None),
        "localhost",
    );
    assert_eq!(
        c.peer_certificates()
            .and_then(|chain| chain.first().cloned()),
        Some(der_of(&after))
    );
    std::fs::write(&spec.key, "not a key").expect("the key is spoiled");
    std::fs::File::options()
        .write(true)
        .open(&spec.key)
        .and_then(|f| f.set_modified(later + std::time::Duration::from_secs(10)))
        .expect("the key's time moves");
    credential.chooser().reload_now();
    assert_eq!(
        credential.chooser().leaves(),
        std::slice::from_ref(&after.der)
    );
}

#[test]
fn a_trusted_client_certificate_identifies_the_client_to_the_server_and_the_server_accepts_a_client_that_sends_no_certificate()
 {
    let dir = tempfile::tempdir().expect("a temp dir");
    let (server, served) = issued_into(dir.path(), "server", &["localhost"]);
    let (_, caller) = issued_into(dir.path(), "caller", &["caller.example"]);
    let trusted = dir.path().join("caller.pem");
    let credentials = Credentials::load(&[server], &[trusted], &[]).expect("it loads");
    let (_, credential) = credentials.iter().next().expect("one name");
    let (s, _) = shaken(
        credential.config(),
        &client_trusting(&[&served], Some(&caller)),
        "localhost",
    );
    assert_eq!(
        s.peer_certificates()
            .and_then(|chain| chain.first().cloned()),
        Some(der_of(&caller))
    );
    let (anonymous, _) = shaken(
        credential.config(),
        &client_trusting(&[&served], None),
        "localhost",
    );
    assert!(anonymous.peer_certificates().is_none());
}

#[test]
fn a_listener_that_requires_a_client_certificate_rejects_a_client_without_one() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let (server, served) = issued_into(dir.path(), "server", &["localhost"]);
    let (_, caller) = issued_into(dir.path(), "caller", &["caller.example"]);
    let trusted = dir.path().join("caller.pem");
    let credentials =
        Credentials::load(&[server], &[trusted], &["api".to_string()]).expect("it loads");
    assert!(credentials.requires_client("api"));
    let (_, credential) = credentials.iter().next().expect("one name");
    let (s, _) = shaken(
        credential.config(),
        &client_trusting(&[&served], Some(&caller)),
        "localhost",
    );
    assert_eq!(
        s.peer_certificates()
            .and_then(|chain| chain.first().cloned()),
        Some(der_of(&caller))
    );
    let rejected = rejection(
        credential.config(),
        &client_trusting(&[&served], None),
        "localhost",
    );
    assert_eq!(rejected, TlsError::NoCertificatesPresented);
}

#[test]
fn requiring_a_client_certificate_fails_unless_the_credential_exists_and_trusted_certificates_are_configured()
 {
    let dir = tempfile::tempdir().expect("a temp dir");
    let (server, _) = issued_into(dir.path(), "server", &["localhost"]);
    let trusted = dir.path().join("server.pem");
    let unknown = one(Credentials::load(
        std::slice::from_ref(&server),
        std::slice::from_ref(&trusted),
        &["web".to_string()],
    )
    .err()
    .unwrap_or_default());
    assert_eq!(unknown.code, codes::TLS_CREDENTIAL_UNKNOWN);
    assert!(
        unknown.message.contains("`--mtls web`"),
        "{}",
        unknown.message
    );
    let unverified = one(Credentials::load(&[server], &[], &["api".to_string()])
        .err()
        .unwrap_or_default());
    assert_eq!(unverified.code, codes::TLS_CREDENTIAL_INVALID);
    assert!(
        unverified.message.contains("nothing verifies one"),
        "{}",
        unverified.message
    );
}

#[test]
fn a_tls_client_presenting_a_credential_shows_its_certificate_to_the_server() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let (server, served) = issued_into(dir.path(), "server", &["localhost"]);
    let (caller, called) = issued_into(dir.path(), "caller", &["caller.example"]);
    let caller = CredentialSpec {
        name: "caller".to_string(),
        ..caller
    };
    let trusted = [dir.path().join("server.pem"), dir.path().join("caller.pem")];
    let credentials =
        Credentials::load(&[server, caller], &trusted, &["api".to_string()]).expect("it loads");
    let presenting = credentials
        .presenting("caller", Span::DUMMY)
        .expect("the run holds the credential");
    let listener = credentials
        .resolve("api", Span::DUMMY)
        .expect("the run holds the listener's");
    let (s, c) = shaken(&listener, &presenting, "localhost");
    assert_eq!(
        s.peer_certificates()
            .and_then(|chain| chain.first().cloned()),
        Some(der_of(&called))
    );
    assert_eq!(
        c.peer_certificates()
            .and_then(|chain| chain.first().cloned()),
        Some(der_of(&served))
    );
    let missing = credentials
        .presenting("absent", Span::DUMMY)
        .expect_err("no credential of that name");
    assert_eq!(missing.code, codes::TLS_CREDENTIAL_UNKNOWN);
}

#[test]
fn a_client_that_comes_back_resumes_its_session_from_a_ticket() {
    let material = material();
    let credentials = material.credentials("api");
    let (_, credential) = credentials.iter().next().expect("one name");
    let client = Arc::new(client(&material.der, &["http/1.1"]));
    let (_, first) = shaken(credential.config(), &client, "localhost");
    assert_eq!(first.handshake_kind(), Some(rustls::HandshakeKind::Full));
    let (_, second) = shaken(credential.config(), &client, "localhost");
    assert_eq!(
        second.handshake_kind(),
        Some(rustls::HandshakeKind::Resumed)
    );
}

#[test]
fn a_secured_session_reports_the_peer_certificate_and_negotiated_protocol_after_the_handshake() {
    let material = material();
    let peer = peer(&material);
    let client = speak(
        peer.port(),
        material.der.clone(),
        vec!["http/1.1".to_string()],
        RESPONSE.len(),
    );
    let session = peer.accept();
    assert_eq!(
        session.peer_certificate(),
        None,
        "nothing before the handshake"
    );
    assert_eq!(session.protocol(), None);
    assert!(session.handshake().is_some());
    assert_eq!(session.peer_certificate(), None);
    assert_eq!(session.protocol(), Some("http/1.1".to_string()));
    let mut request = Vec::new();
    while !request.ends_with(b"\r\n\r\n") {
        let chunk = session.read(4096).expect("no deadline is set");
        assert!(!chunk.is_empty());
        request.extend_from_slice(&chunk);
    }
    session.write(RESPONSE);
    client
        .join()
        .expect("the client finished")
        .expect("a response");
    session.close();
}

#[test]
fn a_malformed_credential_argument_fails_with_a_usage_error_that_shows_the_expected_format() {
    let good =
        CredentialSpec::parse("api=certs/api.pem,certs/api.key").expect("the canonical form");
    assert_eq!(good.name, "api");
    assert_eq!(good.certificate, PathBuf::from("certs/api.pem"));
    assert_eq!(good.key, PathBuf::from("certs/api.key"));

    for bad in ["api", "api=one.pem", "=a,b", "api=,b", "api=a,"] {
        let message = CredentialSpec::parse(bad).expect_err(bad);
        assert!(
            message.contains("--tls NAME=CERT.pem,KEY.pem"),
            "`{bad}` was rejected without the form: {message}"
        );
    }
}

#[test]
fn an_unknown_credential_name_fails_with_an_error_that_lists_the_configured_credentials() {
    let material = material();
    let credentials = material.credentials("api");
    let diagnostic = credentials
        .resolve("web", Span::DUMMY)
        .expect_err("`web` was never configured");
    assert_eq!(diagnostic.code, codes::TLS_CREDENTIAL_UNKNOWN);
    assert!(
        diagnostic.message.contains("`web`"),
        "{}",
        diagnostic.message
    );
    assert!(
        diagnostic
            .notes
            .iter()
            .any(|n| n.contains("configured: `api`")),
        "{:?}",
        diagnostic.notes
    );

    let none = Credentials::empty()
        .resolve("api", Span::DUMMY)
        .expect_err("nothing is configured");
    assert!(
        none.notes
            .iter()
            .any(|n| n.contains("no `--tls` credential")),
        "{:?}",
        none.notes
    );
    assert!(
        none.notes
            .iter()
            .any(|n| n.contains("--tls api=CERT.pem,KEY.pem")),
        "the note has to be a command someone can run: {:?}",
        none.notes
    );
}

#[test]
fn the_debug_output_of_a_credential_never_contains_its_private_key() {
    let material = material();
    let key = std::fs::read_to_string(&material.key).expect("the key file");
    let rendered = format!("{:?}", material.credentials("api"));
    assert_eq!(rendered, r#"["api"]"#);
    for line in key.lines().filter(|l| !l.starts_with("-----")) {
        assert!(!rendered.contains(line), "a key line reached a `Debug`");
    }
}

fn client(der: &rustls::pki_types::CertificateDer<'static>, alpn: &[&str]) -> ClientConfig {
    let mut roots = RootCertStore::empty();
    roots.add(der.clone()).expect("the generated certificate");
    let mut config = ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .expect("the provider supports both versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = alpn.iter().map(|p| p.as_bytes().to_vec()).collect();
    config
}

struct Peer {
    listener: TcpListener,
    config: Arc<ServerConfig>,
    handshakes: Arc<HandshakeStats>,
}

fn peer(material: &Material) -> Peer {
    let credentials = material.credentials("api");
    let config = credentials
        .resolve("api", Span::DUMMY)
        .expect("the credential is held");
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    Peer {
        listener,
        config,
        handshakes: Arc::new(HandshakeStats::default()),
    }
}

impl Peer {
    fn port(&self) -> u16 {
        self.listener.local_addr().expect("a bound port").port()
    }

    fn accept(&self) -> Session {
        let (stream, _) = self.listener.accept().expect("a connection");
        Session::new(
            Arc::clone(&self.config),
            Arc::new(stream),
            Arc::clone(&self.handshakes),
        )
    }
}

fn speak(
    port: u16,
    der: rustls::pki_types::CertificateDer<'static>,
    alpn: Vec<String>,
    read_back: usize,
) -> std::thread::JoinHandle<std::io::Result<Vec<u8>>> {
    std::thread::spawn(move || {
        let offered: Vec<&str> = alpn.iter().map(String::as_str).collect();
        let config = Arc::new(client(&der, &offered));
        let name = rustls::pki_types::ServerName::try_from("localhost").expect("a server name");
        let connection = ClientConnection::new(config, name).expect("a client connection");
        let socket = TcpStream::connect(("127.0.0.1", port))?;
        let mut stream = StreamOwned::new(connection, socket);
        stream.write_all(REQUEST)?;
        stream.flush()?;
        let mut result = vec![0u8; read_back];
        stream.read_exact(&mut result)?;
        Ok(result)
    })
}

#[test]
fn a_request_over_tls_reads_and_writes_the_same_bytes_the_plaintext_path_would() {
    let material = material();
    let peer = peer(&material);
    let client = speak(
        peer.port(),
        material.der.clone(),
        vec!["http/1.1".to_string()],
        RESPONSE.len(),
    );

    let session = peer.accept();
    let mut request = Vec::new();
    while !request.ends_with(b"\r\n\r\n") {
        let chunk = session.read(4096).expect("no deadline is set");
        assert!(!chunk.is_empty(), "the peer has not finished the request");
        request.extend_from_slice(&chunk);
    }
    assert_eq!(request, REQUEST, "the decrypted bytes are the ones sent");
    assert_eq!(session.write(RESPONSE), RESPONSE.len());

    assert_eq!(
        client
            .join()
            .expect("the client finished")
            .expect("a response"),
        RESPONSE
    );

    let counts = peer.handshakes.snapshot();
    assert_eq!(counts.completed, 1);
    assert_eq!(counts.rejected, 0);
    assert!(counts.reasons.is_empty());

    session.close();
    assert_eq!(session.read(64), Some(Vec::new()));
    assert_eq!(session.write(RESPONSE), 0);
}

#[test]
fn a_client_offering_only_h2_is_rejected_and_one_offering_http_1_1_is_served() {
    let material = material();
    let peer = peer(&material);
    let rejected = speak(peer.port(), material.der.clone(), vec!["h2".to_string()], 0);

    let session = peer.accept();
    assert_eq!(
        session.read(4096),
        Some(Vec::new()),
        "a rejected handshake reads as end of stream"
    );
    assert!(rejected.join().expect("the client finished").is_err());

    let counts = peer.handshakes.snapshot();
    assert_eq!(counts.rejected, 1);
    assert_eq!(counts.completed, 0);
    assert_eq!(counts.reasons, [(REASON_ALPN, 1)]);

    let client = speak(
        peer.port(),
        material.der.clone(),
        vec!["http/1.1".to_string()],
        RESPONSE.len(),
    );
    let session = peer.accept();
    let mut request = Vec::new();
    while !request.ends_with(b"\r\n\r\n") {
        request.extend_from_slice(&session.read(4096).expect("no deadline"));
    }
    assert_eq!(session.write(RESPONSE), RESPONSE.len());
    assert_eq!(
        client
            .join()
            .expect("the client finished")
            .expect("a response"),
        RESPONSE
    );
    assert_eq!(peer.handshakes.snapshot().completed, 1);
}

#[test]
fn a_peer_that_does_not_speak_tls_closes_that_connection_and_nothing_else() {
    let material = material();
    let peer = peer(&material);
    let port = peer.port();
    let plaintext = std::thread::spawn(move || {
        let mut socket = TcpStream::connect(("127.0.0.1", port)).expect("connects");
        let _ = socket.write_all(REQUEST);
        std::thread::sleep(std::time::Duration::from_millis(50));
    });

    let session = peer.accept();
    assert_eq!(session.read(4096), Some(Vec::new()));
    assert_eq!(
        session.write(RESPONSE),
        0,
        "a finished session accepts no bytes"
    );
    plaintext.join().expect("the peer finished");

    let counts = peer.handshakes.snapshot();
    assert_eq!(counts.rejected, 1);
    assert_eq!(counts.reasons, [(REASON_NOT_TLS, 1)]);
}

#[test]
fn a_client_that_disconnects_mid_handshake_is_counted_as_the_peer_going_away() {
    let material = material();
    let peer = peer(&material);
    let port = peer.port();
    let gone = std::thread::spawn(move || {
        let socket = TcpStream::connect(("127.0.0.1", port)).expect("connects");
        drop(socket);
    });

    let session = peer.accept();
    assert_eq!(session.read(4096), Some(Vec::new()));
    gone.join().expect("the peer finished");

    let counts = peer.handshakes.snapshot();
    assert_eq!(counts.rejected, 1);
    assert_eq!(counts.reasons, [(REASON_GONE, 1)]);
}

#[test]
fn a_read_deadline_returns_none_and_leaves_the_session_open() {
    let material = material();
    let peer = peer(&material);
    let port = peer.port();
    let der = material.der.clone();
    let client = std::thread::spawn(move || {
        let config = Arc::new(client(&der, &["http/1.1"]));
        let name = rustls::pki_types::ServerName::try_from("localhost").expect("a server name");
        let connection = ClientConnection::new(config, name).expect("a client connection");
        let socket = TcpStream::connect(("127.0.0.1", port)).expect("connects");
        let mut stream = StreamOwned::new(connection, socket);
        stream.flush().expect("the handshake completes");
        std::thread::sleep(std::time::Duration::from_millis(300));
        stream.write_all(REQUEST).expect("the request is written");
        stream.flush().expect("the request is flushed");
        let mut result = vec![0u8; RESPONSE.len()];
        stream.read_exact(&mut result).expect("a response");
        result
    });

    let session = peer.accept();
    session.deadline(std::time::Duration::from_millis(50));
    assert_eq!(session.read(4096), None, "the deadline expired");

    session.deadline(std::time::Duration::from_secs(10));
    let mut request = Vec::new();
    while !request.ends_with(b"\r\n\r\n") {
        request.extend_from_slice(
            &session
                .read(4096)
                .expect("the session survived the deadline"),
        );
    }
    assert_eq!(request, REQUEST);
    assert_eq!(session.write(RESPONSE), RESPONSE.len());
    assert_eq!(client.join().expect("the client finished"), RESPONSE);
    assert_eq!(peer.handshakes.snapshot().rejected, 0);
}

#[test]
fn every_handshake_rejection_maps_to_one_reason_from_a_fixed_list_each_a_short_message_with_a_suggested_fix()
 {
    let cases = [
        (TlsError::NoApplicationProtocol, REASON_ALPN),
        (
            TlsError::PeerIncompatible(PeerIncompatible::Tls12NotOffered),
            REASON_VERSION,
        ),
        (
            TlsError::PeerIncompatible(PeerIncompatible::Tls12NotOfferedOrEnabled),
            REASON_VERSION,
        ),
        (
            TlsError::PeerIncompatible(PeerIncompatible::SupportedVersionsExtensionRequired),
            REASON_VERSION,
        ),
        (
            TlsError::PeerIncompatible(PeerIncompatible::NoCipherSuitesInCommon),
            REASON_PARAMETERS,
        ),
        (TlsError::DecryptError, REASON_NOT_TLS),
        (
            TlsError::AlertReceived(rustls::AlertDescription::HandshakeFailure),
            REASON_ALERT,
        ),
        (
            TlsError::PeerMisbehaved(rustls::PeerMisbehaved::TooMuchEarlyDataReceived),
            REASON_MISBEHAVED,
        ),
        (TlsError::NoCertificatesPresented, REASON_CERTIFICATE),
        (TlsError::FailedToGetCurrentTime, REASON_OTHER),
    ];
    for (error, expected) in cases {
        let wrapped = io::Error::new(io::ErrorKind::InvalidData, error.clone());
        assert_eq!(reason(true, &wrapped), expected, "{error:?}");
    }

    assert!(REASON_VERSION.contains("TLS 1.3") && REASON_VERSION.contains("TLS 1.2"));
    assert!(REASON_ALPN.contains("http/1.1"));

    let after = io::Error::new(io::ErrorKind::InvalidData, TlsError::DecryptError);
    assert_eq!(reason(false, &after), REASON_TRANSPORT);
}

#[test]
fn handshake_rejection_reasons_are_sorted_by_count_highest_first_then_alphabetically() {
    let handshakes = HandshakeStats::default();
    for reason in [REASON_NOT_TLS, REASON_ALPN, REASON_NOT_TLS, REASON_GONE] {
        handshakes.rejected(reason);
    }
    handshakes.completed();
    let counts = handshakes.snapshot();
    assert_eq!(counts.rejected, 4);
    assert_eq!(counts.completed, 1);
    assert_eq!(
        counts.reasons,
        [(REASON_NOT_TLS, 2), (REASON_ALPN, 1), (REASON_GONE, 1)]
    );
    assert!(!counts.is_empty());
    assert!(HandshakeCounts::default().is_empty());
}

#[test]
fn the_rustls_version_reported_by_the_ply_host_crate_matches_the_version_in_cargo_lock() {
    let lock =
        std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.lock"))
            .expect("the workspace lockfile");
    let resolved = lock
        .split("[[package]]")
        .find(|entry| entry.contains("name = \"rustls\""))
        .and_then(|entry| {
            entry
                .lines()
                .find_map(|line| line.trim().strip_prefix("version = "))
        })
        .map(|version| version.trim_matches('"').to_string())
        .expect("`rustls` is in the lockfile");
    assert_eq!(
        VERSION, resolved,
        "`ply hosts` would print {VERSION} for a build linking {resolved}"
    );
    assert!(!resolved.contains("dev"), "{resolved} is a pre-release");
    assert!(resolved.starts_with("0.23."), "{resolved}");
}

#[test]
fn tls_uses_the_ring_crypto_provider_and_installs_no_process_wide_default_provider() {
    assert_eq!(PROVIDER, "ring");
    assert!(
        CryptoProvider::get_default().is_none(),
        "a process-wide default provider was installed, and `ply hosts` names one that was not consulted"
    );
    let material = material();
    assert!(!material.credentials("api").is_empty());
}

#[test]
fn two_operations_on_one_session_serialise_rather_than_interleave() {
    let material = material();
    let peer = peer(&material);
    let client = speak(
        peer.port(),
        material.der.clone(),
        vec!["http/1.1".to_string()],
        100,
    );
    let session = Arc::new(peer.accept());

    let mut request = Vec::new();
    while !request.ends_with(b"\r\n\r\n") {
        request.extend_from_slice(&session.read(4096).expect("no deadline"));
    }

    let written = Arc::new(AtomicUsize::new(0));
    let mut writers = Vec::new();
    for _ in 0..10 {
        let session = Arc::clone(&session);
        let written = Arc::clone(&written);
        writers.push(std::thread::spawn(move || {
            written.fetch_add(session.write(b"0123456789"), Ordering::Relaxed);
        }));
    }
    for writer in writers {
        writer.join().expect("a writer finished");
    }
    assert_eq!(written.load(Ordering::Relaxed), 100);
    session.close();

    let got = client
        .join()
        .expect("the client finished")
        .expect("the client read every byte");
    let expected: Vec<u8> = b"0123456789".iter().cycle().take(100).copied().collect();
    assert_eq!(got, expected, "a record was interleaved with another");
}
