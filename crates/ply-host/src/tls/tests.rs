//! What the TLS handler claims, and whether it is telling the truth.

use super::*;
use rustls::{ClientConfig, ClientConnection, RootCertStore};
use std::net::TcpStream;
use std::sync::atomic::{AtomicUsize, Ordering};

const REQUEST: &[u8] = b"GET / HTTP/1.1\r\nhost: localhost\r\n\r\n";
const RESPONSE: &[u8] = b"HTTP/1.1 200 OK\r\ncontent-length: 3\r\n\r\nply";

/// A certificate and key on disk, and the DER a client trusts them by.
struct Material {
    dir: tempfile::TempDir,
    certificate: PathBuf,
    key: PathBuf,
    der: rustls::pki_types::CertificateDer<'static>,
}

/// Generated per test rather than checked in: a fixture certificate is either expired, or committed
/// with its private key, and this suite may have neither.
fn material() -> Material {
    let dir = tempfile::tempdir().expect("a temp dir");
    let issued =
        rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).expect("rcgen issues");
    let certificate = dir.path().join("cert.pem");
    let key = dir.path().join("key.pem");
    std::fs::write(&certificate, issued.cert.pem()).expect("the certificate is written");
    std::fs::write(&key, issued.signing_key.serialize_pem()).expect("the key is written");
    Material {
        dir,
        certificate,
        key,
        der: issued.cert.der().clone(),
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
        Credentials::load(&[self.spec(name)]).expect("the generated material loads")
    }

    fn write(&self, rel: &str, text: &str) -> PathBuf {
        let path = self.dir.path().join(rel);
        std::fs::write(&path, text).expect("a fixture file is written");
        path
    }
}

fn load(spec: CredentialSpec) -> Vec<Diagnostic> {
    Credentials::load(&[spec]).err().unwrap_or_default()
}

fn one(diagnostics: Vec<Diagnostic>) -> Diagnostic {
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    diagnostics.into_iter().next().unwrap()
}

// --- The credential ---------------------------------------------------------

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
    // 32 bytes of hex, and the same value twice: the listing is diffed in CI.
    assert_eq!(credential.fingerprint().len(), "sha256:".len() + 64);
    let again = material.credentials("api");
    assert_eq!(
        credential.fingerprint(),
        again.iter().next().expect("one credential").1.fingerprint(),
        "the listing is diffed in CI, so two loads of one file agree"
    );
}

#[test]
fn a_key_that_does_not_match_the_leaf_is_refused_before_anything_runs() {
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
    // Both files, because either one could be the one that is wrong.
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
fn a_malformed_pem_a_certificate_free_file_and_a_key_free_file_are_each_refused() {
    let material = material();

    let garbage = material.write("garbage.pem", "-----BEGIN CERTIFICATE-----\nnot base64!\n");
    let refused = one(load(CredentialSpec {
        name: "api".to_string(),
        certificate: garbage.clone(),
        key: material.key.clone(),
    }));
    assert_eq!(refused.code, codes::TLS_CREDENTIAL_INVALID);
    assert!(
        refused.message.contains("garbage.pem"),
        "{}",
        refused.message
    );

    // Valid PEM, wrong contents: the key file offered where a chain was wanted.
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

/// One run, three broken paths, three diagnostics.
#[test]
fn every_credential_that_fails_is_reported_rather_than_the_first() {
    let material = material();
    let absent = |name: &str| CredentialSpec {
        name: name.to_string(),
        certificate: material.dir.path().join("absent.pem"),
        key: material.key.clone(),
    };
    let diagnostics = Credentials::load(&[absent("a"), absent("b"), material.spec("c")])
        .expect_err("two of the three cannot load");
    assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
    assert!(
        diagnostics
            .iter()
            .all(|d| d.code == codes::TLS_CREDENTIAL_INVALID)
    );
}

#[test]
fn two_credentials_with_one_name_are_refused_rather_than_one_winning() {
    let material = material();
    let diagnostic = one(
        Credentials::load(&[material.spec("api"), material.spec("api")])
            .expect_err("`api` twice is two answers to one question"),
    );
    assert_eq!(diagnostic.code, codes::TLS_CREDENTIAL_INVALID);
    assert!(
        diagnostic.message.contains("`api`"),
        "{}",
        diagnostic.message
    );
}

#[test]
fn the_argument_shape_is_a_usage_error_with_the_form_in_it() {
    let good =
        CredentialSpec::parse("api=certs/api.pem,certs/api.key").expect("the canonical form");
    assert_eq!(good.name, "api");
    assert_eq!(good.certificate, PathBuf::from("certs/api.pem"));
    assert_eq!(good.key, PathBuf::from("certs/api.key"));

    for bad in ["api", "api=one.pem", "=a,b", "api=,b", "api=a,"] {
        let message = CredentialSpec::parse(bad).expect_err(bad);
        assert!(
            message.contains("--tls NAME=CERT.pem,KEY.pem"),
            "`{bad}` was refused without the form: {message}"
        );
    }
}

/// `E0429`, and the fix is a `--tls` argument rather than an edit to the program — so the
/// diagnostic has to say what the run was given.
#[test]
fn an_unconfigured_credential_lists_the_ones_there_are() {
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

/// A `Debug` that could print key material is a key that gets printed.
#[test]
fn nothing_about_a_credential_renders_its_key() {
    let material = material();
    let key = std::fs::read_to_string(&material.key).expect("the key file");
    let rendered = format!("{:?}", material.credentials("api"));
    assert_eq!(rendered, r#"["api"]"#);
    for line in key.lines().filter(|l| !l.starts_with("-----")) {
        assert!(!rendered.contains(line), "a key line reached a `Debug`");
    }
}

// --- A real connection ------------------------------------------------------

/// The client half: an ordinary rustls client that trusts exactly the certificate the test
/// generated, so a round trip proves the handler served a real TLS session rather than that
/// verification was switched off.
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

/// A listener, and the client thread that will talk to it.
struct Peer {
    listener: TcpListener,
    config: Arc<ServerConfig>,
    handshakes: Arc<Handshakes>,
}

fn peer(material: &Material) -> Peer {
    let credentials = material.credentials("api");
    let (listener, config) = listen(&credentials, "api", 0, Span::DUMMY).expect("a TLS listener");
    Peer {
        listener,
        config,
        handshakes: Arc::new(Handshakes::default()),
    }
}

impl Peer {
    fn port(&self) -> u16 {
        self.listener.local_addr().expect("a bound port").port()
    }

    /// Accept one connection and wrap it, doing no I/O — which is the claim: `accept` never
    /// handshakes, so a client sending garbage cannot take the loop down with it.
    fn accept(&self) -> Session {
        let (stream, _) = self.listener.accept().expect("a connection");
        Session::new(
            Arc::clone(&self.config),
            Arc::new(stream),
            Arc::clone(&self.handshakes),
        )
    }
}

/// A client that completes a handshake, sends `REQUEST`, and reports what came back.
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
        let mut answer = vec![0u8; read_back];
        stream.read_exact(&mut answer)?;
        Ok(answer)
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
    assert_eq!(counts.refused, 0);
    assert!(counts.reasons.is_empty());

    session.close();
    // End of stream is not a one-shot answer, and a closed session never wakes.
    assert_eq!(session.read(64), Some(Vec::new()));
    assert_eq!(session.write(RESPONSE), 0);
}

#[test]
fn a_client_offering_only_h2_is_refused_and_one_offering_http_1_1_is_served() {
    let material = material();
    let peer = peer(&material);
    let refused = speak(peer.port(), material.der.clone(), vec!["h2".to_string()], 0);

    let session = peer.accept();
    assert_eq!(
        session.read(4096),
        Some(Vec::new()),
        "a refused handshake reads as end of stream"
    );
    assert!(refused.join().expect("the client finished").is_err());

    let counts = peer.handshakes.snapshot();
    assert_eq!(counts.refused, 1);
    assert_eq!(counts.completed, 0);
    assert_eq!(counts.reasons, [(REASON_ALPN, 1)]);

    // The listener is still there, which is the half of the claim one refused connection cannot
    // make.
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

/// A peer that is not speaking TLS at all — the plaintext request sent to the TLS port, which is
/// what a misconfigured deployment produces every time.
#[test]
fn a_peer_that_does_not_speak_tls_closes_that_connection_and_nothing_else() {
    let material = material();
    let peer = peer(&material);
    let port = peer.port();
    let plaintext = std::thread::spawn(move || {
        let mut socket = TcpStream::connect(("127.0.0.1", port)).expect("connects");
        let _ = socket.write_all(REQUEST);
        // Held open so the server's answer is a refusal rather than an EOF.
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
    assert_eq!(counts.refused, 1);
    assert_eq!(counts.reasons, [(REASON_NOT_TLS, 1)]);
}

/// A client that goes away mid-flight.
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
    assert_eq!(counts.refused, 1);
    assert_eq!(counts.reasons, [(REASON_GONE, 1)]);
}

/// A deadline is not an ending: nothing was handed to the program, rustls keeps whatever partial
/// record it read, and the next read resumes.
#[test]
fn a_read_deadline_answers_none_and_leaves_the_session_open() {
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
        // Handshake, then say nothing for long enough that the server's first read expires, then
        // send the request.
        stream.flush().expect("the handshake completes");
        std::thread::sleep(std::time::Duration::from_millis(300));
        stream.write_all(REQUEST).expect("the request is written");
        stream.flush().expect("the request is flushed");
        let mut answer = vec![0u8; RESPONSE.len()];
        stream.read_exact(&mut answer).expect("a response");
        answer
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
    assert_eq!(peer.handshakes.snapshot().refused, 0);
}

// --- What a refusal is called ----------------------------------------------

/// The reasons are a fixed set, and that is a security property rather than tidiness: they key a
/// map that lives as long as the run, and rustls's own error text embeds values a peer chose.
#[test]
fn every_reason_is_one_of_a_fixed_set_and_names_what_to_do() {
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

    // An unsupported protocol version says which versions there are, because "unsupported" without
    // them is a sentence a reader cannot act on.
    assert!(REASON_VERSION.contains("TLS 1.3") && REASON_VERSION.contains("TLS 1.2"));
    assert!(REASON_ALPN.contains("http/1.1"));

    // After the handshake, a failure is an ordinary transport failure: counting it as a refused
    // handshake would make the summary lie in the direction of alarm.
    let after = io::Error::new(io::ErrorKind::InvalidData, TlsError::DecryptError);
    assert_eq!(reason(false, &after), REASON_TRANSPORT);
}

/// The count is what the run's summary reports, so its order may not depend on which failure
/// happened first.
#[test]
fn the_reasons_are_reported_most_frequent_first_and_then_alphabetically() {
    let handshakes = Handshakes::default();
    for reason in [REASON_NOT_TLS, REASON_ALPN, REASON_NOT_TLS, REASON_GONE] {
        handshakes.refused(reason);
    }
    handshakes.completed();
    let counts = handshakes.snapshot();
    assert_eq!(counts.refused, 4);
    assert_eq!(counts.completed, 1);
    assert_eq!(
        counts.reasons,
        [(REASON_NOT_TLS, 2), (REASON_ALPN, 1), (REASON_GONE, 1)]
    );
    assert!(!counts.is_empty());
    assert!(HandshakeCounts::default().is_empty());
}

// --- What the listing says about itself -------------------------------------

/// The `transport` block names a version, and a listing that named a version other than the one
/// linked would be the trusted computing base lying about itself.
#[test]
fn the_listing_names_the_version_that_is_actually_linked() {
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
    // Never the pre-release: `cargo search` surfaces `0.24.0-dev.1` and it must not be what a
    // trusted computing base is built on.
    assert!(!resolved.contains("dev"), "{resolved} is a pre-release");
    assert!(resolved.starts_with("0.23."), "{resolved}");
}

/// Nothing in the trusted computing base may depend on a provider some other library installed
/// first, so the provider is put on the builder rather than taken from the process-wide default —
/// and the default is left unset, which this asserts by there being no way to have set it.
#[test]
fn the_provider_is_the_one_this_module_names() {
    assert_eq!(PROVIDER, "ring");
    assert!(
        CryptoProvider::get_default().is_none(),
        "a process-wide default provider was installed, and `ply hosts` names one that was not consulted"
    );
    let material = material();
    // Proof that a credential builds without one: `Credentials::load` would have failed had it
    // needed the default.
    assert!(!material.credentials("api").is_empty());
}

/// One connection is one session, and the mutex is what keeps a second operation from interleaving
/// records with the first.
#[test]
fn two_operations_on_one_session_serialise_rather_than_interleave() {
    let material = material();
    let peer = peer(&material);
    // The client reads every byte the writers produce, so no write can lose a race with a peer that
    // stopped listening and the count below is exact.
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

    // Ten writers of one response, from ten threads.
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
