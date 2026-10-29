use crate::units::{self, def};
use ply_compiled::c::Library;
use ply_compiled::c::linked::install;
use ply_compiled::heap::imm_value;

/// A native program's `main` hands the runtime the descriptor its unit lays out, and the unit
/// loads from that alone: nothing of it is looked up by name.
#[test]
fn a_linked_program_loads_from_the_descriptor_its_main_hands_over() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let defs = [def("m.seven", 0, "  return ply_imm(7);")];
    let text = units::text(&defs, &units::exports_of(&defs));
    let Some(path) = units::library(dir.path(), &text) else {
        return;
    };
    // The library stands in for the program's own code, which is there for the process's life.
    let opened = Box::leak(Box::new(Library::open(&path).expect("the unit opens")));
    let unit = std::ptr::from_ref(opened.descriptor().expect("the unit lays a descriptor out"));
    assert!(
        Library::linked().is_none(),
        "nothing is linked before a descriptor is installed"
    );
    assert!(
        unsafe { install(std::ptr::null()) }.is_err(),
        "no descriptor is no program"
    );
    unsafe { install(unit) }.expect("a descriptor installs");
    let native = ply_compiled::c::load_linked().expect("the linked unit loads");
    assert_eq!(native.names(), ["m.seven"]);
    let answer = units::entered(&native, "m.seven", &[]).expect("it answers");
    assert_eq!(imm_value(answer), 7);
    assert!(
        unsafe { install(unit) }.is_err(),
        "a second descriptor is refused"
    );
}
