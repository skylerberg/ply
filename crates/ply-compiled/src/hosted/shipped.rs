use super::{Deterministic, granted, text_at};
use crate::c::{MachineNative, runtime_native};
use crate::native::{Addr, I64, TakenBytes};
use crate::rt::Ctx;
use ply_host::native::{found, int, parts};
use std::sync::OnceLock;

pub trait Ships: Send + Sync {
    fn names(&self) -> Vec<String>;
    fn module(&self, name: &str) -> Option<&'static [u8]>;
    fn read_pack(&self, part: PackPart) -> Option<Vec<u8>>;
    fn stamps(&self) -> String;
    fn definitions(&self) -> String;
}

pub enum PackPart {
    WholeIndex,
    StoreFrame { offset: usize, len: usize },
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
    ]
}

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

pub unsafe extern "C" fn rt_shipped_names(ctx: *mut Ctx) -> TakenBytes {
    shipping(ctx, "shipped_names", |c, ships| {
        let names = ships.names();
        let names: Vec<&[u8]> = names.iter().map(|n| n.as_bytes()).collect();
        c.given(parts(&names))
    })
}

pub unsafe extern "C" fn rt_shipped_module(ctx: *mut Ctx, p: Addr, n: I64) -> TakenBytes {
    shipping(ctx, "shipped_module", |c, ships| match text_at(c, p, n) {
        Some(name) => c.given(found(ships.module(&name))),
        None => TakenBytes(0),
    })
}

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
        let part = match (
            path.as_str(),
            usize::try_from(offset.0),
            usize::try_from(len.0),
        ) {
            ("index", _, _) => Some(PackPart::WholeIndex),
            ("store", Ok(offset), Ok(len)) => Some(PackPart::StoreFrame { offset, len }),
            _ => None,
        };
        c.given(found(part.and_then(|part| ships.read_pack(part))))
    })
}

pub unsafe extern "C" fn rt_shipped_version(ctx: *mut Ctx) -> TakenBytes {
    shipping(ctx, "shipped_version", |c, _| {
        c.given(env!("CARGO_PKG_VERSION").as_bytes().to_vec())
    })
}

pub unsafe extern "C" fn rt_shipped_binary_bytes(ctx: *mut Ctx) -> TakenBytes {
    shipping(ctx, "shipped_binary_bytes", |c, _| {
        let size = std::env::current_exe()
            .and_then(std::fs::metadata)
            .ok()
            .and_then(|binary| i64::try_from(binary.len()).ok());
        c.given(found(size.map(int)))
    })
}

pub unsafe extern "C" fn rt_shipped_stamps(ctx: *mut Ctx) -> TakenBytes {
    shipping(ctx, "shipped_stamps", |c, ships| {
        c.given(ships.stamps().into_bytes())
    })
}

pub unsafe extern "C" fn rt_shipped_described(ctx: *mut Ctx) -> TakenBytes {
    shipping(ctx, "shipped_described", |c, _| {
        c.given(crate::c::described().as_bytes().to_vec())
    })
}

pub unsafe extern "C" fn rt_shipped_definitions(ctx: *mut Ctx) -> TakenBytes {
    shipping(ctx, "shipped_definitions", |c, ships| {
        c.given(ships.definitions().into_bytes())
    })
}
