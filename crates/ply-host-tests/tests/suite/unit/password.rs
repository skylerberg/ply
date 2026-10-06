//! What `std.password`'s operations compute, against each algorithm's published vectors, and what
//! they refuse before computing anything.

use ply_host::password::{
    self, MAX_ARGON2_WORK, MAX_BCRYPT_COST, MAX_LANES, MAX_LENGTH, MAX_MEMORY_KIB,
    MAX_PBKDF2_ITERATIONS, MAX_SCRYPT_WORK, Work,
};

fn hex(text: &str) -> Vec<u8> {
    let digits: Vec<u8> = text
        .bytes()
        .filter(|b| !b.is_ascii_whitespace())
        .map(|b| (b as char).to_digit(16).expect("a hex digit") as u8)
        .collect();
    digits
        .chunks(2)
        .map(|pair| (pair[0] << 4) | pair[1])
        .collect()
}

/// RFC 9106 section 5: one password, salt, secret and associated data under each variant.
#[test]
fn argon2_answers_rfc_9106s_vectors() {
    let vectors = [
        (
            "argon2d",
            "512b391b6f1162975371d30919734294f868e3be3984f3c1a13a4db9fabe4acb",
        ),
        (
            "argon2i",
            "c814d9d1dc7f37aa13f0d77f2494bda1c8de6b016dd388d29952a4c4672b6ce8",
        ),
        (
            "argon2id",
            "0d640df58d78766c08c037a34a8b53c9d01ef0452d75b65eb52520e96b01e659",
        ),
    ];
    for (variant, tag) in vectors {
        let made = password::argon2(variant, &[1; 32], &[2; 16], &[3; 8], &[4; 12], 32, 3, 4, 32)
            .unwrap_or_else(|why| panic!("{variant} was refused: {why}"));
        assert_eq!(made, hex(tag), "{variant}");
    }
}

/// The reference implementation's own vector, with neither a secret nor associated data: the
/// tag of `$argon2id$v=19$m=65536,t=2,p=1$c29tZXNhbHQ$CTFhFdXPJO1aFaMaO6Mm5c8y7cJHAph8ArZWb2GRPPc`.
#[test]
fn argon2id_without_a_key_is_what_the_reference_implementation_answers() {
    let made = password::argon2(
        "argon2id",
        b"password",
        b"somesalt",
        b"",
        b"",
        65536,
        2,
        1,
        32,
    )
    .expect("within the bounds");
    assert_eq!(
        made,
        hex("09316115d5cf24ed5a15a31a3ba326e5cf32edc24702987c02b6566f61913cf7")
    );
}

/// RFC 7914 section 12, without its last vector, which fills a gigabyte.
#[test]
fn scrypt_answers_rfc_7914s_vectors() {
    let vectors: [(&str, &str, i64, i64, i64, &str); 3] = [
        (
            "",
            "",
            4,
            1,
            1,
            "77d6576238657b203b19ca42c18a0497f16b4844e3074ae8dfdffa3fede21442
             fcd0069ded0948f8326a753a0fc81f17e8d3e0fb2e0d3628cf35e20c38d18906",
        ),
        (
            "password",
            "NaCl",
            10,
            8,
            16,
            "fdbabe1c9d3472007856e7190d01e9fe7c6ad7cbc8237830e77376634b373162
             2eaf30d92e22a3886ff109279d9830dac727afb94a83ee6d8360cbdfa2cc0640",
        ),
        (
            "pleaseletmein",
            "SodiumChloride",
            14,
            8,
            1,
            "7023bdcb3afd7348461c06cd81fd38ebfda8fbba904f8e3ea9b543f6545da1f2
             d5432955613f0fcf62d49705242a9af9e61e85dc0d651e40dfcf017b45575887",
        ),
    ];
    for (secret, salt, log_n, r, p, tag) in vectors {
        let made = password::scrypt(secret.as_bytes(), salt.as_bytes(), log_n, r, p, 64)
            .unwrap_or_else(|why| panic!("N = 2^{log_n} was refused: {why}"));
        assert_eq!(made, hex(tag), "N = 2^{log_n}, r = {r}, p = {p}");
    }
}

/// jBCrypt's vectors, as bytes: `$2a$06$DCq7YPn5Rq63x1Lad4cll.TV4S6ytwfsfvkgY8jIucDrjc8deX1s.`,
/// `$2a$06$m0CrhHm10qJ3lXRY.5zDGO3rS2KdeeWLuGmsfGlMfOxih58VYVfxe` and
/// `$2a$08$aTsUwsyowQuzRrDqFflhgekJ8d9/7Z3GV3UcgvzQW3J5zMyrTvlz.`.
#[test]
fn bcrypt_answers_jbcrypts_vectors() {
    let vectors: [(&str, i64, &str, &str); 3] = [
        (
            "",
            6,
            "144b3d691a7b4ecf39cf735c7fa7a79c",
            "557e94f34bf286e8719a26be94ac1e16d95ef9f819dee0",
        ),
        (
            "a",
            6,
            "a3612d8c9a37dac2f99d94da03bd4521",
            "e6d53831f82060dc08a2e8489ce850ce48fbf976978738",
        ),
        (
            "abcdefghijklmnopqrstuvwxyz",
            8,
            "715b96caed2ac92c354ed16c1e19e38a",
            "98bf9ffc1f5be485f959e8b1d526392fbd4ed2d5719f50",
        ),
    ];
    for (secret, cost, salt, tag) in vectors {
        let made = password::bcrypt(secret.as_bytes(), &hex(salt), cost)
            .unwrap_or_else(|why| panic!("cost {cost} was refused: {why}"));
        assert_eq!(made, hex(tag), "`{secret}` at cost {cost}");
    }
}

/// The key is the password and its terminating zero, cut to seventy-two bytes: the seventy-second
/// byte of a password is read, and nothing after it.
#[test]
fn bcrypt_reads_seventy_two_bytes_of_a_password() {
    let salt = [7u8; 16];
    let hashed = |secret: &[u8]| password::bcrypt(secret, &salt, 4).expect("within the bounds");
    let long = [b'x'; 80];
    assert_eq!(hashed(&long[..72]), hashed(&long));
    assert_eq!(hashed(&long[..72]), hashed(&long[..73]));
    assert_ne!(hashed(&long[..71]), hashed(&long[..72]));
    let mut differs_at_72 = long;
    differs_at_72[71] = b'y';
    assert_ne!(hashed(&long), hashed(&differs_at_72));
    let mut differs_at_73 = long;
    differs_at_73[72] = b'y';
    assert_eq!(hashed(&long), hashed(&differs_at_73));
}

/// RFC 6070's second vector under SHA-1 and RFC 7914 section 11's first under SHA-256.
#[test]
fn pbkdf2_answers_the_rfcs_vectors() {
    assert_eq!(
        password::pbkdf2("SHA-1", b"password", b"salt", 2, 20).expect("within the bounds"),
        hex("ea6c014dc72d6f8ccd1ed92ace1d41f0d8de8957")
    );
    assert_eq!(
        password::pbkdf2("SHA-256", b"passwd", b"salt", 1, 64).expect("within the bounds"),
        hex(
            "55ac046e56e3089fec1691c22544b605f94185216dde0465e68b9d57c20dacbc
             49ca9cccf179b645991664b39d77ef317c71b845b1e30bd509112041d3a19783"
        )
    );
}

const PASSWORD: &[u8] = b"correct horse battery staple";

/// What was asked, refused, with no `Work` made of it and so nothing computed.
fn refused(asked: Result<Work, String>, names: &str) {
    let why = match asked {
        Ok(_) => panic!("nothing refused what should say `{names}`"),
        Err(why) => why,
    };
    assert!(
        why.contains(names),
        "the refusal does not say `{names}`: {why}"
    );
    assert!(
        !why.contains("correct") && !why.contains("staple"),
        "the refusal holds the password: {why}"
    );
}

fn argon2id(memory: i64, passes: i64, lanes: i64, length: i64) -> Result<Work, String> {
    Work::argon2(
        "argon2id", PASSWORD, &[2; 16], b"", b"", memory, passes, lanes, length,
    )
}

/// Each bound is met by a hash that is never run here: the largest fills two gigabytes.
#[test]
fn argon2_is_checked_against_its_bounds_before_anything_is_computed() {
    assert!(argon2id(MAX_MEMORY_KIB, 1, 4, 32).is_ok());
    assert!(argon2id(MAX_MEMORY_KIB / 2, 4, 1, 32).is_ok());
    assert!(argon2id(8 * MAX_LANES, 1, MAX_LANES, MAX_LENGTH).is_ok());
    assert_eq!(MAX_ARGON2_WORK, 2 * MAX_MEMORY_KIB);

    refused(argon2id(MAX_MEMORY_KIB + 1, 1, 1, 32), "KiB of memory");
    refused(argon2id(4 << 20, 1, 1, 32), "4194304 KiB of memory");
    refused(argon2id(MAX_MEMORY_KIB, 3, 1, 32), "3 passes");
    refused(argon2id(1 << 20, 5, 1, 32), "5 passes");
    refused(argon2id(i64::MAX, i64::MAX, 1, 32), "KiB of memory");
    refused(argon2id(64, i64::MAX, 1, 32), "passes");
    refused(argon2id(64, 0, 1, 32), "0 passes");
    refused(argon2id(64, 1, 0, 32), "0 lanes");
    refused(argon2id(8 * (MAX_LANES + 1), 1, MAX_LANES + 1, 32), "lanes");
    refused(argon2id(31, 1, 4, 32), "31 KiB of memory");
    refused(argon2id(64, 1, 1, 3), "a hash of 3 bytes");
    refused(argon2id(64, 1, 1, MAX_LENGTH + 1), "a hash of 65 bytes");
    refused(argon2id(64, 1, 1, -1), "a hash of -1 bytes");
    refused(
        Work::argon2("argon2id", PASSWORD, &[2; 7], b"", b"", 64, 1, 1, 32),
        "a salt of 7 bytes",
    );
    refused(
        Work::argon2("argon2id", PASSWORD, &[2; 16], b"", &[4; 33], 64, 1, 1, 32),
        "33 bytes of associated data",
    );
    refused(
        Work::argon2("argon2", PASSWORD, &[2; 16], b"", b"", 64, 1, 1, 32),
        "a variant",
    );
}

#[test]
fn scrypt_is_checked_against_its_bounds_before_anything_is_computed() {
    let asked = |log_n: i64, r: i64, p: i64| Work::scrypt(PASSWORD, b"salt", log_n, r, p, 32);
    assert_eq!(MAX_SCRYPT_WORK, 1 << 23);
    assert!(asked(20, 8, 1).is_ok());
    assert!(asked(17, 8, 8).is_ok());
    assert!(asked(1, 1, 1 << 22).is_ok());

    refused(asked(21, 8, 1), "N = 2^21, r = 8 and p = 1");
    refused(asked(20, 8, 2), "their product");
    refused(asked(20, 9, 1), "their product");
    refused(asked(62, i64::MAX, i64::MAX), "their product");
    refused(asked(63, 1, 1), "N = 2^63");
    refused(asked(0, 8, 1), "N = 2^0");
    refused(asked(14, 0, 1), "r = 0");
    refused(asked(14, 8, 0), "p = 0");
    refused(asked(14, 8, -1), "p = -1");
    refused(
        Work::scrypt(PASSWORD, b"salt", 14, 8, 1, 0),
        "a hash of 0 bytes",
    );
    refused(
        Work::scrypt(PASSWORD, b"salt", 14, 8, 1, MAX_LENGTH + 1),
        "a hash of 65 bytes",
    );
}

#[test]
fn bcrypt_and_pbkdf2_are_checked_against_their_bounds_before_anything_is_computed() {
    let salt = [7u8; 16];
    assert!(Work::bcrypt(PASSWORD, &salt, MAX_BCRYPT_COST).is_ok());
    refused(
        Work::bcrypt(PASSWORD, &salt, MAX_BCRYPT_COST + 1),
        "a cost of 17",
    );
    refused(Work::bcrypt(PASSWORD, &salt, 31), "a cost of 31");
    refused(Work::bcrypt(PASSWORD, &salt, 3), "a cost of 3");
    refused(
        Work::bcrypt(PASSWORD, &salt[..15], 10),
        "a salt of 15 bytes",
    );

    assert!(Work::pbkdf2("SHA-256", PASSWORD, b"salt", MAX_PBKDF2_ITERATIONS, 32).is_ok());
    assert!(Work::pbkdf2("SHA-512", PASSWORD, b"", 1, MAX_LENGTH).is_ok());
    refused(
        Work::pbkdf2("SHA-256", PASSWORD, b"salt", MAX_PBKDF2_ITERATIONS + 1, 32),
        "10000001 iterations",
    );
    refused(
        Work::pbkdf2("SHA-256", PASSWORD, b"salt", 0, 32),
        "0 iterations",
    );
    refused(
        Work::pbkdf2("SHA-256", PASSWORD, b"salt", i64::MAX, 32),
        "iterations",
    );
    refused(
        Work::pbkdf2("SHA-256", PASSWORD, b"salt", 1, 0),
        "a hash of 0 bytes",
    );
    refused(
        Work::pbkdf2("SHA3-256", PASSWORD, b"salt", 1, 32),
        "a hash that is none of",
    );
}
