use super::*;
use crate::emitted::{Block, DropRoot, Handed};

pub const RELEASE_DEPTH: i64 = 32;

pub unsafe extern "C" fn rt_alloc_bytes(ctx: *mut Ctx, bytes: Word) -> Block {
    Block::alloc(unsafe { &mut (*ctx).heap }, bytes)
}

pub unsafe extern "C" fn rt_alloc_kept(ctx: *mut Ctx, bytes: Word) -> Block {
    let tables = unsafe { &(*ctx).tables };
    Block::alloc_immortal(&mut lock(&tables.immortals), bytes)
}

pub unsafe extern "C" fn rt_free(ctx: *mut Ctx, o: Block, bytes: Word) {
    unsafe { o.free(&raw mut (*ctx).heap, bytes) }
}

pub unsafe extern "C" fn rt_dec_shared_last(_ctx: *mut Ctx, o: Block) -> Word {
    Word::from(unsafe { o.last_of_shared() })
}

pub unsafe extern "C" fn rt_handle_free(ctx: *mut Ctx, h: Block) {
    unsafe { h.free_handle(ctx) }
}

pub unsafe extern "C" fn rt_released_twice(ctx: *mut Ctx, o: Block) {
    let c = unsafe { &*ctx };
    panic!(
        "{o:?} was released after its last holder let it go, at root {} bytes {}..{} from its start",
        c.site_root, c.site_start, c.site_end
    );
}

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
