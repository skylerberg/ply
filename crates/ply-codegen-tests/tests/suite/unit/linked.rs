use crate::units::{self, def};
use ply_codegen::c::Library;
use ply_codegen::c::linked::{Row, install, install_unit};
use ply_codegen::heap::imm_value;
use std::ffi::c_void;

static FIRST: u8 = 1;
static SECOND: u8 = 2;

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
        unsafe { install_unit(std::ptr::null()) }.is_err(),
        "no descriptor is no program"
    );
    unsafe { install_unit(unit) }.expect("a descriptor installs");
    let lib = Library::linked().expect("an installed descriptor is a linked program");
    assert_eq!(lib.symbol("ply_exports"), None);
    let (native, refused) = ply_codegen::c::load_linked(None).expect("the linked unit loads");
    assert!(refused.is_empty());
    assert_eq!(native.names(), ["m.seven"]);
    let answer = units::entered(&native, "m.seven", 10, &[]).expect("it answers");
    assert_eq!(imm_value(answer), 7);
    assert!(
        unsafe { install_unit(unit) }.is_err(),
        "a second descriptor is refused"
    );
}

#[test]
fn a_linked_program_answers_each_name_its_table_holds_and_no_other() {
    struct Rows([Row; 2]);
    // SAFETY: the rows point at statics, which outlive every thread.
    unsafe impl Sync for Rows {}
    // Ascending by name, as a native program's `main` hands them over.
    static HELD: Rows = Rows([
        Row {
            name: c"ply_a".as_ptr(),
            address: &FIRST as *const u8 as *mut c_void,
        },
        Row {
            name: c"ply_b".as_ptr(),
            address: &SECOND as *const u8 as *mut c_void,
        },
    ]);
    static OUT_OF_ORDER: Rows = Rows([
        Row {
            name: c"ply_b".as_ptr(),
            address: &SECOND as *const u8 as *mut c_void,
        },
        Row {
            name: c"ply_a".as_ptr(),
            address: &FIRST as *const u8 as *mut c_void,
        },
    ]);
    assert!(
        Library::linked().is_none(),
        "nothing is linked before a table is installed"
    );
    assert!(
        unsafe { install(OUT_OF_ORDER.0.as_ptr(), OUT_OF_ORDER.0.len()) }.is_err(),
        "rows out of order would hide a name from the search, so they are refused"
    );
    unsafe { install(HELD.0.as_ptr(), HELD.0.len()) }.expect("ascending rows install");
    let lib = Library::linked().expect("an installed table is a linked program");
    assert_eq!(
        lib.symbol("ply_a"),
        Some(&FIRST as *const u8 as *mut c_void)
    );
    assert_eq!(
        lib.symbol("ply_b"),
        Some(&SECOND as *const u8 as *mut c_void)
    );
    assert_eq!(lib.symbol("ply_c"), None);
    assert!(
        unsafe { install(HELD.0.as_ptr(), HELD.0.len()) }.is_err(),
        "a second table is refused"
    );
}
