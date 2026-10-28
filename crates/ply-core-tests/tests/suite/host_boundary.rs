//! What the host boundary says when it refuses a call before or after its handler: each refusal names
//! the operation and the handler, and says what would or would not help. Which calls reach these is
//! the CLI suite's `host_boundary` module, over real host operations.

use ply_core::host::{err_blocking_answered_inline, err_footprint_escape, err_withheld};
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

/// A handler declared blocking answers on the host's pool, never inline on the run's thread.
#[test]
fn a_blocking_handler_that_answers_inline_is_refused_by_name() {
    let d = err_blocking_answered_inline(Span::DUMMY, "t.db.get[users]", "audit::blocking");
    assert_eq!(d.code, codes::HOST_BLOCKING_ANSWER);
    assert!(d.message.contains("t.db.get[users]"), "{}", d.message);
    assert!(
        format!("{} {}", d.message, d.notes.join(" ")).contains("audit::blocking"),
        "{d:#?}"
    );
}
