//! `compiler.unit`'s `shipped` natives, the family `shipped` that `--allow shipped` grants:
//! what this binary ships, which `ply-privileged` holds and installs, a function of the binary
//! alone, so each is asked as computing; and the units a load enters for its derivers.

use super::{Deterministic, granted, text_at};
use crate::c::{MachineNative, machine_native};
use crate::entry::{self, Ended, Opened, Run};
use crate::native::{Addr, I64, TakenBytes};
use crate::rt::{Ctx, borrowed};
use ply_host::native::{found, int, parts};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// What this binary ships beside its runtime.
pub trait Ships: Send + Sync {
    /// Every shipped module's name.
    fn names(&self) -> Vec<String>;
    /// A shipped module's text, or the bytes of a data file one embeds, by its name.
    fn module(&self, name: &str) -> Option<&'static [u8]>;
    /// The frames of `len` bytes from `offset` of the pack's result file `path`, decompressed;
    /// a `len` of 0 reads to the end.
    fn result(&self, path: &str, offset: usize, len: usize) -> Option<Vec<u8>>;
    /// Each shipped file and its digest, as the launcher lays them out beside the modules.
    fn digests(&self) -> String;
    /// The runtime's stamps; empty in a process the launcher did not start.
    fn stamps(&self) -> String;
    /// The program the launcher entered, a `name hash` line each; empty where it entered none.
    fn definitions(&self) -> String;
}

static SHIPS: OnceLock<Box<dyn Ships>> = OnceLock::new();

/// Installs what the binary ships, once, before a program asks.
pub fn install(ships: Box<dyn Ships>) {
    let _ = SHIPS.set(ships);
}

pub(super) fn natives() -> Vec<MachineNative> {
    vec![
        machine_native(
            "shipped_names",
            rt_shipped_names as unsafe extern "C" fn(_) -> _,
        ),
        machine_native(
            "shipped_module",
            rt_shipped_module as unsafe extern "C" fn(_, _, _) -> _,
        ),
        machine_native(
            "shipped_answer",
            rt_shipped_answer as unsafe extern "C" fn(_, _, _, _, _) -> _,
        ),
        machine_native(
            "shipped_digests",
            rt_shipped_digests as unsafe extern "C" fn(_) -> _,
        ),
        machine_native(
            "shipped_version",
            rt_shipped_version as unsafe extern "C" fn(_) -> _,
        ),
        machine_native(
            "shipped_binary_bytes",
            rt_shipped_binary_bytes as unsafe extern "C" fn(_) -> _,
        ),
        machine_native(
            "shipped_stamps",
            rt_shipped_stamps as unsafe extern "C" fn(_) -> _,
        ),
        machine_native(
            "shipped_described",
            rt_shipped_described as unsafe extern "C" fn(_) -> _,
        ),
        machine_native(
            "shipped_definitions",
            rt_shipped_definitions as unsafe extern "C" fn(_) -> _,
        ),
        machine_native(
            "shipped_entered",
            rt_shipped_entered as unsafe extern "C" fn(_, _, _, _, _, _, _, _) -> _,
        ),
    ]
}

/// What `result` returns of what the binary ships, where the run is granted `shipped`.
fn shipping(
    ctx: *mut Ctx,
    native: &'static str,
    result: impl FnOnce(&mut Ctx, &dyn Ships) -> TakenBytes,
) -> TakenBytes {
    granted(
        ctx,
        "shipped",
        native,
        Deterministic,
        TakenBytes(0),
        |c| match SHIPS.get() {
            Some(ships) => result(c, ships.as_ref()),
            None => {
                super::handed(
                    c,
                    native,
                    "a binary that installed what it ships",
                    None::<()>,
                );
                TakenBytes(0)
            }
        },
    )
}

/// Every shipped module's name, as parts.
pub unsafe extern "C" fn rt_shipped_names(ctx: *mut Ctx) -> TakenBytes {
    shipping(ctx, "shipped_names", |c, ships| {
        let names = ships.names();
        let names: Vec<&[u8]> = names.iter().map(|n| n.as_bytes()).collect();
        c.given(parts(&names))
    })
}

/// The shipped module or data file the name at `p` names, found.
pub unsafe extern "C" fn rt_shipped_module(ctx: *mut Ctx, p: Addr, n: I64) -> TakenBytes {
    shipping(ctx, "shipped_module", |c, ships| match text_at(c, p, n) {
        Some(name) => c.given(found(ships.module(&name))),
        None => TakenBytes(0),
    })
}

/// The frames of `len` bytes from `offset` of the result file the path at `p` names, found;
/// neither is negative.
pub unsafe extern "C" fn rt_shipped_answer(
    ctx: *mut Ctx,
    p: Addr,
    n: I64,
    offset: I64,
    len: I64,
) -> TakenBytes {
    shipping(ctx, "shipped_answer", |c, ships| {
        let Some(path) = text_at(c, p, n) else {
            return TakenBytes(0);
        };
        let frame = match (usize::try_from(offset.0), usize::try_from(len.0)) {
            (Ok(offset), Ok(len)) => ships.result(&path, offset, len),
            _ => None,
        };
        c.given(found(frame))
    })
}

/// Each shipped file and its digest.
pub unsafe extern "C" fn rt_shipped_digests(ctx: *mut Ctx) -> TakenBytes {
    shipping(ctx, "shipped_digests", |c, ships| {
        c.given(ships.digests().into_bytes())
    })
}

/// This binary's version.
pub unsafe extern "C" fn rt_shipped_version(ctx: *mut Ctx) -> TakenBytes {
    shipping(ctx, "shipped_version", |c, _| {
        c.given(env!("CARGO_PKG_VERSION").as_bytes().to_vec())
    })
}

/// The running binary's size in eight bytes, found where it can be measured.
pub unsafe extern "C" fn rt_shipped_binary_bytes(ctx: *mut Ctx) -> TakenBytes {
    shipping(ctx, "shipped_binary_bytes", |c, _| {
        let size = std::env::current_exe()
            .and_then(std::fs::metadata)
            .ok()
            .and_then(|binary| i64::try_from(binary.len()).ok());
        c.given(found(size.map(int)))
    })
}

/// The runtime's stamps.
pub unsafe extern "C" fn rt_shipped_stamps(ctx: *mut Ctx) -> TakenBytes {
    shipping(ctx, "shipped_stamps", |c, ships| {
        c.given(ships.stamps().into_bytes())
    })
}

/// The runtime this binary is linked with, as its library describes itself.
pub unsafe extern "C" fn rt_shipped_described(ctx: *mut Ctx) -> TakenBytes {
    shipping(ctx, "shipped_described", |c, _| {
        c.given(crate::c::described().as_bytes().to_vec())
    })
}

/// The program the launcher entered.
pub unsafe extern "C" fn rt_shipped_definitions(ctx: *mut Ctx) -> TakenBytes {
    shipping(ctx, "shipped_definitions", |c, ships| {
        c.given(ships.definitions().into_bytes())
    })
}

/// What the root at `rp` of the unit compiled into the library at `lp` returns for the arguments,
/// the PLV1 list at `ap`, within `timeout_ms` of wall clock, found: none where it raised, ran out
/// of time or returned nothing a run can hold.
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn rt_shipped_entered(
    ctx: *mut Ctx,
    lp: Addr,
    ln: I64,
    rp: Addr,
    rn: I64,
    ap: Addr,
    an: I64,
    timeout_ms: I64,
) -> TakenBytes {
    shipping(ctx, "shipped_entered", |c, _| {
        let (Some(library), Some(root)) = (text_at(c, lp, ln), text_at(c, rp, rn)) else {
            return TakenBytes(0);
        };
        let opened = match opened_unit(Path::new(&library)) {
            Ok(opened) => opened,
            Err(reason) => {
                c.fail(ply_core::Diagnostic::error(
                    ply_core::codes::INTERNAL_ERROR,
                    format!("an entered unit does not load: {reason}"),
                ));
                return TakenBytes(0);
            }
        };
        let arguments = unsafe { borrowed(ap, an) }.to_vec();
        let run = Run {
            timeout_ms: u64::try_from(timeout_ms.0).unwrap_or(0),
            ..Run::unbounded()
        };
        let returned = entry::on_program_thread(|| {
            match entry::enter(&opened, &root, arguments, &run, None, None).ended {
                Ended::Returned(bytes) => Some(bytes),
                _ => None,
            }
        })
        .flatten();
        c.given(found(returned))
    })
}

/// The unit compiled into `library`, opened the first time this process asks and cached after.
fn opened_unit(library: &Path) -> Result<Arc<Opened>, String> {
    type Units = Mutex<HashMap<PathBuf, Arc<Opened>>>;
    static UNITS: OnceLock<Units> = OnceLock::new();
    let units = UNITS.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(opened) = units.lock().unwrap_or_else(|e| e.into_inner()).get(library) {
        return Ok(Arc::clone(opened));
    }
    let opened = Opened::open(library)?;
    let mut held = units.lock().unwrap_or_else(|e| e.into_inner());
    Ok(Arc::clone(
        held.entry(library.to_path_buf()).or_insert(opened),
    ))
}
