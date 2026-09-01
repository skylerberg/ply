//! TLS: the one place W3 grows the trusted computing base.

use ply_span::{Diagnostic, Span, codes};
use rustls::crypto::CryptoProvider;
use rustls::crypto::hash::HashAlgorithm;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::{ServerConfig, ServerConnection};
use rustls::{Error as TlsError, PeerIncompatible, StreamOwned};
use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

/// The Rust path `ply hosts` prints for `net.listen_tls`, and the function below that it names.
pub const HANDLER: &str = "ply_host::tls::listen";

/// What the `transport` block of `ply hosts --host` discloses.
pub const LIBRARY: &str = "rustls";
pub const VERSION: &str = "0.23.43";

/// `ring`, not rustls's default `aws-lc-rs`, which needs a C toolchain and cmake on some platforms.
pub const PROVIDER: &str = "ring";

/// Exactly `http/1.1`, and the refusal of anything else is the point: every browser offers `h2`
/// first, and a server that negotiates it and then speaks 1.1 produces a connection error the
/// client reports as the server being broken.
pub const ALPN: [&str; 1] = ["http/1.1"];

/// What [`with_safe_default_protocol_versions`] resolves to under the `tls12` feature, in the order
/// rustls prefers them.
pub const VERSIONS: [&str; 2] = ["TLS 1.3", "TLS 1.2"];

// --- Credentials ------------------------------------------------------------

/// One `--tls NAME=CERT,KEY` argument, parsed but not yet loaded.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CredentialSpec {
    pub name: String,
    pub certificate: PathBuf,
    pub key: PathBuf,
}

impl CredentialSpec {
    /// `NAME=CERT,KEY`.
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

/// One loaded credential: a server configuration nothing outside this module can take the key back
/// out of.
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

/// Every credential a run was configured with, by name.
#[derive(Default)]
pub struct Credentials {
    entries: BTreeMap<String, Credential>,
}

impl Credentials {
    pub fn empty() -> Credentials {
        Credentials::default()
    }

    /// Load and validate every credential, or refuse the run.
    pub fn load(specs: &[CredentialSpec]) -> Result<Credentials, Vec<Diagnostic>> {
        let mut entries: BTreeMap<String, Credential> = BTreeMap::new();
        let mut diagnostics = Vec::new();
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
        if diagnostics.is_empty() {
            Ok(Credentials { entries })
        } else {
            Err(diagnostics)
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

    /// The configuration a listener is built with, or [`E0429`] naming the credentials that were
    /// configured — because the fix is a `--tls` argument rather than an edit to the program.
    pub fn resolve(&self, name: &str, span: Span) -> Result<Arc<ServerConfig>, Diagnostic> {
        match self.entries.get(name) {
            Some(credential) => Ok(Arc::clone(&credential.config)),
            None => Err(unknown_credential(name, self.names(), span)),
        }
    }
}

impl fmt::Debug for Credentials {
    /// By name only.
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
    // The one check that needs both files: rustls refuses a key whose `SubjectPublicKeyInfo` does
    // not match the leaf certificate's public key.
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
fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
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
        // Unreachable with `ring`, which ships TLS13_AES_128_GCM_SHA256, and still not a panic: a
        // fingerprint is something a listing prints, and no run should end because one could not be
        // computed.
        return "sha256:unavailable".to_string();
    };
    let mut out = String::from("sha256:");
    for byte in sha256.hash(leaf.as_ref()).as_ref() {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

// --- The listener -----------------------------------------------------------

/// `net.listen_tls`, and the function [`HANDLER`] names.
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

// --- The session ------------------------------------------------------------

/// A `TcpStream` behind an `Arc`, so that `close` can shut the file descriptor down while a pool
/// thread is parked in a read on it. rustls wants something that is `Read + Write` by value; this
/// is that, without a second file descriptor and without `try_clone`.
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

/// One TLS-terminated connection.
pub struct Session {
    socket: Arc<TcpStream>,
    session: Mutex<Option<StreamOwned<ServerConnection, Socket>>>,
    handshakes: Arc<Handshakes>,
}

impl Session {
    /// Wraps an accepted connection.
    pub fn new(
        config: Arc<ServerConfig>,
        socket: Arc<TcpStream>,
        handshakes: Arc<Handshakes>,
    ) -> Session {
        let started = match ServerConnection::new(config) {
            Ok(connection) => Some(StreamOwned::new(connection, Socket(Arc::clone(&socket)))),
            Err(_) => {
                handshakes.refused(REASON_CONFIGURATION);
                let _ = socket.shutdown(Shutdown::Both);
                None
            }
        };
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

    /// Up to `max` decrypted bytes, under `std.net`'s one rule: `None` is the deadline expiring and
    /// `Some` is what the peer sent, empty when it has stopped sending — or when the handshake
    /// failed, or when this connection never established a session at all.
    pub fn read(&self, max: usize) -> Option<Vec<u8>> {
        let mut guard = lock(&self.session);
        // A finished session is an ending and never a deadline: `None` here would tell a caller to
        // wait for a connection that is already gone.
        let Some(stream) = guard.as_mut() else {
            return Some(Vec::new());
        };
        let handshaking = stream.conn.is_handshaking();
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
        let handshaking = stream.conn.is_handshaking();
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

    /// Shuts the connection down.
    pub fn close(&self) {
        if let Ok(mut guard) = self.session.try_lock() {
            if let Some(stream) = guard.as_mut() {
                stream.conn.send_close_notify();
                let _ = stream.flush();
            }
            *guard = None;
        }
        let _ = self.socket.shutdown(Shutdown::Both);
    }

    /// A handshake that has just finished, counted once.
    fn completed(&self, was_handshaking: bool, stream: &StreamOwned<ServerConnection, Socket>) {
        if was_handshaking && !stream.conn.is_handshaking() {
            self.handshakes.completed();
        }
    }

    fn finish(
        &self,
        guard: &mut MutexGuard<'_, Option<StreamOwned<ServerConnection, Socket>>>,
        refused: Option<&'static str>,
    ) {
        if let Some(reason) = refused {
            self.handshakes.refused(reason);
        }
        **guard = None;
        let _ = self.socket.shutdown(Shutdown::Both);
    }
}

// --- Counting what went wrong ----------------------------------------------

const REASON_CONFIGURATION: &str = "the listener's TLS configuration would not start a session";
const REASON_NOT_TLS: &str = "the peer did not speak TLS, or a record was corrupt";
const REASON_VERSION: &str = "no TLS version in common (this listener offers TLS 1.3 and TLS 1.2)";
const REASON_PARAMETERS: &str = "no cipher suite, key exchange group or signature scheme in common";
const REASON_ALPN: &str = "no application protocol in common (this listener offers http/1.1)";
const REASON_ALERT: &str = "the peer sent a fatal alert and gave up";
const REASON_MISBEHAVED: &str = "the peer sent a TLS message the protocol does not allow";
const REASON_CERTIFICATE: &str = "the peer's certificate was refused";
const REASON_GONE: &str = "the peer went away mid-handshake";
const REASON_TRANSPORT: &str = "the connection failed mid-handshake";
const REASON_OTHER: &str = "the TLS session failed";

/// A deadline that expired, which is not an ending.
fn expired(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}

/// Why a handshake was refused, as one of a **fixed** set of strings.
fn reason(handshaking: bool, error: &io::Error) -> &'static str {
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

/// What the run's `--host` summary reports about TLS.
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
    fn completed(&self) {
        lock(&self.counts).completed += 1;
    }

    fn refused(&self, reason: &'static str) {
        *lock(&self.counts).refused.entry(reason).or_default() += 1;
    }

    /// A snapshot: completed, refused, and the reasons ascending by count and then by text, so two
    /// runs over one failure print one order.
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

/// See `tcp::lock`: the state behind these is a map and an option, neither of which has an
/// invariant a panicking caller can break.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

// --- Diagnostics ------------------------------------------------------------

/// `E0429`, shared by the socket handler and the simulated twin.
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
fn err_duplicate(name: &str) -> Diagnostic {
    Diagnostic::error(
        codes::TLS_CREDENTIAL_INVALID,
        format!("two `--tls` credentials are named `{name}`"),
    )
    .note("a credential name selects one certificate and one key, so a repeat is two answers to one question")
    .note("give them different names, or pass only the one this run should serve")
}

#[cfg(test)]
mod tests;
