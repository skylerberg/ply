use ply_corpus::payload::{
    Checked, JSON_SRC, MAP_SRC, ORDER_SRC, derivation_cost, derived_module, json_shape, run_tests,
    write_project,
};
use ply_store::Store;

/// The three programs are the measurement, so a change that stops one of them compiling has to
/// fail here rather than at the moment somebody wants a number.
#[test]
fn every_measurement_program_compiles_and_its_own_test_passes() {
    for (name, source) in [
        ("payload.ply", JSON_SRC),
        ("maps.ply", MAP_SRC),
        ("order.ply", ORDER_SRC),
    ] {
        let dir = write_project(&[(name, source.to_string())]).unwrap();
        let checked = Checked::open(dir.path())
            .unwrap_or_else(|e| panic!("`{name}` does not compile: {e:#}"));
        let mut store = Store::open(dir.path()).unwrap();
        run_tests(&checked.loaded, &mut store)
            .unwrap_or_else(|e| panic!("`{name}`'s own test failed: {e:#}"));
    }
}

/// The two axes have to move independently, or the table cannot separate a per-field cost from
/// a per-byte one — which is the only question it is there to answer.
#[test]
fn widening_a_field_grows_the_bytes_and_not_the_field_count() {
    let points = json_shape(&[(2, 0), (2, 200), (8, 0)], 2, 1).unwrap();
    let [narrow, wide, longer] = &points[..] else {
        panic!("three points were asked for and {} came back", points.len());
    };
    assert_eq!(narrow.fields, wide.fields);
    assert!(wide.payload_bytes > narrow.payload_bytes + 200);
    assert!(longer.fields > narrow.fields && longer.payload_bytes > narrow.payload_bytes);
    // Both halves are timed on their own, so each is a duration rather than a difference: a
    // zero here means a half that did not run.
    assert!(
        narrow.parse_micros > 0.0 && narrow.codec_micros > 0.0,
        "parse {} µs, codec {} µs of a {} µs decode",
        narrow.parse_micros,
        narrow.codec_micros,
        narrow.decode_micros
    );
}

/// Both variants have to be the same program in everything but the derivation, or the
/// comparison prices two projects rather than one feature.
#[test]
fn the_two_derivation_variants_declare_the_same_types_and_the_same_tests() {
    let plain = derived_module(0, 3, false);
    let derived = derived_module(0, 3, true);
    for i in 0..3 {
        let decl = format!("pub type T0x{i} = ");
        assert!(plain.contains(&decl) && derived.contains(&decl));
        assert!(plain.contains(&format!("t0x{i} round-trips")));
        assert!(derived.contains(&format!("t0x{i} round-trips")));
    }
    assert!(!plain.contains("derive json"));
    assert_eq!(derived.matches("derive json for").count(), 3);
}

/// Small, but it exercises the whole path — two projects, four timings, a cache measured —
/// which is what a table nobody can reproduce would hide.
#[test]
fn a_derivation_point_is_produced_for_both_variants() {
    let points = derivation_cost(&[4], 4, 1).unwrap();
    assert_eq!(points.len(), 2);
    let derived = points.iter().find(|p| p.variant == "derived").unwrap();
    let plain = points.iter().find(|p| p.variant == "plain").unwrap();
    assert_eq!(derived.tests, plain.tests);
    assert!(
        derived.definitions > plain.definitions,
        "a derivation that added no definition is not a derivation"
    );
}
