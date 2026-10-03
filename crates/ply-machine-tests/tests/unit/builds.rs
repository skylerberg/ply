use std::collections::BTreeSet;

/// The emitter's program is closed by reading import lines in Rust, because its identity has to be
/// known before any compiler runs. The front end pulls the same imports when it builds the builder,
/// and that is the definition; this holds the Rust reading to it.
#[test]
fn the_emitters_program_is_the_one_the_front_end_pulls() {
    let builder = ply_machine::builds::builder()
        .unwrap_or_else(|d| panic!("the builder is built: {}", d.message));
    let shipped = |names: &mut dyn Iterator<Item = String>| -> BTreeSet<String> {
        names.filter(|name| ply_std::is_std(name)).collect()
    };
    let pulled = shipped(&mut builder.front.files.iter().map(|f| f.name.clone()));
    let read = shipped(
        &mut ply_machine::builds::emitter_program()
            .into_iter()
            .map(|(name, _)| name),
    );
    assert!(!pulled.is_empty(), "the builder pulled no shipped module");
    assert_eq!(
        read, pulled,
        "the shipped modules the emitter's program is read to carry are not the ones the front end \
         pulls for it"
    );
}
