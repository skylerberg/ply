//! The runtime a native Ply program is linked with. The program's `main` calls [`ply_native_enter`]
//! with its unit's descriptor; this binds the runtime to the unit and enters the program's entry
//! with the command line, its pack's shipped modules and its environment lent.

use ply_compiled::c::descriptor::Descriptor;
use std::ffi::{c_char, c_int};

/// Counts what the entry allocates; nothing but the entry is counted, and only when
/// `--count-allocs` asked for it.
#[global_allocator]
static ALLOCATOR: ply_launcher::count::Counting = ply_launcher::count::Counting;

/// The exit code of a run that panicked in the runtime, which no program asks for.
const PANICKED: c_int = 101;

/// The runtime this library is, as `compiler.emit.abi.runtime_of` reads it: what a `ply` building a
/// program for it holds the program's C to. A program linked with nothing but a `main` that prints
/// it is how a build asks.
#[unsafe(no_mangle)]
pub extern "C" fn ply_runtime_described() -> *const c_char {
    static DESCRIBED: std::sync::OnceLock<std::ffi::CString> = std::sync::OnceLock::new();
    DESCRIBED
        .get_or_init(|| {
            std::ffi::CString::new(ply_compiled::c::described())
                .expect("a description holds no NUL")
        })
        .as_ptr()
}

/// The runtime this library is, as JSON, which a released `ply`'s bootstrap reads.
#[unsafe(no_mangle)]
pub extern "C" fn ply_runtime_describe() -> *const c_char {
    static DESCRIBED: std::sync::OnceLock<std::ffi::CString> = std::sync::OnceLock::new();
    DESCRIBED
        .get_or_init(|| {
            std::ffi::CString::new(ply_privileged::shipped::runtime_json())
                .expect("a description holds no NUL")
        })
        .as_ptr()
}

/// Runs the linked program, whose unit lays `unit` out, and answers its exit code. The command
/// line is read from the process, as Rust's own `main` reads it.
///
/// # Safety
/// `unit` points at the unit's descriptor, which lives as long as the process.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ply_native_enter(
    _argc: c_int,
    _argv: *const *const c_char,
    unit: *const Descriptor,
) -> c_int {
    // An unwind may not cross into C, so a panic past the entry's own thread ends here.
    std::panic::catch_unwind(|| unsafe { run(unit) }).unwrap_or(PANICKED)
}

unsafe fn run(unit: *const Descriptor) -> c_int {
    ply_host::time::mark_start(std::time::Instant::now());
    // Rust's own `main` does this before any user code, so a write to a closed pipe is an error the
    // program answers rather than a signal that ends it; entered from C, the runtime does it here.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_IGN) };
    if let Err(why) = unsafe { ply_compiled::c::linked::install(unit) } {
        eprintln!("ply: {why}");
        return 2;
    }
    if let Err(why) = ply_launcher::install_own_pack() {
        eprintln!("ply: {why}");
        return 2;
    }
    let program = match ply_launcher::linked_program(env!("CARGO_PKG_VERSION")) {
        Ok(program) => program,
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            return 1;
        }
    };
    let mut argv: Vec<String> = std::env::args().skip(1).collect();
    // The launcher's own flags, taken out before the program parses the line.
    let count = match ply_launcher::count::flag(&mut argv) {
        Ok(count) => count,
        Err(why) => {
            eprintln!("ply: {why}");
            return 2;
        }
    };
    let root = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    // Every command family's ops are lent unconfigured to the program, which is this binary's own;
    // the program configures what it drives.
    let binds = ply_privileged::enter::Binds {
        lent: ply_privileged::policy::all(),
        executables: ply_launcher::executables(),
        trust: ply_launcher::trust(),
        ..ply_privileged::enter::Binds::default()
    };
    let (answer, warnings) = ply_launcher::run(program, &root, argv, binds, count).into_parts();
    for warning in warnings {
        eprintln!("{warning}");
    }
    let code = match answer {
        Ok(code) => code,
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            1
        }
    };
    std::process::exit(code)
}
