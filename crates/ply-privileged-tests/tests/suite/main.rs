//! The machine's Rust over programs written out by hand: the front `ply run` files and reads back,
//! and the runnable a deployed program carries.

/// The pack this binary reads: a few shipped modules written out by hand, one below another and one
/// with a data file beside it.
#[ctor::ctor(unsafe)]
fn pack() {
    let file =
        std::env::temp_dir().join(format!("ply-privileged-tests-{}.pack", std::process::id()));
    std::fs::write(&file, front::pack(front::SHIPPED)).expect("the pack is written");
    let pack = ply_pack::Pack::of_binary(&file)
        .expect("the pack reads")
        .expect("the file is a pack");
    ply_pack::install(pack);
}

mod front;
mod reused;
mod runnable;
mod shipped;
