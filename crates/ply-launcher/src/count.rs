use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counted {
    pub allocations: u64,
    pub bytes: u64,
}

static ON: AtomicBool = AtomicBool::new(false);
static ATTRIBUTE: AtomicBool = AtomicBool::new(false);
static SITE_WALK_EVERY: AtomicU32 = AtomicU32::new(0);
static SEEN: AtomicU64 = AtomicU64::new(0);
static ALLOCS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);
static ATTRIBUTIONS_IN_FLIGHT: AtomicU64 = AtomicU64::new(0);
static SITES: Mutex<BTreeMap<String, Counted>> = Mutex::new(BTreeMap::new());
static SYMBOL_NAMES_BY_ADDRESS: Mutex<BTreeMap<usize, Vec<String>>> = Mutex::new(BTreeMap::new());

thread_local! {
    static INSIDE: Cell<bool> = const { Cell::new(false) };
}

const SITE_FRAME_LIMIT: usize = 3;

pub struct Counting;

impl Counting {
    #[inline]
    fn note(layout: &Layout) {
        Counting::record(1, layout.size() as u64);
    }

    #[inline]
    fn record(allocations: u64, bytes: u64) {
        if !ON.load(Ordering::Relaxed) {
            return;
        }
        if !ATTRIBUTE.load(Ordering::Relaxed) {
            ALLOCS.fetch_add(allocations, Ordering::Relaxed);
            BYTES.fetch_add(bytes, Ordering::Relaxed);
            return;
        }
        Counting::attribute(allocations, bytes);
    }

    #[inline(never)]
    fn attribute(allocations: u64, bytes: u64) {
        ATTRIBUTIONS_IN_FLIGHT.fetch_add(1, Ordering::SeqCst);
        if !ON.load(Ordering::SeqCst) {
            ATTRIBUTIONS_IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
            return;
        }
        let already = INSIDE.with(|c| c.replace(true));
        if already {
            ATTRIBUTIONS_IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
            return;
        }
        ALLOCS.fetch_add(allocations, Ordering::Relaxed);
        BYTES.fetch_add(bytes, Ordering::Relaxed);
        if sampled() {
            let site = site();
            if let Ok(mut sites) = SITES.lock() {
                let entry = sites.entry(site).or_default();
                entry.allocations += allocations;
                entry.bytes += bytes;
            }
        }
        INSIDE.with(|c| c.set(false));
        ATTRIBUTIONS_IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
    }
}

#[inline]
fn sampled() -> bool {
    is_sampled(
        SEEN.fetch_add(1, Ordering::Relaxed),
        SITE_WALK_EVERY.load(Ordering::Relaxed),
    )
}

pub fn is_sampled(n: u64, every: u32) -> bool {
    every <= 1 || (n.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 58).is_multiple_of(u64::from(every))
}

fn site() -> String {
    let mut frames: Vec<String> = Vec::new();
    backtrace::trace(|frame| {
        named(frame, &mut frames);
        frames.len() < SITE_FRAME_LIMIT
    });
    frames.truncate(SITE_FRAME_LIMIT);
    if frames.is_empty() {
        "<no ply frame>".to_string()
    } else {
        frames.join(" < ")
    }
}

fn named(frame: &backtrace::Frame, into: &mut Vec<String>) {
    for cut in resolved_names(frame) {
        if !into.iter().any(|seen| seen == &cut) {
            into.push(cut);
        }
    }
}

fn resolved_names(frame: &backtrace::Frame) -> Vec<String> {
    let at = frame.ip() as usize;
    if let Ok(cache) = SYMBOL_NAMES_BY_ADDRESS.lock()
        && let Some(hit) = cache.get(&at)
    {
        return hit.clone();
    }
    let mut found: Vec<String> = Vec::new();
    backtrace::resolve_frame(frame, |symbol| {
        let Some(name) = symbol.name() else { return };
        let name = name.to_string();
        if !name.starts_with("ply_") || name.contains("::") {
            return;
        }
        let cut = name.rfind("::h").map(|i| &name[..i]).unwrap_or(&name);
        if !found.iter().any(|seen| seen == cut) {
            found.push(cut.to_string());
        }
    });
    if let Ok(mut cache) = SYMBOL_NAMES_BY_ADDRESS.lock() {
        cache.insert(at, found.clone());
    }
    found
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        Counting::note(&layout);
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        Counting::note(&layout);
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        Counting::record(1, new_size.saturating_sub(layout.size()) as u64);
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

pub fn window<R>(
    f: impl FnOnce() -> R,
    attribute: bool,
) -> (R, Counted, BTreeMap<String, Counted>) {
    count_allocations_sampled(f, u32::from(attribute))
}

pub fn count_allocations_sampled<R>(
    f: impl FnOnce() -> R,
    every: u32,
) -> (R, Counted, BTreeMap<String, Counted>) {
    ALLOCS.store(0, Ordering::Relaxed);
    BYTES.store(0, Ordering::Relaxed);
    SEEN.store(0, Ordering::Relaxed);
    if let Ok(mut sites) = SITES.lock() {
        sites.clear();
    }
    SITE_WALK_EVERY.store(every, Ordering::Relaxed);
    ATTRIBUTE.store(every >= 1, Ordering::Relaxed);
    ON.store(true, Ordering::Relaxed);
    let result = f();
    ON.store(false, Ordering::Relaxed);
    ATTRIBUTE.store(false, Ordering::Relaxed);
    while ATTRIBUTIONS_IN_FLIGHT.load(Ordering::SeqCst) != 0 {
        std::hint::spin_loop();
    }
    let counted = Counted {
        allocations: ALLOCS.load(Ordering::Relaxed),
        bytes: BYTES.load(Ordering::Relaxed),
    };
    let sites = SITES.lock().map(|sites| sites.clone()).unwrap_or_default();
    (result, counted, sites)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asked {
    pub path: std::path::PathBuf,
    pub sites: bool,
    pub exact: bool,
}

impl Asked {
    pub fn every(&self) -> u32 {
        if !self.sites {
            0
        } else if self.exact {
            1
        } else {
            SAMPLED
        }
    }
}

pub const SAMPLED: u32 = 64;

pub fn flag(argv: &mut Vec<String>) -> Result<Option<Asked>, String> {
    let mut asked: Option<Asked> = None;
    let mut i = 0;
    while i < argv.len() {
        let mut consumed = false;
        for (name, sites, exact) in [
            ("--count-allocations", false, false),
            ("--count-allocation-sites", true, false),
            ("--count-allocation-sites-exact", true, true),
        ] {
            let joined = format!("{name}=");
            if let Some(path) = argv[i].strip_prefix(&joined).map(str::to_string) {
                argv.remove(i);
                note(&mut asked, path, sites, exact);
                consumed = true;
                break;
            }
            if argv[i] == name {
                if i + 1 >= argv.len() {
                    return Err(format!("`{name}` takes a path to write the count to"));
                }
                let path = argv[i + 1].clone();
                argv.drain(i..i + 2);
                note(&mut asked, path, sites, exact);
                consumed = true;
                break;
            }
        }
        if !consumed {
            i += 1;
        }
    }
    Ok(asked)
}

fn note(asked: &mut Option<Asked>, path: String, sites: bool, exact: bool) {
    let sites = sites || asked.as_ref().is_some_and(|a| a.sites);
    let exact = exact || asked.as_ref().is_some_and(|a| a.exact);
    *asked = Some(Asked {
        path: std::path::PathBuf::from(path),
        sites,
        exact,
    });
}

pub fn write(
    path: &std::path::Path,
    counted: Counted,
    sites: &BTreeMap<String, Counted>,
    every: u32,
) -> std::io::Result<()> {
    let mut out = format!(
        "{{\n  \"allocations\": {},\n  \"bytes\": {}",
        counted.allocations, counted.bytes
    );
    if every >= 1 {
        out.push_str(&format!(
            ",\n  \"exact\": {},\n  \"sampled_every\": {every}",
            every == 1
        ));
        let scale = u64::from(every.max(1));
        let mut rows: Vec<(&String, &Counted)> = sites.iter().collect();
        rows.sort_by(|a, b| {
            b.1.allocations
                .cmp(&a.1.allocations)
                .then_with(|| a.0.cmp(b.0))
        });
        out.push_str(",\n  \"sites\": [");
        for (i, (site, at)) in rows.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&format!(
                "\n    {{\"site\": {}, \"allocations\": {}, \"bytes\": {}}}",
                json_string(site),
                at.allocations * scale,
                at.bytes * scale
            ));
        }
        out.push_str("\n  ]");
    }
    out.push_str("\n}\n");
    std::fs::write(path, out)?;
    ply_host::observe::process_wrote(&std::fs::canonicalize(path)?);
    Ok(())
}

fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
