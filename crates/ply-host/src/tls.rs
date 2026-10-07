//! TLS credentials and sessions, terminated through rustls.

use ply_eval::{Diagnostic, Span, codes};
use rustls::client::{ClientConnection, ResolvesClientCert};
use rustls::crypto::CryptoProvider;
use rustls::crypto::hash::HashAlgorithm;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use rustls::server::danger::ClientCertVerifier;
use rustls::server::{
    ClientHello, ProducesTickets, ResolvesServerCert, ServerConfig, ServerConnection,
    ServerSessionMemoryCache, WebPkiClientVerifier,
};
use rustls::sign::CertifiedKey;
use rustls::{
    ClientConfig, Error as TlsError, PeerIncompatible, RootCertStore, SignatureScheme, StreamOwned,
};
use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

/// The Rust path `ply hosts` prints for `net.listen_tls`; it must name [`listen`].
pub const HANDLER: &str = "ply_host::tls::listen";

/// The Rust path `ply hosts` prints for `net.connect_tls`; it must name [`connect`].
pub const CONNECT_HANDLER: &str = "ply_host::tls::connect";

/// The Rust path `ply hosts` prints for `net.handshake`; it must name [`Session::handshake`].
pub const HANDSHAKE_HANDLER: &str = "ply_host::tls::handshake";

/// The Rust path `ply hosts` prints for `net.start_tls`, the client's end of an upgrade.
pub const START_HANDLER: &str = "ply_host::tls::start";

/// The Rust path `ply hosts` prints for `net.serve_tls`, the server's end of an upgrade.
pub const SERVE_HANDLER: &str = "ply_host::tls::serve";

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
    /// SHA-256 of the first certificate's leaf, as `ply hosts` prints it.
    fingerprint: String,
    certificates: usize,
    chooser: Arc<Chooser>,
}

/// One certificate a credential answers with, and when its two files last changed.
#[derive(Debug)]
struct Loaded {
    spec: CredentialSpec,
    key: Arc<CertifiedKey>,
    stamp: Option<(SystemTime, SystemTime)>,
}

/// The certificates one credential name holds: a handshake gets the first whose names cover the
/// name its client asks for (SNI), or the first of all, and a certificate whose files change is
/// read again, a look at most a second, so a renewed one is served without a restart. One that
/// no longer loads leaves the one before it in place.
#[derive(Debug)]
pub struct Chooser {
    loaded: Mutex<Vec<Loaded>>,
    looked: Mutex<Instant>,
}

/// How often a chooser looks at its files.
const RELOAD_LOOK: Duration = Duration::from_secs(1);

impl Chooser {
    /// Reads changed files again, at most once a `RELOAD_LOOK` unless `now`.
    fn refresh(&self, now: bool) {
        {
            let mut looked = lock(&self.looked);
            if !now && looked.elapsed() < RELOAD_LOOK {
                return;
            }
            *looked = Instant::now();
        }
        let mut loaded = lock(&self.loaded);
        for entry in loaded.iter_mut() {
            let now = stamp(&entry.spec);
            if now != entry.stamp
                && let Ok(fresh) = certified(&entry.spec)
            {
                entry.key = fresh.key;
                entry.stamp = now;
            }
        }
    }

    /// Reads every file again now, whatever the clock says: what a test of reloading asks for.
    pub fn reload_now(&self) {
        self.refresh(true);
    }

    /// The leaf each certificate answers with now, in the order given.
    pub fn leaves(&self) -> Vec<Vec<u8>> {
        lock(&self.loaded)
            .iter()
            .filter_map(|l| l.key.end_entity_cert().ok().map(|c| c.as_ref().to_vec()))
            .collect()
    }
}

/// A client presents the first certificate a credential holds, as a server asks for one.
impl ResolvesClientCert for Chooser {
    fn resolve(
        &self,
        _root_hint_subjects: &[&[u8]],
        _sigschemes: &[SignatureScheme],
    ) -> Option<Arc<CertifiedKey>> {
        self.refresh(false);
        lock(&self.loaded).first().map(|l| Arc::clone(&l.key))
    }

    fn has_certs(&self) -> bool {
        true
    }
}

impl ResolvesServerCert for Chooser {
    fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        self.refresh(false);
        let loaded = lock(&self.loaded);
        let asked = hello
            .server_name()
            .and_then(|name| ServerName::try_from(name.to_string()).ok());
        let chosen = asked.and_then(|name| loaded.iter().find(|l| covers(&l.key, &name)));
        chosen
            .or_else(|| loaded.first())
            .map(|l| Arc::clone(&l.key))
    }
}

/// Whether `key`'s leaf names `name` among its subject alternative names.
fn covers(key: &CertifiedKey, name: &ServerName<'_>) -> bool {
    key.end_entity_cert()
        .ok()
        .and_then(|der| webpki::EndEntityCert::try_from(der).ok())
        .is_some_and(|leaf| leaf.verify_is_valid_for_subject_name(name).is_ok())
}

/// When a credential's two files last changed, where both can be looked at.
fn stamp(spec: &CredentialSpec) -> Option<(SystemTime, SystemTime)> {
    let modified = |path: &Path| std::fs::metadata(path).and_then(|m| m.modified()).ok();
    Some((modified(&spec.certificate)?, modified(&spec.key)?))
}

/// A certificate chain and its key, read and checked to go together.
struct Certified {
    key: Arc<CertifiedKey>,
    fingerprint: String,
    certificates: usize,
}

fn certified(spec: &CredentialSpec) -> Result<Certified, Diagnostic> {
    let chain =
        certificates(&spec.certificate).map_err(|why| err_invalid(&spec.certificate, why))?;
    let key = private_key(&spec.key)?;
    let fingerprint = fingerprint(&chain[0]);
    let certificates = chain.len();
    let key =
        CertifiedKey::from_der(chain, key, &provider()).map_err(|e| err_mismatch(spec, &e))?;
    // rustls refuses a key that does not match the leaf certificate's public key.
    key.keys_match().map_err(|e| err_mismatch(spec, &e))?;
    Ok(Certified {
        key: Arc::new(key),
        fingerprint,
        certificates,
    })
}

impl Credential {
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub fn certificates(&self) -> usize {
        self.certificates
    }

    /// The certificates the name holds, as handshakes choose among them.
    pub fn chooser(&self) -> &Arc<Chooser> {
        &self.chooser
    }

    /// The configuration a listener under this name answers with.
    pub fn config(&self) -> &Arc<ServerConfig> {
        &self.config
    }
}

#[derive(Default)]
pub struct Credentials {
    entries: BTreeMap<String, Credential>,
    /// What `net.connect_tls` verifies a server against.
    client: Option<Arc<ClientConfig>>,
    /// The same, presenting each credential to a server that asks a client for a certificate.
    presenting: BTreeMap<String, Arc<ClientConfig>>,
    /// The credentials whose listeners refuse a client that presents no certificate.
    required: Vec<String>,
    trusted: usize,
}

impl Credentials {
    pub fn empty() -> Credentials {
        Credentials::default()
    }

    /// `trusted` names PEM files whose certificates `net.connect_tls` accepts beside the roots,
    /// and which a listener verifies a client's certificate against: where there are any, every
    /// listener asks a client for one, and serves a client that presents none unless `required`
    /// names its credential (`--mtls`). A name given several times holds several certificates,
    /// chosen among by the name a client asks for.
    pub fn load(
        specs: &[CredentialSpec],
        trusted: &[PathBuf],
        required: &[String],
    ) -> Result<Credentials, Vec<Diagnostic>> {
        let mut grouped: BTreeMap<String, Vec<Loaded>> = BTreeMap::new();
        let mut firsts: BTreeMap<String, (String, usize)> = BTreeMap::new();
        let mut diagnostics = Vec::new();
        let mut anchors = Vec::new();
        for path in trusted {
            match certificates(path) {
                Ok(chain) => anchors.extend(chain),
                Err(why) => diagnostics.push(err_untrusted(path, why)),
            }
        }
        for spec in specs {
            match certified(spec) {
                Ok(c) => {
                    firsts
                        .entry(spec.name.clone())
                        .or_insert((c.fingerprint, c.certificates));
                    grouped.entry(spec.name.clone()).or_default().push(Loaded {
                        spec: spec.clone(),
                        key: c.key,
                        stamp: stamp(spec),
                    });
                }
                Err(diagnostic) => diagnostics.push(diagnostic),
            }
        }
        for name in required {
            if !grouped.contains_key(name) {
                diagnostics.push(err_required_unknown(name));
            } else if anchors.is_empty() {
                diagnostics.push(err_required_untrusted(name));
            }
        }
        if !diagnostics.is_empty() {
            return Err(diagnostics);
        }
        let shared = Shared::new(&anchors).map_err(|d| vec![d])?;
        let mut entries: BTreeMap<String, Credential> = BTreeMap::new();
        let mut presenting: BTreeMap<String, Arc<ClientConfig>> = BTreeMap::new();
        for (name, loaded) in grouped {
            let chooser = Arc::new(Chooser {
                loaded: Mutex::new(loaded),
                looked: Mutex::new(Instant::now()),
            });
            let (fingerprint, certificates) = firsts.remove(&name).unwrap_or_default();
            let config = shared
                .server(&chooser, required.contains(&name))
                .map_err(|e| vec![err_configured(&name, &e)])?;
            presenting.insert(
                name.clone(),
                client_config(anchors.clone(), Some(Arc::clone(&chooser))).map_err(|d| vec![d])?,
            );
            entries.insert(
                name,
                Credential {
                    config,
                    fingerprint,
                    certificates,
                    chooser,
                },
            );
        }
        let trusted = anchors.len();
        let client = client_config(anchors, None).map_err(|d| vec![d])?;
        Ok(Credentials {
            entries,
            client: Some(client),
            presenting,
            required: required.to_vec(),
            trusted,
        })
    }

    /// Whether the listener of the credential `name` refuses a client that presents no certificate.
    pub fn requires_client(&self, name: &str) -> bool {
        self.required.iter().any(|r| r == name)
    }

    /// The configuration `net.start_tls` verifies a server with, presenting the credential `name`
    /// where the server asks for a client certificate; `E0429` naming the configured credentials
    /// where the run holds none of that name.
    pub fn presenting(&self, name: &str, span: Span) -> Result<Arc<ClientConfig>, Diagnostic> {
        match self.presenting.get(name) {
            Some(config) => Ok(Arc::clone(config)),
            None => Err(unknown_credential(name, self.names(), span)),
        }
    }

    /// How many trusted certificates join the roots.
    pub fn trusted(&self) -> usize {
        self.trusted
    }

    /// The configuration `net.connect_tls` verifies a server with; built once per run.
    pub fn client(&self) -> Arc<ClientConfig> {
        match &self.client {
            Some(config) => Arc::clone(config),
            None => client_config(Vec::new(), None)
                .expect("the provider supports the versions it names"),
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

/// What every listener of a run shares: the client verifiers the trusted certificates make, one
/// that serves a client presenting no certificate and one that refuses it, and one ticket key,
/// which rotates, so a session one listener began resumes at another.
struct Shared {
    verifier: Option<Arc<dyn ClientCertVerifier>>,
    requiring: Option<Arc<dyn ClientCertVerifier>>,
    tickets: Arc<dyn ProducesTickets>,
}

impl Shared {
    fn new(anchors: &[CertificateDer<'static>]) -> Result<Shared, Diagnostic> {
        let tickets = rustls::crypto::ring::Ticketer::new().map_err(|e| {
            Diagnostic::error(
                codes::TLS_CREDENTIAL_INVALID,
                format!("TLS session tickets could not be set up: {e}"),
            )
        })?;
        let verifier = client_verifier(anchors, false)?;
        let requiring = client_verifier(anchors, true)?;
        Ok(Shared {
            verifier,
            requiring,
            tickets,
        })
    }

    fn server(
        &self,
        chooser: &Arc<Chooser>,
        required: bool,
    ) -> Result<Arc<ServerConfig>, TlsError> {
        let versions = ServerConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()?;
        let verifier = if required {
            &self.requiring
        } else {
            &self.verifier
        };
        let verified = match verifier {
            Some(verifier) => versions.with_client_cert_verifier(Arc::clone(verifier)),
            None => versions.with_no_client_auth(),
        };
        let mut config =
            verified.with_cert_resolver(Arc::clone(chooser) as Arc<dyn ResolvesServerCert>);
        config.alpn_protocols = ALPN.iter().map(|p| p.as_bytes().to_vec()).collect();
        config.ticketer = Arc::clone(&self.tickets);
        config.session_storage = ServerSessionMemoryCache::new(SESSIONS);
        Ok(Arc::new(config))
    }
}

/// What verifies a client's certificate against `anchors`, none where there are none: one that
/// serves a client presenting no certificate, or where `required` one that refuses it.
fn client_verifier(
    anchors: &[CertificateDer<'static>],
    required: bool,
) -> Result<Option<Arc<dyn ClientCertVerifier>>, Diagnostic> {
    if anchors.is_empty() {
        return Ok(None);
    }
    let mut roots = RootCertStore::empty();
    roots.add_parsable_certificates(anchors.to_vec());
    let builder = WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider());
    let builder = if required {
        builder
    } else {
        builder.allow_unauthenticated()
    };
    let built = builder.build().map_err(|e| {
        Diagnostic::error(
            codes::TLS_CREDENTIAL_INVALID,
            format!("the trusted certificates cannot verify a client: {e}"),
        )
    })?;
    Ok(Some(built))
}

/// Sessions a listener keeps for resumption by id, besides those its tickets carry.
const SESSIONS: usize = 1024;

/// The provider, installed explicitly on each builder.
pub fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// A client's configuration: the roots and `trusted` to verify a server against, and where
/// `presenting` names a credential, its certificate for a server that asks for one.
fn client_config(
    trusted: Vec<CertificateDer<'static>>,
    presenting: Option<Arc<Chooser>>,
) -> Result<Arc<ClientConfig>, Diagnostic> {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let (_, ignored) = roots.add_parsable_certificates(trusted);
    if ignored > 0 {
        return Err(err_trust(ignored));
    }
    let rooted = ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(|e| {
            Diagnostic::error(
                codes::TLS_CREDENTIAL_INVALID,
                format!("the TLS client could not be configured: {e}"),
            )
        })?
        .with_root_certificates(roots);
    let mut config = match presenting {
        Some(chooser) => rooted.with_client_cert_resolver(chooser as Arc<dyn ResolvesClientCert>),
        None => rooted.with_no_client_auth(),
    };
    config.alpn_protocols = ALPN.iter().map(|p| p.as_bytes().to_vec()).collect();
    Ok(Arc::new(config))
}

/// The certificates a PEM file holds, or why it holds none a caller can use.
fn certificates(path: &Path) -> Result<Vec<CertificateDer<'static>>, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("it could not be read: {e}"))?;
    let chain: Result<Vec<CertificateDer<'static>>, io::Error> =
        rustls_pemfile::certs(&mut BufReader::new(file)).collect();
    let chain = chain.map_err(|e| format!("the PEM in this file does not parse: {e}"))?;
    if chain.is_empty() {
        return Err("this file holds no `BEGIN CERTIFICATE` block".to_string());
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
pub fn fingerprint(leaf: &CertificateDer<'_>) -> String {
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
    /// One round of the handshake, driven from outside the read/write path.
    fn complete_io(&mut self) -> io::Result<(usize, usize)> {
        match self {
            Stream::Server(s) => s.conn.complete_io(&mut s.sock),
            Stream::Client(s) => s.conn.complete_io(&mut s.sock),
        }
    }

    fn is_handshaking(&self) -> bool {
        match self {
            Stream::Server(s) => s.conn.is_handshaking(),
            Stream::Client(s) => s.conn.is_handshaking(),
        }
    }

    /// The far end's end-entity certificate as it presented it: a server's always, a client's where
    /// it was asked for one and gave it.
    fn peer_leaf(&self) -> Option<Vec<u8>> {
        let chain = match self {
            Stream::Server(s) => s.conn.peer_certificates(),
            Stream::Client(s) => s.conn.peer_certificates(),
        }?;
        chain.first().map(|leaf| leaf.as_ref().to_vec())
    }

    fn send_close_notify(&mut self) {
        match self {
            Stream::Server(s) => s.conn.send_close_notify(),
            Stream::Client(s) => s.conn.send_close_notify(),
        }
    }

    /// The application protocol the handshake agreed on.
    fn protocol(&self) -> Option<String> {
        let agreed = match self {
            Stream::Server(s) => s.conn.alpn_protocol(),
            Stream::Client(s) => s.conn.alpn_protocol(),
        };
        agreed.map(|p| String::from_utf8_lossy(p).into_owned())
    }
}

/// Why a handshake asked to complete within a deadline did not.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Unsettled {
    Expired,
    /// One of the `REASON_` strings.
    Refused(&'static str),
}

/// The configuration `config` with `alpn` as the protocols it offers, none when `alpn` is empty.
pub fn client_offering(config: &ClientConfig, alpn: &[String]) -> Arc<ClientConfig> {
    let mut config = config.clone();
    config.alpn_protocols = alpn.iter().map(|p| p.as_bytes().to_vec()).collect();
    Arc::new(config)
}

/// The configuration `config` with `alpn` as the protocols it accepts; empty accepts a client
/// whatever it offers and agrees on none.
pub fn server_accepting(config: &ServerConfig, alpn: &[String]) -> Arc<ServerConfig> {
    let mut config = config.clone();
    config.alpn_protocols = alpn.iter().map(|p| p.as_bytes().to_vec()).collect();
    Arc::new(config)
}

/// Whether bytes are waiting to be read on a connection that is about to start a handshake as the
/// client, where the server never speaks first: plaintext sent behind a go-ahead.
pub fn plaintext_waiting(socket: &TcpStream) -> bool {
    if socket.set_nonblocking(true).is_err() {
        return false;
    }
    let mut byte = [0u8; 1];
    let waiting = matches!(socket.peek(&mut byte), Ok(n) if n > 0);
    let _ = socket.set_nonblocking(false);
    waiting
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
    /// What the handshake took, once one has completed: microseconds, measured by whichever
    /// operation got there first — `net.handshake`, or the read or write that needed the connection.
    took: Mutex<Option<u64>>,
    /// The far end's certificate and the protocol the two agreed, kept once the handshake
    /// completes, so reading them never waits on a read that holds the session.
    peer: Mutex<Option<Vec<u8>>>,
    agreed: Mutex<Option<String>>,
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
        match started {
            Some(stream) => Session {
                socket,
                session: Mutex::new(Some(stream)),
                handshakes,
                took: Mutex::new(None),
                peer: Mutex::new(None),
                agreed: Mutex::new(None),
            },
            None => Session::refused(socket, handshakes, REASON_CONFIGURATION),
        }
    }

    /// A session that never starts: the connection is ended, counted under `reason`, and reads as
    /// a peer done sending.
    pub fn refused(
        socket: Arc<TcpStream>,
        handshakes: Arc<Handshakes>,
        reason: &'static str,
    ) -> Session {
        handshakes.refused(reason);
        let _ = socket.shutdown(Shutdown::Both);
        Session {
            socket,
            session: Mutex::new(None),
            handshakes,
            took: Mutex::new(None),
            peer: Mutex::new(None),
            agreed: Mutex::new(None),
        }
    }

    /// The application protocol the handshake agreed, once it has completed.
    pub fn protocol(&self) -> Option<String> {
        lock(&self.agreed).clone()
    }

    /// What a completed handshake leaves to be read without the session: its length, the peer's
    /// certificate and the protocol.
    fn settled(&self, stream: &Stream, took: u64) {
        lock(&self.took).get_or_insert(took);
        *lock(&self.peer) = stream.peer_leaf();
        *lock(&self.agreed) = stream.protocol();
        self.handshakes.completed();
    }

    /// The far end's end-entity certificate (DER) once the handshake has completed: the server's
    /// on a client's session, and on a server's the client's where it presented one this run
    /// verified. What RFC 5929's `tls-server-end-point` hashes.
    pub fn peer_certificate(&self) -> Option<Vec<u8>> {
        lock(&self.peer).clone()
    }

    /// This end's address, which the session shares with the socket it runs over.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.socket.peer_addr()
    }

    /// The socket the session runs over, for the options that are the socket's.
    pub fn socket(&self) -> &TcpStream {
        &self.socket
    }

    /// The handshake, completed within `timeout` in all, answering the application protocol it
    /// agreed on. A handshake that does not complete ends the connection, a deadline included:
    /// an upgrade that timed out has left bytes of a handshake on the wire.
    pub fn settle_within(&self, timeout: Duration) -> Result<Option<String>, Unsettled> {
        let began = Instant::now();
        let mut guard = lock(&self.session);
        let Some(stream) = guard.as_mut() else {
            return Err(Unsettled::Refused(REASON_CONFIGURATION));
        };
        loop {
            if !stream.is_handshaking() {
                self.settled(stream, began.elapsed().as_micros() as u64);
                return Ok(stream.protocol());
            }
            let Some(left) = timeout
                .checked_sub(began.elapsed())
                .filter(|d| !d.is_zero())
            else {
                self.finish(&mut guard, Some(REASON_SLOW));
                return Err(Unsettled::Expired);
            };
            let _ = self.socket.set_read_timeout(Some(left));
            let _ = self.socket.set_write_timeout(Some(left));
            match stream.complete_io() {
                Ok((0, 0)) if stream.is_handshaking() => {
                    self.finish(&mut guard, Some(REASON_GONE));
                    return Err(Unsettled::Refused(REASON_GONE));
                }
                Ok(_) => {}
                Err(e) if expired(&e) => {
                    self.finish(&mut guard, Some(REASON_SLOW));
                    return Err(Unsettled::Expired);
                }
                Err(e) => {
                    let why = reason(true, &e);
                    self.finish(&mut guard, Some(why));
                    return Err(Unsettled::Refused(why));
                }
            }
        }
    }

    /// Ends a session that will not be handshaken, counted under `reason`.
    pub fn refuse(&self, reason: &'static str) {
        let mut guard = lock(&self.session);
        self.finish(&mut guard, Some(reason));
    }

    /// Ends this end's sending: `close_notify`, then the socket's write half. Reads go on until the
    /// peer ends its own.
    pub fn close_write(&self) {
        let mut guard = lock(&self.session);
        if let Some(stream) = guard.as_mut() {
            stream.send_close_notify();
            let _ = stream.flush();
        }
        let _ = self.socket.shutdown(Shutdown::Write);
    }

    /// The handshake, completed now rather than when a request needs it, answering what it took in
    /// microseconds.
    ///
    /// `None` when this connection has none to complete: a session whose configuration refused to
    /// start, or one whose handshake failed, which closes the connection the way the read/write path
    /// would. A session that already handshook answers what that earlier handshake took.
    pub fn handshake(&self) -> Option<u64> {
        let began = Instant::now();
        let mut guard = lock(&self.session);
        let stream = guard.as_mut()?;
        if !stream.is_handshaking() {
            return Some(lock(&self.took).unwrap_or(0));
        }
        loop {
            match stream.complete_io() {
                // Neither read nor wrote, and still handshaking: the peer is gone.
                Ok((0, 0)) if stream.is_handshaking() => {
                    self.finish(&mut guard, Some(REASON_GONE));
                    return None;
                }
                Ok(_) if !stream.is_handshaking() => {
                    let took = began.elapsed().as_micros() as u64;
                    self.settled(stream, took);
                    return Some(took);
                }
                Ok(_) => {}
                Err(e) => {
                    let refused = (!expired(&e)).then(|| reason(true, &e));
                    self.finish(&mut guard, refused);
                    return None;
                }
            }
        }
    }

    /// The transport deadline every later operation on this session runs under.
    pub fn deadline(&self, timeout: Duration) {
        let _ = self.socket.set_read_timeout(Some(timeout));
        let _ = self.socket.set_write_timeout(Some(timeout));
    }

    /// Up to `max` decrypted bytes: `None` on deadline, empty once the peer or session is gone.
    pub fn read(&self, max: usize) -> Option<Vec<u8>> {
        let began = Instant::now();
        let mut guard = lock(&self.session);
        // An ending, never a deadline: `None` would have the caller wait on a dead connection.
        let Some(stream) = guard.as_mut() else {
            return Some(Vec::new());
        };
        let handshaking = stream.is_handshaking();
        let mut buffer = vec![0u8; max];
        match stream.read(&mut buffer) {
            Ok(0) => {
                self.completed(began, handshaking, stream);
                self.finish(&mut guard, None);
                Some(Vec::new())
            }
            Ok(n) => {
                self.completed(began, handshaking, stream);
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
        let began = Instant::now();
        let mut guard = lock(&self.session);
        let Some(stream) = guard.as_mut() else {
            return 0;
        };
        let handshaking = stream.is_handshaking();
        match stream.write_all(payload).and_then(|()| stream.flush()) {
            Ok(()) => {
                self.completed(began, handshaking, stream);
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

    /// A handshake that has just finished; `began` is when the operation that drove it started, so
    /// the number is the handshake's own and not the wait before it.
    fn completed(&self, began: Instant, was_handshaking: bool, stream: &Stream) {
        if was_handshaking && !stream.is_handshaking() {
            self.settled(stream, began.elapsed().as_micros() as u64);
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
pub const REASON_SLOW: &str = "the handshake did not complete before its deadline";
pub const REASON_INJECTED: &str = "plaintext was waiting when the handshake was to begin";
pub const REASON_NAME: &str =
    "the name is neither a DNS name nor an IP address, so no certificate can be verified as it";
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
fn err_configured(name: &str, error: &TlsError) -> Diagnostic {
    Diagnostic::error(
        codes::TLS_CREDENTIAL_INVALID,
        format!("the credential `{name}` could not be configured: {error}"),
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
        format!("{ignored} certificate(s) to trust could not be parsed"),
    )
    .note(TRUSTED)
}

#[cold]
fn err_untrusted(path: &Path, why: String) -> Diagnostic {
    Diagnostic::error(
        codes::TLS_CREDENTIAL_INVALID,
        format!("`{}` is not a certificate to trust: {why}", path.display()),
    )
    .note(TRUSTED)
    .note("roots are loaded before anything runs, so this is refused rather than discovered on the first handshake")
}

#[cold]
fn err_required_unknown(name: &str) -> Diagnostic {
    Diagnostic::error(
        codes::TLS_CREDENTIAL_UNKNOWN,
        format!("`--mtls {name}` names a credential this run was not given"),
    )
    .note(format!(
        "pass `--tls {name}=CERT.pem,KEY.pem` beside it: `--mtls` makes that credential's listener require a client certificate"
    ))
}

#[cold]
fn err_required_untrusted(name: &str) -> Diagnostic {
    Diagnostic::error(
        codes::TLS_CREDENTIAL_INVALID,
        format!("`--mtls {name}` requires a client certificate, and nothing verifies one"),
    )
    .note("pass `--trust CERT.pem` naming the certificates a client's must be issued by")
}

const TRUSTED: &str = "`--trust CERT.pem` and `PLY_TRUST` name PEM files of certificates, each a root `net.connect_tls` may accept";
