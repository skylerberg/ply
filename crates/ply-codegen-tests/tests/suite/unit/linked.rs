use ply_codegen::c::Library;
use ply_codegen::c::linked::{Row, install};
use std::ffi::c_void;

static FIRST: u8 = 1;
static SECOND: u8 = 2;

#[test]
fn a_linked_program_answers_each_name_its_table_holds_and_no_other() {
    // Rows ascending by name, as a native program's `main` hands them over.
    static ROWS: [Row; 2] = [
        Row {
            name: c"ply_a".as_ptr(),
            address: &FIRST as *const u8 as *mut c_void,
        },
        Row {
            name: c"ply_b".as_ptr(),
            address: &SECOND as *const u8 as *mut c_void,
        },
    ];
    struct Rows(&'static [Row; 2]);
    // SAFETY: the rows point at statics, which outlive every thread.
    unsafe impl Sync for Rows {}
    static HELD: Rows = Rows(&ROWS);
    static BACKWARDS: [Row; 2] = [
        Row {
            name: c"ply_b".as_ptr(),
            address: &SECOND as *const u8 as *mut c_void,
        },
        Row {
            name: c"ply_a".as_ptr(),
            address: &FIRST as *const u8 as *mut c_void,
        },
    ];
    static OUT_OF_ORDER: Rows = Rows(&BACKWARDS);
    assert!(Library::linked().is_none(), "nothing is linked before a table is installed");
    assert!(
        unsafe { install(OUT_OF_ORDER.0.as_ptr(), OUT_OF_ORDER.0.len()) }.is_err(),
        "rows out of order would hide a name from the search, so they are refused"
    );
    unsafe { install(HELD.0.as_ptr(), HELD.0.len()) }.expect("ascending rows install");
    let lib = Library::linked().expect("an installed table is a linked program");
    assert_eq!(lib.symbol("ply_a"), Some(&FIRST as *const u8 as *mut c_void));
    assert_eq!(lib.symbol("ply_b"), Some(&SECOND as *const u8 as *mut c_void));
    assert_eq!(lib.symbol("ply_c"), None);
    assert!(
        unsafe { install(HELD.0.as_ptr(), HELD.0.len()) }.is_err(),
        "a second table is refused"
    );
}
