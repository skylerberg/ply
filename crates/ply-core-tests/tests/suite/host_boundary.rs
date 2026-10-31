use ply_core::host::err_unbound_operation;
use ply_core::{Span, Symbol, codes};

#[test]
fn an_unbound_handler_error_names_the_effect_and_the_handler_and_says_which_command_the_handler_serves_it_under()
 {
    let d = err_unbound_operation(
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
