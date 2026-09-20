//! TLS credentials and sessions, terminated through rustls.

use ply_span::{Diagnostic, Span, codes};
use rustls::client::ClientConnection;
use rustls::crypto::CryptoProvider;
use rustls::crypto::hash::HashAlgorithm;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use rustls::server::{ServerConfig, ServerConnection};
use rustls::{ClientConfig, Error as TlsError, PeerIncompatible, RootCertStore, StreamOwned};
use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

/// The Rust path `ply hosts` prints for `net.listen_tls`; it must name [`listen`].
pub const HANDLER: &str = "ply_host::tls::listen";

/// The Rust path `ply hosts` prints for `net.connect_tls`; it must name [`connect`].
pub const CONNECT_HANDLER: &str = "ply_host::tls::connect";

pub const LIBRARY: &str = "rustls";
pub const VERSION: &str = "0.23.43";

/// The root certificates `net.connect_tls` verifies a server against, beside any `--trust`.
pub const ROOTS: &str = "webpki-roots";
pub const ROOTS_VERSION: &str = "1.0.9";

/// `ring`, not rustls's default `aws-lc-rs`, which needs a C toolchain and cmake on some platforms.
pub const PROVIDER: &str = "ring";

/// Exactly `http/1.1`: browsers offer `h2` first, and negotiating it then speaking 1.1 breaks them.
pub const ALPN: [&str; 1] = ["http/1.1"];

/// What `with_safe_default_protocol_versions` resolves to under `tls12`, in rustls's order.
pub const VERSIONS: [&str; 2] = ["TLS 1.3", "TLS 1.2"];

/// One `--tls NAME=CERT,KEY` argument, parsed but not yet loaded.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CredentialSpec {
    pub name: String,
    pub certificate: PathBuf,
    pub key: PathBuf,
}

impl CredentialSpec {
    pub fn parse(text: &str) -> Result<CredentialSpec, String> {
        let (name, paths) = text
            .split_once('=')
            .ok_or_else(|| malformed(text, "there is no `=`"))?;
        let (certificate, key) = paths.split_once(',').ok_or_else(|| {
            malformed(text, "there is no `,` between the certificate and the key")
        })?;
        if name.is_empty() {
            return Err(malformed(text, "the credential has no name"));
        }
        if certificate.is_empty() || key.is_empty() {
            return Err(malformed(text, "a path is empty"));
        }
        Ok(CredentialSpec {
            name: name.to_string(),
            certificate: PathBuf::from(certificate),
            key: PathBuf::from(key),
        })
    }
}

fn malformed(text: &str, why: &str) -> String {
    format!("`{text}` is not a TLS credential: {why}; write `--tls NAME=CERT.pem,KEY.pem`")
}

/// A loaded credential; nothing outside this module can take the key back out.
pub struct Credential {
    config: Arc<ServerConfig>,
    /// SHA-256 of the leaf certificate's DER, as `ply hosts` prints it.
    fingerprint: String,
    certificates: usize,
}

impl Credential {
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub fn certificates(&self) -> usize {
        self.certificates
    }
}

#[derive(Default)]
pub struct Credentials {
    entries: BTreeMap<String, Credential>,
    /// What `net.connect_tls` verifies a server against.
    client: Option<Arc<ClientConfig>>,
    trusted: usize,
}

impl Credentials {
    pub fn empty() -> Credentials {
        Credentials::default()
    }

    /// `trusted` names PEM files whose certificates `net.connect_tls` accepts beside the roots.
    pub fn load(
        specs: &[CredentialSpec],
        trusted: &[PathBuf],
    ) -> Result<Credentials, Vec<Diagnostic>> {
        let mut entries: BTreeMap<String, Credential> = BTreeMap::new();
        let mut diagnostics = Vec::new();
        let mut anchors = Vec::new();
        for path in trusted {
            match certificates(path) {
                Ok(chain) => anchors.extend(chain),
                Err(diagnostic) => diagnostics.push(diagnostic),
            }
        }
        for spec in specs {
            if entries.contains_key(&spec.name) {
                diagnostics.push(err_duplicate(&spec.name));
                continue;
            }
            match load_one(spec) {
                Ok(credential) => {
                    entries.insert(spec.name.clone(), credential);
                }
                Err(diagnostic) => diagnostics.push(diagnostic),
            }
        }
        if !diagnostics.is_empty() {
            return Err(diagnostics);
        }
        let trusted = anchors.len();
        let client = client_config(anchors).map_err(|d| vec![d])?;
        Ok(Credentials {
            entries,
            client: Some(client),
            trusted,
        })
    }

    /// How many `--trust` certificates join the roots.
    pub fn trusted(&self) -> usize {
        self.trusted
    }

    /// The configuration `net.connect_tls` verifies a server with; built once per run.
    pub fn client(&self) -> Arc<ClientConfig> {
        match &self.client {
            Some(config) => Arc::clone(config),
            None => client_config(Vec::new()).expect("the provider supports the versions it names"),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &Credential)> {
        self.entries.iter().map(|(name, c)| (name.as_str(), c))
    }

    /// The configuration a listener is built with, or `E0429` naming the configured credentials.
    pub fn resolve(&self, name: &str, span: Span) -> Result<Arc<ServerConfig>, Diagnostic> {
        match self.entries.get(name) {
            Some(credential) => Ok(Arc::clone(&credential.config)),
            None => Err(unknown_credential(name, self.names(), span)),
        }
    }
}

impl fmt::Debug for Credentials {
    /// By name only, so no key material is printed.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.names()).finish()
    }
}

fn load_one(spec: &CredentialSpec) -> Result<Credential, Diagnostic> {
    let chain = certificates(&spec.certificate)?;
    let key = private_key(&spec.key)?;
    let fingerprint = fingerprint(&chain[0]);
    let certificates = chain.len();

    let versions = ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(|e| err_provider(spec, &e))?;
    // rustls refuses a key that does not match the leaf certificate's public key.
    let mut config = versions
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .map_err(|e| err_mismatch(spec, &e))?;
    config.alpn_protocols = ALPN.iter().map(|p| p.as_bytes().to_vec()).collect();
    Ok(Credential {
        config: Arc::new(config),
        fingerprint,
        certificates,
    })
}

/// The provider, installed explicitly on each builder.
pub fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn client_config(trusted: Vec<CertificateDer<'static>>) -> Result<Arc<ClientConfig>, Diagnostic> {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let (_, ignored) = roots.add_parsable_certificates(trusted);
    if ignored > 0 {
        return Err(err_trust(ignored));
    }
    let mut config = ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(|e| {
            Diagnostic::error(
                codes::TLS_CREDENTIAL_INVALID,
                format!("the TLS client could not be configured: {e}"),
            )
        })?
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = ALPN.iter().map(|p| p.as_bytes().to_vec()).collect();
    Ok(Arc::new(config))
}

fn certificates(path: &Path) -> Result<Vec<CertificateDer<'static>>, Diagnostic> {
    let file = std::fs::File::open(path).map_err(|e| err_unreadable(path, &e))?;
    let chain: Result<Vec<CertificateDer<'static>>, io::Error> =
        rustls_pemfile::certs(&mut BufReader::new(file)).collect();
    let chain = chain
        .map_err(|e| err_invalid(path, format!("the PEM in this file does not parse: {e}")))?;
    if chain.is_empty() {
        return Err(err_invalid(
            path,
            "this file holds no `BEGIN CERTIFICATE` block".to_string(),
        ));
    }
    Ok(chain)
}

fn private_key(path: &Path) -> Result<PrivateKeyDer<'static>, Diagnostic> {
    let file = std::fs::File::open(path).map_err(|e| err_unreadable(path, &e))?;
    let key = rustls_pemfile::private_key(&mut BufReader::new(file))
        .map_err(|e| err_invalid(path, format!("the PEM in this file does not parse: {e}")))?;
    key.ok_or_else(|| {
        err_invalid(
            path,
            "this file holds no private key: PKCS#8, PKCS#1 and SEC1 are read".to_string(),
        )
    })
}

/// SHA-256 of the leaf's DER, through the provider's own hash rather than a second implementation.
fn fingerprint(leaf: &CertificateDer<'_>) -> String {
    let Some(sha256) = provider()
        .cipher_suites
        .iter()
        .filter_map(|suite| suite.tls13())
        .map(|suite| suite.common.hash_provider)
        .find(|hash| hash.algorithm() == HashAlgorithm::SHA256)
    else {
        // Unreachable with `ring`; not a panic, since no run should end over a listed fingerprint.
        return "sha256:unavailable".to_string();
    };
    let mut out = String::from("sha256:");
    for byte in sha256.hash(leaf.as_ref()).as_ref() {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

pub fn listen(
    credentials: &Credentials,
    credential: &str,
    port: u16,
    span: Span,
) -> Result<(TcpListener, Arc<ServerConfig>), Diagnostic> {
    let config = credentials.resolve(credential, span)?;
    let listener = crate::tcp::bind("`net.listen_tls`", port, span)?;
    Ok((listener, config))
}

/// `Read + Write` by value over a shared stream, so `close` can shut it down under a parked read.
struct Socket(Arc<TcpStream>);

impl Read for Socket {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        (&*self.0).read(buf)
    }
}

impl Write for Socket {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        (&*self.0).write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        (&*self.0).flush()
    }
}

/// The two ends a session can be; rustls types them apart.
enum Stream {
    Server(StreamOwned<ServerConnection, Socket>),
    Client(StreamOwned<ClientConnection, Socket>),
}

impl Stream {
    fn is_handshaking(&self) -> bool {
        match self {
            Stream::Server(s) => s.conn.is_handshaking(),
            Stream::Client(s) => s.conn.is_handshaking(),
        }
    }

    fn send_close_notify(&mut self) {
        match self {
            Stream::Server(s) => s.conn.send_close_notify(),
            Stream::Client(s) => s.conn.send_close_notify(),
        }
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Stream::Server(s) => s.read(buf),
            Stream::Client(s) => s.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Stream::Server(s) => s.write(buf),
            Stream::Client(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Stream::Server(s) => s.flush(),
            Stream::Client(s) => s.flush(),
        }
    }
}

pub struct Session {
    socket: Arc<TcpStream>,
    session: Mutex<Option<Stream>>,
    handshakes: Arc<Handshakes>,
}

impl Session {
    pub fn new(
        config: Arc<ServerConfig>,
        socket: Arc<TcpStream>,
        handshakes: Arc<Handshakes>,
    ) -> Session {
        let started = ServerConnection::new(config).ok().map(|connection| {
            Stream::Server(StreamOwned::new(connection, Socket(Arc::clone(&socket))))
        });
        Session::started(started, socket, handshakes)
    }

    /// The client end, verifying `name`; it handshakes on the first read or write, like a server.
    pub fn connect(
        config: Arc<ClientConfig>,
        name: ServerName<'static>,
        socket: Arc<TcpStream>,
        handshakes: Arc<Handshakes>,
    ) -> Session {
        let started = ClientConnection::new(config, name).ok().map(|connection| {
            Stream::Client(StreamOwned::new(connection, Socket(Arc::clone(&socket))))
        });
        Session::started(started, socket, handshakes)
    }

    fn started(
        started: Option<Stream>,
        socket: Arc<TcpStream>,
        handshakes: Arc<Handshakes>,
    ) -> Session {
        if started.is_none() {
            handshakes.refused(REASON_CONFIGURATION);
            let _ = socket.shutdown(Shutdown::Both);
        }
        Session {
            socket,
            session: Mutex::new(started),
            handshakes,
        }
    }

    /// The transport deadline every later operation on this session runs under.
    pub fn deadline(&self, timeout: Duration) {
        let _ = self.socket.set_read_timeout(Some(timeout));
        let _ = self.socket.set_write_timeout(Some(timeout));
    }

    /// Up to `max` decrypted bytes: `None` on deadline, empty once the peer or session is gone.
    pub fn read(&self, max: usize) -> Option<Vec<u8>> {
        let mut guard = lock(&self.session);
        // An ending, never a deadline: `None` would have the caller wait on a dead connection.
        let Some(stream) = guard.as_mut() else {
            return Some(Vec::new());
        };
        let handshaking = stream.is_handshaking();
        let mut buffer = vec![0u8; max];
        match stream.read(&mut buffer) {
            Ok(0) => {
                self.completed(handshaking, stream);
                self.finish(&mut guard, None);
                Some(Vec::new())
            }
            Ok(n) => {
                self.completed(handshaking, stream);
                buffer.truncate(n);
                Some(buffer)
            }
            Err(e) if expired(&e) => None,
            Err(e) => {
                self.finish(&mut guard, Some(reason(handshaking, &e)));
                Some(Vec::new())
            }
        }
    }

    /// The whole payload, or `0` for a connection that is finished.
    pub fn write(&self, payload: &[u8]) -> usize {
        let mut guard = lock(&self.session);
        let Some(stream) = guard.as_mut() else {
            return 0;
        };
        let handshaking = stream.is_handshaking();
        match stream.write_all(payload).and_then(|()| stream.flush()) {
            Ok(()) => {
                self.completed(handshaking, stream);
                payload.len()
            }
            Err(e) => {
                let refused = (!expired(&e)).then(|| reason(handshaking, &e));
                self.finish(&mut guard, refused);
                0
            }
        }
    }

    pub fn close(&self) {
        if let Ok(mut guard) = self.session.try_lock() {
            if let Some(stream) = guard.as_mut() {
                stream.send_close_notify();
                let _ = stream.flush();
            }
            *guard = None;
        }
        let _ = self.socket.shutdown(Shutdown::Both);
    }

    /// A handshake that has just finished, counted once.
    fn completed(&self, was_handshaking: bool, stream: &Stream) {
        if was_handshaking && !stream.is_handshaking() {
            self.handshakes.completed();
        }
    }

    fn finish(&self, guard: &mut MutexGuard<'_, Option<Stream>>, refused: Option<&'static str>) {
        if let Some(reason) = refused {
            self.handshakes.refused(reason);
        }
        **guard = None;
        let _ = self.socket.shutdown(Shutdown::Both);
    }
}

const REASON_CONFIGURATION: &str = "the TLS configuration would not start a session";
pub const REASON_NOT_TLS: &str = "the peer did not speak TLS, or a record was corrupt";
pub const REASON_VERSION: &str =
    "no TLS version in common (this listener offers TLS 1.3 and TLS 1.2)";
pub const REASON_PARAMETERS: &str =
    "no cipher suite, key exchange group or signature scheme in common";
pub const REASON_ALPN: &str = "no application protocol in common (this listener offers http/1.1)";
pub const REASON_ALERT: &str = "the peer sent a fatal alert and gave up";
pub const REASON_MISBEHAVED: &str = "the peer sent a TLS message the protocol does not allow";
pub const REASON_CERTIFICATE: &str = "the peer's certificate was refused";
pub const REASON_GONE: &str = "the peer went away mid-handshake";
pub const REASON_TRANSPORT: &str = "the connection failed mid-handshake";
pub const REASON_OTHER: &str = "the TLS session failed";

/// A deadline that expired, which is not an ending.
fn expired(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}

/// Why a handshake was refused, as one of a fixed set of strings.
pub fn reason(handshaking: bool, error: &io::Error) -> &'static str {
    if !handshaking {
        return REASON_TRANSPORT;
    }
    if let Some(tls) = error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<TlsError>())
    {
        return match tls {
            TlsError::NoApplicationProtocol => REASON_ALPN,
            TlsError::PeerIncompatible(
                PeerIncompatible::Tls12NotOffered
                | PeerIncompatible::Tls12NotOfferedOrEnabled
                | PeerIncompatible::SupportedVersionsExtensionRequired
                | PeerIncompatible::Tls13RequiredForQuic,
            ) => REASON_VERSION,
            TlsError::PeerIncompatible(_) => REASON_PARAMETERS,
            TlsError::InvalidMessage(_)
            | TlsError::DecryptError
            | TlsError::EncryptError
            | TlsError::PeerSentOversizedRecord => REASON_NOT_TLS,
            TlsError::AlertReceived(_) => REASON_ALERT,
            TlsError::PeerMisbehaved(_)
            | TlsError::InappropriateMessage { .. }
            | TlsError::InappropriateHandshakeMessage { .. } => REASON_MISBEHAVED,
            TlsError::InvalidCertificate(_) | TlsError::NoCertificatesPresented => {
                REASON_CERTIFICATE
            }
            _ => REASON_OTHER,
        };
    }
    match error.kind() {
        io::ErrorKind::UnexpectedEof
        | io::ErrorKind::ConnectionAborted
        | io::ErrorKind::ConnectionReset
        | io::ErrorKind::BrokenPipe
        | io::ErrorKind::NotConnected => REASON_GONE,
        _ => REASON_TRANSPORT,
    }
}

#[derive(Default)]
pub struct Handshakes {
    counts: Mutex<Counts>,
}

#[derive(Default)]
struct Counts {
    completed: u64,
    refused: BTreeMap<&'static str, u64>,
}

impl Handshakes {
    pub fn completed(&self) {
        lock(&self.counts).completed += 1;
    }

    pub fn refused(&self, reason: &'static str) {
        *lock(&self.counts).refused.entry(reason).or_default() += 1;
    }

    /// Reasons sort by descending count, then text, so one failure always prints one order.
    pub fn snapshot(&self) -> HandshakeCounts {
        let counts = lock(&self.counts);
        let mut reasons: Vec<(&'static str, u64)> =
            counts.refused.iter().map(|(r, n)| (*r, *n)).collect();
        reasons.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        HandshakeCounts {
            completed: counts.completed,
            refused: reasons.iter().map(|(_, n)| n).sum(),
            reasons,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct HandshakeCounts {
    pub completed: u64,
    pub refused: u64,
    pub reasons: Vec<(&'static str, u64)>,
}

impl HandshakeCounts {
    pub fn is_empty(&self) -> bool {
        self.completed == 0 && self.refused == 0
    }
}

/// The guarded state has no invariant a panicking caller can break.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

#[cold]
pub fn unknown_credential<'a>(
    name: &str,
    configured: impl Iterator<Item = &'a str>,
    span: Span,
) -> Diagnostic {
    let configured: Vec<String> = configured.map(|n| format!("`{n}`")).collect();
    let mut diagnostic = Diagnostic::error(
        codes::TLS_CREDENTIAL_UNKNOWN,
        format!("no TLS credential named `{name}` was configured for this run"),
    )
    .primary(
        span,
        "this listener names a credential the run does not hold",
    );
    diagnostic = if configured.is_empty() {
        diagnostic
            .note("this run was given no `--tls` credential at all")
            .note(format!(
                "pass `--tls {name}=CERT.pem,KEY.pem`, naming a certificate chain and its private key"
            ))
    } else {
        diagnostic
            .note(format!("configured: {}", configured.join(", ")))
            .note(format!(
                "pass `--tls {name}=CERT.pem,KEY.pem`, or name one of the credentials above"
            ))
    };
    diagnostic.note("the credential is named rather than passed as bytes, so that no private key enters a definition's hash or the content-addressed store")
}

/// Unreachable with `ring`, which supports both versions [`VERSIONS`] names.
#[cold]
fn err_provider(spec: &CredentialSpec, error: &TlsError) -> Diagnostic {
    Diagnostic::error(
        codes::TLS_CREDENTIAL_INVALID,
        format!(
            "the credential `{}` could not be configured: {error}",
            spec.name
        ),
    )
    .note(format!(
        "this build's TLS provider is {PROVIDER}, and it reported that it supports none of {}",
        VERSIONS.join(", ")
    ))
}

#[cold]
fn err_unreadable(path: &Path, error: &io::Error) -> Diagnostic {
    err_invalid(path, format!("it could not be read: {error}"))
}

#[cold]
fn err_invalid(path: &Path, why: String) -> Diagnostic {
    Diagnostic::error(
        codes::TLS_CREDENTIAL_INVALID,
        format!("`{}` is not usable TLS material: {why}", path.display()),
    )
    .note("`--tls NAME=CERT,KEY` wants a certificate chain in PEM, leaf first, and a private key in PKCS#8, PKCS#1 or SEC1")
    .note("credentials are loaded before anything runs, so this is refused rather than discovered on the first handshake")
}

#[cold]
fn err_mismatch(spec: &CredentialSpec, error: &TlsError) -> Diagnostic {
    Diagnostic::error(
        codes::TLS_CREDENTIAL_INVALID,
        format!(
            "the key in `{}` does not go with the certificate in `{}`",
            spec.key.display(),
            spec.certificate.display()
        ),
    )
    .note(format!("rustls refused the pair: {error}"))
    .note("the private key's public half must match the public key of the first certificate in the chain")
    .note("check that the two files are from the same issuance, and that the chain is leaf first")
}

#[cold]
fn err_trust(ignored: usize) -> Diagnostic {
    Diagnostic::error(
        codes::TLS_CREDENTIAL_INVALID,
        format!("{ignored} `--trust` certificate(s) could not be parsed"),
    )
    .note("`--trust CERT.pem` wants one or more certificates in PEM, each a root `net.connect_tls` may accept")
}

#[cold]
fn err_duplicate(name: &str) -> Diagnostic {
    Diagnostic::error(
        codes::TLS_CREDENTIAL_INVALID,
        format!("two `--tls` credentials are named `{name}`"),
    )
    .note("a credential name selects one certificate and one key, so a repeat is two answers to one question")
    .note("give them different names, or pass only the one this run should serve")
}
