use crate::tls;

pub struct Issued {
    pub certificate: String,
    pub key: String,
    pub der: Vec<u8>,
    pub fingerprint: String,
}

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
