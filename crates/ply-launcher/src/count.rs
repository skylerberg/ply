//! Counting what an entry allocates. A counting allocator is a whole-binary decision, so it
//! lives here beside the big-stack thread: the binary installs [`Counting`], and a run that asked
//! for the count opens a window around the entry. Nothing is counted outside a window.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// What one window allocated.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counted {
    pub allocations: u64,
    pub bytes: u64,
}

static ON: AtomicBool = AtomicBool::new(false);
/// Whether each allocation is also attributed to the `ply_*` frames that asked for it, which
/// means a stack walk per allocation: off unless the run asked for sites.
static ATTRIBUTE: AtomicBool = AtomicBool::new(false);
/// The window's every-Nth sample: every allocation is counted, one in this many is walked, and the
/// rows are scaled by it. One when the run asked for exact sites, zero when it asked for no sites.
static SAMPLE: AtomicU32 = AtomicU32::new(0);
/// Allocations seen since the window opened, which picks the sample.
static SEEN: AtomicU64 = AtomicU64::new(0);
static ALLOCS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);
/// Attributions in flight. A window waits for this to reach zero before it answers, so its rows
/// are the whole of its total rather than whatever had finished by the time it looked.
static ATTRIBUTING: AtomicU64 = AtomicU64::new(0);
/// Site -> what it allocated, in site order.
static SITES: Mutex<BTreeMap<String, Counted>> = Mutex::new(BTreeMap::new());
/// Instruction pointer -> the `ply_*` names at that frame, in the order the symbolizer reports
/// them. A frame with none is an entry with an empty list.
static RESOLVED: Mutex<BTreeMap<usize, Vec<String>>> = Mutex::new(BTreeMap::new());

thread_local! {
    /// A backtrace allocates, and a site that counted those allocations would be the walker's
    /// rather than the program's.
    static INSIDE: Cell<bool> = const { Cell::new(false) };
}

/// How many `ply_*` frames name one allocation; a `RawVec::grow` frame names the allocator, not
/// the code that wanted the room.
const FRAMES: usize = 3;

/// The allocator a binary installs as its `#[global_allocator]`. Every thread's allocations are
/// counted, so a served request's own tasks are counted too, and the check outside a window is
/// one relaxed load.
pub struct Counting;

impl Counting {
    #[inline]
    fn note(layout: &Layout) {
        Counting::record(1, layout.size() as u64);
    }

    /// One allocation's contribution: the count, the bytes it added, and — when the run asked for
    /// sites — the `ply_*` frame that made it. The alloc and realloc paths both come through here,
    /// so the two modes count the same window and every allocation is attributed.
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
        // Registering before the re-check is what makes the window's answer coherent: an
        // attribution that counts has announced itself before the window can observe zero, so the
        // window's wait sees it, and one that arrives after the window closed finds `ON` false and
        // counts nothing.
        ATTRIBUTING.fetch_add(1, Ordering::SeqCst);
        if !ON.load(Ordering::SeqCst) {
            ATTRIBUTING.fetch_sub(1, Ordering::SeqCst);
            return;
        }
        // Naming a site walks the stack, and the walk itself allocates: those allocations are the
        // instrument's, not the program's, so they are neither counted nor attributed.
        let already = INSIDE.with(|c| c.replace(true));
        if already {
            ATTRIBUTING.fetch_sub(1, Ordering::SeqCst);
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
        ATTRIBUTING.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Whether this allocation is one of the window's walked ones.
#[inline]
fn sampled() -> bool {
    sampled_nth(
        SEEN.fetch_add(1, Ordering::Relaxed),
        SAMPLE.load(Ordering::Relaxed),
    )
}

/// Whether the `n`th counted allocation of a window is one of the sampled ones: every `every`
/// allocations on average, one when `every` is one or less.
///
/// By a hash of the count rather than by `n % every`, because a program that allocates exactly
/// `every` times around a loop — or any other period it happens to have — would otherwise land on
/// the sample every time and attribute the same site, which is the one thing a site census is
/// trying to find out.
pub fn sampled_nth(n: u64, every: u32) -> bool {
    every <= 1 || (n.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 58).is_multiple_of(u64::from(every))
}

/// The nearest few `ply_*` frames of the current stack, outermost last.
fn site() -> String {
    let mut frames: Vec<String> = Vec::new();
    backtrace::trace(|frame| {
        named(frame, &mut frames);
        frames.len() < FRAMES
    });
    frames.truncate(FRAMES);
    if frames.is_empty() {
        "<no ply frame>".to_string()
    } else {
        frames.join(" < ")
    }
}

/// The `ply_*` names at one frame, which is the attribution that matters: the emitted program's
/// own functions are `ply_`-prefixed C symbols, while the launcher's and the toolchain's Rust
/// frames carry `::`, so a site is the program's code and not the tool running it.
fn named(frame: &backtrace::Frame, into: &mut Vec<String>) {
    for cut in resolved_names(frame) {
        if !into.iter().any(|seen| seen == &cut) {
            into.push(cut);
        }
    }
}

/// The `ply_*` names at one frame, resolved once per instruction pointer.
///
/// One call site allocates over and over, and symbolizing answers the same question every time;
/// the answer is kept per address. A frame with no `ply_*` name is kept too, since a Rust frame in
/// the toolchain is the common case.
fn resolved_names(frame: &backtrace::Frame) -> Vec<String> {
    let at = frame.ip() as usize;
    if let Ok(cache) = RESOLVED.lock()
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
    if let Ok(mut cache) = RESOLVED.lock() {
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
        // A realloc is an allocation; the bytes it holds are the ones it asked for now, so the
        // count is one and the bytes are the growth.
        Counting::record(1, new_size.saturating_sub(layout.size()) as u64);
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

/// Runs `f` with the window open, answering what it allocated, and where when the run asked for
/// sites. Windows do not nest: the outermost one is the entry.
pub fn window<R>(
    f: impl FnOnce() -> R,
    attribute: bool,
) -> (R, Counted, BTreeMap<String, Counted>) {
    window_sampled(f, u32::from(attribute))
}

/// Runs `f` with the window open, walking one allocation in `every` for its site. `every` is zero
/// for no sites and one for every allocation; a larger value is what makes a site census over a
/// long served window answerable, since the walk is most of its cost. The rows that come back are
/// the *sampled* ones — [`write`] scales them by `every` and says so — so a caller reading them
/// directly should scale by `every` too.
pub fn window_sampled<R>(
    f: impl FnOnce() -> R,
    every: u32,
) -> (R, Counted, BTreeMap<String, Counted>) {
    ALLOCS.store(0, Ordering::Relaxed);
    BYTES.store(0, Ordering::Relaxed);
    SEEN.store(0, Ordering::Relaxed);
    if let Ok(mut sites) = SITES.lock() {
        sites.clear();
    }
    SAMPLE.store(every, Ordering::Relaxed);
    ATTRIBUTE.store(every >= 1, Ordering::Relaxed);
    ON.store(true, Ordering::Relaxed);
    let answer = f();
    ON.store(false, Ordering::Relaxed);
    ATTRIBUTE.store(false, Ordering::Relaxed);
    // Every attribution that was counted has announced itself, so waiting them out makes the rows
    // below the whole of the totals above rather than a snapshot of how far they had got.
    while ATTRIBUTING.load(Ordering::SeqCst) != 0 {
        std::hint::spin_loop();
    }
    let counted = Counted {
        allocations: ALLOCS.load(Ordering::Relaxed),
        bytes: BYTES.load(Ordering::Relaxed),
    };
    let sites = SITES.lock().map(|sites| sites.clone()).unwrap_or_default();
    (answer, counted, sites)
}

/// What a run asked to be told about its allocations, taken out of the line before the program
/// parses it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asked {
    pub path: std::path::PathBuf,
    /// Whether each allocation is also attributed to the `ply_*` frames that asked for it.
    pub sites: bool,
    /// Whether every allocation is walked rather than one in [`SAMPLED`], which is what
    /// `--count-alloc-sites-exact` asks for.
    pub exact: bool,
}

impl Asked {
    /// One allocation in this many is walked: zero when no sites were asked for, one for exact
    /// sites, [`SAMPLED`] otherwise.
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

/// How many allocations one walked site stands for when the run did not ask for exact sites. The
/// walk is the whole cost of a site census, so sampling by this keeps a served window in the same
/// order as a plain count while leaving the biggest sites clear: the row's allocations are its
/// sampled ones times this.
pub const SAMPLED: u32 = 64;

/// `--count-allocs=PATH` (totals), `--count-alloc-sites=PATH` (totals and where, one allocation in
/// [`SAMPLED`] walked) and `--count-alloc-sites-exact=PATH` (every allocation walked), wherever
/// they are written: the launcher's own flags. `None` when none was given.
pub fn flag(argv: &mut Vec<String>) -> Result<Option<Asked>, String> {
    let mut asked: Option<Asked> = None;
    let mut i = 0;
    while i < argv.len() {
        let mut consumed = false;
        for (name, sites, exact) in [
            ("--count-allocs", false, false),
            ("--count-alloc-sites", true, false),
            ("--count-alloc-sites-exact", true, true),
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

/// Folds one flag into what the run asked for: the last path wins, asking for sites at all means
/// sites, and asking for exact sites anywhere means exact — the stronger request is the one the
/// run gets.
fn note(asked: &mut Option<Asked>, path: String, sites: bool, exact: bool) {
    let sites = sites || asked.as_ref().is_some_and(|a| a.sites);
    let exact = exact || asked.as_ref().is_some_and(|a| a.exact);
    *asked = Some(Asked {
        path: std::path::PathBuf::from(path),
        sites,
        exact,
    });
}

/// Writes what the run allocated where the flag asked for it. A handful of numbers, so the
/// document is written here rather than through a JSON library the launcher would carry for
/// nothing. When sites were asked for it also says whether they were sampled, and the rows are in
/// the totals' units: `every` walked allocations scaled back up by `every`.
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
        // How the rows were read, so a reader knows whether they are the program's allocations or
        // a sample of them scaled up.
        out.push_str(&format!(
            ",\n  \"exact\": {},\n  \"sampled_every\": {every}",
            every == 1
        ));
        // One walked allocation stands for `every` of them, so the rows read in the totals' units
        // rather than the sample's.
        let scale = u64::from(every.max(1));
        // Most to least, so a reader starts at what mattered; the site name breaks ties.
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
    std::fs::write(path, out)
}

/// A JSON string: site names are `ply_*` symbols and separators, so only `"` and `\` can appear.
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
