//! What the `secret_*` and `crypto_*` builtins compute: the operations that read a credential, so
//! that no Ply value ever holds one opened.
//!
//! The arithmetic is `ring`'s, `curve25519-dalek`'s (X25519 under a private key the caller names)
//! and RustCrypto's (ECDH on P-256 under a named private key, and ECDSA with RFC 6979's nonce on
//! P-256, P-384, P-521 and secp256k1), each a function of its arguments alone: nothing here draws a
//! random number, and an RSA-PSS salt is made of the key and the message. XChaCha20-Poly1305 is
//! `ring`'s ChaCha20-Poly1305 under the subkey [`hchacha20`] makes.

use crate::value::{Fields, Value, type_error};
use crate::{Diagnostic, Span, Symbol, codes};
use p256::ecdsa::signature::{Signer, Verifier};
use p256::elliptic_curve::sec1::ToSec1Point;
// `ring` takes a generator only of its own types; this one answers the bytes it is handed.
use ring::signature::KeyPair;
#[allow(deprecated)]
use ring::test::rand::FixedSliceRandom;
use std::sync::Arc;
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

/// A credential's bytes: a `Secret` of bytes, or of a string as its UTF-8.
fn opened<'a>(v: &'a Value, span: Span, what: &str) -> Result<&'a [u8], Diagnostic> {
    match v {
        Value::Secret(held) => Ok(held.bytes()),
        _ => Err(type_error(span, what, "Secret", v)),
    }
}

fn sealed(bytes: &[u8]) -> Value {
    Value::secret_bytes(bytes)
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

#[derive(Clone, Copy)]
struct Encoding {
    ranges: &'static [(u8, u8, u8)],
    bits: u32,
    /// The symbols a padded text is filled to a multiple of.
    quantum: usize,
    padded: bool,
}

/// The encodings a credential is written in and read back from by name.
fn named(name: &str) -> Option<Encoding> {
    let coded = |ranges, bits, quantum, padded| Encoding {
        ranges,
        bits,
        quantum,
        padded,
    };
    Some(match name {
        "hex" => coded(HEX, 4, 2, false),
        "base64" => coded(BASE64, 6, 4, true),
        "base64url" => coded(BASE64URL, 6, 4, false),
        "base32" => coded(BASE32, 5, 8, true),
        _ => return None,
    })
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
    // Sized once: a buffer that grew would leave its old bytes behind unwiped.
    let mut out = Zeroizing::new(Vec::with_capacity(text.len()));
    out.extend(
        text.iter()
            .filter(|c| !matches!(**c, b' ' | b'\t' | b'-'))
            .map(u8::to_ascii_uppercase),
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
    let out = match (name, named(name)) {
        (_, Some(e)) => decoded(&e, text),
        ("base32-typed", None) => decoded(
            &Encoding {
                ranges: BASE32,
                bits: 5,
                quantum: 8,
                padded: false,
            },
            &typed(text),
        ),
        (other, None) => {
            return Err(misused(
                span,
                what,
                format!("`{other}` is no encoding it reads"),
            ));
        }
    };
    Ok(option(out.map(|bytes| sealed(&bytes))))
}

/// The symbol a value is under `ranges`, by arithmetic alone, as [`symbol`] reads one back.
fn character(value: u32, ranges: &[(u8, u8, u8)]) -> u8 {
    let mut out = 0;
    for (lo, hi, base) in ranges {
        let here = within(value as u8, *base, base + (hi - lo));
        out |= here & (value as i32 - i32::from(*base) + i32::from(*lo));
    }
    out as u8
}

/// The canonical text of `bytes` in an encoding, which [`decoded`] reads back. Its steps follow the
/// length of `bytes`, never a byte's value.
fn encoded(e: &Encoding, bytes: &[u8]) -> Zeroizing<Vec<u8>> {
    let mask = (1u32 << e.bits) - 1;
    let mut out = Zeroizing::new(Vec::with_capacity(
        bytes.len() * 8 / e.bits as usize + e.quantum,
    ));
    let (mut held, mut count) = (0u32, 0u32);
    for b in bytes {
        held = (held << 8) | u32::from(*b);
        count += 8;
        while count >= e.bits {
            count -= e.bits;
            out.push(character((held >> count) & mask, e.ranges));
        }
        held &= (1 << count) - 1;
    }
    if count > 0 {
        out.push(character((held << (e.bits - count)) & mask, e.ranges));
    }
    while e.padded && !out.len().is_multiple_of(e.quantum) {
        out.push(b'=');
    }
    out
}

pub(crate) fn encode(encoding: &Value, bytes: &Value, span: Span) -> Result<Value, Diagnostic> {
    let what = "`secret_encode`";
    let name = encoding.as_str(span, what)?;
    let bytes = opened(bytes, span, what)?;
    let Some(e) = named(name) else {
        return Err(misused(
            span,
            what,
            format!("`{name}` is no encoding it writes"),
        ));
    };
    let text = encoded(&e, bytes);
    let text = std::str::from_utf8(&text).expect("an encoding writes ASCII");
    Ok(Value::secret_text(text))
}

/// How a credential crosses the edge of a program inside plain bytes, which is the only way one
/// does: plain bytes before it, the credential in an encoding, and plain bytes after it. A host
/// operation that writes a credential out writes [`Framing::framed`], and one that reads a
/// credential in answers [`Framing::unframed`].
#[derive(Clone, Copy)]
pub struct Framing {
    /// `None` is `"raw"`: the bytes themselves.
    encoding: Option<Encoding>,
}

impl Framing {
    /// The encoding `name` names: `"raw"`, or one `secret_encode` writes. Any other is the runtime
    /// error the operation `what` raises.
    pub fn named(name: &str, what: &str, span: Span) -> Result<Framing, Diagnostic> {
        if name == "raw" {
            return Ok(Framing { encoding: None });
        }
        named(name).map(|e| Framing { encoding: Some(e) }).ok_or_else(|| {
            Diagnostic::error(
                codes::RUNTIME_ERROR,
                format!("{what} knows no encoding `{name}`"),
            )
            .primary(span, "performed here")
            .note("a credential is written and read as `raw`, `hex`, `base64`, `base64url` or `base32`")
        })
    }

    /// `before`, the credential in this encoding, then `after`, in one buffer sized once and wiped
    /// when dropped. Its steps follow the lengths alone.
    pub fn framed(self, before: &[u8], secret: &[u8], after: &[u8]) -> Zeroizing<Vec<u8>> {
        let text = self.encoding.map(|e| encoded(&e, secret));
        let middle: &[u8] = text.as_deref().map_or(secret, |t| &t[..]);
        let mut out = Zeroizing::new(Vec::with_capacity(
            before.len() + middle.len() + after.len(),
        ));
        out.extend_from_slice(before);
        out.extend_from_slice(middle);
        out.extend_from_slice(after);
        out
    }

    /// The credential `held` frames, as [`Framing::framed`] wrote it: `None` where `held` does not
    /// open with `before` and close with `after`, or what lies between is not the canonical text of
    /// any bytes in this encoding. Its steps follow the lengths alone.
    pub fn unframed(self, held: &[u8], before: &[u8], after: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
        if held.len() < before.len() + after.len() {
            return None;
        }
        let (head, rest) = held.split_at(before.len());
        let (inner, tail) = rest.split_at(rest.len() - after.len());
        let fits = crate::value::constant_time_eq(head, before)
            & crate::value::constant_time_eq(tail, after);
        let read = match self.encoding {
            Some(e) => decoded(&e, inner),
            None => Some(Zeroizing::new(inner.to_vec())),
        };
        read.filter(|_| fits)
    }
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
        "p384" => p384_key(key).map(|k| Value::bytes(k.public_key().to_sec1_point(false))),
        "p521" => p521_key(key).map(|k| Value::bytes(k.public_key().to_sec1_point(false))),
        "k256" => k256_key(key).map(|k| Value::bytes(k.public_key().to_sec1_point(false))),
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

/// A P-384 private key: forty-eight bytes, as [`p256_key`] reads thirty-two.
fn p384_key(key: &[u8]) -> Option<p384::SecretKey> {
    <&[u8; 48]>::try_from(key)
        .ok()
        .and_then(|k| p384::SecretKey::from_slice(k).ok())
}

/// A P-521 private key: sixty-six bytes, the top seven bits of the first zero.
fn p521_key(key: &[u8]) -> Option<p521::SecretKey> {
    <&[u8; 66]>::try_from(key)
        .ok()
        .and_then(|k| p521::SecretKey::from_slice(k).ok())
}

/// A secp256k1 private key: thirty-two bytes, as [`p256_key`] reads them.
fn k256_key(key: &[u8]) -> Option<k256::SecretKey> {
    <&[u8; 32]>::try_from(key)
        .ok()
        .and_then(|k| k256::SecretKey::from_slice(k).ok())
}

/// How `ring` pads an RSA signature, and for PSS the hash its salt is made with.
fn rsa_signing(
    scheme: &str,
) -> Option<(
    &'static dyn ring::signature::RsaEncoding,
    Option<ring::hmac::Algorithm>,
)> {
    match scheme {
        "rsa-pkcs1-sha256" => Some((&ring::signature::RSA_PKCS1_SHA256, None)),
        "rsa-pkcs1-sha384" => Some((&ring::signature::RSA_PKCS1_SHA384, None)),
        "rsa-pkcs1-sha512" => Some((&ring::signature::RSA_PKCS1_SHA512, None)),
        "rsa-pss-sha256" => Some((
            &ring::signature::RSA_PSS_SHA256,
            Some(ring::hmac::HMAC_SHA256),
        )),
        "rsa-pss-sha384" => Some((
            &ring::signature::RSA_PSS_SHA384,
            Some(ring::hmac::HMAC_SHA384),
        )),
        "rsa-pss-sha512" => Some((
            &ring::signature::RSA_PSS_SHA512,
            Some(ring::hmac::HMAC_SHA512),
        )),
        _ => None,
    }
}

/// A PSS salt as long as the digest: HMAC, under the key's own document, of a label and the
/// message's digest. So a signature is a function of the key and the message, as RFC 6979 makes
/// an ECDSA nonce one, and nobody without the key foresees a salt (RFC 8017 §8.1 asks no more of
/// one). It is no secret: a verifier reads it back out of the signature.
fn pss_salt(hmac: ring::hmac::Algorithm, document: &[u8], message: &[u8]) -> Zeroizing<Vec<u8>> {
    let mut salted = ring::hmac::Context::with_key(&ring::hmac::Key::new(hmac, document));
    salted.update(b"ply rsa-pss salt\0");
    salted.update(ring::digest::digest(hmac.digest_algorithm(), message).as_ref());
    Zeroizing::new(salted.sign().as_ref().to_vec())
}

/// The generator `ring`'s RSA signing is handed: `bytes` as the one draw PSS makes, for its salt,
/// and never the host's. PKCS #1 v1.5 draws nothing.
#[allow(deprecated)]
fn fixed_draw(bytes: &[u8]) -> FixedSliceRandom<'_> {
    FixedSliceRandom { bytes }
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
        "ecdsa-p384-sha384" => p384_key(key).map(|k| {
            let signature: p384::ecdsa::Signature = p384::ecdsa::SigningKey::from(k).sign(message);
            Value::bytes(signature.to_bytes())
        }),
        "ecdsa-p521-sha512" => p521_key(key).map(|k| {
            let signature: p521::ecdsa::Signature = p521::ecdsa::SigningKey::from(k).sign(message);
            Value::bytes(signature.to_bytes())
        }),
        "ecdsa-k256-sha256" => k256_key(key).map(|k| {
            let signature: k256::ecdsa::Signature = k256::ecdsa::SigningKey::from(k).sign(message);
            Value::bytes(signature.to_bytes())
        }),
        other => match rsa_signing(other) {
            Some((padding, salted)) => rsa_key(key).and_then(|pair| {
                let salt = salted.map(|hmac| pss_salt(hmac, key, message));
                let mut signature = vec![0; pair.public().modulus_len()];
                pair.sign(
                    padding,
                    &fixed_draw(salt.as_deref().map_or(&[][..], |s| &s[..])),
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
        "ecdsa-p521-sha512" => p521::ecdsa::VerifyingKey::from_sec1_bytes(public)
            .ok()
            .zip(p521::ecdsa::Signature::from_slice(signature).ok())
            .is_some_and(|(key, signature)| key.verify(message, &signature).is_ok()),
        // `k256` refuses a high `s` as Bitcoin does; JOSE and X.509 take it as its low twin.
        "ecdsa-k256-sha256" => k256::ecdsa::VerifyingKey::from_sec1_bytes(public)
            .ok()
            .zip(k256::ecdsa::Signature::from_slice(signature).ok())
            .is_some_and(|(key, signature)| key.verify(message, &signature.normalize_s()).is_ok()),
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

/// The scheme and scalar of an elliptic-curve key, by the curve its document names: P-256, P-384,
/// P-521 or secp256k1, in PKCS #8 or in SEC 1.
fn curve_key(der: &[u8]) -> Option<(&'static str, Value)> {
    use p256::pkcs8::DecodePrivateKey;
    p256::SecretKey::from_pkcs8_der(der)
        .or_else(|_| p256::SecretKey::from_sec1_der(der))
        .map(|key| ("p256", sealed(&key.to_bytes())))
        .or_else(|_| {
            p384::SecretKey::from_pkcs8_der(der)
                .or_else(|_| p384::SecretKey::from_sec1_der(der))
                .map(|key| ("p384", sealed(&key.to_bytes())))
        })
        .or_else(|_| {
            p521::SecretKey::from_pkcs8_der(der)
                .or_else(|_| p521::SecretKey::from_sec1_der(der))
                .map(|key| ("p521", sealed(&key.to_bytes())))
        })
        .or_else(|_| {
            k256::SecretKey::from_pkcs8_der(der)
                .or_else(|_| k256::SecretKey::from_sec1_der(der))
                .map(|key| ("k256", sealed(&key.to_bytes())))
        })
        .ok()
}

/// The private key a DER document holds, and the scheme it is a key of: PKCS #8 for any of them, a
/// SEC 1 `ECPrivateKey` on an elliptic curve, or a PKCS #1 `RSAPrivateKey`. An RSA key is answered
/// as the document it came in, which is what `rsa` signs under.
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
        Ok(info) if info.algorithm.oid == EC => curve_key(der),
        Ok(info) if info.algorithm.oid == RSA => rsa_key(der).map(|_| ("rsa", sealed(der))),
        Ok(_) => None,
        Err(_) => curve_key(der).or_else(|| rsa_key(der).map(|_| ("rsa", sealed(der)))),
    };
    Ok(option(found.map(|(scheme, key)| {
        Value::Record(Arc::new(Fields::from_iter([
            (Symbol::new("key"), key),
            (Symbol::new("scheme"), Value::str(scheme)),
        ])))
    })))
}

fn length_der(len: usize, out: &mut Vec<u8>) {
    if len < 0x80 {
        out.push(len as u8);
    } else {
        let bytes = len.to_be_bytes();
        let skip = bytes.iter().take_while(|b| **b == 0).count();
        out.push(0x80 | (bytes.len() - skip) as u8);
        out.extend_from_slice(&bytes[skip..]);
    }
}

/// `magnitude`, an unsigned big-endian number, as a DER `INTEGER`.
fn integer_der(magnitude: &[u8], out: &mut Vec<u8>) {
    let digits = &magnitude[magnitude.iter().take_while(|b| **b == 0).count()..];
    let pad = digits.first().is_none_or(|b| b & 0x80 != 0);
    out.push(0x02);
    length_der(digits.len() + usize::from(pad), out);
    if pad {
        out.push(0);
    }
    out.extend_from_slice(digits);
}

/// The PKCS #1 `RSAPrivateKey` an RSA JSON Web Key's numbers make (RFC 7518 §6.3): `n` and `e` as
/// bytes, and a list of `d`, `p`, `q`, `dp`, `dq` and `qi` as their base64url texts, sealed. Each
/// is decoded into memory that is wiped, and the document is answered only where `ring` takes it
/// as a key, so numbers that disagree are no key. The lengths of the numbers are not kept secret.
pub(crate) fn rsa_jwk(args: &[Value], span: Span) -> Result<Value, Diagnostic> {
    let what = "`secret_rsa_jwk`";
    let unpadded = Encoding {
        ranges: BASE64URL,
        bits: 6,
        quantum: 4,
        padded: false,
    };
    let n = args[0].as_bytes(span, what)?;
    let e = args[1].as_bytes(span, what)?;
    let Value::List(texts) = &args[2] else {
        return Err(type_error(span, what, "a list of secrets", &args[2]));
    };
    if texts.len() != 6 {
        return Err(misused(
            span,
            what,
            format!(
                "an RSA key's private numbers are six, and this list holds {}",
                texts.len()
            ),
        ));
    }
    let mut private = Vec::with_capacity(6);
    for text in texts.iter() {
        match decoded(&unpadded, opened(text, span, what)?) {
            Some(number) => private.push(number),
            None => return Ok(none()),
        }
    }
    // Room for every number, a byte to pad each and its header, so neither buffer ever grows: a
    // grown buffer leaves its old bytes behind unwiped.
    let room = n.len() + e.len() + private.iter().map(|p| p.len()).sum::<usize>() + 9 * 8;
    let mut body = Zeroizing::new(Vec::with_capacity(room));
    integer_der(&[0], &mut body);
    integer_der(n, &mut body);
    integer_der(e, &mut body);
    for number in &private {
        integer_der(number, &mut body);
    }
    let mut document = Zeroizing::new(Vec::with_capacity(body.len() + 8));
    document.push(0x30);
    length_der(body.len(), &mut document);
    document.extend_from_slice(&body);
    Ok(option(
        ring::rsa::KeyPair::from_der(&document)
            .ok()
            .map(|_| sealed(&document)),
    ))
}
