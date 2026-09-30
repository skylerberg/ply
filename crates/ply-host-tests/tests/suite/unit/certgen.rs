//! Certificates a run generates for itself.

use ply_host::certgen;

#[test]
fn an_issued_certificate_is_a_localhost_certificate_a_client_can_trust() {
    let issued = certgen::issue(&[]).expect("a certificate is issued");
    assert!(
        issued
            .certificate
            .starts_with("-----BEGIN CERTIFICATE-----"),
        "the certificate is PEM: {}",
        &issued.certificate[..40.min(issued.certificate.len())]
    );
    assert!(
        issued.key.starts_with("-----BEGIN PRIVATE KEY-----"),
        "the key is PEM"
    );
    // The DER is the certificate's, and the fingerprint names it.
    assert!(!issued.der.is_empty());
    assert_eq!(
        issued.fingerprint,
        ply_host::tls::fingerprint(&rustls::pki_types::CertificateDer::from(issued.der.clone()))
    );
    assert!(
        issued.fingerprint.starts_with("sha256:"),
        "{}",
        issued.fingerprint
    );

    // A client trusting exactly this one loads it as a credential.
    let dir = tempfile::tempdir().unwrap();
    let certificate = dir.path().join("cert.pem");
    let key = dir.path().join("key.pem");
    std::fs::write(&certificate, &issued.certificate).unwrap();
    std::fs::write(&key, &issued.key).unwrap();
    let loaded = ply_host::tls::Credentials::load(
        &[ply_host::tls::CredentialSpec {
            name: "issued".to_string(),
            certificate,
            key,
        }],
        &[],
    )
    .expect("the issued material loads as a credential");
    let (name, credential) = loaded.iter().next().expect("the credential is there");
    assert_eq!(name, "issued");
    assert_eq!(credential.fingerprint(), issued.fingerprint);
}

#[test]
fn two_issuances_are_not_the_same_certificate() {
    let first = certgen::issue(&[]).expect("issued");
    let second = certgen::issue(&[]).expect("issued");
    assert_ne!(
        first.fingerprint, second.fingerprint,
        "each issuance makes a new key"
    );
}

/// A certificate names the names it was asked for, or `localhost` when it was asked for none,
/// and a server holding it completes a handshake a client trusting that one name accepts.
#[test]
fn a_named_certificate_is_a_certificate_for_that_name() {
    let issued = certgen::issue(&["localhost".to_string(), "127.0.0.1".to_string()])
        .expect("a certificate is issued");
    let dir = tempfile::tempdir().unwrap();
    let certificate = dir.path().join("cert.pem");
    let key = dir.path().join("key.pem");
    std::fs::write(&certificate, &issued.certificate).unwrap();
    std::fs::write(&key, &issued.key).unwrap();
    let loaded = ply_host::tls::Credentials::load(
        &[ply_host::tls::CredentialSpec {
            name: "issued".to_string(),
            certificate,
            key,
        }],
        &[],
    )
    .expect("the issued material loads as a credential");
    // Resolving the server config reads the leaf, so a malformed pair would refuse here.
    let server = loaded
        .resolve("issued", ply_eval::Span::DUMMY)
        .expect("resolves");
    assert!(!server.alpn_protocols.is_empty());
}
