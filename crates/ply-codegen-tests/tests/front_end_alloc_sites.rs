//! Where a parse allocates, by site, under the interpreter and under compiled code —
//! `docs/BOOTSTRAP-PATH.md` step 4's first measurement.
//!
//! The front-end row (`benches/front-end`) says the compiled parse is a fifth faster than the
//! interpreted one, not five times, which means the cost left is not dispatch. This is the census
//! that says where it is: the tracing allocator and the site attribution are
//! `w6_alloc_sites`'s, over the parser spike parsing one example file. Allocation counts are
//! deterministic, so this runs anywhere and is not a timing.

use ply_eval::{Machine, Provider};
use ply_syntax::ast::{ModuleName, Program};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
    static INSIDE: Cell<bool> = const { Cell::new(false) };
    static SITES: RefCell<HashMap<String, (usize, usize)>> = RefCell::new(HashMap::new());
    static NAMES: RefCell<HashMap<usize, Vec<String>>> = RefCell::new(HashMap::new());
    static TOTAL: Cell<usize> = const { Cell::new(0) };
}

struct Tracing;

unsafe impl GlobalAlloc for Tracing {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let armed = ARMED.try_with(Cell::get).unwrap_or(false);
        if armed && !INSIDE.try_with(Cell::get).unwrap_or(true) {
            let _ = INSIDE.try_with(|c| c.set(true));
            let _ = TOTAL.try_with(|c| c.set(c.get() + 1));
            let key = site();
            let _ = SITES.try_with(|s| {
                let mut s = s.borrow_mut();
                let e = s.entry(key).or_insert((0, 0));
                e.0 += 1;
                e.1 += layout.size();
            });
            let _ = INSIDE.try_with(|c| c.set(false));
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Tracing = Tracing;

/// The nearest few `ply_*` frames: the code that wanted the room, not the allocator's own frame.
fn site() -> String {
    let mut frames: Vec<String> = Vec::new();
    backtrace::trace(|frame| {
        frames.extend(named(frame));
        frames.len() < 3
    });
    frames.truncate(3);
    if frames.is_empty() {
        "<no ply frame>".to_string()
    } else {
        frames.join(" < ")
    }
}

fn named(frame: &backtrace::Frame) -> Vec<String> {
    let ip = frame.ip() as usize;
    if let Some(hit) = NAMES.with(|c| c.borrow().get(&ip).cloned()) {
        return hit;
    }
    let mut found: Vec<String> = Vec::new();
    backtrace::resolve_frame(frame, |symbol| {
        let Some(name) = symbol.name() else { return };
        let name = name.to_string();
        if !name.starts_with("ply_") || name.contains("front_end_alloc_sites") {
            return;
        }
        let cut = name.rfind("::h").map(|i| &name[..i]).unwrap_or(&name);
        found.push(cut.to_string());
    });
    NAMES.with(|c| c.borrow_mut().insert(ip, found.clone()));
    found
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate lives two levels below the repository root")
        .to_path_buf()
}

struct Loaded {
    program: &'static Program,
    resolved: &'static ply_syntax::resolve::Resolved,
    check: &'static ply_core::CheckOutput,
}

/// The example this census parses: mid-sized, so the attribution is a table rather than a wait.
const EXAMPLE: &str = "examples/orders.ply";

/// A byte literal of `bytes`, escaped the way the front-end bench's probe writes one.
fn literal(bytes: &[u8]) -> String {
    let mut out = String::from("b\"");
    for &b in bytes {
        match b {
            0x22 => out.push_str("\\\""),
            0x5c => out.push_str("\\\\"),
            0x20..=0x7e => out.push(b as char),
            _ => out.push_str(&format!("\\x{b:02x}")),
        }
    }
    out.push('"');
    out
}

/// The shipped standard library, the parser spike's six modules, and one probe module parsing
/// `EXAMPLE` — the front-end row's workload for one file.
fn load() -> Loaded {
    let root = repo();
    let mut sources = ply_span::SourceMap::new();
    let mut owned: Vec<(ModuleName, String)> = ply_std::sources()
        .map(|(module, text)| (ModuleName::from_dotted(module), text.to_string()))
        .collect();
    for name in ["lexer", "spine", "types", "patterns", "exprs", "items"] {
        let text = std::fs::read_to_string(root.join(format!("spikes/ply-parser/{name}.ply")))
            .expect("a spike module");
        owned.push((ModuleName::from_dotted(name), text));
    }
    let example = std::fs::read(root.join(EXAMPLE)).expect("the example");
    let probe = format!(
        "import items (parse)\n\nfn source() -> Bytes = {}\n\ntest \"row: {EXAMPLE}\" {{ assert(len(parse(source()).node.items) >= 0) }}\n",
        literal(&example)
    );
    owned.push((ModuleName::from_dotted("probe"), probe));
    let mut inputs = Vec::new();
    let owned: &'static [(ModuleName, String)] = Box::leak(owned.into_boxed_slice());
    for (module, text) in owned {
        let id = sources.add(ply_std::pseudo_path(module), text.clone());
        inputs.push((id, module.clone(), text.as_str()));
    }
    let mut ast = ply_syntax::parse_program(inputs).expect("the workload parses");
    let expanded = ply_derive::expand_program(&mut ast);
    assert!(expanded.is_empty(), "{expanded:?}");
    let resolved = ply_syntax::resolve::resolve(&mut ast).expect("the workload resolves");
    let check = ply_core::check_program(&ast, &resolved).expect("the workload checks");
    Loaded {
        program: Box::leak(Box::new(ast)),
        resolved: Box::leak(Box::new(resolved)),
        check: Box::leak(Box::new(check)),
    }
}

struct Window {
    total: usize,
    bytes: usize,
    sites: HashMap<String, (usize, usize)>,
}

fn capture(f: impl FnOnce()) -> Window {
    SITES.with(|s| s.borrow_mut().clear());
    TOTAL.with(|c| c.set(0));
    ARMED.with(|c| c.set(true));
    f();
    ARMED.with(|c| c.set(false));
    let sites = SITES.with(|s| s.borrow().clone());
    Window {
        total: TOTAL.with(Cell::get),
        bytes: sites.values().map(|v| v.1).sum(),
        sites,
    }
}

/// Runs every `row:` test once.
fn parse_once(machine: &mut Machine<'_>) {
    for i in 0..machine.test_count() {
        if machine.test_name(i).is_some_and(|n| n.starts_with("row:")) {
            machine.eval_test(i).expect("the parse test passes");
        }
    }
}

fn report(name: &str, window: &Window) {
    let mut rows: Vec<(&String, usize, usize)> =
        window.sites.iter().map(|(k, v)| (k, v.0, v.1)).collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.1));
    println!(
        "\n== {name}: {} allocations, {} bytes, for one parse of {EXAMPLE}",
        window.total, window.bytes
    );
    for (site, count, bytes) in rows.iter().take(25) {
        println!(
            "  {count:>8} {bytes:>10}B  {:>5.1}%  {site}",
            100.0 * *count as f64 / window.total as f64
        );
    }
    assert!(window.total > 0, "`{name}` allocated nothing, so nothing was ranked");
}

/// The census this file exists for, printed under `--no-capture`: both arms, ranked by site, and
/// the compiled arm's share of the interpreted arm's total.
#[test]
fn the_parses_allocation_sites_are_ranked_under_both_engines() {
    let loaded = load();

    let mut interpreter = Machine::new(loaded.program, loaded.resolved, loaded.check);
    parse_once(&mut interpreter);
    let interpreted = capture(|| parse_once(&mut interpreter));

    let unit = ply_codegen::Cranelift::over(loaded.program, loaded.resolved, loaded.check)
        .expect("this host has a cranelift backend");
    let backend = unit.attach(&ply_eval::BackendSpec::honest());
    let mut compiled = Machine::new(loaded.program, loaded.resolved, loaded.check);
    compiled.set_compiled(backend);
    parse_once(&mut compiled);
    let offered_before = unit.offers().offered;
    let native = capture(|| parse_once(&mut compiled));
    assert!(
        unit.offers().offered > offered_before,
        "the backend was offered nothing, so the compiled arm is the interpreter under another name"
    );

    report("interpreter", &interpreted);
    report("compiled", &native);
    println!(
        "\n  compiled arm: {:.2} of the interpreter's allocations, {:.2} of its bytes",
        native.total as f64 / interpreted.total as f64,
        native.bytes as f64 / interpreted.bytes as f64
    );
}
