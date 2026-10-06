//! What the `secret_*` and `crypto_*` builtins compute: the operations that read a credential, so
//! that no Ply value ever holds one opened.
//!
//! The arithmetic is `ring`'s, `curve25519-dalek`'s (X25519 under a private key the caller names)
//! and `p256`'s (ECDH under a named private key, and ECDSA with RFC 6979's nonce), each a function of
//! its arguments alone: nothing here draws a random number. XChaCha20-Poly1305 is `ring`'s
//! ChaCha20-Poly1305 under the subkey [`hchacha20`] makes.

use crate::value::{Fields, Value, type_error};
use crate::{Diagnostic, Span, Symbol, codes};
use p256::ecdsa::signature::{Signer, Verifier};
use p256::elliptic_curve::sec1::ToSec1Point;
use ring::signature::KeyPair;
use std::sync::Arc;
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

/// A credential's bytes: a `Secret` of bytes, or of a string as its UTF-8.
fn opened<'a>(v: &'a Value, span: Span, what: &str) -> Result<&'a [u8], Diagnostic> {
    match v {
        Value::Secret(held) => match &**held {
            Value::Bytes(b) => Ok(b),
            Value::Str(s) => Ok(s.as_bytes()),
            _ => Err(type_error(span, what, "a Secret of bytes", v)),
        },
        _ => Err(type_error(span, what, "Secret", v)),
    }
}

fn sealed(bytes: &[u8]) -> Value {
    Value::secret(Value::bytes(bytes))
}

fn some(v: Value) -> Value {
    Value::ctor("Some", vec![v])
}

fn none() -> Value {
    Value::ctor("None", Vec::new())
}

fn option(v: Option<Value>) -> Value {
    v.map_or_else(none, some)
}

#[cold]
fn misused(span: Span, what: &str, why: String) -> Diagnostic {
    Diagnostic::error(codes::RUNTIME_ERROR, format!("{what}: {why}"))
        .primary(span, "called here")
        .note("`std.crypto` and `std.hash` name what the runtime takes; they are the callers these builtins are for")
}

pub(crate) fn of_bytes(v: &Value, span: Span) -> Result<Value, Diagnostic> {
    v.as_bytes(span, "`secret_of_bytes`")?;
    Ok(Value::secret(v.clone()))
}

pub(crate) fn utf8(v: &Value, span: Span) -> Result<Value, Diagnostic> {
    Ok(sealed(opened(v, span, "`secret_bytes`")?))
}

pub(crate) fn len(v: &Value, span: Span) -> Result<Value, Diagnostic> {
    Ok(Value::Int(opened(v, span, "`secret_len`")?.len() as i64))
}

pub(crate) fn concat(a: &Value, b: &Value, span: Span) -> Result<Value, Diagnostic> {
    let a = opened(a, span, "`secret_concat`")?;
    let b = opened(b, span, "`secret_concat`")?;
    let mut out = Zeroizing::new(Vec::with_capacity(a.len() + b.len()));
    out.extend_from_slice(a);
    out.extend_from_slice(b);
    Ok(sealed(&out))
}

// --- Text encodings of a credential ---------------------------------------------------------

/// All ones when `lo <= c <= hi` and zero otherwise, by arithmetic alone: a decoder that branched
/// on a character, or looked it up in a table, would tell a cache what the credential holds.
fn within(c: u8, lo: u8, hi: u8) -> i32 {
    let c = i32::from(c);
    ((i32::from(lo) - 1 - c) & (c - i32::from(hi) - 1)) >> 31
}

/// A symbol's value under `ranges`, each `(lo, hi, value of lo)`, and all ones when it has one.
fn symbol(c: u8, ranges: &[(u8, u8, u8)]) -> (u32, i32) {
    let mut value = 0;
    let mut known = 0;
    for (lo, hi, base) in ranges {
        let here = within(c, *lo, *hi);
        value |= here & (i32::from(c) - i32::from(*lo) + i32::from(*base));
        known |= here;
    }
    (value as u32, known)
}

struct Encoding {
    ranges: &'static [(u8, u8, u8)],
    bits: u32,
    /// The symbols a padded text is filled to a multiple of.
    quantum: usize,
    padded: bool,
}

const HEX: &[(u8, u8, u8)] = &[(b'0', b'9', 0), (b'a', b'f', 10)];
const BASE32: &[(u8, u8, u8)] = &[(b'A', b'Z', 0), (b'2', b'7', 26)];
const BASE64: &[(u8, u8, u8)] = &[
    (b'A', b'Z', 0),
    (b'a', b'z', 26),
    (b'0', b'9', 52),
    (b'+', b'+', 62),
    (b'/', b'/', 63),
];
const BASE64URL: &[(u8, u8, u8)] = &[
    (b'A', b'Z', 0),
    (b'a', b'z', 26),
    (b'0', b'9', 52),
    (b'-', b'-', 62),
    (b'_', b'_', 63),
];

/// The bytes `text` is the canonical encoding of: what `std.radix` reads at the same alphabet.
/// Its steps follow the text's length and where its padding starts, never a symbol's value.
fn decoded(e: &Encoding, text: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    let body = if e.padded {
        if !text.len().is_multiple_of(e.quantum) {
            return None;
        }
        let fill = text
            .iter()
            .rev()
            .take(e.quantum - 1)
            .take_while(|c| **c == b'=')
            .count();
        let body = text.len() - fill;
        if fill != (e.quantum - body % e.quantum) % e.quantum {
            return None;
        }
        body
    } else {
        text.len()
    };
    let spare = (body as u32 % e.quantum as u32 * e.bits) % 8;
    if spare >= e.bits {
        return None;
    }
    let mut out = Zeroizing::new(Vec::with_capacity(body * e.bits as usize / 8));
    let (mut held, mut count, mut known) = (0u32, 0u32, -1i32);
    for c in &text[..body] {
        let (value, here) = symbol(*c, e.ranges);
        known &= here;
        held = (held << e.bits) | value;
        count += e.bits;
        if count >= 8 {
            count -= 8;
            out.push((held >> count) as u8);
        }
        held &= (1 << count) - 1;
    }
    (known == -1 && held == 0).then_some(out)
}

/// A base32 text as a person typed it, made the one the unpadded decoder reads, as
/// `std.base32.normalize` makes one.
fn typed(text: &[u8]) -> Zeroizing<Vec<u8>> {
    let mut out: Zeroizing<Vec<u8>> = Zeroizing::new(
        text.iter()
            .filter(|c| !matches!(**c, b' ' | b'\t' | b'-'))
            .map(u8::to_ascii_uppercase)
            .collect(),
    );
    while out.last() == Some(&b'=') {
        out.pop();
    }
    out
}

pub(crate) fn decode(encoding: &Value, text: &Value, span: Span) -> Result<Value, Diagnostic> {
    let what = "`secret_decode`";
    let name = encoding.as_str(span, what)?;
    let text = opened(text, span, what)?;
    let coded = |ranges, bits, quantum, padded| Encoding {
        ranges,
        bits,
        quantum,
        padded,
    };
    let out = match name {
        "hex" => decoded(&coded(HEX, 4, 2, false), text),
        "base64" => decoded(&coded(BASE64, 6, 4, true), text),
        "base64url" => decoded(&coded(BASE64URL, 6, 4, false), text),
        "base32" => decoded(&coded(BASE32, 5, 8, true), text),
        "base32-typed" => decoded(&coded(BASE32, 5, 8, false), &typed(text)),
        other => {
            return Err(misused(
                span,
                what,
                format!("`{other}` is no encoding it reads"),
            ));
        }
    };
    Ok(option(out.map(|bytes| sealed(&bytes))))
}

// --- Keyed hashing ----------------------------------------------------------------------------

/// The HMAC the runtime computes over the hash a `std.hash.Hash` names.
fn hmac_named(hash: &str) -> Option<ring::hmac::Algorithm> {
    match hash {
        "SHA-1" => Some(ring::hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY),
        "SHA-256" => Some(ring::hmac::HMAC_SHA256),
        "SHA-384" => Some(ring::hmac::HMAC_SHA384),
        "SHA-512" => Some(ring::hmac::HMAC_SHA512),
        _ => None,
    }
}

pub(crate) fn hmac(
    hash: &Value,
    key: &Value,
    message: &Value,
    span: Span,
) -> Result<Value, Diagnostic> {
    let what = "`secret_hmac`";
    let algorithm = hmac_named(hash.as_str(span, what)?);
    let key = opened(key, span, what)?;
    let message = message.as_bytes(span, what)?;
    Ok(option(algorithm.map(|algorithm| {
        Value::bytes(ring::hmac::sign(
            &ring::hmac::Key::new(algorithm, key),
            message,
        ))
    })))
}

pub(crate) fn hkdf_extract(
    hash: &Value,
    salt: &Value,
    key: &Value,
    span: Span,
) -> Result<Value, Diagnostic> {
    let what = "`secret_hkdf_extract`";
    let algorithm = hmac_named(hash.as_str(span, what)?);
    let salt = salt.as_bytes(span, what)?;
    let key = opened(key, span, what)?;
    Ok(option(algorithm.map(|algorithm| {
        sealed(ring::hmac::sign(&ring::hmac::Key::new(algorithm, salt), key).as_ref())
    })))
}

struct Length(usize);

impl ring::hkdf::KeyType for Length {
    fn len(&self) -> usize {
        self.0
    }
}

pub(crate) fn hkdf_expand(
    hash: &Value,
    key: &Value,
    info: &Value,
    length: &Value,
    span: Span,
) -> Result<Value, Diagnostic> {
    let what = "`secret_hkdf_expand`";
    let algorithm = match hash.as_str(span, what)? {
        "SHA-1" => Some(ring::hkdf::HKDF_SHA1_FOR_LEGACY_USE_ONLY),
        "SHA-256" => Some(ring::hkdf::HKDF_SHA256),
        "SHA-384" => Some(ring::hkdf::HKDF_SHA384),
        "SHA-512" => Some(ring::hkdf::HKDF_SHA512),
        _ => None,
    };
    let key = opened(key, span, what)?;
    let info = info.as_bytes(span, what)?;
    let length = usize::try_from(length.as_int(span, what)?).ok();
    let derived = algorithm.zip(length).and_then(|(algorithm, length)| {
        let mut out = Zeroizing::new(vec![0; length]);
        ring::hkdf::Prk::new_less_safe(algorithm, key)
            .expand(&[&info[..]], Length(length))
            .and_then(|okm| okm.fill(&mut out))
            .ok()
            .map(|()| sealed(&out))
    });
    Ok(option(derived))
}

/// A derived key past this is a mistake in a length, not a key.
const MAX_DERIVED: usize = 1 << 20;

pub(crate) fn pbkdf2(args: &[Value], span: Span) -> Result<Value, Diagnostic> {
    let what = "`secret_pbkdf2`";
    let algorithm = match args[0].as_str(span, what)? {
        "SHA-1" => Some(ring::pbkdf2::PBKDF2_HMAC_SHA1),
        "SHA-256" => Some(ring::pbkdf2::PBKDF2_HMAC_SHA256),
        "SHA-384" => Some(ring::pbkdf2::PBKDF2_HMAC_SHA384),
        "SHA-512" => Some(ring::pbkdf2::PBKDF2_HMAC_SHA512),
        _ => None,
    };
    let password = opened(&args[1], span, what)?;
    let salt = args[2].as_bytes(span, what)?;
    let iterations = u32::try_from(args[3].as_int(span, what)?)
        .ok()
        .and_then(std::num::NonZeroU32::new);
    let length = usize::try_from(args[4].as_int(span, what)?)
        .ok()
        .filter(|n| (1..=MAX_DERIVED).contains(n));
    let derived = algorithm
        .zip(iterations)
        .zip(length)
        .map(|((algorithm, iterations), length)| {
            let mut out = Zeroizing::new(vec![0; length]);
            ring::pbkdf2::derive(algorithm, iterations, salt, password, &mut out);
            sealed(&out)
        });
    Ok(option(derived))
}

// --- Authenticated encryption -----------------------------------------------------------------

/// HChaCha20: the subkey XChaCha20 runs ChaCha20 under, out of a key and the first sixteen bytes of
/// a twenty-four byte nonce. Additions, rotations and exclusive-ors over the whole state, so its
/// time is no function of either.
pub fn hchacha20(key: &[u8; 32], nonce: &[u8; 16]) -> [u8; 32] {
    fn quarter(s: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
        s[a] = s[a].wrapping_add(s[b]);
        s[d] = (s[d] ^ s[a]).rotate_left(16);
        s[c] = s[c].wrapping_add(s[d]);
        s[b] = (s[b] ^ s[c]).rotate_left(12);
        s[a] = s[a].wrapping_add(s[b]);
        s[d] = (s[d] ^ s[a]).rotate_left(8);
        s[c] = s[c].wrapping_add(s[d]);
        s[b] = (s[b] ^ s[c]).rotate_left(7);
    }
    let word = |b: &[u8]| u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    let mut s = [0u32; 16];
    s[..4].copy_from_slice(&[0x6170_7865, 0x3320_646e, 0x7962_2d32, 0x6b20_6574]);
    for (i, chunk) in key.chunks_exact(4).enumerate() {
        s[4 + i] = word(chunk);
    }
    for (i, chunk) in nonce.chunks_exact(4).enumerate() {
        s[12 + i] = word(chunk);
    }
    for _ in 0..10 {
        quarter(&mut s, 0, 4, 8, 12);
        quarter(&mut s, 1, 5, 9, 13);
        quarter(&mut s, 2, 6, 10, 14);
        quarter(&mut s, 3, 7, 11, 15);
        quarter(&mut s, 0, 5, 10, 15);
        quarter(&mut s, 1, 6, 11, 12);
        quarter(&mut s, 2, 7, 8, 13);
        quarter(&mut s, 3, 4, 9, 14);
    }
    let mut out = [0u8; 32];
    for (i, w) in s[..4].iter().chain(&s[12..]).enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&w.to_le_bytes());
    }
    zeroize::Zeroize::zeroize(&mut s);
    out
}

/// A cipher's key and the nonce `ring` takes for it.
fn keyed(
    cipher: &str,
    key: &[u8],
    nonce: &[u8],
    span: Span,
    what: &str,
) -> Result<(ring::aead::LessSafeKey, ring::aead::Nonce), Diagnostic> {
    let (algorithm, nonce_len) = match cipher {
        "aes-256-gcm" => (&ring::aead::AES_256_GCM, 12),
        "chacha20-poly1305" => (&ring::aead::CHACHA20_POLY1305, 12),
        "xchacha20-poly1305" => (&ring::aead::CHACHA20_POLY1305, 24),
        other => {
            return Err(misused(
                span,
                what,
                format!("`{other}` is no cipher it has"),
            ));
        }
    };
    let key: &[u8; 32] = key.try_into().map_err(|_| {
        misused(
            span,
            what,
            format!("a key is 32 bytes, and this one is {}", key.len()),
        )
    })?;
    if nonce.len() != nonce_len {
        return Err(misused(
            span,
            what,
            format!(
                "a `{cipher}` nonce is {nonce_len} bytes, and this one is {}",
                nonce.len()
            ),
        ));
    }
    let bound = |key: &[u8], nonce: [u8; 12]| {
        let key =
            ring::aead::UnboundKey::new(algorithm, key).expect("a key of the cipher's length");
        (
            ring::aead::LessSafeKey::new(key),
            ring::aead::Nonce::assume_unique_for_key(nonce),
        )
    };
    Ok(if nonce_len == 24 {
        let subkey = Zeroizing::new(hchacha20(
            key,
            nonce[..16].try_into().expect("sixteen of twenty-four"),
        ));
        let mut short = [0u8; 12];
        short[4..].copy_from_slice(&nonce[16..]);
        bound(&subkey[..], short)
    } else {
        bound(key, nonce.try_into().expect("twelve bytes"))
    })
}

pub(crate) fn seal(args: &[Value], span: Span) -> Result<Value, Diagnostic> {
    let what = "`crypto_seal`";
    let (key, nonce) = keyed(
        args[0].as_str(span, what)?,
        opened(&args[1], span, what)?,
        args[2].as_bytes(span, what)?,
        span,
        what,
    )?;
    let associated = args[3].as_bytes(span, what)?;
    let mut text = args[4].as_bytes(span, what)?.to_vec();
    key.seal_in_place_append_tag(nonce, ring::aead::Aad::from(&associated[..]), &mut text)
        .map_err(|_| {
            misused(
                span,
                what,
                "the message is longer than the cipher seals under one nonce".to_string(),
            )
        })?;
    Ok(Value::bytes(text))
}

pub(crate) fn open(args: &[Value], span: Span) -> Result<Value, Diagnostic> {
    let what = "`crypto_open`";
    let (key, nonce) = keyed(
        args[0].as_str(span, what)?,
        opened(&args[1], span, what)?,
        args[2].as_bytes(span, what)?,
        span,
        what,
    )?;
    let associated = args[3].as_bytes(span, what)?;
    let mut text = Zeroizing::new(args[4].as_bytes(span, what)?.to_vec());
    let plain = key
        .open_in_place(nonce, ring::aead::Aad::from(&associated[..]), &mut text)
        .ok()
        .map(|plain| Value::bytes(&*plain));
    Ok(option(plain))
}

// --- Keys, signatures and agreement -------------------------------------------------------------

fn rsa_key(der: &[u8]) -> Option<ring::rsa::KeyPair> {
    ring::rsa::KeyPair::from_pkcs8(der)
        .or_else(|_| ring::rsa::KeyPair::from_der(der))
        .ok()
}

pub(crate) fn public(scheme: &Value, key: &Value, span: Span) -> Result<Value, Diagnostic> {
    let what = "`crypto_public`";
    let scheme = scheme.as_str(span, what)?;
    let key = opened(key, span, what)?;
    let out = match scheme {
        "ed25519" => ring::signature::Ed25519KeyPair::from_seed_unchecked(key)
            .ok()
            .map(|pair| Value::bytes(pair.public_key())),
        "x25519" => <[u8; 32]>::try_from(key).ok().map(|k| {
            Value::bytes(curve25519_dalek::MontgomeryPoint::mul_base_clamped(k).to_bytes())
        }),
        "p256" => p256_key(key).map(|k| Value::bytes(k.public_key().to_sec1_point(false))),
        "rsa" => rsa_key(key).map(|pair| Value::bytes(pair.public())),
        other => return Err(misused(span, what, format!("`{other}` is no key it has"))),
    };
    Ok(option(out))
}

/// A P-256 private key: thirty-two bytes, big-endian, of a scalar from one to the order less one.
fn p256_key(key: &[u8]) -> Option<p256::SecretKey> {
    <&[u8; 32]>::try_from(key)
        .ok()
        .and_then(|k| p256::SecretKey::from_slice(k).ok())
}

fn rsa_signing(scheme: &str) -> Option<&'static dyn ring::signature::RsaEncoding> {
    match scheme {
        "rsa-pkcs1-sha256" => Some(&ring::signature::RSA_PKCS1_SHA256),
        "rsa-pkcs1-sha384" => Some(&ring::signature::RSA_PKCS1_SHA384),
        "rsa-pkcs1-sha512" => Some(&ring::signature::RSA_PKCS1_SHA512),
        _ => None,
    }
}

pub(crate) fn sign(
    scheme: &Value,
    key: &Value,
    message: &Value,
    span: Span,
) -> Result<Value, Diagnostic> {
    let what = "`crypto_sign`";
    let scheme = scheme.as_str(span, what)?;
    let key = opened(key, span, what)?;
    let message = message.as_bytes(span, what)?;
    let out = match scheme {
        "ed25519" => ring::signature::Ed25519KeyPair::from_seed_unchecked(key)
            .ok()
            .map(|pair| Value::bytes(pair.sign(message))),
        "ecdsa-p256-sha256" => p256_key(key).map(|k| {
            let signature: p256::ecdsa::Signature = p256::ecdsa::SigningKey::from(k).sign(message);
            Value::bytes(signature.to_bytes())
        }),
        other => match rsa_signing(other) {
            // PKCS #1 v1.5 pads with no random byte, so the generator is never read.
            Some(padding) => rsa_key(key).and_then(|pair| {
                let mut signature = vec![0; pair.public().modulus_len()];
                pair.sign(
                    padding,
                    &ring::rand::SystemRandom::new(),
                    message,
                    &mut signature,
                )
                .ok()
                .map(|()| Value::bytes(signature))
            }),
            None => {
                return Err(misused(
                    span,
                    what,
                    format!("`{other}` is no signature it makes"),
                ));
            }
        },
    };
    Ok(option(out))
}

/// What `ring` checks whole: RSA, and the ECDSA pairs a certificate is signed under beside
/// P-256 with SHA-256, each over a signature in DER.
fn ring_verifying(scheme: &str) -> Option<&'static dyn ring::signature::VerificationAlgorithm> {
    match scheme {
        "ecdsa-p256-sha384" => Some(&ring::signature::ECDSA_P256_SHA384_ASN1),
        "ecdsa-p384-sha256" => Some(&ring::signature::ECDSA_P384_SHA256_ASN1),
        "ecdsa-p384-sha384" => Some(&ring::signature::ECDSA_P384_SHA384_ASN1),
        "rsa-pkcs1-sha256" => Some(&ring::signature::RSA_PKCS1_2048_8192_SHA256),
        "rsa-pkcs1-sha384" => Some(&ring::signature::RSA_PKCS1_2048_8192_SHA384),
        "rsa-pkcs1-sha512" => Some(&ring::signature::RSA_PKCS1_2048_8192_SHA512),
        "rsa-pss-sha256" => Some(&ring::signature::RSA_PSS_2048_8192_SHA256),
        "rsa-pss-sha384" => Some(&ring::signature::RSA_PSS_2048_8192_SHA384),
        "rsa-pss-sha512" => Some(&ring::signature::RSA_PSS_2048_8192_SHA512),
        _ => None,
    }
}

pub(crate) fn verify(args: &[Value], span: Span) -> Result<Value, Diagnostic> {
    let what = "`crypto_verify`";
    let scheme = args[0].as_str(span, what)?;
    let public = args[1].as_bytes(span, what)?;
    let message = args[2].as_bytes(span, what)?;
    let signature = args[3].as_bytes(span, what)?;
    let holds = match scheme {
        "ed25519" => {
            ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, &public[..])
                .verify(message, signature)
                .is_ok()
        }
        "ecdsa-p256-sha256" => p256::ecdsa::VerifyingKey::from_sec1_bytes(public)
            .ok()
            .zip(p256::ecdsa::Signature::from_slice(signature).ok())
            .is_some_and(|(key, signature)| key.verify(message, &signature).is_ok()),
        other => match ring_verifying(other) {
            Some(parameters) => ring::signature::UnparsedPublicKey::new(parameters, &public[..])
                .verify(message, signature)
                .is_ok(),
            None => {
                return Err(misused(
                    span,
                    what,
                    format!("`{other}` is no signature it checks"),
                ));
            }
        },
    };
    Ok(Value::Bool(holds))
}

pub(crate) fn agree(
    scheme: &Value,
    key: &Value,
    peer: &Value,
    span: Span,
) -> Result<Value, Diagnostic> {
    let what = "`crypto_agree`";
    let scheme = scheme.as_str(span, what)?;
    let key = opened(key, span, what)?;
    let peer = peer.as_bytes(span, what)?;
    let out = match scheme {
        "x25519" => <[u8; 32]>::try_from(key)
            .ok()
            .zip(<[u8; 32]>::try_from(&peer[..]).ok())
            .and_then(|(key, peer)| {
                let shared =
                    Zeroizing::new(curve25519_dalek::MontgomeryPoint(peer).mul_clamped(key).0);
                // A point of small order sends every key to zero: a secret anyone could have made.
                (!bool::from(shared.ct_eq(&[0u8; 32]))).then(|| sealed(&shared[..]))
            }),
        "p256" => p256_key(key)
            .zip(p256::PublicKey::from_sec1_bytes(peer).ok())
            .map(|(key, peer)| sealed(key.diffie_hellman(&peer).raw_secret_bytes())),
        other => {
            return Err(misused(
                span,
                what,
                format!("`{other}` is no agreement it has"),
            ));
        }
    };
    Ok(option(out))
}

/// The private key a DER document holds, and the scheme it is a key of: PKCS #8 for any of them, a
/// SEC 1 `ECPrivateKey` on P-256, or a PKCS #1 `RSAPrivateKey`. An RSA key is answered as the
/// document it came in, which is what `rsa` signs under.
pub(crate) fn private_key(der: &Value, span: Span) -> Result<Value, Diagnostic> {
    use p256::pkcs8::{
        ObjectIdentifier, PrivateKeyInfoRef, der::Decode, der::asn1::OctetStringRef,
    };
    const ED25519: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.101.112");
    const X25519: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.101.110");
    const RSA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.1");
    const EC: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.2.1");
    let der = opened(der, span, "`secret_private_key`")?;
    let curve25519 = |inner: &[u8]| {
        <&OctetStringRef>::from_der(inner)
            .ok()
            .filter(|seed| seed.as_bytes().len() == 32)
            .map(|seed| sealed(seed.as_bytes()))
    };
    let found = match PrivateKeyInfoRef::try_from(der) {
        Ok(info) if info.algorithm.oid == ED25519 && info.algorithm.parameters.is_none() => {
            curve25519(info.private_key.as_bytes()).map(|key| ("ed25519", key))
        }
        Ok(info) if info.algorithm.oid == X25519 && info.algorithm.parameters.is_none() => {
            curve25519(info.private_key.as_bytes()).map(|key| ("x25519", key))
        }
        Ok(info) if info.algorithm.oid == EC => p256::SecretKey::try_from(info)
            .ok()
            .map(|key| ("p256", sealed(&key.to_bytes()))),
        Ok(info) if info.algorithm.oid == RSA => rsa_key(der).map(|_| ("rsa", sealed(der))),
        Ok(_) => None,
        Err(_) => p256::SecretKey::from_sec1_der(der)
            .ok()
            .map(|key| ("p256", sealed(&key.to_bytes())))
            .or_else(|| rsa_key(der).map(|_| ("rsa", sealed(der)))),
    };
    Ok(option(found.map(|(scheme, key)| {
        Value::Record(Arc::new(Fields::from_iter([
            (Symbol::new("key"), key),
            (Symbol::new("scheme"), Value::str(scheme)),
        ])))
    })))
}
