//! What this binary ships, and the binding a run is lent, over a pack written out by hand.

/// The pack this binary reads: a few shipped modules written out by hand, one below another and one
/// with a data file beside it.
#[ctor::ctor(unsafe)]
fn installed() {
    let file =
        std::env::temp_dir().join(format!("ply-privileged-tests-{}.pack", std::process::id()));
    std::fs::write(&file, pack::pack(pack::SHIPPED)).expect("the pack is written");
    let pack = ply_pack::Pack::of_binary(&file)
        .expect("the pack reads")
        .expect("the file is a pack");
    ply_pack::install(pack);
}

mod hosts;
mod pack;
mod shipped;
