use ply_core::crypto::{self, EncodingSupport};
use ply_core::{Span, codes};

fn hex(digits: &str) -> Vec<u8> {
    (0..digits.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&digits[i..i + 2], 16).unwrap())
        .collect()
}

fn decoded(encoding: &str, text: &[u8]) -> Option<Vec<u8>> {
    crypto::decode(encoding, text).map(|b| b.to_vec())
}

fn encoded(encoding: &str, bytes: &[u8]) -> String {
    let text = crypto::encode(encoding, bytes).expect("an encoding it writes");
    String::from_utf8(text.to_vec()).unwrap()
}

#[test]
fn hchacha20_matches_the_test_vector_in_the_xchacha20_draft() {
    let key: [u8; 32] = std::array::from_fn(|i| i as u8);
    let nonce: [u8; 16] = hex("000000090000004a0000000031415927").try_into().unwrap();
    assert_eq!(
        crypto::hchacha20(&key, &nonce).to_vec(),
        hex("82413b4227b27bfed30e42508a877d73a0f9e4d58a74a853c12ec41326d3ecdc")
    );
}

#[test]
fn hmac_matches_an_rfc_4231_vector_and_each_algorithm_gives_a_public_key_or_a_64_byte_signature() {
    let key = [7; 32];
    assert_eq!(
        crypto::hmac("SHA-256", b"Jefe", b"what do ya want for nothing?"),
        Some(hex(
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        ))
    );
    for scheme in ["ed25519", "x25519", "p256"] {
        let public = crypto::public(scheme, &key).unwrap();
        assert!(public.is_some_and(|p| !p.is_empty()), "{scheme}");
    }
    for scheme in ["ed25519", "ecdsa-p256-sha256"] {
        let signature = crypto::sign(scheme, &key, b"message").unwrap().unwrap();
        assert_eq!(signature.len(), 64, "{scheme}");
    }
}

#[test]
fn hmac_hkdf_pbkdf2_and_digest_length_accept_sha_1_256_384_and_512_and_reject_any_other_hash_name()
{
    let key = [7; 32];
    let results = |hash: &str| {
        [
            crypto::hmac(hash, &key, b"").is_some(),
            crypto::hkdf_extract(hash, b"salt", &key).is_some(),
            crypto::hkdf_expand(hash, &key, b"info", 16).is_some(),
            crypto::pbkdf2(hash, &key, b"salt", 1, 16).is_some(),
            crypto::digest_length(hash) > 0,
        ]
    };
    for hash in ["SHA-1", "SHA-256", "SHA-384", "SHA-512"] {
        assert_eq!(results(hash), [true; 5], "{hash}");
    }
    for hash in ["SHA3-256", "BLAKE2b", "MD5", "sha-256", ""] {
        assert_eq!(results(hash), [false; 5], "{hash}");
    }
}

#[test]
fn decoding_accepts_only_the_canonical_text_of_each_encoding_and_returns_none_for_anything_else() {
    let cases: [(&str, &str, Option<&[u8]>); 22] = [
        ("hex", "", Some(b"")),
        ("hex", "666f", Some(b"fo")),
        ("hex", "666F", None),
        ("hex", "666", None),
        ("hex", "6g", None),
        ("base64", "Zm9vYg==", Some(b"foob")),
        ("base64", "Zm9vYmFy", Some(b"foobar")),
        ("base64", "Zm9vYg", None),
        ("base64", "Zh==", None),
        ("base64", "Zg=a", None),
        ("base64", "====", None),
        ("base64url", "Zm9vYg", Some(b"foob")),
        ("base64url", "__-_", Some(b"\xff\xff\xbf")),
        ("base64url", "Zm9vYg==", None),
        ("base64url", "Z", None),
        ("base32", "MZXW6YQ=", Some(b"foob")),
        ("base32", "MZXW6YTBOI======", Some(b"foobar")),
        ("base32", "mzxw6yq=", None),
        ("base32", "MZXW6YQ", None),
        ("base32-typed", "mzxw 6ytb-oi==", Some(b"foobar")),
        ("base32-typed", "MZXW7", None),
        ("base32-typed", "MZ=W6", None),
    ];
    for (encoding, written, bytes) in cases {
        assert_eq!(
            decoded(encoding, written.as_bytes()),
            bytes.map(<[u8]>::to_vec),
            "{encoding} of {written:?}"
        );
    }
}

#[test]
fn each_encoding_reports_whether_it_can_encode_and_decode_and_an_unknown_encoding_name_does_neither()
 {
    for name in ["hex", "base64", "base64url", "base32"] {
        assert_eq!(
            crypto::encoding(name),
            EncodingSupport::WritesAndReads,
            "{name}"
        );
    }
    assert_eq!(crypto::encoding("base32-typed"), EncodingSupport::Reads);
    assert!(crypto::encode("base32-typed", b"key").is_none());
    for name in ["rot13", "raw", "HEX", ""] {
        assert_eq!(crypto::encoding(name), EncodingSupport::Neither, "{name}");
        assert!(crypto::encode(name, b"key").is_none(), "{name}");
        assert!(crypto::decode(name, b"6b6579").is_none(), "{name}");
    }
}

#[test]
fn crypto_functions_given_invalid_keys_or_nonces_fail_with_an_error_that_does_not_contain_the_key()
{
    let short = b"hunter2hunter2";
    let key = [7; 32];
    let rejections = [
        crypto::seal("aes-256-gcm", short, &[0; 12], b"", b"message").unwrap_err(),
        crypto::open("chacha20-poly1305", &key, &[0; 11], b"", b"message").unwrap_err(),
        crypto::seal("aes-128-gcm", &key, &[0; 12], b"", b"").unwrap_err(),
        crypto::sign("ecdsa-p224-sha224", short, b"").unwrap_err(),
        crypto::public("rsa-pss", short).unwrap_err(),
        crypto::verify("ecdsa-p224-sha224", short, b"", b"").unwrap_err(),
        crypto::agree("p384", short, b"").unwrap_err(),
    ];
    for reason in rejections {
        assert!(!reason.contains("hunter2"), "{reason}");
    }
    assert!(!crypto::agrees("p384"));
    assert_eq!(crypto::nonce_length("aes-128-gcm"), None);
}

#[test]
fn key_bytes_that_are_invalid_for_a_scheme_return_no_public_key_signature_or_shared_secret() {
    let short = b"short";
    assert_eq!(crypto::public("ed25519", short), Ok(None));
    assert_eq!(crypto::public("p256", &[0; 32]), Ok(None));
    assert_eq!(crypto::public("p256", &[0xff; 32]), Ok(None));
    assert_eq!(crypto::public("rsa", short), Ok(None));
    assert_eq!(crypto::sign("rsa-pkcs1-sha256", short, b""), Ok(None));
    let agreed = |scheme: &str, peer: &[u8]| {
        crypto::agree(scheme, &[7; 32], peer)
            .unwrap()
            .map(|s| s.to_vec())
    };
    assert_eq!(agreed("x25519", &[0; 32]), None);
    assert_eq!(agreed("p256", &[0]), None);
    let mut off = vec![4u8];
    off.extend([1u8; 64]);
    assert_eq!(agreed("p256", &off), None);
    assert!(crypto::private_key(short).is_none());
}

#[test]
fn a_private_key_is_read_from_its_document_as_the_scheme_it_names() {
    let seed = hex("d4ee72dbf913584ad5b6d8f1f769f8ad3afe7c28cbf1d4fbe097a88f44755842");
    let mut document = hex("302e020100300506032b657004220420");
    document.extend(&seed);
    let (scheme, key) = crypto::private_key(&document).expect("a key");
    assert_eq!((scheme, key.to_vec()), ("ed25519", seed.clone()));
    assert_eq!(
        crypto::private_key_as(&document, "ed25519").map(|k| k.to_vec()),
        Some(seed)
    );
    assert!(crypto::private_key_as(&document, "x25519").is_none());
}

#[test]
fn a_sealed_message_opens_but_fails_to_open_if_the_associated_data_differs_or_any_bit_is_flipped() {
    let key = [7; 32];
    for cipher in ["aes-256-gcm", "chacha20-poly1305", "xchacha20-poly1305"] {
        let nonce = vec![1u8; crypto::nonce_length(cipher).expect("a cipher it has")];
        let sealed = crypto::seal(cipher, &key, &nonce, b"bound", b"message").unwrap();
        assert_eq!(sealed.len(), 7 + 16, "{cipher}");
        let opened = |sealed: &[u8], bound: &[u8]| {
            crypto::open(cipher, &key, &nonce, bound, sealed).unwrap()
        };
        assert_eq!(
            opened(&sealed, b"bound").as_deref(),
            Some(&b"message"[..]),
            "{cipher}"
        );
        assert_eq!(opened(&sealed, b"bounD"), None, "{cipher}");
        for at in 0..sealed.len() {
            let mut turned = sealed.clone();
            turned[at] ^= 1;
            assert_eq!(opened(&turned, b"bound"), None, "{cipher} at {at}");
        }
    }
}

#[test]
fn bytes_encode_to_the_rfc_4648_test_vectors_and_decode_back() {
    let words = ["", "f", "fo", "foo", "foob", "fooba", "foobar"];
    let base64 = [
        "", "Zg==", "Zm8=", "Zm9v", "Zm9vYg==", "Zm9vYmE=", "Zm9vYmFy",
    ];
    let base32 = [
        "",
        "MY======",
        "MZXQ====",
        "MZXW6===",
        "MZXW6YQ=",
        "MZXW6YTB",
        "MZXW6YTBOI======",
    ];
    let hex_text = [
        "",
        "66",
        "666f",
        "666f6f",
        "666f6f62",
        "666f6f6261",
        "666f6f626172",
    ];
    for (i, word) in words.iter().enumerate() {
        assert_eq!(encoded("base64", word.as_bytes()), base64[i]);
        assert_eq!(encoded("base32", word.as_bytes()), base32[i]);
        assert_eq!(encoded("hex", word.as_bytes()), hex_text[i]);
        assert_eq!(
            encoded("base64url", word.as_bytes()),
            base64[i].trim_end_matches('=')
        );
    }
    let every: Vec<u8> = (0..=255).collect();
    for encoding in ["hex", "base64", "base64url", "base32"] {
        for n in 0..40 {
            let bytes = &every[255 - n..];
            let text = crypto::encode(encoding, bytes).unwrap();
            assert_eq!(
                decoded(encoding, &text).as_deref(),
                Some(bytes),
                "{encoding} of {n}"
            );
        }
    }
}

#[test]
fn a_secret_framed_with_a_prefix_and_suffix_reads_back_exactly_and_a_different_prefix_or_suffix_is_rejected()
 {
    use ply_core::crypto::Framing;
    let named = |name: &str| Framing::named(name, "`fs.write_secret`", Span::DUMMY);
    let every: Vec<u8> = (0..=255).collect();
    for encoding in ["raw", "hex", "base64", "base64url", "base32"] {
        let framing = named(encoding).unwrap_or_else(|_| panic!("{encoding}"));
        for n in [0, 1, 2, 3, 5, 32, 256] {
            let secret = &every[256 - n..];
            let line = framing.framed(b"key ", secret, b"\n");
            assert!(line.starts_with(b"key ") && line.ends_with(b"\n"));
            let back = framing.decode_framed(&line, b"key ", b"\n");
            assert_eq!(
                back.as_deref().map(|b| &b[..]),
                Some(secret),
                "{encoding} of {n}"
            );
            assert!(framing.decode_framed(&line, b"kez ", b"\n").is_none());
            assert!(framing.decode_framed(&line, b"key ", b"\r\n").is_none());
            assert!(framing.decode_framed(&line[..3], b"key ", b"\n").is_none());
        }
    }
    let hex = named("hex").unwrap();
    assert_eq!(&hex.framed(b"", b"\xab", b"")[..], b"ab");
    assert!(hex.decode_framed(b"AB", b"", b"").is_none());
    assert!(hex.decode_framed(b"abc", b"", b"").is_none());
    for rejected in ["rot13", "base32-typed", "RAW", ""] {
        let d = named(rejected)
            .err()
            .unwrap_or_else(|| panic!("{rejected}"));
        assert_eq!(d.code, codes::RUNTIME_ERROR);
        assert!(d.message.contains("`fs.write_secret`"), "{}", d.message);
    }
}

fn rsa_document() -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rsa_key.der"),
    )
    .expect("the tests' RSA key")
}

#[test]
fn an_rsa_pss_signature_is_the_same_for_the_same_key_and_message_and_verifies_only_for_that_message()
 {
    let document = rsa_document();
    let public = crypto::public("rsa", &document).unwrap().unwrap();
    for digest in ["sha256", "sha384", "sha512"] {
        let scheme = format!("rsa-pss-{digest}");
        let signed = |message: &[u8]| crypto::sign(&scheme, &document, message).unwrap().unwrap();
        let once = signed(b"what was built");
        assert_eq!(once, signed(b"what was built"), "{scheme}");
        assert_ne!(once, signed(b"what was not"), "{scheme}");
        let holds =
            |scheme: &str, message: &[u8]| crypto::verify(scheme, &public, message, &once).unwrap();
        assert!(holds(&scheme, b"what was built"), "{scheme}");
        assert!(!holds(&scheme, b"what was not"), "{scheme}");
        assert!(!holds(&format!("rsa-pkcs1-{digest}"), b"what was built"));
    }
}

#[test]
fn ecdsa_on_each_curve_gives_correct_key_and_signature_lengths_and_verifies_and_so_does_a_secp256k1_signature_with_s_replaced_by_order_minus_s()
 {
    for (curve, scheme, size) in [
        ("p256", "ecdsa-p256-sha256", 32),
        ("p384", "ecdsa-p384-sha384", 48),
        ("p521", "ecdsa-p521-sha512", 66),
        ("k256", "ecdsa-k256-sha256", 32),
    ] {
        let mut scalar = vec![0u8; size];
        scalar[size - 1] = 7;
        scalar[size - 2] = 1;
        let public = crypto::public(curve, &scalar).unwrap().unwrap();
        assert_eq!(public.len(), 2 * size + 1, "{curve}");
        let raw = crypto::sign(scheme, &scalar, b"m").unwrap().unwrap();
        assert_eq!(raw.len(), 2 * size, "{curve}");
        if curve == "p384" {
            continue;
        }
        let verified =
            |signature: &[u8], message: &[u8]| crypto::verify(scheme, &public, message, signature);
        assert_eq!(verified(&raw, b"m"), Ok(true), "{curve}");
        assert_eq!(verified(&raw, b"n"), Ok(false), "{curve}");
    }
    let order = hex("fffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141");
    let scalar = [3u8; 32];
    let public = crypto::public("k256", &scalar).unwrap().unwrap();
    let raw = crypto::sign("ecdsa-k256-sha256", &scalar, b"m")
        .unwrap()
        .unwrap();
    let mut high = raw[..32].to_vec();
    let mut borrow = 0i16;
    let mut s = [0u8; 32];
    for i in (0..32).rev() {
        let d = i16::from(order[i]) - i16::from(raw[32 + i]) - borrow;
        borrow = i16::from(d < 0);
        s[i] = (d + 256 * borrow) as u8;
    }
    high.extend(s);
    for signature in [&raw, &high] {
        assert_eq!(
            crypto::verify("ecdsa-k256-sha256", &public, b"m", signature),
            Ok(true)
        );
    }
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
        out.push(der[at..at + n].to_vec());
        at += n;
    }
    out
}

#[test]
fn an_rsa_jwk_converts_back_to_the_der_private_key_it_came_from_and_inconsistent_numbers_return_none()
 {
    let document = rsa_document();
    let numbers = integers(&document);
    assert_eq!(numbers.len(), 9);
    let unsigned = |n: &[u8]| {
        n.iter()
            .skip_while(|b| **b == 0)
            .copied()
            .collect::<Vec<u8>>()
    };
    let texts = |private: &[Vec<u8>]| {
        private
            .iter()
            .map(|n| encoded("base64url", &unsigned(n)).into_bytes())
            .collect::<Vec<_>>()
    };
    let made = |private: &[Vec<u8>]| {
        let private: [&[u8]; 6] = std::array::from_fn(|i| &private[i][..]);
        crypto::rsa_jwk(&unsigned(&numbers[1]), &unsigned(&numbers[2]), private).map(|d| d.to_vec())
    };
    assert_eq!(made(&texts(&numbers[3..])), Some(document));
    let mut wrong = numbers[3..].to_vec();
    wrong.swap(1, 2);
    wrong[4][0] ^= 1;
    assert_eq!(made(&texts(&wrong)), None);
    let mut unread = texts(&numbers[3..]);
    unread[0] = b"not base64url!".to_vec();
    assert_eq!(made(&unread), None);
}
