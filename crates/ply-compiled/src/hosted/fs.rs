//! `std.fs`'s natives: each operation on a path under the root its label names, or on a file a
//! run holds open there, a job of the run's on the filing pool, and confined to its root there.
//! Each answers what `ply_host::fs` writes of it. A read is no function of its arguments, and
//! asking twice may answer otherwise, so every one is asked as a write.

use super::{WRITES, asked, from_host, handed, opened, secret_of, text_at};
use crate::c::{MachineNative, machine_native};
use crate::emitted::SecretHandle;
use crate::native::{Addr, I64};
use crate::rt::{Ctx, lent};
use ply_core::Value;
use ply_core::crypto::Framing;
use ply_host::fs::{Chunk, Op, Opening, Second};
use ply_host::jobs::Work;
use std::sync::Arc;

/// The ticket of `op` on the path at `pp` under the root the label at `lp` names, handed what
/// `second` reads beside the path; 0, with the run failed, where it reads nothing.
fn filed(
    ctx: *mut Ctx,
    native: &'static str,
    op: Op,
    (lp, ln): (Addr, I64),
    (pp, pn): (Addr, I64),
    second: impl FnOnce(&mut Ctx) -> Option<Second>,
) -> I64 {
    asked(ctx, native, WRITES, I64(0), |c| {
        let (Some(label), Some(path)) = (text_at(c, lp, ln), text_at(c, pp, pn)) else {
            return I64(0);
        };
        let Some(second) = second(c) else {
            return I64(0);
        };
        let Some(files) = from_host(c, |facilities| Arc::clone(facilities.files())) else {
            return I64(0);
        };
        match files.asked(op, &label, path, second, c.site()) {
            Ok(job) => c.refusing(native, op.what(), Work::Filing, job),
            Err(d) => {
                c.fail(d);
                I64(0)
            }
        }
    })
}

/// The ticket of `chunk` asked of the file `file` names under the root the label at `lp` names.
fn on_file(ctx: *mut Ctx, native: &'static str, lp: Addr, ln: I64, file: I64, chunk: Chunk) -> I64 {
    asked(ctx, native, WRITES, I64(0), |c| {
        let Some(label) = text_at(c, lp, ln) else {
            return I64(0);
        };
        let Some(files) = from_host(c, |facilities| Arc::clone(facilities.files())) else {
            return I64(0);
        };
        let op = match chunk {
            Chunk::Read(_) => Op::ReadChunk,
            Chunk::Write(_) => Op::WriteChunk,
            Chunk::Close => Op::Close,
        };
        match files.on_file(&label, file.0, chunk, c.site()) {
            Ok(job) => c.refusing(native, op.what(), Work::Filing, job),
            Err(d) => {
                c.fail(d);
                I64(0)
            }
        }
    })
}

/// The natives that take a label and a path alone, each the operation beside its name.
macro_rules! on_paths {
    ($($native:ident = $name:literal => $op:ident),+ $(,)?) => {
        $(
            #[doc = concat!("The ticket of `", $name, "` on the path at `pp` under the root the label at `lp` names.")]
            pub unsafe extern "C" fn $native(ctx: *mut Ctx, lp: Addr, ln: I64, pp: Addr, pn: I64) -> I64 {
                filed(ctx, $name, Op::$op, (lp, ln), (pp, pn), |_| Some(Second::None))
            }
        )+

        fn path_natives() -> Vec<MachineNative> {
            vec![$(machine_native($name, $native as unsafe extern "C" fn(_, _, _, _, _) -> _)),+]
        }
    };
}

on_paths! {
    rt_fs_read_file = "fs_read_file" => ReadFile,
    rt_fs_list_dir = "fs_list_dir" => ListDir,
    rt_fs_kind = "fs_kind" => Kind,
    rt_fs_resolved = "fs_resolved" => Resolved,
    rt_fs_exists = "fs_exists" => Exists,
    rt_fs_file_size = "fs_file_size" => FileSize,
    rt_fs_modified_ms = "fs_modified_ms" => ModifiedMs,
    rt_fs_create_dir = "fs_create_dir" => CreateDir,
    rt_fs_remove = "fs_remove" => Remove,
    rt_fs_sync = "fs_sync" => Sync,
    rt_fs_lock = "fs_lock" => Lock,
    rt_fs_unlock = "fs_unlock" => Unlock,
    rt_fs_remove_tree = "fs_remove_tree" => RemoveTree,
    rt_fs_canonical = "fs_canonical" => Canonical,
    rt_fs_mode = "fs_mode" => Mode,
    rt_fs_read_link = "fs_read_link" => ReadLink,
    rt_fs_walk = "fs_walk" => Walk,
    rt_fs_stat = "fs_stat" => Stat,
    rt_fs_space = "fs_space" => Space,
}

pub(super) fn natives() -> Vec<MachineNative> {
    let mut natives = path_natives();
    natives.extend([
        machine_native(
            "fs_rename",
            rt_fs_rename as unsafe extern "C" fn(_, _, _, _, _, _, _) -> _,
        ),
        machine_native(
            "fs_copy",
            rt_fs_copy as unsafe extern "C" fn(_, _, _, _, _, _, _) -> _,
        ),
        machine_native(
            "fs_link",
            rt_fs_link as unsafe extern "C" fn(_, _, _, _, _, _, _) -> _,
        ),
        machine_native(
            "fs_symlink",
            rt_fs_symlink as unsafe extern "C" fn(_, _, _, _, _, _, _) -> _,
        ),
        machine_native(
            "fs_temp_dir",
            rt_fs_temp_dir as unsafe extern "C" fn(_, _, _, _, _, _, _) -> _,
        ),
        machine_native(
            "fs_write_file",
            rt_fs_write_file as unsafe extern "C" fn(_, _, _, _, _, _, _) -> _,
        ),
        machine_native(
            "fs_append",
            rt_fs_append as unsafe extern "C" fn(_, _, _, _, _, _, _) -> _,
        ),
        machine_native(
            "fs_read_at",
            rt_fs_read_at as unsafe extern "C" fn(_, _, _, _, _, _, _) -> _,
        ),
        machine_native(
            "fs_set_mode",
            rt_fs_set_mode as unsafe extern "C" fn(_, _, _, _, _, _) -> _,
        ),
        machine_native(
            "fs_set_modified",
            rt_fs_set_modified as unsafe extern "C" fn(_, _, _, _, _, _) -> _,
        ),
        machine_native(
            "fs_scan",
            rt_fs_scan as unsafe extern "C" fn(_, _, _, _, _, _) -> _,
        ),
        machine_native(
            "fs_open",
            rt_fs_open as unsafe extern "C" fn(_, _, _, _, _, _) -> _,
        ),
        machine_native(
            "fs_read_chunk",
            rt_fs_read_chunk as unsafe extern "C" fn(_, _, _, _, _) -> _,
        ),
        machine_native(
            "fs_write_chunk",
            rt_fs_write_chunk as unsafe extern "C" fn(_, _, _, _, _, _) -> _,
        ),
        machine_native(
            "fs_close",
            rt_fs_close as unsafe extern "C" fn(_, _, _, _) -> _,
        ),
        machine_native(
            "fs_write_secret",
            rt_fs_write_secret as unsafe extern "C" fn(_, _, _, _, _, _, _, _, _, _, _, _) -> _,
        ),
        machine_native(
            "fs_read_secret",
            rt_fs_read_secret as unsafe extern "C" fn(_, _, _, _, _, _, _, _, _, _, _) -> _,
        ),
        machine_native(
            "fs_secret_taken",
            rt_fs_secret_taken as unsafe extern "C" fn(_, _) -> _,
        ),
    ]);
    natives
}

/// The ticket of `fs.rename` from the path at `pp` to the one at `tp`.
pub unsafe extern "C" fn rt_fs_rename(
    ctx: *mut Ctx,
    lp: Addr,
    ln: I64,
    pp: Addr,
    pn: I64,
    tp: Addr,
    tn: I64,
) -> I64 {
    filed(ctx, "fs_rename", Op::Rename, (lp, ln), (pp, pn), |c| {
        text_at(c, tp, tn).map(Second::Path)
    })
}

/// The ticket of `fs.copy` of the file at `pp` to the path at `tp`.
pub unsafe extern "C" fn rt_fs_copy(
    ctx: *mut Ctx,
    lp: Addr,
    ln: I64,
    pp: Addr,
    pn: I64,
    tp: Addr,
    tn: I64,
) -> I64 {
    filed(ctx, "fs_copy", Op::Copy, (lp, ln), (pp, pn), |c| {
        text_at(c, tp, tn).map(Second::Path)
    })
}

/// The ticket of `fs.link`: the path at `pp` made another name for the file at `tp`.
pub unsafe extern "C" fn rt_fs_link(
    ctx: *mut Ctx,
    lp: Addr,
    ln: I64,
    pp: Addr,
    pn: I64,
    tp: Addr,
    tn: I64,
) -> I64 {
    filed(ctx, "fs_link", Op::Link, (lp, ln), (pp, pn), |c| {
        text_at(c, tp, tn).map(Second::Path)
    })
}

/// The ticket of `fs.symlink`: the path at `pp` made a link to the target at `tp`.
pub unsafe extern "C" fn rt_fs_symlink(
    ctx: *mut Ctx,
    lp: Addr,
    ln: I64,
    pp: Addr,
    pn: I64,
    tp: Addr,
    tn: I64,
) -> I64 {
    filed(ctx, "fs_symlink", Op::Symlink, (lp, ln), (pp, pn), |c| {
        text_at(c, tp, tn).map(Second::Target)
    })
}

/// The ticket of `fs.temp_dir` under the directory at `pp`, its name starting with the text at `xp`.
pub unsafe extern "C" fn rt_fs_temp_dir(
    ctx: *mut Ctx,
    lp: Addr,
    ln: I64,
    pp: Addr,
    pn: I64,
    xp: Addr,
    xn: I64,
) -> I64 {
    filed(ctx, "fs_temp_dir", Op::TempDir, (lp, ln), (pp, pn), |c| {
        text_at(c, xp, xn).map(Second::Name)
    })
}

/// The ticket of `fs.write_file` of the bytes at `bp`.
pub unsafe extern "C" fn rt_fs_write_file(
    ctx: *mut Ctx,
    lp: Addr,
    ln: I64,
    pp: Addr,
    pn: I64,
    bp: Addr,
    bn: I64,
) -> I64 {
    filed(
        ctx,
        "fs_write_file",
        Op::WriteFile,
        (lp, ln),
        (pp, pn),
        |_| Some(Second::Body(unsafe { lent(bp, bn) }.to_vec())),
    )
}

/// The ticket of `fs.append` of the bytes at `bp`.
pub unsafe extern "C" fn rt_fs_append(
    ctx: *mut Ctx,
    lp: Addr,
    ln: I64,
    pp: Addr,
    pn: I64,
    bp: Addr,
    bn: I64,
) -> I64 {
    filed(ctx, "fs_append", Op::Append, (lp, ln), (pp, pn), |_| {
        Some(Second::Body(unsafe { lent(bp, bn) }.to_vec()))
    })
}

/// The ticket of `fs.read_at`: what is there of `len` bytes from `offset`, neither negative.
pub unsafe extern "C" fn rt_fs_read_at(
    ctx: *mut Ctx,
    lp: Addr,
    ln: I64,
    pp: Addr,
    pn: I64,
    offset: I64,
    len: I64,
) -> I64 {
    filed(ctx, "fs_read_at", Op::ReadAt, (lp, ln), (pp, pn), |_| {
        Some(Second::Range {
            offset: offset.0,
            len: len.0,
        })
    })
}

/// The ticket of `fs.set_mode` to the nine permission bits `bits`.
pub unsafe extern "C" fn rt_fs_set_mode(
    ctx: *mut Ctx,
    lp: Addr,
    ln: I64,
    pp: Addr,
    pn: I64,
    bits: I64,
) -> I64 {
    filed(ctx, "fs_set_mode", Op::SetMode, (lp, ln), (pp, pn), |c| {
        let bits = u32::try_from(bits.0).ok().filter(|bits| *bits <= 0o777);
        handed(c, "fs_set_mode", "a mode", bits).map(Second::Mode)
    })
}

/// The ticket of `fs.set_modified` to `ms` milliseconds since the epoch, never before it.
pub unsafe extern "C" fn rt_fs_set_modified(
    ctx: *mut Ctx,
    lp: Addr,
    ln: I64,
    pp: Addr,
    pn: I64,
    ms: I64,
) -> I64 {
    filed(
        ctx,
        "fs_set_modified",
        Op::SetModified,
        (lp, ln),
        (pp, pn),
        |_| Some(Second::Millis(ms.0)),
    )
}

/// The ticket of `fs.scan`, below the directory's own entries where `deep` is 1.
pub unsafe extern "C" fn rt_fs_scan(
    ctx: *mut Ctx,
    lp: Addr,
    ln: I64,
    pp: Addr,
    pn: I64,
    deep: I64,
) -> I64 {
    filed(ctx, "fs_scan", Op::Scan, (lp, ln), (pp, pn), |_| {
        Some(Second::Deep(deep.0 != 0))
    })
}

/// The ticket of `fs.open` as the opening `how` numbers.
pub unsafe extern "C" fn rt_fs_open(
    ctx: *mut Ctx,
    lp: Addr,
    ln: I64,
    pp: Addr,
    pn: I64,
    how: I64,
) -> I64 {
    filed(ctx, "fs_open", Op::Open, (lp, ln), (pp, pn), |c| {
        handed(c, "fs_open", "an opening", Opening::of(how.0)).map(Second::Opening)
    })
}

/// The ticket of at most `max` bytes of the file `file`, `max` never negative.
pub unsafe extern "C" fn rt_fs_read_chunk(
    ctx: *mut Ctx,
    lp: Addr,
    ln: I64,
    file: I64,
    max: I64,
) -> I64 {
    let max = u64::try_from(max.0).unwrap_or(0);
    on_file(ctx, "fs_read_chunk", lp, ln, file, Chunk::Read(max))
}

/// The ticket of the bytes at `bp` written to the file `file`.
pub unsafe extern "C" fn rt_fs_write_chunk(
    ctx: *mut Ctx,
    lp: Addr,
    ln: I64,
    file: I64,
    bp: Addr,
    bn: I64,
) -> I64 {
    let body = unsafe { lent(bp, bn) }.to_vec();
    on_file(ctx, "fs_write_chunk", lp, ln, file, Chunk::Write(body))
}

/// The ticket of the file `file` let go.
pub unsafe extern "C" fn rt_fs_close(ctx: *mut Ctx, lp: Addr, ln: I64, file: I64) -> I64 {
    on_file(ctx, "fs_close", lp, ln, file, Chunk::Close)
}

/// The ticket of `fs.write_secret`: the bytes at `bp`, the secret `secret` in the encoding named
/// at `ep`, and the bytes at `ap`, framed here and wiped once written.
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn rt_fs_write_secret(
    ctx: *mut Ctx,
    lp: Addr,
    ln: I64,
    pp: Addr,
    pn: I64,
    bp: Addr,
    bn: I64,
    secret: SecretHandle,
    ep: Addr,
    en: I64,
    ap: Addr,
    an: I64,
) -> I64 {
    filed(
        ctx,
        "fs_write_secret",
        Op::WriteSecret,
        (lp, ln),
        (pp, pn),
        |c| {
            let held = secret_of(c, &secret)?;
            let encoding = text_at(c, ep, en)?;
            let framing = Framing::named(&encoding, Op::WriteSecret.what(), c.site())
                .map_err(|d| c.fail(d))
                .ok()?;
            let (before, after) = unsafe { (lent(bp, bn), lent(ap, an)) };
            Some(Second::Sealed(framing.framed(before, opened(&held), after)))
        },
    )
}

/// The ticket of `fs.read_secret`: the secret between the bytes at `bp` and those at `ap`, in the
/// encoding named at `ep`, kept by the run under the number answered, found.
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn rt_fs_read_secret(
    ctx: *mut Ctx,
    lp: Addr,
    ln: I64,
    pp: Addr,
    pn: I64,
    bp: Addr,
    bn: I64,
    ep: Addr,
    en: I64,
    ap: Addr,
    an: I64,
) -> I64 {
    filed(
        ctx,
        "fs_read_secret",
        Op::ReadSecret,
        (lp, ln),
        (pp, pn),
        |c| {
            let encoding = text_at(c, ep, en)?;
            let framing = Framing::named(&encoding, Op::ReadSecret.what(), c.site())
                .map_err(|d| c.fail(d))
                .ok()?;
            let keeper = from_host(c, |facilities| Arc::downgrade(facilities.run()))?;
            let (before, after) = unsafe { (lent(bp, bn), lent(ap, an)) };
            Some(Second::Frame {
                before: before.to_vec(),
                framing,
                after: after.to_vec(),
                keeper,
            })
        },
    )
}

/// The secret the run keeps under `number`, which it no longer keeps.
pub unsafe extern "C" fn rt_fs_secret_taken(ctx: *mut Ctx, number: I64) -> SecretHandle {
    asked(ctx, "fs_secret_taken", WRITES, SecretHandle::of(0), |c| {
        let kept = from_host(c, |facilities| facilities.run().take_secret(number.0)).flatten();
        match handed(c, "fs_secret_taken", "a secret's number", kept) {
            Some(kept) => SecretHandle::of(c.heap.bridge(Value::secret_bytes(&kept))),
            None => SecretHandle::of(0),
        }
    })
}
