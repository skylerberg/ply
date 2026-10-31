use ply_core::Span;
use ply_core::rc::*;

#[test]
fn turning_on_recording_of_update_sites_clears_the_update_sites_recorded_before() {
    let span = Span::new(ply_core::SourceId(0), 0, 1);
    record_sites(true);
    note_update_of(true, 0, span);
    assert_eq!(sites().len(), 1, "the update was attributed to its span");

    record_sites(true);
    assert!(
        sites().is_empty(),
        "arming inherited the last caller's residue: {:?}",
        sites()
    );
    record_sites(false);
}
