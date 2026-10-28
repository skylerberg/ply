//! What the host boundary says when it refuses a call: each refusal names the operation and what
//! serves it, and says what would or would not help. Which calls reach these is the CLI suite's
//! `host_boundary` module, over real host operations.

use ply_core::host::err_withheld;
use ply_core::{Span, Symbol, codes};

/// A run that withholds an operation names the handler that serves it, where it serves it, and the
/// twin a test answers it over.
#[test]
fn a_withheld_refusal_names_the_handler_and_where_it_serves() {
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
    assert!(notes.contains("`std.process`'s twin"), "{notes}");
}
