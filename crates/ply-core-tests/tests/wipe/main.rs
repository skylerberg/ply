use ply_core::Span;
use ply_core::crypto::{self, Framing};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use zeroize::Zeroizing;

const SLOTS: usize = 1 << 18;
const EMPTY: usize = 0;
const FREED: usize = 1;
const MARKER_CAPACITY: usize = 64;

static PTRS: [AtomicUsize; SLOTS] = [const { AtomicUsize::new(EMPTY) }; SLOTS];
static SIZES: [AtomicUsize; SLOTS] = [const { AtomicUsize::new(0) }; SLOTS];
static MARKER: [AtomicU8; MARKER_CAPACITY] = [const { AtomicU8::new(0) }; MARKER_CAPACITY];
static MARKER_LEN: AtomicUsize = AtomicUsize::new(0);
static WATCHING: AtomicBool = AtomicBool::new(false);
static LOCK: AtomicBool = AtomicBool::new(false);
static FULL: AtomicBool = AtomicBool::new(false);
static FREED_WITH_MARKER: AtomicUsize = AtomicUsize::new(0);

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
                if held == EMPTY || held == FREED {
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
                PTRS[at].store(FREED, Ordering::Relaxed);
                if unsafe { holds(ptr, SIZES[at].load(Ordering::Relaxed)) } {
                    FREED_WITH_MARKER.fetch_add(1, Ordering::Relaxed);
                }
                break;
            }
            at = (at + 1) & (SLOTS - 1);
        }
        unlock();
        unsafe { System.dealloc(ptr, layout) }
    }
}

fn watch(marker: &[u8]) {
    assert!(marker.len() <= MARKER_CAPACITY);
    lock();
    for slot in &PTRS {
        slot.store(EMPTY, Ordering::Relaxed);
    }
    for (i, b) in marker.iter().enumerate() {
        MARKER[i].store(*b, Ordering::Relaxed);
    }
    MARKER_LEN.store(marker.len(), Ordering::Relaxed);
    FREED_WITH_MARKER.store(0, Ordering::Relaxed);
    FULL.store(false, Ordering::Relaxed);
    WATCHING.store(true, Ordering::Relaxed);
    unlock();
}

fn seen() -> (usize, usize) {
    lock();
    let mut live = 0;
    for (slot, size) in PTRS.iter().zip(&SIZES) {
        let ptr = slot.load(Ordering::Relaxed);
        if ptr > FREED && unsafe { holds(ptr as *const u8, size.load(Ordering::Relaxed)) } {
            live += 1;
        }
    }
    let freed = FREED_WITH_MARKER.load(Ordering::Relaxed);
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

type Wiped = Zeroizing<Vec<u8>>;

struct Path {
    name: &'static str,
    marker: Vec<u8>,
    plain: Vec<Vec<u8>>,
    made: fn(&[Vec<u8>]) -> Vec<Wiped>,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn rsa_document() -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rsa_key.der"),
    )
    .expect("the tests' RSA key")
}

fn paths() -> Vec<Path> {
    let key: Vec<u8> = (0..32).map(|i| 0xa0 ^ (i * 7) as u8).collect();
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
            name: "decoded from hex and encoded back",
            marker: key.clone(),
            plain: vec![hex(&key).into_bytes()],
            made: |plain| {
                let bytes = crypto::decode("hex", &plain[0]).expect("hex");
                let text = crypto::encode("base64", &bytes).expect("base64");
                vec![bytes, text]
            },
        },
        Path {
            name: "framed as a host operation writes it and read back out of the frame",
            marker: key.clone(),
            plain: vec![key.clone()],
            made: |plain| {
                let mut out = vec![Zeroizing::new(plain[0].clone())];
                for name in ["raw", "hex", "base64"] {
                    let framing = Framing::named(name, "a frame", Span::DUMMY).expect("a framing");
                    let line = framing.framed(b"key ", &plain[0], b"\n");
                    let framing = Framing::named(name, "a frame", Span::DUMMY).expect("a framing");
                    out.push(
                        framing
                            .decode_framed(&line, b"key ", b"\n")
                            .expect("its own frame"),
                    );
                    out.push(line);
                }
                out
            },
        },
        Path {
            name: "derived by PBKDF2",
            marker: derived.to_vec(),
            plain: vec![key.clone()],
            made: |plain| {
                vec![crypto::pbkdf2("SHA-256", &plain[0], b"salt", 2, 32).expect("a key")]
            },
        },
        Path {
            name: "expanded by HKDF",
            marker: expanded.to_vec(),
            plain: vec![key.clone()],
            made: |plain| {
                vec![crypto::hkdf_expand("SHA-256", &plain[0], b"info", 32).expect("a key")]
            },
        },
        Path {
            name: "read out of a PKCS #8 document, then signed and agreed with",
            marker: key.clone(),
            plain: vec![pkcs8],
            made: |plain| {
                let (_, seed) = crypto::private_key(&plain[0]).expect("a private key");
                let signature = crypto::sign("ed25519", &seed, b"m")
                    .expect("a scheme")
                    .expect("a signature");
                let public = crypto::public("x25519", &seed)
                    .expect("a scheme")
                    .expect("a public key");
                let shared = crypto::agree("x25519", &seed, &public)
                    .expect("a scheme")
                    .expect("a shared secret");
                vec![seed, Zeroizing::new(signature), shared]
            },
        },
        Path {
            name: "an RSA document read and signed with under PSS",
            marker: document[document.len() - 48..].to_vec(),
            plain: vec![document.clone()],
            made: |plain| {
                let held = Zeroizing::new(plain[0].clone());
                let signature = crypto::sign("rsa-pss-sha256", &held, b"m")
                    .expect("a scheme")
                    .expect("a signature");
                vec![held, Zeroizing::new(signature)]
            },
        },
        Path {
            name: "an RSA document made of a JSON Web Key's numbers",
            marker: document[document.len() - 48..].to_vec(),
            plain: jwk_texts(&document),
            made: |plain| {
                let private = [
                    &plain[2][..],
                    &plain[3][..],
                    &plain[4][..],
                    &plain[5][..],
                    &plain[6][..],
                    &plain[7][..],
                ];
                vec![crypto::rsa_jwk(&plain[0], &plain[1], private).expect("a key")]
            },
        },
    ]
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

fn jwk_texts(document: &[u8]) -> Vec<Vec<u8>> {
    let numbers = integers(document);
    let mut out = vec![numbers[1].clone(), numbers[2].clone()];
    for number in &numbers[3..] {
        out.push(
            crypto::encode("base64url", number)
                .expect("base64url")
                .to_vec(),
        );
    }
    out
}

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
fn the_memory_wipe_watch_reports_a_freed_ordinary_copy_of_a_value_as_never_zeroed() {
    let _one = SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let marker = b"a plain value, freed as it stands";
    watch(marker);
    let plain = marker.to_vec();
    let (live, _) = seen();
    drop(plain);
    let (after, freed) = seen();
    stop();
    assert_eq!((live, after, freed), (1, 0, 1));
}
