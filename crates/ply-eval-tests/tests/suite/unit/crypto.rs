use ply_eval::builtins::{Builtin, call};
use ply_eval::{Plain, Span, Value, codes};

fn done(b: Builtin, args: Vec<Value>) -> Value {
    call(b, args, Span::DUMMY).unwrap()
}

fn secret(b: &[u8]) -> Value {
    Value::secret_bytes(b)
}

fn text(s: &str) -> Value {
    Value::secret_text(s)
}

fn hex(digits: &str) -> Vec<u8> {
    (0..digits.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&digits[i..i + 2], 16).unwrap())
        .collect()
}

/// What an `Option` holds, or `None`.
fn held(v: &Value) -> Option<&Value> {
    match v {
        Value::Ctor { name, args } if name.as_str() == "Some" => args.first(),
        _ => None,
    }
}

/// A credential's bytes, read where only a test may.
fn opened(v: &Value) -> Vec<u8> {
    match v {
        Value::Secret(inner) if !inner.is_text() => inner.bytes().to_vec(),
        other => panic!("no secret of bytes: {other:?}"),
    }
}

/// The text a `Secret<String>` holds, read where only a test may.
fn opened_text(v: &Value) -> String {
    match v {
        Value::Secret(inner) => inner.text().expect("a secret of text").to_string(),
        other => panic!("no secret: {other:?}"),
    }
}

fn plain(v: &Value) -> Vec<u8> {
    match v {
        Value::Bytes(b) => b.to_vec(),
        other => panic!("no bytes: {other:?}"),
    }
}

#[test]
fn hchacha20_is_the_drafts() {
    let key: [u8; 32] = std::array::from_fn(|i| i as u8);
    let nonce: [u8; 16] = hex("000000090000004a0000000031415927").try_into().unwrap();
    assert_eq!(
        ply_eval::crypto::hchacha20(&key, &nonce).to_vec(),
        hex("82413b4227b27bfed30e42508a877d73a0f9e4d58a74a853c12ec41326d3ecdc")
    );
}

#[test]
fn what_is_made_of_a_secret_is_a_secret() {
    let key = secret(&[7; 32]);
    let made = [
        done(Builtin::SecretOfBytes, vec![Value::bytes(b"hunter2")]),
        done(Builtin::SecretBytes, vec![text("hunter2")]),
        done(Builtin::SecretConcat, vec![secret(b"hun"), secret(b"ter2")]),
    ];
    for v in &made {
        assert_eq!(Plain::of(v), Plain::Secret);
        assert_eq!(opened(v), b"hunter2");
    }
    let answered = [
        done(
            Builtin::SecretDecode,
            vec![Value::str("hex"), text("68756e74657232")],
        ),
        done(
            Builtin::SecretHkdfExtract,
            vec![Value::str("SHA-256"), Value::bytes(b"salt"), key.clone()],
        ),
        done(
            Builtin::SecretHkdfExpand,
            vec![
                Value::str("SHA-256"),
                key.clone(),
                Value::bytes(b"info"),
                Value::Int(16),
            ],
        ),
        done(
            Builtin::SecretPbkdf2,
            vec![
                Value::str("SHA-256"),
                key.clone(),
                Value::bytes(b"salt"),
                Value::Int(1),
                Value::Int(16),
            ],
        ),
        done(
            Builtin::CryptoAgree,
            vec![Value::str("x25519"), key.clone(), Value::bytes([9; 32])],
        ),
    ];
    for v in &answered {
        assert_eq!(
            Plain::of(v),
            Plain::Ctor("Some".to_string(), vec![Plain::Secret])
        );
    }
}

#[test]
fn a_length_a_tag_a_public_key_and_a_signature_are_no_secret() {
    let key = secret(&[7; 32]);
    assert_eq!(done(Builtin::SecretLen, vec![key.clone()]), Value::Int(32));
    // RFC 4231, case 2.
    let tag = done(
        Builtin::SecretHmac,
        vec![
            Value::str("SHA-256"),
            secret(b"Jefe"),
            Value::bytes(b"what do ya want for nothing?"),
        ],
    );
    assert_eq!(
        plain(held(&tag).unwrap()),
        hex("5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843")
    );
    for scheme in ["ed25519", "x25519", "p256"] {
        let public = done(Builtin::CryptoPublic, vec![Value::str(scheme), key.clone()]);
        assert!(matches!(held(&public), Some(Value::Bytes(_))), "{scheme}");
    }
    for scheme in ["ed25519", "ecdsa-p256-sha256"] {
        let signature = done(
            Builtin::CryptoSign,
            vec![Value::str(scheme), key.clone(), Value::bytes(b"message")],
        );
        assert_eq!(plain(held(&signature).unwrap()).len(), 64, "{scheme}");
    }
}

#[test]
fn a_hash_the_runtime_does_not_key_answers_nothing() {
    let key = secret(&[7; 32]);
    for hash in ["SHA3-256", "BLAKE2b", "MD5", "sha-256", ""] {
        let tag = done(
            Builtin::SecretHmac,
            vec![Value::str(hash), key.clone(), Value::bytes(b"")],
        );
        assert!(held(&tag).is_none(), "{hash}");
        let derived = done(
            Builtin::SecretPbkdf2,
            vec![
                Value::str(hash),
                key.clone(),
                Value::bytes(b""),
                Value::Int(1),
                Value::Int(16),
            ],
        );
        assert!(held(&derived).is_none(), "{hash}");
    }
}

#[test]
fn a_secret_text_decodes_as_its_encoding_writes_and_a_refusal_says_nothing() {
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
        let read = done(
            Builtin::SecretDecode,
            vec![Value::str(encoding), text(written)],
        );
        assert_eq!(
            held(&read).map(opened),
            bytes.map(<[u8]>::to_vec),
            "{encoding} of {written:?}"
        );
        assert!(
            !format!("{read:?}").contains(written) || written.is_empty(),
            "{read:?}"
        );
    }
}

#[test]
fn a_misuse_raises_and_names_no_key() {
    let short = secret(b"hunter2hunter2");
    let misuses = [
        (
            Builtin::CryptoSeal,
            vec![
                Value::str("aes-256-gcm"),
                short.clone(),
                Value::bytes([0; 12]),
                Value::bytes(b""),
                Value::bytes(b"message"),
            ],
        ),
        (
            Builtin::CryptoOpen,
            vec![
                Value::str("chacha20-poly1305"),
                secret(&[7; 32]),
                Value::bytes([0; 11]),
                Value::bytes(b""),
                Value::bytes(b"message"),
            ],
        ),
        (
            Builtin::CryptoSeal,
            vec![
                Value::str("aes-128-gcm"),
                secret(&[7; 32]),
                Value::bytes([0; 12]),
                Value::bytes(b""),
                Value::bytes(b""),
            ],
        ),
        (
            Builtin::CryptoSign,
            vec![
                Value::str("ecdsa-p224-sha224"),
                short.clone(),
                Value::bytes(b""),
            ],
        ),
        (
            Builtin::CryptoAgree,
            vec![Value::str("p384"), short.clone(), Value::bytes(b"")],
        ),
        (
            Builtin::SecretDecode,
            vec![Value::str("rot13"), text("hunter2hunter2")],
        ),
    ];
    for (builtin, args) in misuses {
        assert!(builtin.raises(), "{builtin:?}");
        let d = call(builtin, args, Span::DUMMY).expect_err("a misuse raises");
        assert_eq!(d.code, codes::RUNTIME_ERROR, "{builtin:?}");
        assert!(!format!("{d:#?}").contains("hunter2"), "{d:#?}");
    }
}

#[test]
fn a_key_that_is_none_of_its_scheme_answers_nothing() {
    let none = |b: Builtin, args: Vec<Value>| assert!(held(&done(b, args)).is_none(), "{b:?}");
    let short = secret(b"short");
    none(
        Builtin::CryptoPublic,
        vec![Value::str("ed25519"), short.clone()],
    );
    none(
        Builtin::CryptoPublic,
        vec![Value::str("p256"), secret(&[0; 32])],
    );
    none(
        Builtin::CryptoPublic,
        vec![Value::str("p256"), secret(&[0xff; 32])],
    );
    none(
        Builtin::CryptoPublic,
        vec![Value::str("rsa"), short.clone()],
    );
    none(
        Builtin::CryptoSign,
        vec![
            Value::str("rsa-pkcs1-sha256"),
            short.clone(),
            Value::bytes(b""),
        ],
    );
    // A point of small order sends every key to zero.
    none(
        Builtin::CryptoAgree,
        vec![
            Value::str("x25519"),
            secret(&[7; 32]),
            Value::bytes([0; 32]),
        ],
    );
    // The point at infinity's place, and a point off the curve.
    none(
        Builtin::CryptoAgree,
        vec![Value::str("p256"), secret(&[7; 32]), Value::bytes([0])],
    );
    let mut off = vec![4u8];
    off.extend([1u8; 64]);
    none(
        Builtin::CryptoAgree,
        vec![Value::str("p256"), secret(&[7; 32]), Value::bytes(off)],
    );
    none(Builtin::SecretPrivateKey, vec![short]);
}

#[test]
fn a_private_key_is_read_from_its_document_and_stays_sealed() {
    // RFC 8410, section 10.3.
    let seed = hex("d4ee72dbf913584ad5b6d8f1f769f8ad3afe7c28cbf1d4fbe097a88f44755842");
    let mut document = hex("302e020100300506032b657004220420");
    document.extend(&seed);
    let found = done(Builtin::SecretPrivateKey, vec![secret(&document)]);
    let Some(Value::Record(fields)) = held(&found) else {
        panic!("no key: {found:?}");
    };
    let field = |name: &str| {
        fields
            .into_iter()
            .find(|(k, _)| k.as_str() == name)
            .map(|(_, v)| v.clone())
            .unwrap()
    };
    assert_eq!(field("scheme"), Value::str("ed25519"));
    assert_eq!(Plain::of(&field("key")), Plain::Secret);
    assert_eq!(opened(&field("key")), seed);
}

#[test]
fn what_is_sealed_opens_and_a_turned_bit_does_not() {
    for (cipher, nonce) in [
        ("aes-256-gcm", 12),
        ("chacha20-poly1305", 12),
        ("xchacha20-poly1305", 24),
    ] {
        let args = |text: Value| {
            vec![
                Value::str(cipher),
                secret(&[7; 32]),
                Value::bytes(vec![1u8; nonce]),
                Value::bytes(b"bound"),
                text,
            ]
        };
        let sealed = plain(&done(Builtin::CryptoSeal, args(Value::bytes(b"message"))));
        assert_eq!(sealed.len(), 7 + 16, "{cipher}");
        let read = done(Builtin::CryptoOpen, args(Value::bytes(&sealed)));
        assert_eq!(plain(held(&read).unwrap()), b"message", "{cipher}");
        for at in 0..sealed.len() {
            let mut turned = sealed.clone();
            turned[at] ^= 1;
            let read = done(Builtin::CryptoOpen, args(Value::bytes(turned)));
            assert!(held(&read).is_none(), "{cipher} at {at}");
        }
    }
}

fn encoded(encoding: &str, bytes: &[u8]) -> String {
    opened_text(&done(
        Builtin::SecretEncode,
        vec![Value::str(encoding), secret(bytes)],
    ))
}

#[test]
fn a_secret_encodes_as_rfc_4648_writes_and_decodes_back() {
    // RFC 4648, section 10.
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
            let sealed = done(
                Builtin::SecretEncode,
                vec![Value::str(encoding), secret(bytes)],
            );
            assert_eq!(Plain::of(&sealed), Plain::Secret);
            let back = done(Builtin::SecretDecode, vec![Value::str(encoding), sealed]);
            assert_eq!(opened(held(&back).unwrap()), bytes, "{encoding} of {n}");
        }
    }
    let d = call(
        Builtin::SecretEncode,
        vec![Value::str("rot13"), secret(b"key")],
        Span::DUMMY,
    )
    .unwrap_err();
    assert!(!format!("{d:#?}").contains("key\""), "{d:#?}");
}

#[test]
fn a_frame_reads_back_what_it_wrote_and_nothing_it_did_not() {
    use ply_eval::crypto::Framing;
    let named = |name: &str| Framing::named(name, "`fs.write_secret`", Span::DUMMY);
    let every: Vec<u8> = (0..=255).collect();
    for encoding in ["raw", "hex", "base64", "base64url", "base32"] {
        let framing = named(encoding).unwrap_or_else(|_| panic!("{encoding}"));
        for n in [0, 1, 2, 3, 5, 32, 256] {
            let secret = &every[256 - n..];
            let line = framing.framed(b"key ", secret, b"\n");
            assert!(line.starts_with(b"key ") && line.ends_with(b"\n"));
            let back = framing.unframed(&line, b"key ", b"\n");
            assert_eq!(
                back.as_deref().map(|b| &b[..]),
                Some(secret),
                "{encoding} of {n}"
            );
            assert!(framing.unframed(&line, b"kez ", b"\n").is_none());
            assert!(framing.unframed(&line, b"key ", b"\r\n").is_none());
            assert!(framing.unframed(&line[..3], b"key ", b"\n").is_none());
        }
    }
    let hex = named("hex").unwrap();
    assert_eq!(&hex.framed(b"", b"\xab", b"")[..], b"ab");
    assert!(hex.unframed(b"AB", b"", b"").is_none());
    assert!(hex.unframed(b"abc", b"", b"").is_none());
    for refused in ["rot13", "base32-typed", "RAW", ""] {
        let d = named(refused).err().unwrap_or_else(|| panic!("{refused}"));
        assert_eq!(d.code, codes::RUNTIME_ERROR);
        assert!(d.message.contains("`fs.write_secret`"), "{}", d.message);
    }
}

fn rsa_document() -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../ply-corpus/stdlib/asn1/rsa_key.der"),
    )
    .expect("the corpus's RSA key")
}

#[test]
fn a_pss_signature_is_a_function_of_the_key_and_the_message_and_verifies() {
    let document = rsa_document();
    let public = plain(
        held(&done(
            Builtin::CryptoPublic,
            vec![Value::str("rsa"), secret(&document)],
        ))
        .unwrap(),
    );
    for digest in ["sha256", "sha384", "sha512"] {
        let scheme = format!("rsa-pss-{digest}");
        let signed = |message: &[u8]| {
            plain(
                held(&done(
                    Builtin::CryptoSign,
                    vec![
                        Value::str(&scheme),
                        secret(&document),
                        Value::bytes(message),
                    ],
                ))
                .unwrap(),
            )
        };
        let once = signed(b"what was built");
        assert_eq!(once, signed(b"what was built"), "{scheme}");
        assert_ne!(once, signed(b"what was not"), "{scheme}");
        let holds = |scheme: &str, message: &[u8]| {
            done(
                Builtin::CryptoVerify,
                vec![
                    Value::str(scheme),
                    Value::bytes(&public),
                    Value::bytes(message),
                    Value::bytes(&once),
                ],
            ) == Value::Bool(true)
        };
        assert!(holds(&scheme, b"what was built"), "{scheme}");
        assert!(!holds(&scheme, b"what was not"), "{scheme}");
        assert!(!holds(&format!("rsa-pkcs1-{digest}"), b"what was built"));
    }
}

#[test]
fn every_curve_signs_as_its_verifier_reads_and_a_high_s_secp256k1_signature_is_its_twin() {
    for (curve, scheme, size) in [
        ("p256", "ecdsa-p256-sha256", 32),
        ("p384", "ecdsa-p384-sha384", 48),
        ("p521", "ecdsa-p521-sha512", 66),
        ("k256", "ecdsa-k256-sha256", 32),
    ] {
        let mut scalar = vec![0u8; size];
        scalar[size - 1] = 7;
        scalar[size - 2] = 1;
        let public = plain(
            held(&done(
                Builtin::CryptoPublic,
                vec![Value::str(curve), secret(&scalar)],
            ))
            .unwrap(),
        );
        assert_eq!(public.len(), 2 * size + 1, "{curve}");
        let raw = plain(
            held(&done(
                Builtin::CryptoSign,
                vec![Value::str(scheme), secret(&scalar), Value::bytes(b"m")],
            ))
            .unwrap(),
        );
        assert_eq!(raw.len(), 2 * size, "{curve}");
        if curve == "p384" {
            continue;
        }
        let verified = |signature: &[u8], message: &[u8]| {
            done(
                Builtin::CryptoVerify,
                vec![
                    Value::str(scheme),
                    Value::bytes(&public),
                    Value::bytes(message),
                    Value::bytes(signature),
                ],
            ) == Value::Bool(true)
        };
        assert!(verified(&raw, b"m"), "{curve}");
        assert!(!verified(&raw, b"n"), "{curve}");
    }
    // secp256k1's order; `s` and `n - s` are one signature.
    let order = hex("fffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141");
    let scalar = secret(&[3u8; 32]);
    let public = plain(
        held(&done(
            Builtin::CryptoPublic,
            vec![Value::str("k256"), scalar.clone()],
        ))
        .unwrap(),
    );
    let raw = plain(
        held(&done(
            Builtin::CryptoSign,
            vec![Value::str("ecdsa-k256-sha256"), scalar, Value::bytes(b"m")],
        ))
        .unwrap(),
    );
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
            done(
                Builtin::CryptoVerify,
                vec![
                    Value::str("ecdsa-k256-sha256"),
                    Value::bytes(&public),
                    Value::bytes(b"m"),
                    Value::bytes(signature),
                ],
            ),
            Value::Bool(true)
        );
    }
}

/// The contents of each `INTEGER` a DER `SEQUENCE` of them holds.
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
fn an_rsa_jwk_makes_the_document_its_numbers_came_from_and_numbers_that_disagree_make_none() {
    let document = rsa_document();
    let numbers = integers(&document);
    assert_eq!(numbers.len(), 9);
    let unsigned = |n: &[u8]| {
        n.iter()
            .skip_while(|b| **b == 0)
            .copied()
            .collect::<Vec<u8>>()
    };
    let sealed_text = |n: &[u8]| {
        done(
            Builtin::SecretEncode,
            vec![Value::str("base64url"), secret(&unsigned(n))],
        )
    };
    let args = |private: Vec<Value>| {
        vec![
            Value::bytes(unsigned(&numbers[1])),
            Value::bytes(unsigned(&numbers[2])),
            Value::list(private),
        ]
    };
    let texts = |private: &[Vec<u8>]| private.iter().map(|n| sealed_text(n)).collect::<Vec<_>>();
    let made = done(Builtin::SecretRsaJwk, args(texts(&numbers[3..])));
    assert_eq!(Plain::of(held(&made).unwrap()), Plain::Secret);
    assert_eq!(opened(held(&made).unwrap()), document);
    let mut wrong = numbers[3..].to_vec();
    wrong.swap(1, 2);
    wrong[4][0] ^= 1;
    assert!(held(&done(Builtin::SecretRsaJwk, args(texts(&wrong)))).is_none());
    let mut unread = texts(&numbers[3..]);
    unread[0] = text("not base64url!");
    assert!(held(&done(Builtin::SecretRsaJwk, args(unread))).is_none());
    let short = texts(&numbers[3..8]);
    assert!(call(Builtin::SecretRsaJwk, args(short), Span::DUMMY).is_err());
}
