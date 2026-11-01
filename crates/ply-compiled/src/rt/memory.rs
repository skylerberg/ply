use super::*;
use crate::emitted::{Block, DropRoot, OwnedWord};

pub const RELEASE_DEPTH: i64 = 32;

pub unsafe extern "C" fn rt_alloc_bytes(context: *mut Context, bytes: Word) -> Block {
    Block::alloc(unsafe { &mut (*context).heap }, bytes)
}

pub unsafe extern "C" fn rt_alloc_kept(context: *mut Context, bytes: Word) -> Block {
    let tables = unsafe { &(*context).tables };
    Block::alloc_immortal(&mut lock(&tables.immortals), bytes)
}

pub unsafe extern "C" fn rt_free(context: *mut Context, o: Block, bytes: Word) {
    unsafe { o.free(&raw mut (*context).heap, bytes) }
}

pub unsafe extern "C" fn rt_dec_shared_last(_context: *mut Context, o: Block) -> Word {
    Word::from(unsafe { o.last_of_shared() })
}

pub unsafe extern "C" fn rt_handle_free(context: *mut Context, h: Block) {
    unsafe { h.free_handle(context) }
}

pub unsafe extern "C" fn rt_released_twice(context: *mut Context, o: Block) {
    let c = unsafe { &*context };
    panic!(
        "{o:?} was released after its last holder let it go, at root {} bytes {}..{} from its start",
        c.site_root, c.site_start, c.site_end
    );
}

pub unsafe extern "C" fn rt_release_later(context: *mut Context, w: OwnedWord, root: DropRoot) {
    let c = unsafe { &mut *context };
    debug_assert!(
        c.release_depth >= RELEASE_DEPTH,
        "a release was put off at depth {}, short of the bound",
        c.release_depth
    );
    c.release_later.push((w, root));
    c.release_waiting = c.release_later.len() as i64;
}

pub unsafe extern "C" fn rt_release_drain(context: *mut Context) {
    debug_assert_eq!(
        unsafe { (*context).release_depth },
        0,
        "a drop root drained short of the outermost"
    );
    unsafe { (*context).release_depth = 1 };
    while let Some((w, root)) = unsafe { (*context).release_later.pop() } {
        unsafe {
            (*context).release_waiting = (*context).release_later.len() as i64;
            root(context, w);
        }
    }
    unsafe { (*context).release_depth = 0 };
}
