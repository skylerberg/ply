//! Certificates a run generates for itself: throwaway localhost credentials for benches and
//! tests, beside the TLS code that loads the ones a run is given.

use crate::tls;

/// One generated certificate: what a server is given, and what a client trusts.
pub struct Issued {
    /// The certificate, PEM-encoded.
    pub certificate: String,
    /// Its private key, PEM-encoded.
    pub key: String,
    /// The certificate's DER bytes, which is what a client's root store wants.
    pub der: Vec<u8>,
    /// SHA-256 of the DER, lowercase hex.
    pub fingerprint: String,
}

/// A self-signed certificate for `names`, valid immediately. Nothing checked in is trusted by
/// anything, and each call makes a new key.
pub fn issue(names: &[String]) -> Result<Issued, String> {
    let names: Vec<String> = if names.is_empty() {
        vec!["localhost".to_string()]
    } else {
        names.to_vec()
    };
    let issued =
        rcgen::generate_simple_self_signed(names).map_err(|e| format!("generating: {e}"))?;
    let der = issued.cert.der().to_vec();
    Ok(Issued {
        certificate: issued.cert.pem(),
        key: issued.signing_key.serialize_pem(),
        fingerprint: tls::fingerprint(issued.cert.der()),
        der,
    })
}
