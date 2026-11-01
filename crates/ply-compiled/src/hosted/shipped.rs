use super::{Deterministic, granted, text_at};
use crate::c::{MachineNative, runtime_native};
use crate::entry::{self, Opened};
use crate::native::{Address, I64, TakenBytes};
use crate::rt::{Context, borrowed};
use ply_host::native::{found, int, parts};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

pub trait Ships: Send + Sync {
    fn names(&self) -> Vec<String>;
    fn module(&self, name: &str) -> Option<&'static [u8]>;
    fn result(&self, path: &str, offset: usize, len: usize) -> Option<Vec<u8>>;
    fn digests(&self) -> String;
    fn stamps(&self) -> String;
    fn definitions(&self) -> String;
}

static SHIPS: OnceLock<Box<dyn Ships>> = OnceLock::new();

pub fn install(ships: Box<dyn Ships>) {
    let _ = SHIPS.set(ships);
}

pub(super) fn natives() -> Vec<MachineNative> {
    vec![
        runtime_native(
            "shipped_names",
            rt_shipped_names as unsafe extern "C" fn(_) -> _,
        ),
        runtime_native(
            "shipped_module",
            rt_shipped_module as unsafe extern "C" fn(_, _, _) -> _,
        ),
        runtime_native(
            "shipped_answer",
            rt_shipped_answer as unsafe extern "C" fn(_, _, _, _, _) -> _,
        ),
        runtime_native(
            "shipped_digests",
            rt_shipped_digests as unsafe extern "C" fn(_) -> _,
        ),
        runtime_native(
            "shipped_version",
            rt_shipped_version as unsafe extern "C" fn(_) -> _,
        ),
        runtime_native(
            "shipped_binary_bytes",
            rt_shipped_binary_bytes as unsafe extern "C" fn(_) -> _,
        ),
        runtime_native(
            "shipped_stamps",
            rt_shipped_stamps as unsafe extern "C" fn(_) -> _,
        ),
        runtime_native(
            "shipped_described",
            rt_shipped_described as unsafe extern "C" fn(_) -> _,
        ),
        runtime_native(
            "shipped_definitions",
            rt_shipped_definitions as unsafe extern "C" fn(_) -> _,
        ),
        runtime_native(
            "shipped_entered",
            rt_shipped_entered as unsafe extern "C" fn(_, _, _, _, _, _, _, _) -> _,
        ),
    ]
}

fn shipping(
    context: *mut Context,
    native: &'static str,
    result: impl FnOnce(&mut Context, &dyn Ships) -> TakenBytes,
) -> TakenBytes {
    granted(
        context,
        "shipped",
        native,
        Deterministic,
        TakenBytes(0),
        |c| match SHIPS.get() {
            Some(ships) => result(c, ships.as_ref()),
            None => {
                super::required_argument(
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

pub unsafe extern "C" fn rt_shipped_names(context: *mut Context) -> TakenBytes {
    shipping(context, "shipped_names", |c, ships| {
        let names = ships.names();
        let names: Vec<&[u8]> = names.iter().map(|n| n.as_bytes()).collect();
        c.given(parts(&names))
    })
}

pub unsafe extern "C" fn rt_shipped_module(
    context: *mut Context,
    p: Address,
    n: I64,
) -> TakenBytes {
    shipping(context, "shipped_module", |c, ships| {
        match text_at(c, p, n) {
            Some(name) => c.given(found(ships.module(&name))),
            None => TakenBytes(0),
        }
    })
}

pub unsafe extern "C" fn rt_shipped_answer(
    context: *mut Context,
    p: Address,
    n: I64,
    offset: I64,
    len: I64,
) -> TakenBytes {
    shipping(context, "shipped_answer", |c, ships| {
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

pub unsafe extern "C" fn rt_shipped_digests(context: *mut Context) -> TakenBytes {
    shipping(context, "shipped_digests", |c, ships| {
        c.given(ships.digests().into_bytes())
    })
}

pub unsafe extern "C" fn rt_shipped_version(context: *mut Context) -> TakenBytes {
    shipping(context, "shipped_version", |c, _| {
        c.given(env!("CARGO_PKG_VERSION").as_bytes().to_vec())
    })
}

pub unsafe extern "C" fn rt_shipped_binary_bytes(context: *mut Context) -> TakenBytes {
    shipping(context, "shipped_binary_bytes", |c, _| {
        let size = std::env::current_exe()
            .and_then(std::fs::metadata)
            .ok()
            .and_then(|binary| i64::try_from(binary.len()).ok());
        c.given(found(size.map(int)))
    })
}

pub unsafe extern "C" fn rt_shipped_stamps(context: *mut Context) -> TakenBytes {
    shipping(context, "shipped_stamps", |c, ships| {
        c.given(ships.stamps().into_bytes())
    })
}

pub unsafe extern "C" fn rt_shipped_described(context: *mut Context) -> TakenBytes {
    shipping(context, "shipped_described", |c, _| {
        c.given(crate::c::described().as_bytes().to_vec())
    })
}

pub unsafe extern "C" fn rt_shipped_definitions(context: *mut Context) -> TakenBytes {
    shipping(context, "shipped_definitions", |c, ships| {
        c.given(ships.definitions().into_bytes())
    })
}

#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn rt_shipped_entered(
    context: *mut Context,
    lp: Address,
    ln: I64,
    rp: Address,
    rn: I64,
    ap: Address,
    an: I64,
    timeout_ms: I64,
) -> TakenBytes {
    shipping(context, "shipped_entered", |c, _| {
        let (Some(library), Some(root)) = (text_at(c, lp, ln), text_at(c, rp, rn)) else {
            return TakenBytes(0);
        };
        let opened = match open_cached(Path::new(&library)) {
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
        let timeout_ms = u64::try_from(timeout_ms.0).unwrap_or(0);
        let returned = entry::run_to_result(&opened, &root, arguments, timeout_ms);
        c.given(found(returned.ok()))
    })
}

fn open_cached(library: &Path) -> Result<Arc<Opened>, String> {
    type LibraryCache = Mutex<HashMap<PathBuf, Arc<Opened>>>;
    static UNITS: OnceLock<LibraryCache> = OnceLock::new();
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
