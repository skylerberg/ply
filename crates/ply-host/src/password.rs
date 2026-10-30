//! The functions a password is hashed with, which `std.password`'s natives run.
//!
//! Argon2, scrypt, bcrypt and PBKDF2 are each a function of what they are handed, and each is
//! built to take a long time, so it runs as a job of the process's computing pool while the task
//! that asked waits. A password is copied once, for that job, into a buffer that is wiped when the
//! hash is made.
//!
//! Every number that multiplies the work is bounded before a job is started: a program may perform
//! these directly, and the parameters of a stored hash are whatever its writer chose.

use ply_core::{Diagnostic, Span, codes};
use std::num::NonZeroU32;
use zeroize::Zeroizing;

/// The most memory one Argon2 hash fills, in KiB: 2 GiB, RFC 9106's first recommended option.
pub const MAX_MEMORY_KIB: i64 = 1 << 21;

/// The most KiB one Argon2 hash fills over all its passes: 1 GiB four times.
pub const MAX_ARGON2_WORK: i64 = 1 << 22;

/// The lanes a PHC string can name.
pub const MAX_LANES: i64 = 255;

/// The most `N * r * p` of one scrypt hash, which bounds its memory at `128 * r * N` bytes too:
/// 1 GiB filled once.
pub const MAX_SCRYPT_WORK: i64 = 1 << 23;

/// The highest bcrypt cost, each one more doubling the work.
pub const MAX_BCRYPT_COST: i64 = 16;

pub const MAX_PBKDF2_ITERATIONS: i64 = 10_000_000;

/// The longest hash any of these returns, in bytes.
pub const MAX_LENGTH: i64 = 64;

/// The bytes of a password bcrypt reads, its terminating zero among them.
pub const BCRYPT_KEY_BYTES: usize = 72;

/// One hash to make, checked against the bounds, holding its own copy of everything it reads.
pub struct Work(Kind);

enum Kind {
    Argon2 {
        algorithm: argon2::Algorithm,
        params: argon2::Params,
        password: Zeroizing<Vec<u8>>,
        salt: Vec<u8>,
        key: Zeroizing<Vec<u8>>,
        length: usize,
    },
    Scrypt {
        params: scrypt::Params,
        password: Zeroizing<Vec<u8>>,
        salt: Vec<u8>,
        length: usize,
    },
    Bcrypt {
        cost: u32,
        key: Zeroizing<Vec<u8>>,
        salt: [u8; 16],
    },
    Pbkdf2 {
        algorithm: ring::pbkdf2::Algorithm,
        iterations: NonZeroU32,
        password: Zeroizing<Vec<u8>>,
        salt: Vec<u8>,
        length: usize,
    },
}

/// Argon2 version 1.3 as RFC 9106 defines it: `variant` is `argon2d`, `argon2i` or `argon2id`,
/// `key` and `data` its optional secret and associated data, `memory` in KiB.
///
/// # Errors
/// What was asked that the algorithm does not define or that is past a bound of this module,
/// before anything is computed.
#[allow(clippy::too_many_arguments)]
pub fn argon2(
    variant: &str,
    password: &[u8],
    salt: &[u8],
    key: &[u8],
    data: &[u8],
    memory: i64,
    passes: i64,
    lanes: i64,
    length: i64,
) -> Result<Vec<u8>, String> {
    Work::argon2(
        variant, password, salt, key, data, memory, passes, lanes, length,
    )?
    .run()
}

/// scrypt as RFC 7914 defines it, with `N = 2^log_n`.
///
/// # Errors
/// As [`argon2`].
pub fn scrypt(
    password: &[u8],
    salt: &[u8],
    log_n: i64,
    r: i64,
    p: i64,
    length: i64,
) -> Result<Vec<u8>, String> {
    Work::scrypt(password, salt, log_n, r, p, length)?.run()
}

/// The twenty-three bytes a bcrypt hash string carries, for a sixteen-byte salt. The key is the
/// password and a terminating zero, cut to [`BCRYPT_KEY_BYTES`], which is how `$2b$` reads one.
///
/// # Errors
/// As [`argon2`].
pub fn bcrypt(password: &[u8], salt: &[u8], cost: i64) -> Result<Vec<u8>, String> {
    Work::bcrypt(password, salt, cost)?.run()
}

/// PBKDF2 under HMAC with the hash FIPS 180 names `hash`.
///
/// # Errors
/// As [`argon2`].
pub fn pbkdf2(
    hash: &str,
    password: &[u8],
    salt: &[u8],
    iterations: i64,
    length: i64,
) -> Result<Vec<u8>, String> {
    Work::pbkdf2(hash, password, salt, iterations, length)?.run()
}

impl Work {
    /// What [`argon2`] would compute, checked and not yet computed.
    ///
    /// # Errors
    /// As [`argon2`].
    #[allow(clippy::too_many_arguments)]
    pub fn argon2(
        variant: &str,
        password: &[u8],
        salt: &[u8],
        key: &[u8],
        data: &[u8],
        memory: i64,
        passes: i64,
        lanes: i64,
        length: i64,
    ) -> Result<Work, String> {
        let algorithm = match variant {
            "argon2d" => argon2::Algorithm::Argon2d,
            "argon2i" => argon2::Algorithm::Argon2i,
            "argon2id" => argon2::Algorithm::Argon2id,
            _ => {
                return Err(
                    "a variant that is none of `argon2d`, `argon2i` and `argon2id`".to_string(),
                );
            }
        };
        if !(1..=MAX_LANES).contains(&lanes) {
            return Err(format!("{lanes} lanes, and a hash has 1 to {MAX_LANES}"));
        }
        if memory < 8 * lanes {
            return Err(format!(
                "{memory} KiB of memory, and {lanes} lane(s) take at least {}",
                8 * lanes
            ));
        }
        if memory > MAX_MEMORY_KIB {
            return Err(format!(
                "{memory} KiB of memory, and one hash fills at most {MAX_MEMORY_KIB}"
            ));
        }
        if passes < 1 {
            return Err(format!("{passes} passes, and a hash makes at least 1"));
        }
        if memory.saturating_mul(passes) > MAX_ARGON2_WORK {
            return Err(format!(
                "{passes} passes over {memory} KiB, and one hash fills at most {MAX_ARGON2_WORK} KiB over all its passes"
            ));
        }
        let length = length_in(length, argon2::Params::MIN_OUTPUT_LEN)?;
        if salt.len() < argon2::MIN_SALT_LEN {
            return Err(format!(
                "a salt of {} bytes, and Argon2 takes at least {}",
                salt.len(),
                argon2::MIN_SALT_LEN
            ));
        }
        let mut builder = argon2::ParamsBuilder::new();
        builder
            .m_cost(narrow(memory)?)
            .t_cost(narrow(passes)?)
            .p_cost(narrow(lanes)?)
            .output_len(length)
            .data(argon2::AssociatedData::new(data).map_err(|_| {
                format!(
                    "{} bytes of associated data, and Argon2 takes at most {}",
                    data.len(),
                    argon2::Params::MAX_DATA_LEN
                )
            })?);
        let params = builder.build().map_err(|e| e.to_string())?;
        Ok(Work(Kind::Argon2 {
            algorithm,
            params,
            password: Zeroizing::new(password.to_vec()),
            salt: salt.to_vec(),
            key: Zeroizing::new(key.to_vec()),
            length,
        }))
    }

    /// What [`scrypt`] would compute, checked and not yet computed.
    ///
    /// # Errors
    /// As [`argon2`].
    pub fn scrypt(
        password: &[u8],
        salt: &[u8],
        log_n: i64,
        r: i64,
        p: i64,
        length: i64,
    ) -> Result<Work, String> {
        if !(1..=62).contains(&log_n) || r < 1 || p < 1 {
            return Err(format!(
                "N = 2^{log_n}, r = {r} and p = {p}, and each is at least 1 with N at least 2"
            ));
        }
        let work = (1i64 << log_n)
            .checked_mul(r)
            .and_then(|nr| nr.checked_mul(p));
        if work.is_none_or(|work| work > MAX_SCRYPT_WORK) {
            return Err(format!(
                "N = 2^{log_n}, r = {r} and p = {p}, and their product is at most {MAX_SCRYPT_WORK}"
            ));
        }
        let length = length_in(length, 1)?;
        let log_n = u8::try_from(log_n).map_err(|e| e.to_string())?;
        let params = scrypt::Params::new(log_n, narrow(r)?, narrow(p)?)
            .map_err(|_| "parameters scrypt does not define".to_string())?;
        Ok(Work(Kind::Scrypt {
            params,
            password: Zeroizing::new(password.to_vec()),
            salt: salt.to_vec(),
            length,
        }))
    }

    /// What [`bcrypt`] would compute, checked and not yet computed.
    ///
    /// # Errors
    /// As [`argon2`].
    pub fn bcrypt(password: &[u8], salt: &[u8], cost: i64) -> Result<Work, String> {
        if !(4..=MAX_BCRYPT_COST).contains(&cost) {
            return Err(format!(
                "a cost of {cost}, and a hash costs 4 to {MAX_BCRYPT_COST}"
            ));
        }
        let salt: [u8; 16] = salt.try_into().map_err(|_| {
            format!(
                "a salt of {} bytes, and bcrypt takes exactly 16",
                salt.len()
            )
        })?;
        let mut key = Zeroizing::new(Vec::with_capacity(BCRYPT_KEY_BYTES));
        key.extend(password.iter().take(BCRYPT_KEY_BYTES));
        if key.len() < BCRYPT_KEY_BYTES {
            key.push(0);
        }
        Ok(Work(Kind::Bcrypt {
            cost: narrow(cost)?,
            key,
            salt,
        }))
    }

    /// What [`pbkdf2`] would compute, checked and not yet computed.
    ///
    /// # Errors
    /// As [`argon2`].
    pub fn pbkdf2(
        hash: &str,
        password: &[u8],
        salt: &[u8],
        iterations: i64,
        length: i64,
    ) -> Result<Work, String> {
        let algorithm = match hash {
            "SHA-1" => ring::pbkdf2::PBKDF2_HMAC_SHA1,
            "SHA-256" => ring::pbkdf2::PBKDF2_HMAC_SHA256,
            "SHA-384" => ring::pbkdf2::PBKDF2_HMAC_SHA384,
            "SHA-512" => ring::pbkdf2::PBKDF2_HMAC_SHA512,
            _ => {
                return Err(
                    "a hash that is none of `SHA-1`, `SHA-256`, `SHA-384` and `SHA-512`"
                        .to_string(),
                );
            }
        };
        if !(1..=MAX_PBKDF2_ITERATIONS).contains(&iterations) {
            return Err(format!(
                "{iterations} iterations, and a hash makes 1 to {MAX_PBKDF2_ITERATIONS}"
            ));
        }
        let iterations =
            NonZeroU32::new(narrow(iterations)?).ok_or_else(|| "no iterations".to_string())?;
        Ok(Work(Kind::Pbkdf2 {
            algorithm,
            iterations,
            password: Zeroizing::new(password.to_vec()),
            salt: salt.to_vec(),
            length: length_in(length, 1)?,
        }))
    }

    /// The hash, on the thread that calls this.
    ///
    /// # Errors
    /// The memory the hash fills could not be had.
    pub fn run(self) -> Result<Vec<u8>, String> {
        match self.0 {
            Kind::Argon2 {
                algorithm,
                params,
                password,
                salt,
                key,
                length,
            } => {
                let version = argon2::Version::V0x13;
                let context = if key.is_empty() {
                    argon2::Argon2::new(algorithm, version, params)
                } else {
                    argon2::Argon2::new_with_secret(&key, algorithm, version, params)
                        .map_err(|e| e.to_string())?
                };
                let mut tag = vec![0; length];
                context
                    .hash_password_into(&password, &salt, &mut tag)
                    .map_err(|e| e.to_string())?;
                Ok(tag)
            }
            Kind::Scrypt {
                params,
                password,
                salt,
                length,
            } => {
                let mut tag = vec![0; length];
                scrypt::scrypt(&password, &salt, &params, &mut tag).map_err(|e| e.to_string())?;
                Ok(tag)
            }
            Kind::Bcrypt { cost, key, salt } => {
                let mut state = blowfish::Blowfish::bc_init_state();
                state.salted_expand_key(&salt, &key);
                for _ in 0..1u32 << cost {
                    state.bc_expand_key(&key);
                    state.bc_expand_key(&salt);
                }
                // "OrpheanBeholderScryDoubt", which bcrypt encrypts sixty-four times.
                let mut text: [u32; 6] = [
                    0x4f72_7068,
                    0x6561_6e42,
                    0x6568_6f6c,
                    0x6465_7253,
                    0x6372_7944,
                    0x6f75_6274,
                ];
                for pair in text.chunks_exact_mut(2) {
                    for _ in 0..64 {
                        let [left, right] = state.bc_encrypt([pair[0], pair[1]]);
                        pair[0] = left;
                        pair[1] = right;
                    }
                }
                let mut tag: Vec<u8> = text.iter().flat_map(|word| word.to_be_bytes()).collect();
                // The hash string has room for twenty-three of the twenty-four.
                tag.truncate(23);
                Ok(tag)
            }
            Kind::Pbkdf2 {
                algorithm,
                iterations,
                password,
                salt,
                length,
            } => {
                let mut tag = vec![0; length];
                ring::pbkdf2::derive(algorithm, iterations, &salt, &password, &mut tag);
                Ok(tag)
            }
        }
    }
}

fn length_in(length: i64, least: usize) -> Result<usize, String> {
    usize::try_from(length)
        .ok()
        .filter(|n| *n >= least && length <= MAX_LENGTH)
        .ok_or_else(|| format!("a hash of {length} bytes, and one is {least} to {MAX_LENGTH}"))
}

fn narrow(n: i64) -> Result<u32, String> {
    u32::try_from(n).map_err(|_| format!("{n}, which no parameter of a hash reaches"))
}

/// The rejection of `what` (`kdf.argon2`, ..) for `reason`, which ends the run where it was asked.
#[cold]
pub fn rejected(what: &str, reason: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::RUNTIME_ERROR,
        format!("{what} was asked for {reason}"),
    )
    .primary(span, "performed here")
    .note("the host bounds what one hash may cost before it starts one")
    .note("`std.password` holds the parameters of a stored hash to the same bounds and answers a refusal, so only a perform of the operation itself reaches this")
}
