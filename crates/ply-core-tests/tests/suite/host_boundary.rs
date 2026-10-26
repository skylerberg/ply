//! What the host boundary says when it refuses a call: each refusal names the operation and what
//! serves it, and says what would or would not help. Which calls reach these is the CLI suite's
//! `host_boundary` module, over real host operations.

use ply_core::host::{err_footprint_escape, err_withheld};
use ply_core::{EffectAtom, Footprint, Mode, Resource, Span, Symbol, codes};

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

/// The declared footprint is checked before the handler runs, or the packet is already out.
#[test]
fn an_answer_outside_the_declared_footprint_names_what_was_declared() {
    let atom = EffectAtom::operation(
        "t.net",
        Resource::Named(Symbol::new("socket")),
        Mode::Write,
        "send",
    );
    let declared = Footprint::from_atoms([EffectAtom::new(
        "t.net",
        Resource::Named(Symbol::new("other")),
        Mode::Write,
    )]);
    assert!(!declared.covers(&atom));
    let d = err_footprint_escape(
        Span::DUMMY,
        "t.net.send[socket]",
        &atom,
        &declared,
        "test::send",
    );
    assert_eq!(d.code, codes::HOST_FOOTPRINT_ESCAPE);
    assert!(d.message.contains("t.net.send[socket]"), "{}", d.message);
    let said = format!("{} {}", d.message, d.notes.join(" "));
    assert!(said.contains("t.net.write[other]"), "{said}");
}
