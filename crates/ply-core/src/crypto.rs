//! What the operations that read a credential compute, over bytes, so that no Ply value ever holds
//! one opened: a credential's bytes in, and plain bytes, a credential's bytes or a verdict out.
//!
//! The arithmetic is `ring`'s, `curve25519-dalek`'s (X25519 under a private key the caller names)
//! and RustCrypto's (ECDH on P-256 under a named private key, and ECDSA with RFC 6979's nonce on
//! P-256, P-384, P-521 and secp256k1), each a function of its arguments alone: nothing here draws a
//! random number, and an RSA-PSS salt is made of the key and the message. XChaCha20-Poly1305 is
//! `ring`'s ChaCha20-Poly1305 under the subkey [`hchacha20`] makes.

use crate::{Diagnostic, Span, codes};
use p256::ecdsa::signature::{Signer, Verifier};
use p256::elliptic_curve::sec1::ToSec1Point;
// `ring` takes a generator only of its own types; this one returns the bytes it is handed.
use ring::signature::KeyPair;
#[allow(deprecated)]
use ring::test::rand::FixedSliceRandom;
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

/// The runtime error of the operation `what` handed what it takes no part of, `reason` saying what.
#[cold]
pub fn misuse(span: Span, what: &str, reason: String) -> Diagnostic {
    Diagnostic::error(codes::RUNTIME_ERROR, format!("{what}: {reason}"))
        .primary(span, "called here")
        .note("`std.crypto` and `std.hash` name what the runtime takes; they are the callers these builtins are for")
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

/// Base32 as a person types it, which [`typed`] makes the unpadded text this reads.
const BASE32_TYPED: Encoding = Encoding {
    ranges: BASE32,
    bits: 5,
    quantum: 8,
    padded: false,
};

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

/// What the encoding a credential's text is named in does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Encodes {
    /// Written and read back: `"hex"` (lower case), `"base64"`, `"base64url"` (unpadded) or
    /// `"base32"`.
    WritesAndReads,
    /// Read only: `"base32-typed"`, base32 as a person types it, whatever its case, spaces and
    /// dashes, its padding optional.
    Reads,
    Neither,
}

/// What the encoding named `name` does.
pub fn encoding(name: &str) -> Encodes {
    match (name, named(name)) {
        (_, Some(_)) => Encodes::WritesAndReads,
        ("base32-typed", None) => Encodes::Reads,
        _ => Encodes::Neither,
    }
}

/// The bytes the text `text` is, in the encoding named `name`; `None` for a text it does not write,
/// and for an encoding that reads none.
pub fn decode(name: &str, text: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    match (name, named(name)) {
        (_, Some(e)) => decoded(&e, text),
        ("base32-typed", None) => decoded(&BASE32_TYPED, &typed(text)),
        _ => None,
    }
}

/// The text of `bytes` in the encoding named `name`, which [`decode`] reads back; `None` for an
/// encoding that writes none.
pub fn encode(name: &str, bytes: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    named(name).map(|e| encoded(&e, bytes))
}

/// How a credential crosses the edge of a program inside plain bytes, which is the only way one
/// does: plain bytes before it, the credential in an encoding, and plain bytes after it. A host
/// operation that writes a credential out writes [`Framing::framed`], and one that reads a
/// credential in returns [`Framing::unframed`].
#[derive(Clone, Copy)]
pub struct Framing {
    /// `None` is `"raw"`: the bytes themselves.
    encoding: Option<Encoding>,
}

impl Framing {
    /// The encoding `name` names: `"raw"`, or one `Secret::encode` writes. Any other is the runtime
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
        let fits = constant_time_eq(head, before) & constant_time_eq(tail, after);
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

/// How many bytes a digest of the hash named `hash` is, of those HMAC, HKDF and PBKDF2 run over
/// here: `"SHA-1"`, `"SHA-256"`, `"SHA-384"` and `"SHA-512"`; 0 for any other.
pub fn digest_length(hash: &str) -> usize {
    hmac_named(hash).map_or(0, |a| a.digest_algorithm().output_len())
}

/// HMAC under `key` of `message`, over the hash named `hash`; `None` for a hash it has not.
pub fn hmac(hash: &str, key: &[u8], message: &[u8]) -> Option<Vec<u8>> {
    hmac_named(hash)
        .map(|algorithm| ring::hmac::sign(&ring::hmac::Key::new(algorithm, key), message))
        .map(|tag| tag.as_ref().to_vec())
}

/// HKDF's first step, RFC 5869: the key `salt` and `key` make; `None` for a hash it has not.
pub fn hkdf_extract(hash: &str, salt: &[u8], key: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    hmac_named(hash).map(|algorithm| {
        let prk = ring::hmac::sign(&ring::hmac::Key::new(algorithm, salt), key);
        Zeroizing::new(prk.as_ref().to_vec())
    })
}

struct Length(usize);

impl ring::hkdf::KeyType for Length {
    fn len(&self) -> usize {
        self.0
    }
}

/// HKDF's second step: `length` bytes of the key `key` expands to under `info`; `None` for a hash it
/// has not, and for a length below zero or past 255 digests.
pub fn hkdf_expand(hash: &str, key: &[u8], info: &[u8], length: i64) -> Option<Zeroizing<Vec<u8>>> {
    let algorithm = match hash {
        "SHA-1" => ring::hkdf::HKDF_SHA1_FOR_LEGACY_USE_ONLY,
        "SHA-256" => ring::hkdf::HKDF_SHA256,
        "SHA-384" => ring::hkdf::HKDF_SHA384,
        "SHA-512" => ring::hkdf::HKDF_SHA512,
        _ => return None,
    };
    let length = usize::try_from(length).ok()?;
    let most = 255 * algorithm.hmac_algorithm().digest_algorithm().output_len();
    if length > most {
        return None;
    }
    let mut out = Zeroizing::new(vec![0; length]);
    ring::hkdf::Prk::new_less_safe(algorithm, key)
        .expand(&[info], Length(length))
        .and_then(|okm| okm.fill(&mut out))
        .ok()
        .map(|()| out)
}

/// A derived key past this is a mistake in a length, not a key.
const MAX_DERIVED: usize = 1 << 20;

/// PBKDF2, RFC 8018, over HMAC over the hash named `hash`: `length` bytes; `None` for a hash it has
/// not, an iteration count below one or past `u32`, and a length below one or past `2^20`.
pub fn pbkdf2(
    hash: &str,
    password: &[u8],
    salt: &[u8],
    iterations: i64,
    length: i64,
) -> Option<Zeroizing<Vec<u8>>> {
    let algorithm = match hash {
        "SHA-1" => ring::pbkdf2::PBKDF2_HMAC_SHA1,
        "SHA-256" => ring::pbkdf2::PBKDF2_HMAC_SHA256,
        "SHA-384" => ring::pbkdf2::PBKDF2_HMAC_SHA384,
        "SHA-512" => ring::pbkdf2::PBKDF2_HMAC_SHA512,
        _ => return None,
    };
    let iterations = u32::try_from(iterations)
        .ok()
        .and_then(std::num::NonZeroU32::new)?;
    let length = usize::try_from(length)
        .ok()
        .filter(|n| (1..=MAX_DERIVED).contains(n))?;
    let mut out = Zeroizing::new(vec![0; length]);
    ring::pbkdf2::derive(algorithm, iterations, salt, password, &mut out);
    Some(out)
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

/// The cipher named `cipher`, and how many bytes its nonce is.
fn cipher_named(cipher: &str) -> Option<(&'static ring::aead::Algorithm, usize)> {
    match cipher {
        "aes-256-gcm" => Some((&ring::aead::AES_256_GCM, 12)),
        "chacha20-poly1305" => Some((&ring::aead::CHACHA20_POLY1305, 12)),
        "xchacha20-poly1305" => Some((&ring::aead::CHACHA20_POLY1305, 24)),
        _ => None,
    }
}

/// How many bytes a nonce of the cipher named `cipher` is: `"aes-256-gcm"` and
/// `"chacha20-poly1305"` take 12, and `"xchacha20-poly1305"` 24; `None` for any other cipher.
pub fn nonce_length(cipher: &str) -> Option<usize> {
    cipher_named(cipher).map(|(_, nonce)| nonce)
}

/// A cipher's key and the nonce `ring` takes for it; `Err` saying why where the cipher, the key or
/// the nonce is none it seals under.
fn keyed(
    cipher: &str,
    key: &[u8],
    nonce: &[u8],
) -> Result<(ring::aead::LessSafeKey, ring::aead::Nonce), String> {
    let Some((algorithm, nonce_len)) = cipher_named(cipher) else {
        return Err(format!("`{cipher}` is no cipher it has"));
    };
    let key: &[u8; 32] = key
        .try_into()
        .map_err(|_| format!("a key is 32 bytes, and this one is {}", key.len()))?;
    if nonce.len() != nonce_len {
        return Err(format!(
            "a `{cipher}` nonce is {nonce_len} bytes, and this one is {}",
            nonce.len()
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

/// `plaintext` sealed under a 32-byte key and a nonce of the cipher named `cipher`, bound to
/// `associated`: the ciphertext and then its 16-byte tag. `Err` saying why for a cipher, a key or a
/// nonce it does not seal under, and for a message longer than the cipher seals under one nonce.
pub fn seal(
    cipher: &str,
    key: &[u8],
    nonce: &[u8],
    associated: &[u8],
    plaintext: &[u8],
) -> Result<Vec<u8>, String> {
    let (key, nonce) = keyed(cipher, key, nonce)?;
    let mut text = plaintext.to_vec();
    key.seal_in_place_append_tag(nonce, ring::aead::Aad::from(associated), &mut text)
        .map_err(|_| "the message is longer than the cipher seals under one nonce".to_string())?;
    Ok(text)
}

/// What [`seal`] sealed; `None` where the key, the nonce, `associated` or a byte of `sealed` is not
/// what it was sealed with. `Err` as [`seal`] returns one for its cipher, key and nonce.
pub fn open(
    cipher: &str,
    key: &[u8],
    nonce: &[u8],
    associated: &[u8],
    sealed: &[u8],
) -> Result<Option<Vec<u8>>, String> {
    let (key, nonce) = keyed(cipher, key, nonce)?;
    let mut text = Zeroizing::new(sealed.to_vec());
    Ok(key
        .open_in_place(nonce, ring::aead::Aad::from(associated), &mut text)
        .ok()
        .map(|plain| plain.to_vec()))
}

// --- Keys, signatures and agreement -------------------------------------------------------------

fn rsa_key(der: &[u8]) -> Option<ring::rsa::KeyPair> {
    ring::rsa::KeyPair::from_pkcs8(der)
        .or_else(|_| ring::rsa::KeyPair::from_der(der))
        .ok()
}

/// A private key's public key under the scheme named `scheme`, as `std.internal.crypto`'s
/// `crypto_public` says; `None` for a key that is none of its scheme, and `Err` saying why for a
/// scheme it has not.
pub fn public(scheme: &str, key: &[u8]) -> Result<Option<Vec<u8>>, String> {
    Ok(match scheme {
        "ed25519" => ring::signature::Ed25519KeyPair::from_seed_unchecked(key)
            .ok()
            .map(|pair| pair.public_key().as_ref().to_vec()),
        "x25519" => <[u8; 32]>::try_from(key).ok().map(|k| {
            curve25519_dalek::MontgomeryPoint::mul_base_clamped(k)
                .to_bytes()
                .to_vec()
        }),
        "p256" => p256_key(key).map(|k| k.public_key().to_sec1_point(false).as_ref().to_vec()),
        "p384" => p384_key(key).map(|k| k.public_key().to_sec1_point(false).as_ref().to_vec()),
        "p521" => p521_key(key).map(|k| k.public_key().to_sec1_point(false).as_ref().to_vec()),
        "k256" => k256_key(key).map(|k| k.public_key().to_sec1_point(false).as_ref().to_vec()),
        "rsa" => rsa_key(key).map(|pair| pair.public().as_ref().to_vec()),
        other => return Err(format!("`{other}` is no key it has")),
    })
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

/// A signature over `message` under `key` and the scheme named `scheme`, as `std.internal.crypto`'s
/// `crypto_sign` says; `None` for a key that is none of its scheme, and `Err` saying why for a
/// scheme it has not.
pub fn sign(scheme: &str, key: &[u8], message: &[u8]) -> Result<Option<Vec<u8>>, String> {
    Ok(match scheme {
        "ed25519" => ring::signature::Ed25519KeyPair::from_seed_unchecked(key)
            .ok()
            .map(|pair| pair.sign(message).as_ref().to_vec()),
        "ecdsa-p256-sha256" => p256_key(key).map(|k| {
            let signature: p256::ecdsa::Signature = p256::ecdsa::SigningKey::from(k).sign(message);
            signature.to_bytes().to_vec()
        }),
        "ecdsa-p384-sha384" => p384_key(key).map(|k| {
            let signature: p384::ecdsa::Signature = p384::ecdsa::SigningKey::from(k).sign(message);
            signature.to_bytes().to_vec()
        }),
        "ecdsa-p521-sha512" => p521_key(key).map(|k| {
            let signature: p521::ecdsa::Signature = p521::ecdsa::SigningKey::from(k).sign(message);
            signature.to_bytes().to_vec()
        }),
        "ecdsa-k256-sha256" => k256_key(key).map(|k| {
            let signature: k256::ecdsa::Signature = k256::ecdsa::SigningKey::from(k).sign(message);
            signature.to_bytes().to_vec()
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
                .map(|()| signature)
            }),
            None => return Err(format!("`{other}` is no signature it makes")),
        },
    })
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

/// Whether `signature` is `message` signed by the key `public` is, under the scheme named `scheme`,
/// as `std.internal.crypto`'s `crypto_verify` says; `Err` saying why for a scheme it has not.
pub fn verify(
    scheme: &str,
    public: &[u8],
    message: &[u8],
    signature: &[u8],
) -> Result<bool, String> {
    Ok(match scheme {
        "ed25519" => ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, public)
            .verify(message, signature)
            .is_ok(),
        "ecdsa-p256-sha256" => p256::ecdsa::VerifyingKey::from_sec1_bytes(public)
            .ok()
            .zip(p256::ecdsa::Signature::from_slice(signature).ok())
            .is_some_and(|(key, signature)| key.verify(message, &signature).is_ok()),
        "ecdsa-p521-sha512" => p521::ecdsa::VerifyingKey::from_sec1_bytes(public)
            .ok()
            .zip(p521::ecdsa::Signature::from_slice(signature).ok())
            .is_some_and(|(key, signature)| key.verify(message, &signature).is_ok()),
        // `k256` rejects a high `s` as Bitcoin does; JOSE and X.509 take it as its low twin.
        "ecdsa-k256-sha256" => k256::ecdsa::VerifyingKey::from_sec1_bytes(public)
            .ok()
            .zip(k256::ecdsa::Signature::from_slice(signature).ok())
            .is_some_and(|(key, signature)| key.verify(message, &signature.normalize_s()).is_ok()),
        other => match ring_verifying(other) {
            Some(parameters) => ring::signature::UnparsedPublicKey::new(parameters, public)
                .verify(message, signature)
                .is_ok(),
            None => return Err(format!("`{other}` is no signature it checks")),
        },
    })
}

/// Whether [`agree`] has the agreement named `scheme`: `"x25519"` or `"p256"`.
pub fn agrees(scheme: &str) -> bool {
    matches!(scheme, "x25519" | "p256")
}

/// The secret `key` and the public key `peer` share under the agreement named `scheme`, as
/// `std.internal.crypto`'s `crypto_agree` says; `None` for a key or a peer that is none of its
/// scheme and for an X25519 result of all zeros, and `Err` saying why for a scheme it has not.
pub fn agree(scheme: &str, key: &[u8], peer: &[u8]) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
    Ok(match scheme {
        "x25519" => <[u8; 32]>::try_from(key)
            .ok()
            .zip(<[u8; 32]>::try_from(peer).ok())
            .and_then(|(key, peer)| {
                let shared =
                    Zeroizing::new(curve25519_dalek::MontgomeryPoint(peer).mul_clamped(key).0);
                // A point of small order sends every key to zero: a secret anyone could have made.
                (!bool::from(shared.ct_eq(&[0u8; 32]))).then(|| Zeroizing::new(shared.to_vec()))
            }),
        "p256" => p256_key(key)
            .zip(p256::PublicKey::from_sec1_bytes(peer).ok())
            .map(|(key, peer)| {
                Zeroizing::new(key.diffie_hellman(&peer).raw_secret_bytes().to_vec())
            }),
        other => return Err(format!("`{other}` is no agreement it has")),
    })
}

/// The scheme and scalar of an elliptic-curve key, by the curve its document names: P-256, P-384,
/// P-521 or secp256k1, in PKCS #8 or in SEC 1.
fn curve_key(der: &[u8]) -> Option<(&'static str, Zeroizing<Vec<u8>>)> {
    use p256::pkcs8::DecodePrivateKey;
    let scalar = |bytes: &[u8]| Zeroizing::new(bytes.to_vec());
    p256::SecretKey::from_pkcs8_der(der)
        .or_else(|_| p256::SecretKey::from_sec1_der(der))
        .map(|key| ("p256", scalar(&key.to_bytes())))
        .or_else(|_| {
            p384::SecretKey::from_pkcs8_der(der)
                .or_else(|_| p384::SecretKey::from_sec1_der(der))
                .map(|key| ("p384", scalar(&key.to_bytes())))
        })
        .or_else(|_| {
            p521::SecretKey::from_pkcs8_der(der)
                .or_else(|_| p521::SecretKey::from_sec1_der(der))
                .map(|key| ("p521", scalar(&key.to_bytes())))
        })
        .or_else(|_| {
            k256::SecretKey::from_pkcs8_der(der)
                .or_else(|_| k256::SecretKey::from_sec1_der(der))
                .map(|key| ("k256", scalar(&key.to_bytes())))
        })
        .ok()
}

/// The private key a DER document holds, and the scheme it is a key of: PKCS #8 for any of them, a
/// SEC 1 `ECPrivateKey` on an elliptic curve, or a PKCS #1 `RSAPrivateKey`. An RSA key is returned
/// as the document it came in, which is what `rsa` signs under.
pub fn private_key(der: &[u8]) -> Option<(&'static str, Zeroizing<Vec<u8>>)> {
    use p256::pkcs8::{
        ObjectIdentifier, PrivateKeyInfoRef, der::Decode, der::asn1::OctetStringRef,
    };
    const ED25519: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.101.112");
    const X25519: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.101.110");
    const RSA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.1");
    const EC: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.2.1");
    let curve25519 = |inner: &[u8]| {
        <&OctetStringRef>::from_der(inner)
            .ok()
            .filter(|seed| seed.as_bytes().len() == 32)
            .map(|seed| Zeroizing::new(seed.as_bytes().to_vec()))
    };
    let document = || Zeroizing::new(der.to_vec());
    match PrivateKeyInfoRef::try_from(der) {
        Ok(info) if info.algorithm.oid == ED25519 && info.algorithm.parameters.is_none() => {
            curve25519(info.private_key.as_bytes()).map(|key| ("ed25519", key))
        }
        Ok(info) if info.algorithm.oid == X25519 && info.algorithm.parameters.is_none() => {
            curve25519(info.private_key.as_bytes()).map(|key| ("x25519", key))
        }
        Ok(info) if info.algorithm.oid == EC => curve_key(der),
        Ok(info) if info.algorithm.oid == RSA => rsa_key(der).map(|_| ("rsa", document())),
        Ok(_) => None,
        Err(_) => curve_key(der).or_else(|| rsa_key(der).map(|_| ("rsa", document()))),
    }
}

/// The key [`private_key`] reads out of `der`, where it names `scheme` the scheme of that key: an
/// RSA document is returned whole without being read again, so its key is checked once, by
/// [`private_key`]. `None` where `der` holds no key of `scheme`.
pub fn private_key_as(der: &[u8], scheme: &str) -> Option<Zeroizing<Vec<u8>>> {
    if scheme == "rsa" {
        return Some(Zeroizing::new(der.to_vec()));
    }
    private_key(der)
        .filter(|(found, _)| *found == scheme)
        .map(|(_, key)| key)
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
/// bytes, and `d`, `p`, `q`, `dp`, `dq` and `qi` as their unpadded base64url texts. Each is decoded
/// into memory that is wiped, and the document is returned only where `ring` takes it as a key, so
/// numbers that disagree are no key. The lengths of the numbers are not kept secret.
pub fn rsa_jwk(n: &[u8], e: &[u8], private: [&[u8]; 6]) -> Option<Zeroizing<Vec<u8>>> {
    let unpadded = Encoding {
        ranges: BASE64URL,
        bits: 6,
        quantum: 4,
        padded: false,
    };
    let mut numbers = Vec::with_capacity(6);
    for text in private {
        numbers.push(decoded(&unpadded, text)?);
    }
    // Room for every number, a byte to pad each and its header, so neither buffer ever grows: a
    // grown buffer leaves its old bytes behind unwiped.
    let room = n.len() + e.len() + numbers.iter().map(|p| p.len()).sum::<usize>() + 9 * 8;
    let mut body = Zeroizing::new(Vec::with_capacity(room));
    integer_der(&[0], &mut body);
    integer_der(n, &mut body);
    integer_der(e, &mut body);
    for number in &numbers {
        integer_der(number, &mut body);
    }
    let mut document = Zeroizing::new(Vec::with_capacity(body.len() + 8));
    document.push(0x30);
    length_der(body.len(), &mut document);
    document.extend_from_slice(&body);
    ring::rsa::KeyPair::from_der(&document)
        .ok()
        .map(|_| document)
}

/// Byte equality whose time depends on the lengths, not on where they first differ: two of one
/// length are read to their ends whatever they hold, and two of different lengths differ at once.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && bool::from(a.ct_eq(b))
}
