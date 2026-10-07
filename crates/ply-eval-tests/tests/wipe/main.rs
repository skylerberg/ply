//! A binary of its own: it installs a `#[global_allocator]` that, while a test watches, keeps every
//! block allocated and looks at each one as it is freed, so the test can say whether a credential's
//! bytes are anywhere on the heap and whether a block that held them was given back unwiped.

use ply_eval::builtins::{Builtin, call};
use ply_eval::crypto::Framing;
use ply_eval::{Span, Value};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};

const SLOTS: usize = 1 << 18;
const EMPTY: usize = 0;
const GONE: usize = 1;
const MARK: usize = 64;

static PTRS: [AtomicUsize; SLOTS] = [const { AtomicUsize::new(EMPTY) }; SLOTS];
static SIZES: [AtomicUsize; SLOTS] = [const { AtomicUsize::new(0) }; SLOTS];
static MARKER: [AtomicU8; MARK] = [const { AtomicU8::new(0) }; MARK];
static MARKER_LEN: AtomicUsize = AtomicUsize::new(0);
static WATCHING: AtomicBool = AtomicBool::new(false);
static LOCK: AtomicBool = AtomicBool::new(false);
static FULL: AtomicBool = AtomicBool::new(false);
static FREED_HOLDING: AtomicUsize = AtomicUsize::new(0);

struct Watching;

#[global_allocator]
static ALLOCATOR: Watching = Watching;

fn lock() {
    while LOCK
        .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        std::hint::spin_loop();
    }
}

fn unlock() {
    LOCK.store(false, Ordering::Release);
}

fn home(ptr: usize) -> usize {
    ((ptr >> 4).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 46) & (SLOTS - 1)
}

/// Whether `size` bytes at `ptr` hold the marker. Called under the lock, allocating nothing.
unsafe fn holds(ptr: *const u8, size: usize) -> bool {
    let len = MARKER_LEN.load(Ordering::Relaxed);
    if len == 0 || size < len {
        return false;
    }
    (0..=size - len).any(|at| {
        (0..len).all(|i| {
            let byte: u8 = unsafe { std::ptr::read_volatile(ptr.add(at + i)) };
            byte == MARKER[i].load(Ordering::Relaxed)
        })
    })
}

unsafe impl GlobalAlloc for Watching {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() && WATCHING.load(Ordering::Relaxed) {
            lock();
            let mut at = home(ptr as usize);
            let mut placed = false;
            for _ in 0..SLOTS {
                let held = PTRS[at].load(Ordering::Relaxed);
                if held == EMPTY || held == GONE {
                    PTRS[at].store(ptr as usize, Ordering::Relaxed);
                    SIZES[at].store(layout.size(), Ordering::Relaxed);
                    placed = true;
                    break;
                }
                at = (at + 1) & (SLOTS - 1);
            }
            if !placed {
                FULL.store(true, Ordering::Relaxed);
            }
            unlock();
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        lock();
        let mut at = home(ptr as usize);
        for _ in 0..SLOTS {
            let held = PTRS[at].load(Ordering::Relaxed);
            if held == EMPTY {
                break;
            }
            if held == ptr as usize {
                PTRS[at].store(GONE, Ordering::Relaxed);
                if unsafe { holds(ptr, SIZES[at].load(Ordering::Relaxed)) } {
                    FREED_HOLDING.fetch_add(1, Ordering::Relaxed);
                }
                break;
            }
            at = (at + 1) & (SLOTS - 1);
        }
        unlock();
        unsafe { System.dealloc(ptr, layout) }
    }
}

/// From here on every block allocated is kept, and `marker` is what is looked for.
fn watch(marker: &[u8]) {
    assert!(marker.len() <= MARK);
    lock();
    for slot in &PTRS {
        slot.store(EMPTY, Ordering::Relaxed);
    }
    for (i, b) in marker.iter().enumerate() {
        MARKER[i].store(*b, Ordering::Relaxed);
    }
    MARKER_LEN.store(marker.len(), Ordering::Relaxed);
    FREED_HOLDING.store(0, Ordering::Relaxed);
    FULL.store(false, Ordering::Relaxed);
    WATCHING.store(true, Ordering::Relaxed);
    unlock();
}

/// How many blocks kept since `watch` hold the marker now, and how many held it when freed.
fn seen() -> (usize, usize) {
    lock();
    let mut live = 0;
    for (slot, size) in PTRS.iter().zip(&SIZES) {
        let ptr = slot.load(Ordering::Relaxed);
        if ptr > GONE && unsafe { holds(ptr as *const u8, size.load(Ordering::Relaxed)) } {
            live += 1;
        }
    }
    let freed = FREED_HOLDING.load(Ordering::Relaxed);
    unlock();
    assert!(!FULL.load(Ordering::Relaxed), "the watch table filled");
    (live, freed)
}

fn stop() {
    lock();
    WATCHING.store(false, Ordering::Relaxed);
    MARKER_LEN.store(0, Ordering::Relaxed);
    unlock();
}

fn done(b: Builtin, args: Vec<Value>) -> Value {
    call(b, args, Span::DUMMY).expect("the builtin answers")
}

fn held(v: Value) -> Value {
    match &v {
        Value::Ctor { name, args } if name.as_str() == "Some" => args[0].clone(),
        other => panic!("no answer: {other:?}"),
    }
}

/// A path a secret takes: what it is called, the bytes looked for, and what it makes of the plain
/// values it is handed, every one of them made before the watch starts.
struct Path {
    name: &'static str,
    marker: Vec<u8>,
    plain: Vec<Value>,
    made: fn(&[Value]) -> Vec<Value>,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn rsa_document() -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../ply-corpus/stdlib/asn1/rsa_key.der"),
    )
    .expect("the corpus's RSA key")
}

fn paths() -> Vec<Path> {
    let key: Vec<u8> = (0..32).map(|i| 0xa0 ^ (i * 7) as u8).collect();
    let salt = Value::bytes(b"salt");
    let mut derived = [0u8; 32];
    ring::pbkdf2::derive(
        ring::pbkdf2::PBKDF2_HMAC_SHA256,
        std::num::NonZeroU32::new(2).unwrap(),
        b"salt",
        &key,
        &mut derived,
    );
    let mut expanded = [0u8; 32];
    ring::hkdf::Prk::new_less_safe(ring::hkdf::HKDF_SHA256, &key)
        .expand(&[b"info"], ring::hkdf::HKDF_SHA256)
        .unwrap()
        .fill(&mut expanded)
        .unwrap();
    let document = rsa_document();
    let mut pkcs8 = vec![
        0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04,
        0x20,
    ];
    pkcs8.extend(&key);
    vec![
        Path {
            name: "sealed from bytes, cloned, and held in a list and a map",
            marker: key.clone(),
            plain: vec![Value::bytes(&key)],
            made: |plain| {
                let secret = done(Builtin::SecretOfBytes, vec![plain[0].clone()]);
                vec![
                    secret.clone(),
                    Value::list(vec![secret.clone()]),
                    Value::map([(Value::Int(1), secret)]),
                ]
            },
        },
        Path {
            name: "sealed from a string and turned to bytes",
            marker: hex(&key).into_bytes(),
            plain: vec![Value::str(hex(&key))],
            made: |plain| {
                let text = done(Builtin::SecretOfString, vec![plain[0].clone()]);
                vec![done(Builtin::SecretBytes, vec![text.clone()]), text]
            },
        },
        Path {
            name: "joined from two halves",
            marker: key.clone(),
            plain: vec![Value::bytes(&key[..16]), Value::bytes(&key[16..])],
            made: |plain| {
                let a = done(Builtin::SecretOfBytes, vec![plain[0].clone()]);
                let b = done(Builtin::SecretOfBytes, vec![plain[1].clone()]);
                vec![done(Builtin::SecretConcat, vec![a, b])]
            },
        },
        Path {
            name: "decoded from hex and encoded back",
            marker: key.clone(),
            plain: vec![Value::str(hex(&key))],
            made: |plain| {
                let text = done(Builtin::SecretOfString, vec![plain[0].clone()]);
                let bytes = held(done(Builtin::SecretDecode, vec![Value::str("hex"), text]));
                vec![
                    done(
                        Builtin::SecretEncode,
                        vec![Value::str("base64"), bytes.clone()],
                    ),
                    bytes,
                ]
            },
        },
        Path {
            name: "framed as a host operation writes it and read back out of the frame",
            marker: key.clone(),
            plain: vec![Value::bytes(&key)],
            made: |plain| {
                let secret = done(Builtin::SecretOfBytes, vec![plain[0].clone()]);
                let Value::Secret(held) = &secret else {
                    panic!("a secret")
                };
                let mut out = Vec::new();
                for name in ["raw", "hex", "base64"] {
                    let framing = Framing::named(name, "a frame", Span::DUMMY).expect("a framing");
                    let line = framing.framed(b"key ", held.bytes(), b"\n");
                    let back = framing
                        .unframed(&line, b"key ", b"\n")
                        .expect("its own frame");
                    out.push(Value::secret_bytes(&back));
                }
                out.push(secret);
                out
            },
        },
        Path {
            name: "derived by PBKDF2",
            marker: derived.to_vec(),
            plain: vec![Value::bytes(&key), salt],
            made: |plain| {
                let password = done(Builtin::SecretOfBytes, vec![plain[0].clone()]);
                vec![held(done(
                    Builtin::SecretPbkdf2,
                    vec![
                        Value::str("SHA-256"),
                        password,
                        plain[1].clone(),
                        Value::Int(2),
                        Value::Int(32),
                    ],
                ))]
            },
        },
        Path {
            name: "expanded by HKDF",
            marker: expanded.to_vec(),
            plain: vec![Value::bytes(&key)],
            made: |plain| {
                let prk = done(Builtin::SecretOfBytes, vec![plain[0].clone()]);
                vec![held(done(
                    Builtin::SecretHkdfExpand,
                    vec![
                        Value::str("SHA-256"),
                        prk,
                        Value::bytes(b"info"),
                        Value::Int(32),
                    ],
                ))]
            },
        },
        Path {
            name: "read out of a PKCS #8 document, then signed and agreed with",
            marker: key.clone(),
            plain: vec![Value::bytes(&pkcs8)],
            made: |plain| {
                let document = done(Builtin::SecretOfBytes, vec![plain[0].clone()]);
                let found = held(done(Builtin::SecretPrivateKey, vec![document]));
                let Value::Record(found) = &found else {
                    panic!("a record")
                };
                let seed = found
                    .into_iter()
                    .find(|(k, _)| k.as_str() == "key")
                    .map(|(_, v)| v.clone())
                    .unwrap();
                let signature = done(
                    Builtin::CryptoSign,
                    vec![Value::str("ed25519"), seed.clone(), Value::bytes(b"m")],
                );
                let public = done(
                    Builtin::CryptoPublic,
                    vec![Value::str("x25519"), seed.clone()],
                );
                let shared = done(
                    Builtin::CryptoAgree,
                    vec![Value::str("x25519"), seed.clone(), held(public)],
                );
                vec![seed, signature, shared]
            },
        },
        Path {
            name: "an RSA document read and signed with under PSS",
            marker: document[document.len() - 48..].to_vec(),
            plain: vec![Value::bytes(&document)],
            made: |plain| {
                let document = done(Builtin::SecretOfBytes, vec![plain[0].clone()]);
                let signature = done(
                    Builtin::CryptoSign,
                    vec![
                        Value::str("rsa-pss-sha256"),
                        document.clone(),
                        Value::bytes(b"m"),
                    ],
                );
                vec![document, signature]
            },
        },
        Path {
            name: "an RSA document made of a JSON Web Key's numbers",
            marker: document[document.len() - 48..].to_vec(),
            plain: jwk_texts(&document),
            made: |plain| {
                let texts = plain[2..]
                    .iter()
                    .map(|text| done(Builtin::SecretOfString, vec![text.clone()]))
                    .collect();
                let args = vec![plain[0].clone(), plain[1].clone(), Value::list(texts)];
                vec![held(done(Builtin::SecretRsaJwk, args))]
            },
        },
    ]
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
        at += 1;
        let n = length(der, &mut at);
        out.push(
            der[at..at + n]
                .iter()
                .skip_while(|b| **b == 0)
                .copied()
                .collect(),
        );
        at += n;
    }
    out
}

/// `n` and `e` as bytes, then each private number of a PKCS #1 document as its base64url text.
fn jwk_texts(document: &[u8]) -> Vec<Value> {
    let numbers = integers(document);
    let mut out = vec![Value::bytes(&numbers[1]), Value::bytes(&numbers[2])];
    for number in &numbers[3..] {
        let sealed = done(
            Builtin::SecretEncode,
            vec![Value::str("base64url"), Value::secret_bytes(number)],
        );
        let Value::Secret(text) = &sealed else {
            panic!("a secret")
        };
        out.push(Value::str(text.text().expect("a text")));
    }
    out
}

/// One test at a time: the watch is the process's.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn a_dropped_secret_leaves_no_copy_of_its_bytes_on_the_heap() {
    let _one = SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for path in paths() {
        watch(&path.marker);
        let made = (path.made)(&path.plain);
        let (live, freed) = seen();
        assert!(
            live > 0,
            "{}: the watch saw no copy while the secret was held, so it proves nothing",
            path.name
        );
        assert_eq!(freed, 0, "{}: a copy was freed unwiped", path.name);
        drop(made);
        let (live, freed) = seen();
        stop();
        assert_eq!(live, 0, "{}: a copy outlived the secret", path.name);
        assert_eq!(freed, 0, "{}: a copy was freed unwiped", path.name);
    }
}

#[test]
fn the_watch_sees_a_plain_copy_freed_unwiped() {
    let _one = SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let marker = b"a plain value, freed as it stands";
    watch(marker);
    let plain = Value::bytes(marker);
    let (live, _) = seen();
    drop(plain);
    let (after, freed) = seen();
    stop();
    assert_eq!((live, after, freed), (1, 0, 1));
}
