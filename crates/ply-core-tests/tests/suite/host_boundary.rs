use ply_core::host::err_withheld;
use ply_core::{Span, Symbol, codes};

#[test]
fn a_withheld_rejection_names_the_handler_and_where_it_serves() {
    let d = err_withheld(
        Span::DUMMY,
        "std.process.process.out[proc]",
        &Symbol::new("std.process.process"),
        "test::out",
        "under `ply run`",
    );
    assert_eq!(d.code, codes::HOST_UNBOUND);
    assert!(d.message.contains("process.out[proc]"), "{}", d.message);
    let notes = d.notes.join(" ");
    assert!(
        notes.contains("`test::out` serves this under `ply run`"),
        "{notes}"
    );
    assert!(notes.contains("`std.process`'s fake"), "{notes}");
}
