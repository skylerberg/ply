#![allow(clippy::missing_safety_doc)]

use ply_compiled::c::descriptor::Descriptor;
use ply_compiled::entry::{self, Opened};
use std::ffi::{c_char, c_int};
use std::io::Write;

const PANICKED: c_int = 101;

#[unsafe(no_mangle)]
pub extern "C" fn ply_runtime_described() -> *const c_char {
    static DESCRIPTION: std::sync::OnceLock<std::ffi::CString> = std::sync::OnceLock::new();
    DESCRIPTION
        .get_or_init(|| {
            std::ffi::CString::new(ply_compiled::c::described())
                .expect("a description holds no NUL")
        })
        .as_ptr()
}

#[unsafe(no_mangle)]
pub extern "C" fn ply_unit_entered() -> c_int {
    std::panic::catch_unwind(unit_entered).unwrap_or(PANICKED)
}

fn unit_entered() -> c_int {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let [library, root, arguments, timeout_ms] = argv.as_slice() else {
        eprintln!("ply: a unit is entered with LIBRARY ROOT ARGUMENTS TIMEOUT_MS");
        return 2;
    };
    let arguments = match std::fs::read(arguments) {
        Ok(bytes) => bytes,
        Err(reason) => {
            eprintln!("ply: `{arguments}` cannot be read: {reason}");
            return 2;
        }
    };
    let opened = match Opened::open(std::path::Path::new(library)) {
        Ok(opened) => opened,
        Err(reason) => {
            eprintln!("ply: `{library}` does not load: {reason}");
            return 2;
        }
    };
    let timeout_ms = timeout_ms.parse().unwrap_or(0);
    match entry::run_to_result(&opened, root, arguments, timeout_ms) {
        Ok(bytes) => {
            let mut out = std::io::stdout();
            match out.write_all(&bytes).and_then(|()| out.flush()) {
                Ok(()) => 0,
                Err(reason) => {
                    eprintln!("ply: what `{root}` returned could not be written: {reason}");
                    2
                }
            }
        }
        Err(reason) => {
            eprintln!("ply: `{root}` returned nothing: {reason}");
            1
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ply_native_enter(
    _argc: c_int,
    _argv: *const *const c_char,
    unit: *const Descriptor,
) -> c_int {
    std::panic::catch_unwind(|| unsafe { run(unit) }).unwrap_or(PANICKED)
}

unsafe fn run(unit: *const Descriptor) -> c_int {
    ply_host::time::mark_start(std::time::Instant::now());
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_IGN) };
    if let Err(reason) = unsafe { ply_compiled::c::linked::install(unit) } {
        eprintln!("ply: {reason}");
        return 2;
    }
    if let Err(reason) = ply_launcher::install_binary_pack() {
        eprintln!("ply: {reason}");
        return 2;
    }
    let program = match ply_launcher::linked_program() {
        Ok(program) => program,
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            return 1;
        }
    };
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let root = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let launched = ply_launcher::run(program, &root, argv);
    for warning in launched.warnings {
        eprintln!("{warning}");
    }
    let code = match launched.code {
        Ok(code) => code,
        Err(said) => {
            eprintln!("{said}");
            1
        }
    };
    std::process::exit(code)
}
