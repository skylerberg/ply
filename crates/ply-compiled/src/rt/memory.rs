//! Memory for blocks emitted code lays out: allocated by bytes, freed by size, counted at offset 0
//! alone, and released past a bounded depth through a buffer the outermost drop root drains.
//!
//! A drop root keeps the bound as emitted code writes it: on entry, with
//! [`Ctx::release_depth`] at [`RELEASE_DEPTH`] or past it, it hands its word and itself to
//! [`rt_release_later`] and returns; otherwise it adds one to the depth, and on its way out takes
//! the one back and, where that leaves the depth 0 and [`Ctx::release_waiting`] is not, calls
//! [`rt_release_drain`].

use super::*;
use crate::emitted::{Block, DropRoot, Handed};

/// How deeply drop roots nest before a release is put off.
pub const RELEASE_DEPTH: i64 = 32;

/// A block of `bytes`, its header among them, for emitted code to lay out: counted once, and given
/// back by [`rt_free`] at the same size.
pub unsafe extern "C" fn rt_alloc_bytes(ctx: *mut Ctx, bytes: Word) -> Block {
    Block::alloc(unsafe { &mut (*ctx).heap }, bytes)
}

/// A block of `bytes` on the unit's persistent heap, for a kept value's copy: immortal, so it
/// outlives every entry and nothing frees it.
pub unsafe extern "C" fn rt_alloc_kept(ctx: *mut Ctx, bytes: Word) -> Block {
    let tables = unsafe { &(*ctx).tables };
    Block::alloc_immortal(&mut lock(&tables.immortals), bytes)
}

/// Gives back a block whose last holder was emitted code, `bytes` long with its header, the size it
/// was allocated at. Walks nothing: emitted code let go of what the block held.
pub unsafe extern "C" fn rt_free(ctx: *mut Ctx, o: Block, bytes: Word) {
    unsafe { o.free(&raw mut (*ctx).heap, bytes) }
}

/// One holder fewer of a shared block: 1 where that was its last, the caller then releasing it,
/// else 0.
pub unsafe extern "C" fn rt_dec_shared_last(_ctx: *mut Ctx, o: Block) -> Word {
    Word::from(unsafe { o.last_of_shared() })
}

/// Gives back a runtime handle, a bridge, what a native made, a hold or a continuation, whose last
/// holder was emitted code, after letting go of what the runtime keeps for it. Emitted code lets go of what a hold
/// holds first: this reads none of a hold's words.
pub unsafe extern "C" fn rt_handle_free(ctx: *mut Ctx, h: Block) {
    unsafe { h.free_handle(ctx) }
}

/// A checked build's trap: emitted code released a block whose last holder had let it go. Never
/// returns.
pub unsafe extern "C" fn rt_released_twice(ctx: *mut Ctx, o: Block) {
    let c = unsafe { &*ctx };
    panic!(
        "{o:?} was released after its last holder let it go, at root {} bytes {}..{} from its start",
        c.site_root, c.site_start, c.site_end
    );
}

/// Puts off the release of `w` by `root` until the outermost drop root drains the buffer, so the
/// drop root at [`RELEASE_DEPTH`] nests no deeper.
pub unsafe extern "C" fn rt_release_later(ctx: *mut Ctx, w: Handed, root: DropRoot) {
    let c = unsafe { &mut *ctx };
    debug_assert!(
        c.release_depth >= RELEASE_DEPTH,
        "a release was put off at depth {}, short of the bound",
        c.release_depth
    );
    c.release_later.push((w, root));
    c.release_waiting = c.release_later.len() as i64;
}

/// Makes every release put off, the last put off first, until none waits. Each root runs one deep,
/// so the roots it calls on their way out leave the draining here.
pub unsafe extern "C" fn rt_release_drain(ctx: *mut Ctx) {
    debug_assert_eq!(
        unsafe { (*ctx).release_depth },
        0,
        "a drop root drained short of the outermost"
    );
    unsafe { (*ctx).release_depth = 1 };
    while let Some((w, root)) = unsafe { (*ctx).release_later.pop() } {
        unsafe {
            (*ctx).release_waiting = (*ctx).release_later.len() as i64;
            root(ctx, w);
        }
    }
    unsafe { (*ctx).release_depth = 0 };
}
