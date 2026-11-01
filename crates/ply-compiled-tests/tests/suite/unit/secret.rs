use crate::bare::{helper, new_context};
use ply_compiled::c::builtin_helper_name;
use ply_compiled::heap::{self, Word};
use ply_compiled::rt::Context;
use ply_compiled::secret::Sealed;

type A1 = unsafe extern "C" fn(*mut Context, Word) -> Word;
type A2 = unsafe extern "C" fn(*mut Context, Word, Word) -> Word;
type A3 = unsafe extern "C" fn(*mut Context, Word, Word, Word) -> Word;
type A5 = unsafe extern "C" fn(*mut Context, Word, Word, Word, Word, Word) -> Word;
type A6 = unsafe extern "C" fn(*mut Context, Word, Word, Word, Word, Word, Word) -> Word;
type A7 = unsafe extern "C" fn(*mut Context, Word, Word, Word, Word, Word, Word, Word) -> Word;
type A8 =
    unsafe extern "C" fn(*mut Context, Word, Word, Word, Word, Word, Word, Word, Word) -> Word;
type A9 = unsafe extern "C" fn(
    *mut Context,
    Word,
    Word,
    Word,
    Word,
    Word,
    Word,
    Word,
    Word,
    Word,
) -> Word;
type N2 = unsafe extern "C" fn(*mut Context, Word, Word);

fn native<F: Copy>(name: &str) -> F {
    helper(&builtin_helper_name(name))
}

fn borrowed(bytes: &[u8]) -> (Word, Word) {
    (bytes.as_ptr() as Word, bytes.len() as Word)
}

pub(super) fn sealed(context: &mut Context, bytes: &[u8], text: bool) -> Word {
    let (p, n) = borrowed(bytes);
    unsafe { native::<A3>("?secret_of")(context, p, n, Word::from(text)) }
}

fn held(w: Word) -> &'static Sealed {
    unsafe { heap::native_of::<Sealed>(w) }.expect("a secret")
}

fn taken(context: &mut Context, n: Word) -> Option<Vec<u8>> {
    let mut out = vec![0u8; usize::try_from(n).ok()?];
    unsafe { native::<N2>("?taken")(context, out.as_mut_ptr() as Word, n) };
    Some(out)
}

fn hex(digits: &str) -> Vec<u8> {
    (0..digits.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&digits[i..i + 2], 16).unwrap())
        .collect()
}

fn named_secret(context: &mut Context, native_name: &str, name: &str, secret: Word) -> Word {
    let (p, n) = borrowed(name.as_bytes());
    unsafe { native::<A3>(native_name)(context, p, n, secret) }
}

fn of_name(context: &mut Context, native_name: &str, name: &str) -> Word {
    let (p, n) = borrowed(name.as_bytes());
    unsafe { native::<A2>(native_name)(context, p, n) }
}

#[test]
fn a_secret_stores_its_own_copy_of_the_bytes_and_verify_succeeds_only_for_identical_bytes() {
    let mut context = new_context();
    let verify = |context: &mut Context, s: Word, b: &[u8]| unsafe {
        let (p, n) = borrowed(b);
        native::<A3>("?secret_verify")(context, s, p, n)
    };
    let typed = b"hunter2".to_vec();
    let text = sealed(&mut context, &typed, true);
    let raw = sealed(&mut context, b"hunter2", false);
    drop(typed);
    assert_eq!(held(text).bytes(), b"hunter2");
    assert!(held(text).is_text() && !held(raw).is_text());
    assert_eq!(verify(&mut context, text, b"hunter2"), 1);
    assert_eq!(verify(&mut context, text, b"hunter3"), 0);
    assert_eq!(verify(&mut context, text, b""), 0);
    assert_eq!(verify(&mut context, raw, b"hunter2"), 0);
    let equal = |context: &mut Context, a: Word, b: Word| unsafe {
        native::<A2>("?secret_equal")(context, a, b)
    };
    let utf8 = unsafe { native::<A1>("?secret_bytes")(&mut context, text) };
    assert_eq!(equal(&mut context, utf8, raw), 1);
    assert_eq!(equal(&mut context, text, raw), 0);
    let tail = sealed(&mut context, b"!", false);
    let joined = unsafe { native::<A2>("?secret_concat")(&mut context, raw, tail) };
    assert_eq!(held(joined).bytes(), b"hunter2!");
    assert_eq!(
        unsafe { native::<A1>("?secret_len")(&mut context, joined) },
        8
    );
    assert_eq!(context.failed, 0);
}

#[test]
fn a_secret_is_encoded_to_text_and_decoded_back_using_the_encoding_named_by_the_name_argument() {
    let mut context = new_context();
    for (name, writes) in [("hex", 1), ("base32", 1), ("base32-typed", 0), ("rot13", 0)] {
        assert_eq!(
            of_name(&mut context, "?secret_encodes", name),
            writes,
            "{name}"
        );
    }
    let bytes = sealed(&mut context, b"foobar", false);
    let text = named_secret(&mut context, "?secret_encoded", "hex", bytes);
    assert!(held(text).is_text());
    assert_eq!(held(text).bytes(), b"666f6f626172");
    let unwritten = named_secret(&mut context, "?secret_encoded", "rot13", bytes);
    assert!(held(unwritten).is_text() && held(unwritten).bytes().is_empty());
    let typed = sealed(&mut context, b"mzxw 6ytb-oi", true);
    let wrong = sealed(&mut context, b"6g", true);
    let reads = |context: &mut Context, name: &str, text: Word| {
        (
            named_secret(context, "?secret_decodes", name, text),
            held(named_secret(context, "?secret_decoded", name, text))
                .bytes()
                .to_vec(),
        )
    };
    assert_eq!(reads(&mut context, "hex", text), (0, b"foobar".to_vec()));
    assert_eq!(
        reads(&mut context, "base32-typed", typed),
        (0, b"foobar".to_vec())
    );
    assert_eq!(reads(&mut context, "hex", wrong), (-1, Vec::new()));
    assert_eq!(reads(&mut context, "rot13", text), (-2, Vec::new()));
    assert_eq!(context.failed, 0);
}

fn ed25519_document(seed: &[u8; 32]) -> Vec<u8> {
    [&hex("302e020100300506032b657004220420")[..], seed].concat()
}

#[test]
fn a_private_key_is_read_out_of_its_document_by_its_scheme() {
    let mut context = new_context();
    let seed = [7u8; 32];
    let der = sealed(&mut context, &ed25519_document(&seed), false);
    let scheme = |context: &mut Context, der: Word| {
        let n = unsafe { native::<A1>("?secret_key_scheme")(context, der) };
        taken(context, n)
    };
    let key = |context: &mut Context, der: Word, scheme: &str| {
        let (p, n) = borrowed(scheme.as_bytes());
        held(unsafe { native::<A3>("?secret_key")(context, der, p, n) })
            .bytes()
            .to_vec()
    };
    assert_eq!(scheme(&mut context, der), Some(b"ed25519".to_vec()));
    assert_eq!(key(&mut context, der, "ed25519"), seed);
    assert_eq!(key(&mut context, der, "x25519"), Vec::<u8>::new());
    let junk = sealed(&mut context, b"no key", false);
    assert_eq!(scheme(&mut context, junk), None);
    assert_eq!(context.failed, 0);
}

fn integers(der: &[u8]) -> Vec<Vec<u8>> {
    fn length(der: &[u8], at: &mut usize) -> usize {
        let first = der[*at];
        *at += 1;
        if first < 0x80 {
            return usize::from(first);
        }
        let mut n = 0;
        for _ in 0..first & 0x7f {
            n = n << 8 | usize::from(der[*at]);
            *at += 1;
        }
        n
    }
    let mut at = 1;
    let end = length(der, &mut at) + at;
    let mut out = Vec::new();
    while at < end {
        assert_eq!(der[at], 0x02);
        at += 1;
        let n = length(der, &mut at);
        out.push(
            der[at..at + n]
                .iter()
                .copied()
                .skip_while(|b| *b == 0)
                .collect(),
        );
        at += n;
    }
    out
}

fn joined_texts(context: &mut Context, private: &[Vec<u8>]) -> Word {
    let mut joined = sealed(context, b"", false);
    for number in private {
        let bytes = sealed(context, number, false);
        let text = named_secret(context, "?secret_encoded", "base64url", bytes);
        let utf8 = unsafe { native::<A1>("?secret_bytes")(context, text) };
        let dot = sealed(context, b".", false);
        let behind = unsafe { native::<A2>("?secret_concat")(context, dot, utf8) };
        joined = unsafe { native::<A2>("?secret_concat")(context, joined, behind) };
    }
    joined
}

#[test]
fn an_rsa_json_web_key_rebuilds_the_original_der_key_document_and_returns_empty_for_inconsistent_numbers()
 {
    let mut context = new_context();
    let document = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../ply-core-tests/tests/fixtures/rsa_key.der"),
    )
    .expect("the tests' RSA key");
    let numbers = integers(&document);
    let ((np, nn), (ep, en)) = (borrowed(&numbers[1]), borrowed(&numbers[2]));
    let made = |context: &mut Context, texts: Word| {
        let w = unsafe { native::<A5>("?secret_rsa_jwk")(context, np, nn, ep, en, texts) };
        held(w).bytes().to_vec()
    };
    let six = joined_texts(&mut context, &numbers[3..]);
    assert_eq!(made(&mut context, six), document);
    let mut wrong = numbers[3..].to_vec();
    wrong.swap(1, 2);
    let swapped = joined_texts(&mut context, &wrong);
    assert_eq!(made(&mut context, swapped), Vec::<u8>::new());
    let five = joined_texts(&mut context, &numbers[3..8]);
    assert_eq!(made(&mut context, five), Vec::<u8>::new());
    assert_eq!(context.failed, 0);
}

#[test]
fn a_keyed_hash_is_computed_with_the_hash_function_named_by_its_argument() {
    let mut context = new_context();
    let key = sealed(&mut context, b"key", false);
    let hmac = |context: &mut Context, hash: &str| {
        let ((hp, hn), (mp, mn)) = (
            borrowed(hash.as_bytes()),
            borrowed(b"The quick brown fox jumps over the lazy dog"),
        );
        let n = unsafe { native::<A5>("?secret_hmac")(context, hp, hn, key, mp, mn) };
        taken(context, n)
    };
    assert_eq!(
        hmac(&mut context, "SHA-256"),
        Some(hex(
            "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"
        ))
    );
    assert_eq!(hmac(&mut context, "MD5"), None);
    assert_eq!(context.failed, 0);
}

#[test]
fn hkdf_extracts_and_expands_as_rfc_5869_does_and_rejects_a_length_past_its_bound() {
    let mut context = new_context();
    let (hp, hn) = borrowed(b"SHA-256");
    let ikm = sealed(&mut context, &[0x0b; 22], false);
    let salt = hex("000102030405060708090a0b0c");
    let (stack_pointer, sn) = borrowed(&salt);
    let prk = unsafe {
        native::<A5>("?secret_hkdf_extract")(&mut context, hp, hn, stack_pointer, sn, ikm)
    };
    assert_eq!(
        held(prk).bytes(),
        hex("077709362c2e32df0ddc3f0dc47bba6390b6c73bb50f9c3122ec844ad7c2b3e5")
    );
    let unknown = borrowed(b"MD5");
    let none = unsafe {
        native::<A5>("?secret_hkdf_extract")(
            &mut context,
            unknown.0,
            unknown.1,
            stack_pointer,
            sn,
            ikm,
        )
    };
    assert!(held(none).bytes().is_empty());
    let info = hex("f0f1f2f3f4f5f6f7f8f9");
    let (ip, info_byte_count) = borrowed(&info);
    let expands = |context: &mut Context, length: Word| unsafe {
        native::<A3>("?secret_hkdf_expands")(context, hp, hn, length)
    };
    let expand = |context: &mut Context, length: Word| {
        let w = unsafe {
            native::<A6>("?secret_hkdf_expand")(context, hp, hn, prk, ip, info_byte_count, length)
        };
        held(w).bytes().to_vec()
    };
    assert_eq!(
        expand(&mut context, 42),
        hex("3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865")
    );
    for (length, results) in [(0, 0), (42, 0), (255 * 32, 0)] {
        assert_eq!(expands(&mut context, length), results, "{length}");
        assert_eq!(expand(&mut context, length).len(), length as usize);
    }
    for rejected in [-1, 255 * 32 + 1, Word::MAX] {
        assert_eq!(expands(&mut context, rejected), -1, "{rejected}");
        assert_eq!(
            expand(&mut context, rejected),
            Vec::<u8>::new(),
            "{rejected}"
        );
    }
    let (mp, mn) = borrowed(b"MD5");
    assert_eq!(
        unsafe { native::<A3>("?secret_hkdf_expands")(&mut context, mp, mn, 0) },
        -1
    );
    assert_eq!(context.failed, 0);
}

#[test]
fn pbkdf2_derives_as_rfc_6070_says_within_its_bounds() {
    let mut context = new_context();
    let (hp, hn) = borrowed(b"SHA-1");
    let password = sealed(&mut context, b"password", false);
    let (stack_pointer, sn) = borrowed(b"salt");
    let derive = |context: &mut Context, iterations: Word, length: Word| {
        let w = unsafe {
            native::<A7>("?secret_pbkdf2")(
                context,
                hp,
                hn,
                password,
                stack_pointer,
                sn,
                iterations,
                length,
            )
        };
        held(w).bytes().to_vec()
    };
    assert_eq!(
        derive(&mut context, 1, 20),
        hex("0c60c80f961f0e71f3a9b524af6012062fe037a6")
    );
    for (iterations, length) in [(0, 20), (1, 0), (1, (1 << 20) + 1), (1 << 32, 20)] {
        assert_eq!(derive(&mut context, iterations, length), Vec::<u8>::new());
    }
    assert_eq!(context.failed, 0);
}

fn sealing(
    context: &mut Context,
    native_name: &str,
    cipher: &str,
    key: Word,
    nonce: &[u8],
    text: &[u8],
) -> Vec<u8> {
    let ((cp, cn), (np, nn), (ap, an), (tp, tn)) = (
        borrowed(cipher.as_bytes()),
        borrowed(nonce),
        borrowed(b"bound"),
        borrowed(text),
    );
    let n = unsafe { native::<A9>(native_name)(context, cp, cn, key, np, nn, ap, an, tp, tn) };
    taken(context, n).expect("bytes")
}

#[test]
fn a_sealed_message_opens_with_the_same_key_and_fails_with_a_wrong_key_or_cipher() {
    let mut context = new_context();
    for (cipher, nonce) in [
        ("aes-256-gcm", 12),
        ("chacha20-poly1305", 12),
        ("xchacha20-poly1305", 24),
        ("des", 0),
    ] {
        assert_eq!(of_name(&mut context, "crypto_nonce_length", cipher), nonce);
    }
    let key = sealed(&mut context, &[9; 32], false);
    let nonce = [1u8; 24];
    let cipher = "xchacha20-poly1305";
    let sealed_text = sealing(
        &mut context,
        "crypto_sealed",
        cipher,
        key,
        &nonce,
        b"attack",
    );
    assert_eq!(sealed_text.len(), 6 + 16);
    let opened = sealing(
        &mut context,
        "crypto_opened",
        cipher,
        key,
        &nonce,
        &sealed_text,
    );
    assert_eq!(opened, [&[1u8][..], b"attack"].concat());
    let mut turned = sealed_text.clone();
    turned[0] ^= 1;
    assert_eq!(
        sealing(&mut context, "crypto_opened", cipher, key, &nonce, &turned),
        [0]
    );
    let short = sealed(&mut context, &[9; 16], false);
    let aes = "aes-256-gcm";
    assert!(
        sealing(
            &mut context,
            "crypto_sealed",
            aes,
            short,
            &nonce[..12],
            b"x"
        )
        .is_empty()
    );
    assert!(
        sealing(
            &mut context,
            "crypto_opened",
            aes,
            key,
            &nonce,
            &sealed_text
        )
        .is_empty()
    );
    assert_eq!(context.failed, 0);
}

#[test]
fn a_signature_and_a_public_key_are_produced_and_verified_for_the_named_scheme() {
    let mut context = new_context();
    let seed = sealed(&mut context, &[5; 32], false);
    let public = |context: &mut Context, scheme: &str, key: Word| {
        let n = named_secret(context, "crypto_public_key", scheme, key);
        taken(context, n).expect("bytes")
    };
    let sign = |context: &mut Context, scheme: &str, key: Word| {
        let ((p, n), (mp, mn)) = (borrowed(scheme.as_bytes()), borrowed(b"message"));
        let n = unsafe { native::<A5>("crypto_signature")(context, p, n, key, mp, mn) };
        taken(context, n).expect("bytes")
    };
    let verify = |context: &mut Context, scheme: &str, public: &[u8], signature: &[u8]| {
        let ((p, n), (kp, kn), (mp, mn), (stack_pointer, sn)) = (
            borrowed(scheme.as_bytes()),
            borrowed(public),
            borrowed(b"message"),
            borrowed(signature),
        );
        unsafe { native::<A8>("crypto_verified")(context, p, n, kp, kn, mp, mn, stack_pointer, sn) }
    };
    let found = public(&mut context, "ed25519", seed);
    assert_eq!((found[0], found.len()), (1, 33));
    let signed = sign(&mut context, "ed25519", seed);
    assert_eq!((signed[0], signed.len()), (1, 65));
    assert_eq!(
        verify(&mut context, "ed25519", &found[1..], &signed[1..]),
        1
    );
    let mut turned = signed[1..].to_vec();
    turned[3] ^= 1;
    assert_eq!(verify(&mut context, "ed25519", &found[1..], &turned), 0);
    let short = sealed(&mut context, &[5; 3], false);
    assert_eq!(public(&mut context, "p256", short), [0]);
    assert_eq!(sign(&mut context, "ecdsa-p256-sha256", short), [0]);
    assert!(public(&mut context, "rot13", seed).is_empty());
    assert!(sign(&mut context, "rot13", seed).is_empty());
    assert_eq!(verify(&mut context, "rot13", &found[1..], &signed[1..]), -1);
    assert_eq!(context.failed, 0);
}

#[test]
fn two_keys_agree_on_one_shared_secret_and_a_low_order_point_yields_no_secret() {
    let mut context = new_context();
    assert_eq!(of_name(&mut context, "crypto_agrees", "x25519"), 1);
    assert_eq!(of_name(&mut context, "crypto_agrees", "p384"), 0);
    let (a, b) = (
        sealed(&mut context, &[1; 32], false),
        sealed(&mut context, &[2; 32], false),
    );
    let public = |context: &mut Context, key: Word| {
        let n = named_secret(context, "crypto_public_key", "x25519", key);
        taken(context, n).expect("bytes")[1..].to_vec()
    };
    let agreed = |context: &mut Context, key: Word, peer: &[u8]| {
        let ((p, n), (pp, pn)) = (borrowed(b"x25519"), borrowed(peer));
        held(unsafe { native::<A5>("crypto_agreed")(context, p, n, key, pp, pn) })
            .bytes()
            .to_vec()
    };
    let (to_a, to_b) = (public(&mut context, a), public(&mut context, b));
    let shared = agreed(&mut context, a, &to_b);
    assert_eq!(shared.len(), 32);
    assert_eq!(agreed(&mut context, b, &to_a), shared);
    assert_eq!(agreed(&mut context, a, &[0; 32]), Vec::<u8>::new());
    assert_eq!(context.failed, 0);
}
