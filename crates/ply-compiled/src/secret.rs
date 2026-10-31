use crate::c::{MachineNative, runtime_native};
use crate::emitted::SecretHandle;
use crate::heap;
use crate::native::{Addr, I64, TakenBytes};
use crate::rt::{Ctx, borrowed};
use ply_core::crypto::{self, EncodingSupport};
use ply_core::{Diagnostic, Span, codes};
use zeroize::Zeroizing;

pub fn natives() -> Vec<MachineNative> {
    vec![
        runtime_native(
            "?secret_of",
            rt_secret_of as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
        runtime_native(
            "?secret_verify",
            rt_secret_verify as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
        runtime_native(
            "?secret_equal",
            rt_secret_equal as unsafe extern "C" fn(_, _, _) -> _,
        ),
        runtime_native(
            "?secret_len",
            rt_secret_len as unsafe extern "C" fn(_, _) -> _,
        ),
        runtime_native(
            "?secret_bytes",
            rt_secret_bytes as unsafe extern "C" fn(_, _) -> _,
        ),
        runtime_native(
            "?secret_concat",
            rt_secret_concat as unsafe extern "C" fn(_, _, _) -> _,
        ),
        runtime_native(
            "?secret_encodes",
            rt_secret_encodes as unsafe extern "C" fn(_, _, _) -> _,
        ),
        runtime_native(
            "?secret_encoded",
            rt_secret_encoded as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
        runtime_native(
            "?secret_decodes",
            rt_secret_decodes as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
        runtime_native(
            "?secret_decoded",
            rt_secret_decoded as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
        runtime_native(
            "?secret_key_scheme",
            rt_secret_key_scheme as unsafe extern "C" fn(_, _) -> _,
        ),
        runtime_native(
            "?secret_key",
            rt_secret_key as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
        runtime_native(
            "?secret_rsa_jwk",
            rt_secret_rsa_jwk as unsafe extern "C" fn(_, _, _, _, _, _) -> _,
        ),
        runtime_native(
            "?secret_hmac",
            rt_secret_hmac as unsafe extern "C" fn(_, _, _, _, _, _) -> _,
        ),
        runtime_native(
            "?secret_hkdf_extract",
            rt_secret_hkdf_extract as unsafe extern "C" fn(_, _, _, _, _, _) -> _,
        ),
        runtime_native(
            "?secret_hkdf_expands",
            rt_secret_hkdf_expands as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
        runtime_native(
            "?secret_hkdf_expand",
            rt_secret_hkdf_expand as unsafe extern "C" fn(_, _, _, _, _, _, _) -> _,
        ),
        runtime_native(
            "?secret_pbkdf2",
            rt_secret_pbkdf2 as unsafe extern "C" fn(_, _, _, _, _, _, _, _) -> _,
        ),
        runtime_native(
            "crypto_nonce_length",
            rt_crypto_nonce_length as unsafe extern "C" fn(_, _, _) -> _,
        ),
        runtime_native(
            "crypto_sealed",
            rt_crypto_sealed as unsafe extern "C" fn(_, _, _, _, _, _, _, _, _, _) -> _,
        ),
        runtime_native(
            "crypto_opened",
            rt_crypto_opened as unsafe extern "C" fn(_, _, _, _, _, _, _, _, _, _) -> _,
        ),
        runtime_native(
            "crypto_public_key",
            rt_crypto_public_key as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
        runtime_native(
            "crypto_signature",
            rt_crypto_signature as unsafe extern "C" fn(_, _, _, _, _, _) -> _,
        ),
        runtime_native(
            "crypto_verified",
            rt_crypto_verified as unsafe extern "C" fn(_, _, _, _, _, _, _, _, _) -> _,
        ),
        runtime_native(
            "crypto_agrees",
            rt_crypto_agrees as unsafe extern "C" fn(_, _, _) -> _,
        ),
        runtime_native(
            "crypto_agreed",
            rt_crypto_agreed as unsafe extern "C" fn(_, _, _, _, _, _) -> _,
        ),
    ]
}

pub struct Sealed {
    text: bool,
    bytes: Zeroizing<Vec<u8>>,
}

impl Sealed {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn is_text(&self) -> bool {
        self.text
    }
}

impl heap::Native for Sealed {
    fn release(&mut self, _ctx: *mut Ctx) {}
}

pub(crate) fn sealed(c: &mut Ctx, text: bool, bytes: Zeroizing<Vec<u8>>) -> SecretHandle {
    SecretHandle::of(c.heap.native(Box::new(Sealed { text, bytes })))
}

fn sealed_bytes(c: &mut Ctx, bytes: Zeroizing<Vec<u8>>) -> SecretHandle {
    sealed(c, false, bytes)
}

pub(crate) fn held<'a>(c: &mut Ctx, s: &SecretHandle) -> Option<&'a Sealed> {
    match unsafe { heap::native_of::<Sealed>(s.word()) } {
        Some(held) => Some(held),
        None => {
            c.fail(err_secret("a credential that is no secret"));
            None
        }
    }
}

#[cold]
#[inline(never)]
fn err_secret(reason: &str) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("a secret handed to the runtime: {reason}"),
    )
    .primary(Span::DUMMY, "in compiled code")
    .note("this is Ply's fault, not the program's")
}

unsafe fn name<'a>(p: Addr, n: I64) -> &'a str {
    std::str::from_utf8(unsafe { borrowed(p, n) }).unwrap_or("")
}

fn found(bytes: Option<Vec<u8>>) -> Vec<u8> {
    match bytes {
        Some(bytes) => [&[1u8][..], &bytes].concat(),
        None => vec![0],
    }
}

fn optional_bytes_handle(c: &mut Ctx, bytes: Option<Vec<u8>>) -> I64 {
    match bytes {
        Some(bytes) => c.give(bytes),
        None => I64(-1),
    }
}

pub unsafe extern "C" fn rt_secret_of(ctx: *mut Ctx, p: Addr, n: I64, text: I64) -> SecretHandle {
    let c = unsafe { &mut *ctx };
    let bytes = unsafe { borrowed(p, n) };
    if text.0 != 0 && c.text(bytes).is_none() {
        return SecretHandle::of(0);
    }
    sealed(c, text.0 != 0, Zeroizing::new(bytes.to_vec()))
}

pub unsafe extern "C" fn rt_secret_verify(
    ctx: *mut Ctx,
    stored: SecretHandle,
    p: Addr,
    n: I64,
) -> I64 {
    let c = unsafe { &mut *ctx };
    let Some(held) = held(c, &stored) else {
        return I64(0);
    };
    let Some(candidate) = c.text(unsafe { borrowed(p, n) }) else {
        return I64(0);
    };
    let same = held.text && ply_core::constant_time_eq(&held.bytes, candidate.as_bytes());
    I64(i64::from(same))
}

pub unsafe extern "C" fn rt_secret_equal(ctx: *mut Ctx, a: SecretHandle, b: SecretHandle) -> I64 {
    let c = unsafe { &mut *ctx };
    let (Some(x), Some(y)) = (held(c, &a), held(c, &b)) else {
        return I64(0);
    };
    let same = x.text == y.text && ply_core::constant_time_eq(&x.bytes, &y.bytes);
    I64(i64::from(same))
}

pub unsafe extern "C" fn rt_secret_len(ctx: *mut Ctx, s: SecretHandle) -> I64 {
    let c = unsafe { &mut *ctx };
    I64(held(c, &s).map_or(0, |held| held.bytes.len() as i64))
}

pub unsafe extern "C" fn rt_secret_bytes(ctx: *mut Ctx, s: SecretHandle) -> SecretHandle {
    let c = unsafe { &mut *ctx };
    let Some(held) = held(c, &s) else {
        return SecretHandle::of(0);
    };
    sealed_bytes(c, Zeroizing::new(held.bytes.to_vec()))
}

pub unsafe extern "C" fn rt_secret_concat(
    ctx: *mut Ctx,
    a: SecretHandle,
    b: SecretHandle,
) -> SecretHandle {
    let c = unsafe { &mut *ctx };
    let (Some(x), Some(y)) = (held(c, &a), held(c, &b)) else {
        return SecretHandle::of(0);
    };
    let mut out = Zeroizing::new(Vec::with_capacity(x.bytes.len() + y.bytes.len()));
    out.extend_from_slice(&x.bytes);
    out.extend_from_slice(&y.bytes);
    sealed_bytes(c, out)
}

pub unsafe extern "C" fn rt_secret_encodes(_ctx: *mut Ctx, p: Addr, n: I64) -> I64 {
    I64(i64::from(
        crypto::encoding(unsafe { name(p, n) }) == EncodingSupport::WritesAndReads,
    ))
}

pub unsafe extern "C" fn rt_secret_encoded(
    ctx: *mut Ctx,
    p: Addr,
    n: I64,
    bytes: SecretHandle,
) -> SecretHandle {
    let c = unsafe { &mut *ctx };
    let Some(held) = held(c, &bytes) else {
        return SecretHandle::of(0);
    };
    let text = crypto::encode(unsafe { name(p, n) }, &held.bytes).unwrap_or_default();
    sealed(c, true, text)
}

pub unsafe extern "C" fn rt_secret_decodes(
    ctx: *mut Ctx,
    p: Addr,
    n: I64,
    text: SecretHandle,
) -> I64 {
    let c = unsafe { &mut *ctx };
    let name = unsafe { name(p, n) };
    if crypto::encoding(name) == EncodingSupport::Neither {
        return I64(-2);
    }
    let Some(held) = held(c, &text) else {
        return I64(-1);
    };
    I64(if crypto::decode(name, &held.bytes).is_some() {
        0
    } else {
        -1
    })
}

pub unsafe extern "C" fn rt_secret_decoded(
    ctx: *mut Ctx,
    p: Addr,
    n: I64,
    text: SecretHandle,
) -> SecretHandle {
    let c = unsafe { &mut *ctx };
    let Some(held) = held(c, &text) else {
        return SecretHandle::of(0);
    };
    let bytes = crypto::decode(unsafe { name(p, n) }, &held.bytes).unwrap_or_default();
    sealed_bytes(c, bytes)
}

pub unsafe extern "C" fn rt_secret_key_scheme(ctx: *mut Ctx, der: SecretHandle) -> I64 {
    let c = unsafe { &mut *ctx };
    let Some(held) = held(c, &der) else {
        return I64(-1);
    };
    let scheme = crypto::private_key(&held.bytes).map(|(scheme, _)| scheme.as_bytes().to_vec());
    optional_bytes_handle(c, scheme)
}

pub unsafe extern "C" fn rt_secret_key(
    ctx: *mut Ctx,
    der: SecretHandle,
    p: Addr,
    n: I64,
) -> SecretHandle {
    let c = unsafe { &mut *ctx };
    let Some(held) = held(c, &der) else {
        return SecretHandle::of(0);
    };
    let key = crypto::private_key_as(&held.bytes, unsafe { name(p, n) }).unwrap_or_default();
    sealed_bytes(c, key)
}

pub unsafe extern "C" fn rt_secret_rsa_jwk(
    ctx: *mut Ctx,
    np: Addr,
    nn: I64,
    ep: Addr,
    en: I64,
    texts: SecretHandle,
) -> SecretHandle {
    let c = unsafe { &mut *ctx };
    let Some(held) = held(c, &texts) else {
        return SecretHandle::of(0);
    };
    let mut parts = held.bytes.split(|b| *b == b'.');
    let leading = parts.next();
    let numbers: Vec<&[u8]> = parts.collect();
    let document = match <[&[u8]; 6]>::try_from(numbers) {
        Ok(private) if leading == Some(&[][..]) => {
            let (modulus, exponent) = unsafe { (borrowed(np, nn), borrowed(ep, en)) };
            crypto::rsa_jwk(modulus, exponent, private)
        }
        _ => None,
    };
    sealed_bytes(c, document.unwrap_or_default())
}

pub unsafe extern "C" fn rt_secret_hmac(
    ctx: *mut Ctx,
    hp: Addr,
    hn: I64,
    key: SecretHandle,
    mp: Addr,
    mn: I64,
) -> I64 {
    let c = unsafe { &mut *ctx };
    let Some(held) = held(c, &key) else {
        return I64(-1);
    };
    let message = unsafe { borrowed(mp, mn) };
    let tag = crypto::hmac(unsafe { name(hp, hn) }, &held.bytes, message);
    optional_bytes_handle(c, tag)
}

pub unsafe extern "C" fn rt_secret_hkdf_extract(
    ctx: *mut Ctx,
    hp: Addr,
    hn: I64,
    sp: Addr,
    sn: I64,
    key: SecretHandle,
) -> SecretHandle {
    let c = unsafe { &mut *ctx };
    let Some(held) = held(c, &key) else {
        return SecretHandle::of(0);
    };
    let salt = unsafe { borrowed(sp, sn) };
    let prk = crypto::hkdf_extract(unsafe { name(hp, hn) }, salt, &held.bytes);
    sealed_bytes(c, prk.unwrap_or_default())
}

pub unsafe extern "C" fn rt_secret_hkdf_expands(
    _ctx: *mut Ctx,
    hp: Addr,
    hn: I64,
    length: I64,
) -> I64 {
    let digest = crypto::digest_length(unsafe { name(hp, hn) }) as i64;
    let expands = digest > 0 && (0..=255 * digest).contains(&length.0);
    I64(if expands { 0 } else { -1 })
}

pub unsafe extern "C" fn rt_secret_hkdf_expand(
    ctx: *mut Ctx,
    hp: Addr,
    hn: I64,
    key: SecretHandle,
    ip: Addr,
    info_byte_count: I64,
    length: I64,
) -> SecretHandle {
    let c = unsafe { &mut *ctx };
    let Some(held) = held(c, &key) else {
        return SecretHandle::of(0);
    };
    let info = unsafe { borrowed(ip, info_byte_count) };
    let okm = crypto::hkdf_expand(unsafe { name(hp, hn) }, &held.bytes, info, length.0);
    sealed_bytes(c, okm.unwrap_or_default())
}

#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn rt_secret_pbkdf2(
    ctx: *mut Ctx,
    hp: Addr,
    hn: I64,
    password: SecretHandle,
    sp: Addr,
    sn: I64,
    iterations: I64,
    length: I64,
) -> SecretHandle {
    let c = unsafe { &mut *ctx };
    let Some(held) = held(c, &password) else {
        return SecretHandle::of(0);
    };
    let salt = unsafe { borrowed(sp, sn) };
    let hash = unsafe { name(hp, hn) };
    let key = crypto::pbkdf2(hash, &held.bytes, salt, iterations.0, length.0);
    sealed_bytes(c, key.unwrap_or_default())
}

pub unsafe extern "C" fn rt_crypto_nonce_length(_ctx: *mut Ctx, p: Addr, n: I64) -> I64 {
    I64(crypto::nonce_length(unsafe { name(p, n) }).map_or(0, |n| n as i64))
}

#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn rt_crypto_sealed(
    ctx: *mut Ctx,
    cp: Addr,
    cn: I64,
    key: SecretHandle,
    np: Addr,
    nn: I64,
    ap: Addr,
    an: I64,
    tp: Addr,
    tn: I64,
) -> TakenBytes {
    let c = unsafe { &mut *ctx };
    let Some(held) = held(c, &key) else {
        return TakenBytes(0);
    };
    let (nonce, associated, plaintext) =
        unsafe { (borrowed(np, nn), borrowed(ap, an), borrowed(tp, tn)) };
    let cipher = unsafe { name(cp, cn) };
    let sealed = crypto::seal(cipher, &held.bytes, nonce, associated, plaintext);
    c.given(sealed.unwrap_or_default())
}

#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn rt_crypto_opened(
    ctx: *mut Ctx,
    cp: Addr,
    cn: I64,
    key: SecretHandle,
    np: Addr,
    nn: I64,
    ap: Addr,
    an: I64,
    sp: Addr,
    sn: I64,
) -> TakenBytes {
    let c = unsafe { &mut *ctx };
    let Some(held) = held(c, &key) else {
        return TakenBytes(0);
    };
    let (nonce, associated, sealed) =
        unsafe { (borrowed(np, nn), borrowed(ap, an), borrowed(sp, sn)) };
    let cipher = unsafe { name(cp, cn) };
    let opened = crypto::open(cipher, &held.bytes, nonce, associated, sealed);
    c.given(opened.map_or_else(|_| Vec::new(), found))
}

pub unsafe extern "C" fn rt_crypto_public_key(
    ctx: *mut Ctx,
    p: Addr,
    n: I64,
    key: SecretHandle,
) -> TakenBytes {
    let c = unsafe { &mut *ctx };
    let Some(held) = held(c, &key) else {
        return TakenBytes(0);
    };
    let public = crypto::public(unsafe { name(p, n) }, &held.bytes);
    c.given(public.map_or_else(|_| Vec::new(), found))
}

pub unsafe extern "C" fn rt_crypto_signature(
    ctx: *mut Ctx,
    p: Addr,
    n: I64,
    key: SecretHandle,
    mp: Addr,
    mn: I64,
) -> TakenBytes {
    let c = unsafe { &mut *ctx };
    let Some(held) = held(c, &key) else {
        return TakenBytes(0);
    };
    let message = unsafe { borrowed(mp, mn) };
    let signature = crypto::sign(unsafe { name(p, n) }, &held.bytes, message);
    c.given(signature.map_or_else(|_| Vec::new(), found))
}

#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn rt_crypto_verified(
    _ctx: *mut Ctx,
    p: Addr,
    n: I64,
    kp: Addr,
    kn: I64,
    mp: Addr,
    mn: I64,
    sp: Addr,
    sn: I64,
) -> I64 {
    let (public, message, signature) =
        unsafe { (borrowed(kp, kn), borrowed(mp, mn), borrowed(sp, sn)) };
    match crypto::verify(unsafe { name(p, n) }, public, message, signature) {
        Ok(holds) => I64(i64::from(holds)),
        Err(_) => I64(-1),
    }
}

pub unsafe extern "C" fn rt_crypto_agrees(_ctx: *mut Ctx, p: Addr, n: I64) -> I64 {
    I64(i64::from(crypto::agrees(unsafe { name(p, n) })))
}

pub unsafe extern "C" fn rt_crypto_agreed(
    ctx: *mut Ctx,
    p: Addr,
    n: I64,
    key: SecretHandle,
    pp: Addr,
    pn: I64,
) -> SecretHandle {
    let c = unsafe { &mut *ctx };
    let Some(held) = held(c, &key) else {
        return SecretHandle::of(0);
    };
    let peer = unsafe { borrowed(pp, pn) };
    let shared = crypto::agree(unsafe { name(p, n) }, &held.bytes, peer);
    sealed_bytes(c, shared.ok().flatten().unwrap_or_default())
}
