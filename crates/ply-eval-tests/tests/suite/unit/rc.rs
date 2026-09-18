use ply_eval::rc::*;
use ply_span::Span;

#[test]
fn arming_site_recording_clears_what_the_last_caller_left() {
    let span = Span::new(ply_span::SourceId(0), 0, 1);
    record_sites(true);
    note_update_of(true, 0, span);
    assert_eq!(sites().len(), 1, "the update was attributed to its span");

    // The caller panicked here: no disarm ran.
    record_sites(true);
    assert!(
        sites().is_empty(),
        "arming inherited the last caller's residue: {:?}",
        sites()
    );
    record_sites(false);
}
